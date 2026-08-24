# GKE production deployment

This page describes what one environment is made of. Which environments exist, which GCP project each one is, and how a
single image serves the brand is mapped in [`environments`](environments.md).

The Neon Law Navigator production deployment runs on **GKE Autopilot** with every supporting service managed by Google
or Restate. The daily operator workload is reviewing dependency PRs and glancing at dashboards; no node patching, no DB
failover drills, no Helm chart babysitting.

## Architecture at a glance

```text
internet
   │
   ▼
┌───────────────────────────────────┐
│  Global External App LB (Gateway) │ ← Cloud Armor (DDoS + WAF)
│  ← Certificate Manager (TLS)      │ ← Identity-Aware Proxy (admin)
└───────────────────────────────────┘
   │
   ▼
┌───────────────────────────────────┐
│  GKE Autopilot                    │ ← Workload Identity for GCP
│  ┌─────────────────────────────┐  │
│  │ navigator-web (embedded Rego)│──┼──→ SurrealDB (PSC)
│  │ workflows-service           │──┼──→  Restate Cloud
│  │   + GitHub webhook receiver │──┼──→  Restate Cloud ingress
│  │   + DevX Slack services     │──┼──→  Engineering Slack
│  └─────────────────────────────┘  │──→  GCS (object storage)
└───────────────────────────────────┘   ↑
   ▲                                    │
   │   ┌────────────────────────────────┘
   │   │ Secrets via Secret Manager CSI driver
   │   ▼
   │ Secret Manager
   │
   └── navigator ops ship renders + applies this deployment's manifests
```

Deployment decisions are summarized here and in [`cloud-operations.md`](cloud-operations.md).

## What lives where

