# Cloud operations

This page replaces the old private cloud runbooks with one common operating model. Public docs are the shared surface
every LLM and human maintainer should read first.

Neon Law Navigator is GCP-wired and provider-agnostic. The production path uses GKE Autopilot, a hosted SurrealDB, GCS,
Secret Manager, OpenObserve for direct OTLP telemetry, BigQuery billing export, and Restate Cloud. The application code
keeps the cloud boundary behind traits, protocols, and env vars: `cloud::StorageService`, SurrealDB, OIDC, embedded
Rego, Restate, SendGrid, Kubernetes, and `portal::agent_router::AgentRouter`.

## Former private-runbook coverage

- **KIND local dev** — source of truth: [`AGENTS.md`](../AGENTS.md#local-kind-development) and
  [`test-database.md`](test-database.md).
- **GCP REST setup** — source of truth: [`oss-install.md`](oss-install.md), this page, and `cli/src/devx/gcp/` module
  docs.
- **GKE production** — source of truth: [`gke-prod.md`](gke-prod.md) and [`gitops.md`](gitops.md). **Ship** — source of
  truth: [`gke-prod.md`](gke-prod.md#manifest-delivery); [`deploy/gke-ship-example.md`](deploy/gke-ship-example.md) is a
  short worked example.
- **GCP spend** — source of truth: this page. **Prod DB access** — source of truth: this page.
  **Observability/OpenObserve** — source of truth: [`observability.md`](observability.md) and
  [`durable-workflows.md`](durable-workflows.md).
- **OIDC/embedded Rego/Rauthy** — source of truth: [`oidc.md`](oidc.md), [`access-model.md`](access-model.md), and
  [`AGENTS.md`](../AGENTS.md#authentication-and-lawyer-access).

The collapse rule is simple: durable policy, invariants, architecture, and operator recipes live in `docs/`.

## Deployment profiles and operator boundary

Exact `NAVIGATOR_ENVIRONMENT=dev` selects the one disposable development profile, which local KIND uses. All three
persistent hosted rows, `neon-law-stg` included, use exact `production`, because staging is a release-order role rather
than a reduced runtime. Empty or unset also selects production; `staging`, `test`, mixed case, surrounding whitespace,
and every other nonempty value fail parsing. Production keeps hosted-persistence requirements: real GCS and no emulator
endpoint. A normal dev deployment uses real non-production SendGrid and DocuSign demo credentials stamped
`NAVIGATOR_CREDENTIAL_ENVIRONMENT=dev`; only `NAVIGATOR_CI_HARNESS=1` may use fakes.

The **deployment operator** who supplies Kubernetes, cloud accounts, secrets, domains, and environment values is not a
Navigator application **admin**. `persons.role = 'admin'` is an application authorization tier and grants no cloud or
cluster authority.

## Local development

`navigator dev staging` requires exact `NAVIGATOR_ENVIRONMENT=dev`, an explicit non-production context,
Navigator-managed labels, and an immutable environment ID before reset or deletion.

The standard local loop is KIND through the `navigator` CLI:

```bash
cargo run --release -p cli -- dev up   # once; reuses an existing cluster on re-run
set -a; source .devx/env; set +a
cargo run -p neon                                  # Ctrl-C and re-run to iterate
cargo run --release -p cli -- dev down                # full teardown — only for a clean rebuild, not routine cleanup
```

`dev up` brings up SurrealDB, Rauthy, Garage, Restate, `workflows-service`, and OpenObserve in KIND. It writes
`.devx/env` for the host-side `web` process. The cluster is a **persistent dev fixture**: leave it up between sessions
and re-run `dev up` to restore port-forwards after a sleep or reboot (it reuses the existing cluster). See
[`AGENTS.md`](../AGENTS.md#the-shared-dependency-tier).

Scratch artifacts go under `/tmp`, never the repo. Screenshots normally go under `/tmp/navigator-screenshots/`.

The KIND **dependency tier** is the exception to "local stacks are task resources": it is a reusable dev fixture, so
leave the cluster up between sessions. Everything else an agent spins up — rebuilt dev images, browser drivers, the
host-side `web` process — is a per-task resource to stop at handoff. So before handing off a created or updated PR, stop
`web` and task-created browser drivers, remove task-created standalone containers/images, and prune task-created Docker
build cache — but do **not** `down`/`kind delete` the dependency cluster as routine cleanup, and do not prune Docker
volumes unless the user approves the data loss. Full teardown is for a deliberate clean rebuild only.

## GCP setup

`navigator ops gcp setup` is the persistent, production-shaped environment provisioner. It owns one deployment's Cloud
SQL instance, four GCS lanes, runtime identity, Fleet membership, and Autopilot cluster. It runs once per runtime
project; every deployment's `config.toml` selects the `production` runtime and credential profile. They omit
`NAVIGATOR_CONFIG_SYNC_REPO`; `navigator ops ship` is their sole manifest owner.

### The image hub

[`environments.md`](environments.md) splits four projects into one image hub and three runtime projects containing three
isolated deployments. The hub (`ghcr`) holds the Artifact Registry every deployment pulls from, the CI pusher service
account, and the GitHub Workload Identity pool. Nothing runs there.

`navigator ops gcp hub setup --project-id ghcr` provisions exactly that and nothing else. It has no flag that reaches
buckets, GKE, or IAP — those live on the environment provisioner, and the hub's own `HubSetupConfig` cannot express
them. Run `--dry-run` first; its ten-call recorded plan is the regression proof for that boundary.

An environment then takes pull rights on the hub with `navigator ops gcp setup --images-project-id ghcr`. That grants
the environment's workload service account `roles/artifactregistry.reader` on the hub repository and skips the
per-environment registry, CI pusher, and Workload Identity pool entirely. Omitting the flag keeps the single-project
shape, where the registry sits alongside the workloads that pull from it — the right default for a fork running one
project.

`NAVIGATOR_IMAGE_REGISTRY` is what makes `navigator ops ship` resolve image references, and `NAVIGATOR_GCP_PROJECT_ID`
keeps naming the project that holds the GKE cluster and buckets. The images live on GHCR rather than in any GCP project,
so the two are independent and neither can be mistaken for the other — which is the whole reason the Artifact Registry
hub, its per-deployment reader grants, and its Workload Identity pool retired.

A tenant guard refuses either command aimed at the other's project before the first GCP call: the hub never receives
buckets or GKE, and an environment never hosts the shared registry. An unrecorded project ID is always allowed, so a
fork provisions its own projects unimpeded.

A service account from another organization is a foreign identity to the firm-owned registry, so every `setIamPolicy`
against the hub repository — including a routine provisioner re-run — is evaluated against
`constraints/iam.allowedPolicyMemberDomains`. A refusal surfaces as a named error identifying that constraint and the
refused principal, not a bare 403. Clear the constraint on the registry's organization, or scope an exception on the hub
project, then re-run.

The GCP pipelines call REST APIs directly where a stable GCP REST surface exists and use the CLI's recorded `gcloud`
shell seam for the compact GKE and IAM commands. There is no broad Google SDK wrapper. That keeps dry-run interception
and wiremock coverage at one narrow seam.

When touching `cli/src/devx/gcp/`, keep four things correct:

- `GcpService::default_base_url()` in `cli/src/devx/gcp/client.rs`. Each per-step endpoint path in `services.rs`,
  `network.rs`, `buckets.rs`, and `run.rs`. The JSON request body shape. The long-running-operation polling path passed
  to `lro::wait`. Compute operation names are bare IDs and must be polled below the matching project/global or
  project/region collection, and report `status: DONE`, while Google long-running operations report `done: true`.

Every step follows the same conventions:

- POST the create/enable operation and treat `409 Conflict` as success. Wait for LROs on 2xx responses that return an
  operation name; skip the wait on 409. Let `GcpClient` handle dry-run recording. Do not add a `gcloud` fallback or move
  base URLs into env vars.

Live environment setup prints eleven numbered, secret-free stages. Long-running REST writes additionally print their
service, operation ID, scoped polling path, and completion. These lines may name projects, regions, buckets, networks,
SQL instances, service accounts, and clusters; they must never include database URLs, passwords, tokens, credentials, or
any decrypted secret value.

All four deployment buckets stay private. The deployment Google service account receives `roles/storage.objectAdmin` on
each bucket, and Workload Identity maps both `navigator-web` and `workflows-service` to that principal. Marketing assets
become anonymous only when `web` reads them from the dedicated assets bucket through the cacheable same-origin
`/assets/*` route. No bucket receives `allUsers`, and setup never changes organization policy. The application does not
expose corresponding routes for the documents, exports, or logs buckets.

When an endpoint drifts, update the module's wiremock test to match Google's current docs first, then update the
implementation and run the dry-run command from [`oss-install.md`](oss-install.md).

## Production deploy

Code reaches production through PRs and dated images:

1. Merge through the normal PR flow in [`gitops.md`](gitops.md).
2. After the version PR merges, an operator tags that merged `main` commit with `YY.M.D` or
   `YY.M.D-hotfix.N`, which starts `deploy.yml`. The workflow fetches `origin/main` and rejects a tag on an unmerged
   side branch before it publishes anything.
3. The deploy workflow builds and publishes the service images to the Google Artifact Registry: the three brand server
   images and `navigator-workflows-service`.
4. The same run rolls GKE onto that tag — `neon-law-stg` first, then production once staging is green — and reports
   both to `#navigator`. No operator step; the `ops ship` command below remains for a roll outside a release run.

`navigator ops ship --deployment <name> --tag YY.M.D` is the self-contained reconcile: every coordinate comes from
`deployments/<name>/config.toml`, so a stale shell cannot select the wrong deployment. It prints the resolved
deployment, project, location, cluster, namespace, context, and images project before acting, then **refuses to proceed
when the pinned `NAVIGATOR_GKE_CONTEXT` names a different cluster than the one it resolved** — the deployment configs
across separate projects make a copied context a one-line edit away from rolling one deployment's release onto another's
cluster, and both contexts are valid. The check is pure text, so it fires in `--dry-run` too. A context that is not a
`gke_<project>_<location>_<cluster>` name carries no coordinates to compare; ship says so rather than passing quietly.
It then confirms every image is published at the tag, renders the CLI-embedded GKE manifest tree from the deployment's
`NAVIGATOR_*` coordinates **and the tag itself**, **checks the boot invariants against that rendered tree while
everything is still local**, and only then `kubectl diff -k`s and applies it — **unconditionally**, so a manifest change
merged to `main` reaches the cluster instead of silently rotting behind an image-only push. The render mechanics — the
placeholder→env table, the by-name bail on a missing var, and `--dry-run` — are owned by
[`gke-prod.md`](gke-prod.md#manifest-delivery). `navigator-web` and `workflows-service` both land on the same `YY.M.D`
tag; version skew is an avoidable production risk. The GitHub webhook receiver is the `POST /webhooks/github/{secret}`
route on `workflows-service` and reads only its GitHub and Restate-ingress keys; the DevX Slack services
`DevxIssueTriage` and `devx-pr` bind into `workflows-service`, receive the Slack webhook, and are re-registered with
Restate alongside the other durable workflows.

The tag is a substitution token in the manifests, resolved at render time — the apply itself lands the real image. It is
deliberately **not** a placeholder that a follow-up `kubectl set image` corrects: `workflows-service` (`maxSurge: 0`)
deletes the running pod before the replacement is ready, so an apply carrying an unpullable tag takes that tier down for
the whole gap. One write, one `ReplicaSet`.

The preflight ordering is load-bearing, not incidental. `web` enforces its boot invariants at startup and crash-loops
when a required key is missing, so `ship` diffs the requirements against the production Secret plus each web-binary
Deployment's env **before** the reconcile touches the cluster. An unsatisfied requirement therefore aborts a ship that
has changed nothing. It reads the env from the rendered manifests rather than the live Deployments for two reasons: the
manifests are the state about to be applied (the live env is what the apply overwrites), and on a first-ever ship no
Deployment exists to read.

`ship` never patches the Secret for you — it prints the exact `kubectl patch` naming every missing key, and stops. Fill
in the values with `navigator ops secrets apply --deployment <name>` (or the printed patch) and re-run. Keys that the
manifests supply as Deployment env belong in the manifest tree, not the Secret; the preflight counts either as
satisfying a requirement.

If the image tag is unchanged and only a Secret changed, `--restart-only` rollout-restarts the service deployments so
pods re-read `envFrom`.

The roll asserts that the web service account holds `roles/iam.serviceAccountTokenCreator` on itself before anything
rolls, because a pod without it 500s every document download — under Workload Identity the pod holds no private key and
signs each download URL through IAM Credentials `signBlob`. That assertion is a read (`getIamPolicy`) and runs in every
mode, `--dry-run` included. On a row `ops gcp setup` has provisioned the binding is already there and nothing is
written. When it is absent the roll grants it, which needs `setIamPolicy`; `--assert-signing-iam` withdraws that
authority, so the roll instead stops and prints the `gcloud` command for someone who holds the permission. Use it when
the operator holding the release tag is under a no-IAM-changes rule: the invariant is still proved, only the grant moves.

Run production cluster commands under the production secret context. Never paste real secret values into chat, docs,
commits, or PR bodies.

## Production database

Production is a hosted SurrealDB. Ad-hoc access goes through the deployment's own `NAVIGATOR_SURREAL_*` credentials,
read from its encrypted secrets rather than pasted from anywhere else.

Read-only `SELECT`s are allowed when the user asks for inspection. Before any `CREATE`, `UPDATE`, `DELETE`, or `DEFINE`:

- Write the exact SurrealQL to a timestamped file under `/tmp/navigator-prod-sql/`. Show the user the path and contents.
  Wait for explicit approval for that exact statement. Scope the write with a guard on the old value. Verify it
  afterwards.

The canonical seed is idempotent: it inserts missing rows and does not update existing production rows. A live data fix
needs a guarded update, a one-shot backfill job, or an app seam.

## Spend reporting

Report GCP spend from the BigQuery Cloud Billing export, not console guesses or rate-card math. Always show:

- gross cost, credits, which are negative, net cost, which is `gross + credits`, currency, and whether the current day
  is partial because billing export data lags by roughly 24 hours.

Discover the project from env and the billing table from BigQuery. Do not hard-code billing account generated table
names into docs or code.

## Observability

Every service binary emits through `telemetry::init("navigator-<name>")`. With no `OTEL_EXPORTER_OTLP_ENDPOINT`, logs
stay human-readable on stdout. With the endpoint set, logs become JSON and traces/metrics export through OTLP.

The load-bearing rule is:

> Identifiers and counts, never content.

Safe telemetry fields include ids, service names, outcomes, durations, status codes, and counts. Unsafe fields include
client names, email addresses, answer bodies, document bodies, privileged facts, and full request or tool arguments.
This rule applies in local and Cloud OpenObserve, Cloud Logging, BigQuery, and any future sink.

Use `navigator ops doctor`, OpenObserve, the Restate console, and the six-hourly Heartbeat email to debug missing
periodic jobs or durable workflow failures. The architecture details live in [`observability.md`](observability.md) and
[`durable-workflows.md`](durable-workflows.md).

## Website publication

Top-level files in `docs/` are already published at `/docs/:slug` by `portal::docs`. The site bakes the docs into the
binary with `include_str!`, renders markdown under the firm brand, and rewrites top-level doc links to site routes. That
gives every maintainer and LLM the same documentation surface.

Good next steps for the website:

- Add a `/docs` hub that lists every `DocsIndex::docs()` entry instead of requiring users to know a slug. Include a
  short "For agents" section there linking to [`agent-decision-councils.md`](agent-decision-councils.md), this page,
  [`access-model.md`](access-model.md), [`glossary.md`](glossary.md), and
  [`AGENTS.md`](../AGENTS.md#local-kind-development).
- Keep top-level docs concise and push long command transcripts into examples such as
  [`deploy/gke-ship-example.md`](deploy/gke-ship-example.md).
- Keep public docs as the source of truth. If an invariant matters, lift it into `docs/`.