| Concern | Managed by | Manifest |
| --- | --- | --- |
| Compute | GKE Autopilot | (cluster, no manifest) |
| Edge LB | Global External ALB (legacy GKE Ingress) | `examples/deploy/k8s/gke/ingress/ingress.yaml` |
| TLS | Google-managed certificate | `examples/deploy/k8s/gke/ingress/managed-certificate.yaml` |
| Store | Hosted SurrealDB | (out-of-cluster; PSC endpoint) |
| Object storage | GCS | (out-of-cluster; `-assets`, `-documents`, `-exports`, `-logs` buckets per deployment) |
| OIDC | Identity Platform | (out-of-cluster; issuer URL) |
| Workflows | Restate Cloud | (out-of-cluster; bearer-token auth) |
| Per-Project git repos | A constructed link into the deployment's GitHub organization | (no manifest) |
| Secrets | Secret Manager + CSI | `examples/deploy/k8s/gke/secrets/` |
| Image registry | GHCR (`ghcr.io/neon-law-source-code`) | `examples/deploy/k8s/gke/patches/web-image.yaml` |
| Delivery | `ship` renders + applies the embedded tree | [Manifest delivery](#manifest-delivery) |
| Logs / metrics / traces | Cloud Logging + GMP + Cloud Trace | (auto, no manifest) |
| Long-term log archive | Cloud Logging sink → GCS | (gcloud-provisioned; see below) |

## Bootstrap

A fresh cluster requires GCP commands that can't run from CI—they need a human under `gcloud auth login`. Export the
deployment's coordinates from its `config.toml`, preview the exact deployment-specific sequence, then execute it:

```bash
set -a; eval "$(grep ' = "' deployments/neon-law-stg/config.toml | sed 's/ = /=/')"; set +a
navigator ops gcp setup --dry-run
navigator ops gcp setup
```

After the cluster is up, run `navigator ops ship --deployment <name>`. It renders every placeholder from that
deployment's `config.toml`, diffs, and applies the embedded manifest tree. Do not edit the shared examples in place for
one deployment. The four Navigator stacks deliberately leave `NAVIGATOR_CONFIG_SYNC_REPO` unset, so no `RootSync`
competes with this rendered apply. A fork may use the optional Config Sync seam only with its own fully rendered source
and with `ops ship` removed as a competing manifest owner.

## Pointing a hostname at a deployment

A `ManagedCertificate` is authorized **by the load balancer**, not by DNS: Google issues it only after the hostname
already resolves to the ingress address. That ordering is forced and it costs a TLS gap — while the certificate
validates, the host answers `308` on port 80 and returns an empty TLS handshake on 443, so it is neither the old site
nor the new one. Plan the cutover as a short outage rather than a swap:

1. Point the `A` records at `NAVIGATOR_GATEWAY_IP` (`navigator ops dns setup --domain <zone> --gateway-ip <ip>` — with
   no `--host`, it covers `www` and `workflows`). Dry-run first; the command never deletes a record.
2. Watch `gcloud compute ssl-certificates list --project <project>` until the domain reports `ACTIVE`. A status of
   `FAILED_NOT_VISIBLE` is normal beforehand — it records validation attempts made while DNS still pointed elsewhere,
   and it clears itself once DNS is right.
3. Allow a few more minutes after `ACTIVE` for Google's edge to serve the certificate. A zero-byte handshake (`no peer
   certificate available`) in that window means propagation, not misconfiguration — check the target proxy really
   carries the certificate before changing anything.

A Certificate Manager DNS authorization can cut a hostname over with **no** TLS gap, which is how the retired static
marketing site used to move. That option is not available here: the GKE ingress owns its certificates, so plan for the
window above rather than expecting a seamless switch.

Verified on the production cutover of 2026-08-05: `www.neonlaw.com` moved off the marketing site, and the host was
unreachable over HTTPS for roughly half an hour between the DNS change and the edge serving the certificate.

## Data and access boundary

The public edge reaches three service Deployments:

- `navigator-web` serves the portal, AIDA/API routes, and client-facing matter views.
- `workflows-service` hosts the Restate durable worker and, on the automation-home deployment, the `POST
  /webhooks/github/{secret}` receiver on its own listener behind the Envoy sidecar — GitHub posts to the public
  `workflows` host because `www` goes behind the tailnet. The receiver verifies signatures and submits identifier-only
  commands to the Restate ingress. Alongside the legal workflows it also binds the DevX Slack-notice services
  `DevxIssueTriage` and `devx-pr`, which alone receive `SLACK_WEBHOOK_URL` and send the engineering notices.

The storage buckets sit behind those services. `NAVIGATOR_ASSETS_BUCKET` is the only public bucket.
`NAVIGATOR_DOCUMENTS_BUCKET` is private and holds content-addressed client documents through `cloud::StorageService`;
clients never receive bucket IAM, object keys, or raw GCS URLs. A client sees a file only when `web` resolves their
session to a `persons` row and finds a matching `person_project_roles` row for the Project. `NAVIGATOR_EXPORTS_BUCKET`
is the private nightly Parquet/Iceberg lane consumed as `NAVIGATOR_STORAGE_BUCKET` by `workflows-service`;
`NAVIGATOR_LOGS_BUCKET` is the Nearline audit-log archive.

The `navigator-web` backend attaches a GKE `BackendConfig` that injects `X-Navigator-Client-Region:{client_region}`
before requests reach the pod. The web app uses that bounded, edge-derived geography bucket for visitor analytics.
Analytics do not store raw IP addresses and do not trust the left-most `X-Forwarded-For` value as a geography source.

## Deploy flow

CI/CD is two workflows (see [`gitops.md`](gitops.md#cicd-workflows)): a lean PR flow (`ci.yml`), and a publish flow
(`deploy.yml`) that integration-tests the workspace, publishes images to GHCR, and stops. No GitHub App is involved in
either. **Nothing in CI reaches a cluster** — a person runs `ops ship` from the command Slack posts.

```text
PR merged to main
  └─→ .github/workflows/ci.yml runs fmt + clippy + cargo test --workspace
      (no images built — the PR flow is lean by design)

A person bumps the version and lands it — the merge is the publish
  └─→ navigator ops release-version --tag <version>   (Cargo.toml + Cargo.lock, commits)
  └─→ PR, merge. Nothing else is required: no tag is pushed by hand
  └─→ .github/workflows/deploy.yml runs, holding no cloud credential
                  ├─ ops release-check: is this version newer than every release tag?
                  │    (equal → the run ends here, which is almost every merge)
                  ├─ KIND integration suite (e2e + interop + browser)
                  ├─ create the immutable YY.M.D tag at the merged commit
                  ├─ build + push service images to GHCR tagged YY.M.D + latest
                  ├─ attach three CLI archives to the tag's GitHub Release
                  ├─ dispatch the tag to the homebrew-navigator tap, which
                  │    digests those archives and bumps its own formula
                  └─ post three reports to the engineering Slack channel
                        (what published; CLI install; then the ops ship command)

A person decides the version should go in front of clients
  └─→ gcloud auth application-default login
  └─→ navigator ops ship --deployment <name> --tag YY.M.D
        (add --dry-run to rehearse: Secret preflight + diff, nothing applied)
        (an older tag rolls back — ops ship does not care which way it moves)

Prove a change to deploy.yml itself, without releasing
  └─→ push a kind-ci/<topic> branch
        └─→ runs ONLY the KIND integration job: no images, no ship, no Slack

Re-roll an already-published tag (no build, no publish)
  └─→ navigator ops ship --deployment <name> --tag YY.M.D
        (add --dry-run to rehearse: Secret preflight + diff, nothing applied)
```

The release run publishes and stops at the registry. Operator-driven `ship` performs every roll — a first release, a
re-roll, a rehearsal, a rollback, or a deployment the workflow deliberately never reaches.

`navigator ops ship --tag YY.M.D` pins the selected brand server and `workflows-service` to the published tag. Those two
Deployments are the whole rollout: Navigator serves no Git and mounts no repository volume, so a ship waits on exactly
the two service rollouts it started.

The published images live on **GHCR**, at `ghcr.io/neon-law-source-code` — `cli::devx::registry::DEFAULT_REGISTRY`,
which a fork overrides with `NAVIGATOR_IMAGE_REGISTRY`. `ops ship` renders that one value into the `YOUR_IMAGE_REGISTRY`
token every `image:` line in the embedded manifests carries, so what a node pulls is whatever that variable says and
nothing else can disagree with it.

**Confirm the node pull path against the live cluster rather than inferring it from this repository.** A GHCR package
inherits its linked repository's visibility and `neon-law-source-code/navigator` is public, so an anonymous pull should
need no imagePullSecret and no registry credential to rotate. The cluster serving production predates that move: it was
provisioned when images sat in a private Artifact Registry in the images project, pulled cross-project via Workload
Identity with `roles/artifactregistry.reader` on the node identity — a binding that `ops gcp setup` still writes when
given `--images-project-id`. Which arrangement a given cluster is actually on is a property of the deployment tree in
the private `navigator-deploy` repository and of the cluster itself, not of anything checked in here. Check there, or
ask the operator, before treating either as settled — and verify the first release actually pulls before relying on the
public-package path.

Retention is `.github/workflows/ghcr-retention.yml`, nightly at 01:11 UTC: it deletes a version only when it is older
than 30 days **and** outside its image's newest 10 **and** not the one carrying `latest`. That count floor is why a
deferred roll cannot age a running tag off the shelf — the ten most recent versions of every image stay pullable however
long the gap between releases. See [GitOps](gitops.md#image-retention).

A release version is whatever `semver` parses, and `YY.M.D` — optionally with a prerelease such as `-hotfix.N` — is the
convention. The one rule `deploy.yml` enforces is ORDERING: `ops release-check` refuses a version that is not newer than
every release tag already published. Nothing checks the calendar, because a bump is authored days before it merges; and
nothing checks provenance, because a push to `main` is the only thing that publishes. The legacy four-component
`YY.M.D.H` form is retired: Cargo cannot parse it as a version, so no release has been able to carry it since the tag
started coming from `[workspace.package].version`. See [GitOps](gitops.md#one-workflow-owns-publishing----deployyml).
Rolling a published version onto the cluster is always `ops ship`, above, run by a person. To exercise the pipeline
without publishing, push a `kind-ci/**` branch.

## Manifest delivery

`examples/deploy/k8s/gke/` is white-labeled **by design** — it is the rebrandable reference (the `navigator ops rebrand`
seam), so it ships placeholders like `YOUR_PROJECT_ID`, `NAVIGATOR_PUBLIC_HOST`, and `YOUR_OAUTH_CLIENT_ID_*`. Running a
plain `kubectl apply -k` of it against a real cluster would rewrite the ManagedCertificate domains, the OAuth client
ids, and every domain-derived env back to those placeholders. The substitutions the base needs are pure values — the GCP
project, per-deployment bucket/SQL/GSA names, public and workflow hosts (ManagedCertificate + Ingress hosts,
`OAUTH_REDIRECT_URI`, `NAV_BASE_URL`, `GOOGLE_OAUTH_REQUIRED_HD`), the required browser OAuth client ID, and the
post-registration Gemini client ID — not structure.

`navigator ops ship` supplies those values itself. The whole manifest tree (`examples/deploy/k8s/gke` + the shared
`k8s/base` it references) is **embedded in the CLI** with `include_dir!`; every ship renders it — the placeholders
substituted from the selected deployment's `deployments/<name>/config.toml` — into a **throwaway temp dir**, then
`kubectl diff -k` (surface drift) and `kubectl apply -k` (reconcile). The apply is **unconditional**: any structural
change on `main` (a renamed container, a new sidecar, a volume, an env-list edit) reaches the cluster on the next ship
rather than silently rotting. There is no persistent overlay folder and no image-only fall-through — the CLI generates
from the checked-in coordinates what a deployer used to hand-keep. The manifests are embedded; only the `deployments/`
tree of the repository checkout is read at ship time.

The substitution values live in the selected deployment's `config.toml` — plaintext, reviewable coordinates in this
repo:

| placeholder token | `NAVIGATOR_*` var |
| --- | --- |
| `YOUR_PROJECT_ID` | `NAVIGATOR_GCP_PROJECT_ID` |
| `YOUR_GCP_REGION` | `NAVIGATOR_GCP_LOCATION` |
| `NAVIGATOR_WEB_IMAGE` | `NAVIGATOR_WEB_IMAGE` |
| `NAVIGATOR_PUBLIC_HOST` | `NAVIGATOR_PUBLIC_HOST` |
| `NAVIGATOR_WORKFLOWS_HOST` | `NAVIGATOR_WORKFLOWS_HOST` |
| `NAVIGATOR_ASSETS_BUCKET` | `NAVIGATOR_ASSETS_BUCKET` |
| `NAVIGATOR_DOCUMENTS_BUCKET` | `NAVIGATOR_DOCUMENTS_BUCKET` |
| `NAVIGATOR_GATEWAY_IP_NAME` | `NAVIGATOR_GATEWAY_IP_NAME` |
| `NAVIGATOR_GCP_SERVICE_ACCOUNT_ID` | `NAVIGATOR_GCP_SERVICE_ACCOUNT_ID` |
| `navigator-web-secrets` | `NAVIGATOR_WEB_SECRET_NAME` |
| `GOOGLE_OAUTH_REQUIRED_HD` | `GOOGLE_OAUTH_REQUIRED_HD` |
| `namespace: navigator` | `NAVIGATOR_K8S_NAMESPACE` |
| `YOUR_OAUTH_CLIENT_ID_BROWSER` | `NAVIGATOR_OAUTH_CLIENT_ID_BROWSER` |
| `YOUR_OAUTH_CLIENT_ID_GEMINI` | `NAVIGATOR_OAUTH_CLIENT_ID_GEMINI` (nullable until data-store registration) |

A missing or blank required var bails **by name** before anything is written — a half-substituted manifest never reaches
the cluster. Before Gemini Enterprise registration, `ops ship` substitutes the browser ID for the absent Gemini token,
which renders the same ID twice in a set-valued allowlist and leaves browser login functional. Issue
[#1126](https://github.com/neon-law-source-code/navigator/issues/1126) removes that temporary fallback after the staging
data store exists. Firm-specific values that are *secrets or operator toggles* (SendGrid, DocuSign, the inbound-email
host, DKIM enforcement) stay in the `navigator-web-secrets` K8s Secret and arrive via `envFrom`. The base's inline-env
`$patch: replace` does not touch that Secret reference, so a full apply preserves them.

A key that Secret projects must reach a pod **from the Secret**, never as an inline `value`. The scheduled triggers
under `examples/deploy/k8s/exports/` mount no CSI volume, so each reads `RESTATE_INGRESS_URL` and `RESTATE_AUTH_TOKEN`
through an explicit `valueFrom.secretKeyRef`, marked `optional` because a deployment that does not project the key must
get a trigger that exits naming it rather than pods stuck in `CreateContainerConfigError`. Writing either inline breaks
far more than the trigger: a container's `env` list merges by entry name, so a literal in the rendered tree merges with
the cluster's `valueFrom` into one entry carrying both, the API server rejects the whole object, and `kubectl diff -k`
aborts before comparing anything — blocking **every** manifest change to that deployment, not just the trigger's. That
is what made version rolls image-only for two releases. `triggers_source_every_projected_key_from_the_deployment_secret`
in `cli/src/devx/ship.rs` fails the build on the next one, and the render's own `YOUR_*` sweep fails on a placeholder no
substitution resolves.

A healthy cluster's `kubectl diff -k` against the freshly rendered tree is a **near no-op** — that is the acceptance bar
for source == cluster. `--dry-run` renders and diffs but never applies, so "see what will change" needs no folder:

```bash
# Preview the reconcile without applying (renders, diffs, drops the temp dir):
navigator ops ship --deployment <row> --deployments-dir . --tag <YY.M.D> --dry-run
```

The reconcile runs against the deployment `--deployment` names (production remains propose-only — run it yourself; never
let an agent ship to prod, and beware the shared kubeconfig defaulting to the prod GKE context):

```bash
navigator ops ship --deployment <row> --deployments-dir . --tag <YY.M.D>
```

## What lives outside this repo

These are operator-managed resources you maintain via gcloud / console, not via this repo's manifests:

1. **GCS buckets and runtime identity** — setup creates the explicitly named assets, documents, exports, and logs
   buckets plus the deployment GSA and its Workload Identity bindings. Only the assets bucket receives a public binding.
2. **SurrealDB instance** — provisioned at the store provider, not by this repo. Its endpoint is a plaintext coordinate
   in `deployments/<name>/config.toml`, and its root credentials go into that deployment's `secrets.enc.yaml`, which
   `ops secrets apply` writes into that deployment's Secret Manager.
3. **Identity Platform tenant** — configured via the console. OAuth client secret goes into Secret Manager as
   `navigator-oauth-client-secret`.
4. **Restate Cloud tenant** — register at <https://cloud.restate.dev>. The tenant URL and bearer token go into Secret
   Manager as `navigator-restate-broker-url` (consumed as `RESTATE_BROKER_URL`) and `navigator-restate-auth-token`
   (consumed as `RESTATE_AUTH_TOKEN`).
5. **DNS A records** for `www.<navigator-domain>` and `workflows.<navigator-domain>` → the static IP reserved as
   `navigator-gateway-ip`. GitHub posts its webhooks to the `workflows` host at `/webhooks/github/{secret}`, the same
   host Restate Cloud uses.
6. **Cloud Logging → GCS sink** — a log router sink that archives `web` container logs to `gs://YOUR_PROJECT_ID-logs`
   for long-horizon audit. Provisioned via `gcloud` (see "Long-term log archive" below), not Config Sync.

Kubernetes manifests flow through the deployment-rendered `navigator ops ship` path. Managed services remain owned by
the GCP provisioner or the named operator procedure above.

## Long-term log archive

GKE forwards every container's stdout to Cloud Logging automatically, but that alone is not the deployment's durable
archive. A **log router sink** copies Navigator service logs into the private NEARLINE bucket named by
`NAVIGATOR_LOGS_BUCKET`, which `navigator ops gcp setup` already creates. The setup command does not create the sink.

The original design routed these via a Config Connector `LoggingLogSink` CR. That path is shelved: Config Connector does
not reliably reconcile on this cluster's GKE version, so the sink is **provisioned directly with `gcloud`** and lives
outside the repo's manifests on purpose — there is nothing under `examples/deploy/k8s/gke/` to keep it in sync with, and
putting it there would falsely imply Config Sync owns it.

Run this once per deployment after its cluster exists, with that deployment's coordinates exported from its
`config.toml` (the coordinate-export line under [Bootstrap](#bootstrap)):

```bash
# Route only this deployment namespace to its own bucket.
SINK_NAME="${NAVIGATOR_K8S_NAMESPACE}-to-gcs"
gcloud logging sinks create "$SINK_NAME" \
  "storage.googleapis.com/${NAVIGATOR_LOGS_BUCKET}" \
  --log-filter='resource.type="k8s_container"
                resource.labels.namespace_name="'"$NAVIGATOR_K8S_NAMESPACE"'"
                AND (resource.labels.container_name="web"
                     OR resource.labels.container_name="worker")' \
  --project="$NAVIGATOR_GCP_PROJECT_ID"

# Grant the sink's auto-created writer identity permission to write the bucket.
WRITER=$(gcloud logging sinks describe "$SINK_NAME" \
  --project="$NAVIGATOR_GCP_PROJECT_ID" --format='value(writerIdentity)')
gcloud storage buckets add-iam-policy-binding \
  "gs://${NAVIGATOR_LOGS_BUCKET}" \
  --member="$WRITER" \
  --role=roles/storage.objectCreator
```

Verify the sink is writing (objects appear under `logs/...` prefixes within ~1 hour of the next matching log line):

```bash
gcloud logging sinks describe "$SINK_NAME" --project="$NAVIGATOR_GCP_PROJECT_ID"
gcloud storage ls "gs://${NAVIGATOR_LOGS_BUCKET}/**"
```

This is operator-managed state, like the SurrealDB instance and Identity Platform tenant above — it is **not** rebuilt
by `kubectl apply -k`. Apply the organization's retention and lifecycle policy separately; the provisioner guarantees
the bucket's region, private uniform access, and NEARLINE storage class, not a retention duration. If you later get
Config Connector reconciling, the `LoggingLogSink` CR can replace these commands; until then this section is the source
of truth for the sink's existence.

## Verifying a deploy

```bash
# Pod rollout
kubectl --namespace navigator rollout status deployment/navigator-web
kubectl --namespace navigator rollout status deployment/workflows-service

# Image actually in use
kubectl --namespace navigator get deployment/navigator-web \
    -o jsonpath='{.spec.template.spec.containers[?(@.name=="web")].image}'
```

## Trust boundary

**Operator-driven deploy** means GitHub Actions publishes immutable images but holds no cluster credential. An
authorized operator runs `navigator ops ship --deployment <name>`; that command pins the expected kube context, diffs,
applies, and waits. Workload Identity binds each Kubernetes ServiceAccount to a GCP service account, so pods talk to GCP
without JSON keys. The stacks have no `RootSync`, preventing a second controller from reverting the rendered deployment.

### What the operator credential must carry

"Authorized" is two grants, not one. On the cluster, the reconcile needs whatever `kubectl diff`/`apply` and the rollout
waits require — `roles/container.developer` covers it. Separately, the roll's preflight asserts that the web service
account can sign GCS URLs for itself, because a pod under Workload Identity holds no private key and mints every
document-download URL through IAM Credentials `signBlob`. So the operator also needs, on the *web service account's own
resource* (`<service-account-id>@neon-law-stg.iam.gserviceaccount.com` for staging):

| Permission | When it is needed | Role that carries it |
| --- | --- | --- |
| `iam.serviceAccounts.getIamPolicy` | Every roll — this is the steady state | `roles/iam.serviceAccountAdmin` |
| `iam.serviceAccounts.setIamPolicy` | Only when the binding is genuinely absent | `roles/iam.serviceAccountAdmin` |

`roles/container.developer` carries no `iam.serviceAccounts.*` permission at all, so a cluster-only grant is not enough
even for the read. The preflight reads the policy first and writes only on a real absence, which is why the common case
needs the read alone: `ops gcp setup` already wrote that binding when it provisioned the row, so a provisioned
deployment rolls without any IAM write.

`ops ship --dry-run` performs that read. It is the half a dry-run can answer honestly, so an operator missing
`getIamPolicy` finds out from the dry-run rather than from a live roll that stops at its first step. Only the write is
printed instead of performed — and when the binding is absent the dry-run says so, because nothing short of attempting
the write confirms `setIamPolicy`.

There is no flag to skip the check. A missing binding is not cosmetic — every `/…/documents/:doc_id/download` would 500
on `iam.serviceAccounts.signBlob` — so an operator who cannot verify or cannot write hands the binding off to someone
holding `roles/iam.serviceAccountAdmin` rather than rolling past it. The preflight runs before anything mutates and
names which of the two it hit.

## Restore from backup

Backup for GKE snapshots run daily at 05:00 UTC; retention is 30 days. Restore is a single CLI:

```bash
gcloud container backup-restore backups list \
    --location=us-west4 --backup-plan=navigator-daily

gcloud container backup-restore restores create my-restore \
    --location=us-west4 \
    --restore-plan=<plan> \
    --backup=<backup-id>
```

Practice the restore quarterly against a scratch cluster. Untested backups are theatre.
