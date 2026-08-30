---
kind: workshop
title: Operating Neon Law Navigator
description: Stand up and operate your own Neon Law Navigator instance on Google Cloud.
---

# Operating Neon Law Navigator

Our firm runs Neon Law Navigator on Google Cloud, and gives the recipe away. This workshop is for **admin** users of the
application: the people who own billing, secrets, OIDC, runtime configuration, and release verification. It stands up
your **own** instance — the same Rust stack our attorneys use, on your own Google Cloud project, for your own community.
One command does most of the work: `navigator ops gcp setup`, a provisioner written in Rust that talks to Google's REST
APIs directly and ships with a dry-run so you can read the whole plan before a single packet leaves your laptop.

Two things to hold up front. This provisions **billable** Google Cloud resources — a GKE Autopilot cluster and five
storage buckets — so it is not free, and you should set a budget alert before you begin. And this is a deployment guide
for admin users standing up infrastructure. With that said: you can run the same stack we run. Let's stand it up.

> **Bring your own typeface license.** Neon Law Navigator source code is owned by Shook Law PLLC.
> [GORP Serif](https://trashtype.com/fonts/gorp) is proprietary font software licensed separately from TrashType and is
> not covered by the repository's code licenses.
> If your deployment serves GORP, obtain and maintain the appropriate TrashType license, keep its license notice with
> your font files, and follow the [TrashType terms](https://trashtype.com/legal).
> Otherwise, replace GORP with a typeface you are licensed to serve before launch. >
> **Want a free win first?** You do not need a cloud account — or a credit card — to see Neon Law Navigator run.
> `cargo run -p cli -- dev up` brings the whole stack up locally in
> [KIND](https://kind.sigs.k8s.io/) (the store, OIDC, storage, the workflow broker, embedded Rego), then
> `source .devx/env` and
> `cargo run -p neon` serves it on `localhost`. Boot it locally, inspect the canonical seed, and only
> come back here when you want it on the public internet. The full local loop is in
> [`AGENTS.md`](https://github.com/neon-law-source-code/navigator/blob/main/AGENTS.md#local-kind-development).

**Set the budget alert before you provision** — one command caps the surprise so the bill cannot run away while you
learn:

```bash
gcloud billing budgets create --billing-account "$BILLING_ACCOUNT_ID" \
  --display-name "Neon Law Navigator" --budget-amount 200USD \
  --threshold-rule percent=0.5 --threshold-rule percent=0.9
```

Idle, the stack runs on the order of an Autopilot cluster's baseline plus five mostly empty buckets — budget for it,
watch the first invoice, and size the cluster down if it is more than you need.

## Intro

### Deploy your own

Follow this workshop to stand up your own Neon Law Navigator. `navigator ops gcp setup` stands the same Rust stack our
attorneys run up on your Google Cloud project. Free, open, and yours to keep.

---

The rest of this workshop is the self-serve install, end to end. Everything from here down stands up a working instance
you own outright.

### Agenda

Six steps, each tagged with its Bloom verb. You are the admin operator; the `navigator` CLI is the instrument:

- **Create** — stand up a billed project and authenticate.
- **Predict** — run `--dry-run` and read every API call before sending one.
- **Identify** — name the twenty-two Google Cloud APIs the provisioner enables.
- **Explain** — describe the VPC, the five buckets, the private image registry, and the three service deployments.
- **Execute** — bring up the cluster, the static IP, and Fleet membership.
- **Verify** — ship the service images and confirm `/readyz` answers 200.

---

Each step is tagged with the Bloom verb it exercises (the [Anderson & Krathwohl 2001
revision](https://en.wikipedia.org/wiki/Bloom%27s_taxonomy)). You are the admin operator; the `navigator` CLI is the
instrument. **Create** — stand up a billed Google Cloud project and authenticate so `navigator` can act on your behalf.
**Predict** — run `navigator ops gcp setup --dry-run` and read every API call the provisioner _would_ make before
sending one. **Identify** — name the twenty-two Google Cloud APIs the provisioner enables, and why each is needed.
**Explain** — describe the VPC, the five storage buckets, the private image registry, the two service deployments, and
why re-running setup is always safe. **Execute** — bring up the GKE Autopilot cluster, the static IP, and Fleet
membership with one command. **Verify** — ship the service images and confirm the running service answers `/readyz` with
a 200.

## Prepare Google Cloud

### Bring your own project

`navigator ops gcp setup` provisions _into_ a project; it does not create one. Start by creating a project and attaching
a billing account, then authenticate so the CLI can act as you:

```bash
gcloud projects create your-project-id --name "Neon Law Navigator"
gcloud billing projects link your-project-id --billing-account "$BILLING_ACCOUNT_ID"
gcloud auth login --force --update-adc
gcloud auth print-access-token >/dev/null
gcloud auth application-default print-access-token >/dev/null
```

For the Navigator matrix, prove billing is enabled on the hub and all three runtime projects before provisioning:

```bash
for project in \
  ghcr neon-law-stg neon-law-prod neon-law
do
  gcloud billing projects describe "$project" \
    --format='value(projectId,billingEnabled)'
done
```

All five lines must end in `True`. If one is `False`, attach it before running setup:

```bash
gcloud billing projects link neon-law-prod \
  --billing-account "$BILLING_ACCOUNT_ID"
```

---

The [project creation guide](https://cloud.google.com/resource-manager/docs/creating-managing-projects) walks both the
create and the billing link. The provisioner uses both the active `gcloud` credential for shell-outs and [Application
Default Credentials](https://cloud.google.com/docs/authentication/provide-credentials-adc) for its REST client.
`--update-adc` refreshes both in one browser flow. Google Workspace session controls can expire both credentials; rerun
the three authentication lines before resuming when either token probe fails.

You also need `gcloud`, `kubectl`, and `docker` on your `PATH`: the cluster steps shell out to `gcloud`, the image ships
with `docker`, and you reconcile manifests with `kubectl`. Nothing in this workshop hard-codes our project name, region,
or cluster — every value flows through a flag or an environment variable, because this guide documents the deploy path
for you to point at your own cloud.

### Dry-run first

Before you change anything, read the plan. The `--dry-run` flag records every REST call and `gcloud` shell-out and
prints them without sending traffic:

```bash
cargo run -p cli -- ops gcp setup --project-id your-project-id --dry-run
```

---

`gcloud` has no universal dry-run equivalent, so we built one — it prints the plan without sending traffic or touching
your `gcloud` session. You will see the plan in order: REST calls that enable APIs, create the VPC, and create five
buckets. It then creates a deployment-specific Google service account and its direct Secret Manager, bucket, and
signed-URL permissions. A single-project install also provisions its private Artifact Registry; our three deployments
instead point at the shared `ghcr` hub. The last three stages grant registry access, reserve the gateway IP and create
the cluster, then attach Kubernetes Workload Identity and the cluster integrations. Read the project, region, and every
resource prefix, then drop `--dry-run` to execute. Every step is idempotent, so a re-run after a partial failure never
produces duplicates.

The CLI waits for each control-plane write at that API's own operation endpoint before starting its dependent step.
Compute VPC operations are global and subnet operations are regional; their REST resources report completion with a
`DONE` status. Service Usage and Artifact Registry use Google long-running operations and report a true `done` flag. A
newly enabled Compute API can still return `SERVICE_DISABLED` for a short propagation window after its Service Usage
operation completes. The VPC step recognizes only that exact `compute.googleapis.com` response, prints the bounded retry
count, and retries the idempotent insert for up to two minutes. Other `403` responses fail immediately because they may
be real IAM or organization-policy problems. If a terminal or network interruption stops setup after GCP accepted a
write, run the exact same deployment command again. Existing resources return idempotent conflicts. The pipeline
generates no credential of its own, so a run — first or tenth — prints no secret for you to record. The store is
SurrealDB, and its endpoint and root credentials come from your store provider.

Live setup prints fifteen secret-free stages, including the project and exact resource name:

```text
gcp setup [neon-law-stg] 01/15 enable required APIs
gcp setup [neon-law-stg] 04/15 private assets bucket neon-law-stg-assets
gcp setup [neon-law-stg] 08/15 private applications bucket neon-law-stg-applications
gcp setup [neon-law-stg] 11/15 runtime identities and IAM bindings (neon-law-stg-web)
gcp setup [neon-law-stg] 12/15 container registry access
gcp setup [neon-law-stg] 13/15 GKE Autopilot cluster neon-law-stg
gcp setup [neon-law-stg] 14/15 Kubernetes workload identity and cluster integrations
gcp setup [neon-law-stg] 15/15 projects/neon-law-stg/locations/us-west4/keyRings/navigator-secrets/cryptoKeys/deployment-config
gcp setup [neon-law-stg] COMPLETE
```

Long-running REST writes print the service, operation ID, scoped polling path, and `DONE`. Output never includes the
store URL, access token, service-account key, or any other secret value, and the pipeline generates no credential that
would need printing.

On a newly activated project, one of these additional lines can appear between stages 1 and 2, depending on whether the
activation window meets the VPC insert or its operation poll:

```text
gcp api [compute.googleapis.com] activation is still propagating for neon-law-prod; retrying VPC neon-law-prod-vpc (1/25)
gcp operation [Compute] operation-123: compute.googleapis.com activation is still propagating; retrying poll
```

### Private assets and domain restricted sharing

Every deployment bucket remains private. `navigator ops gcp setup` grants the deployment Google service account
`roles/storage.objectAdmin` on its assets, documents, exports, logs, and applications buckets, then maps both Kubernetes
service accounts (`navigator-web` and `workflows-service`) to that Google identity through Workload Identity. It never
adds `allUsers` and never changes `constraints/iam.allowedPolicyMemberDomains`. The applications bucket holds each
Project's published client-portal bundle at `{project-code}/portal/`, which `web` streams same-origin through
`/app/projects/{code}/portal` so the session and Project-participation gate stay on every request.

The public website receives only marketing bytes through `GET /assets/*`. `web` reads that object from
`NAVIGATOR_ASSETS_BUCKET` with the deployment identity and returns it with its stored content type,
`X-Content-Type-Options: nosniff`, and a one-hour public cache. Unsafe keys and missing objects return `404`; storage
failures return `502`. There is no parallel anonymous route for documents, exports, or logs. This is an application
delivery boundary, not a Navigator `persons.role` or Project-participation grant.

Set `NAVIGATOR_ASSET_BASE_URL=$NAV_BASE_URL/assets` in each deployment's `config.toml` — it is a plaintext coordinate
the `ops ship` preflight requires. Setup also gives the active `gcloud` identity the same bucket-scoped object CRUD role
so the operator and the Kubernetes runtime can inspect and repair objects without a public or project-wide grant. After
setup, verify both identities and reject an anonymous principal:

```bash
runtime_gsa="$NAVIGATOR_GCP_SERVICE_ACCOUNT_ID@$NAVIGATOR_GCP_PROJECT_ID.iam.gserviceaccount.com"
operator_account="$(gcloud config get-value account)"
case "$operator_account" in
  *.gserviceaccount.com) operator_member="serviceAccount:$operator_account" ;;
  *) operator_member="user:$operator_account" ;;
esac

for bucket_name in \
  "$NAVIGATOR_ASSETS_BUCKET" \
  "$NAVIGATOR_DOCUMENTS_BUCKET" \
  "$NAVIGATOR_EXPORTS_BUCKET" \
  "$NAVIGATOR_LOGS_BUCKET"
do
  iam_rows="$(
    gcloud storage buckets get-iam-policy "gs://$bucket_name" \
      --flatten='bindings[].members[]' \
      --format='value(bindings.role,bindings.members)'
  )"
  printf '%s\n' "$iam_rows" |
    awk -v runtime="serviceAccount:$runtime_gsa" -v operator="$operator_member" '
      $1 == "roles/storage.objectAdmin" && $2 == runtime { runtime_ok = 1 }
      $1 == "roles/storage.objectAdmin" && $2 == operator { operator_ok = 1 }
      $2 == "allUsers" { public = 1 }
      END { if (!runtime_ok || !operator_ok || public) exit 1 }
    '
done
```

---

No output and a zero exit status prove that both principals have `roles/storage.objectAdmin` and `allUsers` is absent.
The dogfood driver adds the active operator binding when it is absent; the Rust provisioner owns the runtime binding.

---

Pause on the trust boundary: the organization policy correctly rejects `allUsers`, so Navigator never solves asset
delivery by making a bucket public. Walk the verification output for both named principals, then contrast the one public
`/assets/*` application route with the absence of anonymous document, export, or log routes. The important result is
private object CRUD for the operator and workload identity, not a project-wide storage role.

---

The certificates are the part worth dwelling on. A classic Google-managed certificate is validated by a CA *calling the
load balancer*, so DNS has to point there first — moving a live hostname takes it down for the length of issuance, and
Cloud CDN and the HTTP-to-HTTPS redirect both sit in the validation path. Certificate Manager with a **DNS
authorization** avoids that: Google returns a `CNAME`, the CA reads that record, and the certificate reaches `ACTIVE`
while the hostname still serves its current site. Publish the `CNAME` first, wait for `ACTIVE`, then move the `A` record
— a cutover done that way carries no TLS gap. The GKE ingress here owns its own certificates, so it is the classic path
above rather than this one.

---

Name the collision before someone discovers it. A hostname serves one thing, and the matrix on the next slide gives
`www.neonlaw.com` to the `neon-law-prod` deployment — its `NAVIGATOR_PUBLIC_HOST`, certificate, and authorized OAuth
redirect URI are all issued for that name. A marketing site held it first, and the conflict was settled by retiring that
site rather than rehoming it: the load balancer and certificate chain are gone, and only the bucket remains, as an
archive nothing routes to. That is the cost worth naming — a static site is cheap to publish and awkward to unpublish,
because the hostname is the part two things want.

`www.neonlaw.com` was the same collision, and it has since been settled the same way: the firm's Navigator deployment
now holds that exact name and serves it, and the marketing site no longer routes there. `neonlaw.com` redirects to it
and serves nothing itself.

### The Navigator deployment matrix

One `deployments/<name>/` directory is one deployment. Its `config.toml` supplies the project, region, cluster,
namespace, domain, and immutable brand image, and its `secrets.enc.yaml` the mail and provider credentials, for exactly
one site; it never shares a store database, bucket, Kubernetes namespace, or runtime Secret with another row.

| Deployment | GCP project | Site | Image | Resource prefix |
| --- | --- | --- | --- | --- |
| `neon-law-stg` | `neon-law-stg` | `staging.neonlaw.com` | `neon-server` | `neon-law-stg` |
| `neon-law-prod` | `neon-law-prod` | `www.neonlaw.com` | `neon-server` | `neon-law-prod` |
| `neon-law-prod` | `neon-law` | `www.neonlaw.com` | `neon-server` | `neon-law-prod` |

---

The topology is three runtime projects and the deployments checked into `deployments/` — `neon-law-stg`,
`neon-law-prod`, and `neon-law-prod`, each carrying its own `config.toml` and `secrets.enc.yaml`, and each rolled by the
release run. `neon-law-prod` is provisioned in `neon-law-prod` and live: `www.neonlaw.com` resolves to its gateway IP
and is served by its Ingress, so all three rows now serve their own public host. `neon-law-stg` is the only persistent
staging lane; it exercises sample matters through the same shared application, data-access, authorization, API, and
agent-protocol code used by every production brand. The image hub is a fourth project, `ghcr`, and is not an
environment.

---

Read the matrix by row. The `neon-law-prod` row, running `neon-server`, is the firm's **primary website**: the
deployment our public traffic is for. Its hostname is configuration, not code — `NAVIGATOR_PUBLIC_HOST` in the row's
`config.toml` — so the display brand and public URL can change any time without renaming a crate, an image, or a
deployment. Every production brand keeps an isolated resource prefix, database, cluster, namespace, runtime identity,
Secret, and Restate journal. Staging proves the common application once; brand route tests prove the public face shipped
by each immutable image.

### The `/app` mount and HTTP route ownership

`/app` is the container filesystem mount, not a public URL prefix. The images install their executable and common
runtime material there:

| Image | Entrypoint | Shared mounted material |
| --- | --- | --- |
| `neon-server` | `/app/neon` | `/app/public`, `/app/content`, `/app/templates` |
| `neon-server` | `/app/neon` | `/app/public`, `/app/content`, `/app/templates` |

Each executable supplies only its public brand routes to `portal::bootstrap`. The mounted application crate always owns
these HTTP paths and every descendant:

- operational and public ingress: `/health`, `/readyz`, `/version`, `/assets/*`, `/webhook/*`, `/docusign/*`,
  `/public/*`, and `/dioxus-demo`;
- application and control surfaces: `/app/*`, `/app/api/*`, `/auth/*`, `/mcp/*`, and `/docs/*`;
- API documentation: `/app/api` and `/app/api/openapi.json`.

The brand-owned public routes are:

- Neon Law: `/`, `/contact`, `/team`, `/team/{slug}`, `/blog`, `/blog/{slug}`, `/notations`, `/workshops/*`,
  `/presentations/*`, `/privacy`, `/terms`, `/robots.txt`, `/sitemap.xml`, and `/llms.txt`.

This precedence is fail-closed, not merge order. Each brand declares every route it mounts; startup returns an error if
an exact path or descendant overlaps a Navigator-owned prefix. A brand therefore cannot shadow data access,
authorization, control, API, health, or protocol routes.

---

Keep the two mount concepts separate while presenting: `/app` is the shared container filesystem root, while the HTTP
router mounts a brand's bounded public face around the Navigator-owned application and control surfaces. The declared
path inventory makes that boundary executable at startup.

### `neon` — the whole brand seam

A brand crate is exactly one value: the `portal::hosting::Brand` each compiles — its key, its telemetry service name,
and the bounded public routes listed on the previous slide. Everything else is the shared application they mount, and
the portal renders under the mounting brand's chrome, so branding the binary brands the portal too: one brand seam
carries the public site, the signed-in portal, and the telemetry identity together.

That thinness is the customization story. A custom Navigator changes minimal surface area: write your own brand crate in
the `neon` shape — a `Brand` value and a call to the shared run loop, nothing more — build it into your own
`<brand>-server` image, and interact with the remaining published images (`navigator-workflows-service`,
`navigator-gateway`, and the trigger images) unchanged. The two brand Containerfiles are deliberately identical modulo
the brand name, so the image recipe for a new brand is the existing one with your crate's name.

---

Two tests keep this slide honest: the thin-crate rule fails any brand crate that reaches past `portal`, `views`, and
`telemetry` into the domain crates, and the brand-image parity test fails a Containerfile edit applied to one brand
recipe and not the other. To rebrand a _deployment_ without writing a crate at all, use the white-label brand bundle
covered later in this workshop.

### Live rollout checkpoint

As of 31 July 2026, the two production substrates have completed every setup stage, and `neon-law-stg` is a created but
unprovisioned project:

- Both production GKE Autopilot clusters are `RUNNING` in `us-west4`, and no deployment reads Postgres: ENG-22 moved the
  store to SurrealDB, `/health` pings SurrealDB, and `ops gcp setup` provisions no Cloud SQL instance, so a deleted one
  is not recreated on the next run. An operator must export the two legacy production Postgres 15 instances to each
  deployment's own exports bucket and then delete them. Nothing has ever archived those instances — the nightly archive
  lane covers the SurrealDB tables only — so the export is what makes the deletion reversible, and it is not optional.
- `neon-law-stg` and `ghcr` exist in the `neonlaw.com` organization and are linked to billing, but neither has been
  through its provisioner yet. Run `ops gcp hub setup` against the hub first, then `ops gcp setup` against staging — the
  environment provisioner grants against the hub repository, so the hub must exist before any environment names it.
- Each provisioned row has five private assets, documents, exports, logs, and applications buckets.
- On those rows, both the deployment runtime identity and the active operator have bucket-scoped
  `roles/storage.objectAdmin`; none of the buckets grants `allUsers`.

The five-bucket list is the current checkpoint, not the target topology. Issue
[#1103](https://github.com/neon-law-source-code/navigator/issues/1103) coordinates the migration to exactly one private
object-storage bucket per deployment and never one bucket per Project. Project growth adds rows and logical key space,
with lanes such as `{project-code}/documents/` and `{project-code}/exports/` inside the deployment bucket; it does not
create cloud buckets. Marketing bytes remain available through the same-origin `/assets/*` application route while the
bucket itself stays private and does not grant `allUsers`. Until that atomic migration lands, setup reconciles the five
existing bucket resources recorded above.

New clusters are created with `--enable-fleet`; the subsequent idempotent reconciliation can therefore report `Changing
existing fleet membership is not supported`. Navigator treats only that exact response as already reconciled and
continues; unrelated Fleet errors still stop setup.

Infrastructure is not deployment. The two production clusters currently have no application namespace, Deployment,
Service, Gateway, or HTTPRoute, so their public hosts do not yet answer TLS. An operator must ship one immutable release
to Neon production and Neon Law production, then prove `/readyz`, `/version`, certificate readiness, Restate
registration, and the browser surface before posting the `#navigator` handoff.

For a prefix `<name>`, set `NAVIGATOR_GKE_CLUSTER_NAME=<name>`, `NAVIGATOR_GKE_CONTEXT=gke_<project>_<region>_<name>`,
`NAVIGATOR_K8S_NAMESPACE=<name>`, `NAVIGATOR_VPC_NAME=<name>-vpc`, `NAVIGATOR_SUBNETWORK_NAME=<name>-subnet`,
`NAVIGATOR_GATEWAY_IP_NAME=<name>-gateway-ip`, `NAVIGATOR_ASSETS_BUCKET=<name>-assets`,
`NAVIGATOR_DOCUMENTS_BUCKET=<name>-documents`, `NAVIGATOR_EXPORTS_BUCKET=<name>-exports`,
`NAVIGATOR_LOGS_BUCKET=<name>-logs`, `NAVIGATOR_APPLICATIONS_BUCKET=<name>-applications`,
`NAVIGATOR_GCP_SERVICE_ACCOUNT_ID=<name>-web`, `NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID=<name>-drive`, and
`NAVIGATOR_WEB_SECRET_NAME=<name>-web-secrets`. Bucket names are global; add one stable organization prefix if a short
bucket name is already taken.

Also set `NAVIGATOR_GCP_PROJECT_ID`, `NAVIGATOR_GCP_LOCATION`, `NAVIGATOR_PUBLIC_HOST`, `NAVIGATOR_WORKFLOWS_HOST`,
`NAV_BASE_URL=https://$NAVIGATOR_PUBLIC_HOST`, `NAVIGATOR_WORKFLOWS_URL=https://$NAVIGATOR_WORKFLOWS_HOST/`,
`NAVIGATOR_WEB_IMAGE`, and `NAVIGATOR_ASSET_BASE_URL=$NAV_BASE_URL/assets` in that deployment's
`deployments/<name>/config.toml`. Set that row's own `NAVIGATOR_SURREAL_*` coordinates and credentials in its
`secrets.enc.yaml`.

Keep the mail rail complete in every deployment's `secrets.enc.yaml`: `SENDGRID_API_KEY`, `SENDGRID_FROM_EMAIL`,
`SENDGRID_INBOUND_SECRET`, `SENDGRID_EVENTS_SECRET`, and `SENDGRID_EVENTS_PUBLIC_KEY`; staging uses non-production
SendGrid credentials and production uses live credentials authenticated for its domain.

Provision each row with the same region-agnostic command — one run per `deployments/` directory, its coordinates
exported from that directory's `config.toml` (populate the directory first; the config is the source of every coordinate
the provisioner reads, so a row is provisioned only once its directory exists). Each run creates five buckets and one
Autopilot cluster; re-running reconciles only that row:

```bash
set -a; eval "$(grep ' = "' deployments/neon-law-stg/config.toml | sed 's/ = /=/')"; set +a
navigator ops gcp setup --dry-run
```

Check every printed project, region, resource prefix, and the two API batches. Provision the registry hub once before
the runtime stacks; it runs no workloads:

```bash
navigator ops gcp hub setup \
  --project-id ghcr --region us-west4 --dry-run
navigator ops gcp hub setup \
  --project-id ghcr --region us-west4
```

Then apply the already-reviewed plans one deployment at a time. Keep staging first, and do not begin production until
its command finishes:

```bash
set -a; eval "$(grep ' = "' deployments/neon-law-stg/config.toml | sed 's/ = /=/')"; set +a
navigator ops gcp setup
```

That is the live resource boundary, per deployment: five buckets, one VPC/subnet pair, one reserved gateway address, one
Autopilot cluster, one KMS key, and two deployment service accounts. The command also enables the twenty-two required
APIs in that runtime project. A completed command is not yet a deployed website; `ops ship`, DNS, and the browser checks
later in this workshop remain required.

Setup provisions no database and generates no credential, so there is nothing printed to record. The store's own
credentials go into `deployments/<name>/secrets.enc.yaml` (`sops set` — the plaintext never touches disk), then
`navigator ops secrets apply --deployment <name>`; rotation happens at the provider first, per
[`docs/deployment-secrets.md`](/docs/deployment-secrets).

Resolve every reserved address and record the public value as `NAVIGATOR_GATEWAY_IP` in the matching `config.toml` — an
IP is a public coordinate, so it is edited and committed like any other:

```bash
for name in $(ls deployments); do
  set -a; eval "$(grep ' = "' "deployments/${name}/config.toml" | sed 's/ = /=/')"; set +a
  echo "${name}:"
  gcloud compute addresses describe "$NAVIGATOR_GATEWAY_IP_NAME" \
    --global --project "$NAVIGATOR_GCP_PROJECT_ID" --format 'value(address)'
done
```

Populate the matching local Kubernetes context before shipping a row:

```bash
set -a; eval "$(grep ' = "' deployments/neon-law-stg/config.toml | sed 's/ = /=/')"; set +a
gcloud container clusters get-credentials \
  "$NAVIGATOR_GKE_CLUSTER_NAME" --region "$NAVIGATOR_GCP_LOCATION" \
  --project "$NAVIGATOR_GCP_PROJECT_ID"
```

The resulting context name must equal that config's `NAVIGATOR_GKE_CONTEXT`. Repeat once for each deployment you are
operating.

The [canonical matrix](/docs/environments) is the compact reference for every row.

---

Pause on the isolation boundary: staging proves the shared mounted application, while the production data planes stay
isolated. Point to the resource prefix column and verify that every database, bucket set, cluster, namespace, service
account, Secret, and gateway address carries the row's prefix. Then demonstrate the dry run before discussing any
billable live command.

### Set one site to one version

The version is a release tag, never `latest`. To set one site, name its deployment with the required `--deployment` flag
and replace `YY.M.D` with the published tag — every coordinate comes from `deployments/<name>/config.toml`, never the
shell. The command preflights that the selected brand image and worker image exist, checks the Secret keys before
changing Kubernetes, then records the tag in the deployment.

```bash
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D

curl --fail --show-error https://www.neonlaw.com/version
```

The `#navigator` hand-off derives its exact command list from the `deployments/` tree — one run per directory, staging
first, then the two production rows:

```bash
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
```

Staging goes first. Do not start either production row until staging `/readyz` and `/version` checks pass; the two
production rows gate on staging and not on each other, which is why the release run rolls them in parallel. To preview
without changing one site, append `--dry-run`; to refresh its pods after a secret rotation without changing the version,
use `--restart-only`. A deployment can be named here only once it has a `deployments/<name>/` directory; that directory
is what puts it in this list.

---

Demonstrate the single-deployment control surface with `neon-law-stg`: `--deployment` selects the entire stack and the
release tag selects the version. Emphasize that an operator verifies `/readyz` and `/version` before moving to the next
row; by hand the command block is a sequence, not a request to roll every cluster concurrently. The release workflow is
what rolls the two production rows at once, and only because each has already been gated on a green staging roll.

## Provision the Infrastructure

### The APIs that light up

A real run first enables twenty-two Google Cloud APIs in bounded [Service
Usage](https://cloud.google.com/service-usage/docs/enable-disable) `batchEnable` calls: `compute`, `servicenetworking`,
`storage`, `iam`, `iamcredentials`, `sts`, `cloudresourcemanager`, `artifactregistry`, `container`, `gkehub`,
`gkebackup`, `anthosconfigmanagement`, `logging`, `monitoring`, `cloudtrace`, `secretmanager`, `certificatemanager`,
`identitytoolkit`, `speech`, `drive`, `admin`, and `cloudkms`.

---

`batchEnable` completes as a long-running operation. Enabling an already-enabled API is a no-op, so this step — like
every step — is safe to repeat. Service Usage accepts twenty services per request, so Navigator sends two bounded
batches. Nothing else in the run works until these are on, which is why it goes first. Google can briefly report the
Compute API as disabled at its own endpoint after the enable operation is done; the following VPC step waits through
that narrowly identified propagation case instead of making the operator restart the whole provisioning run.

### Network and five buckets

With the APIs on, the CLI provisions the data plane and runtime identity:

- A **custom-mode VPC** with one explicitly named regional subnet and private Google access. The deployment's GKE
  cluster is pinned to both names.
- **Five private Cloud Storage buckets**, all uniform bucket-level access: `-assets` (marketing objects served through
  the same-origin `/assets/*` application route), `-documents` (client documents), `-exports` (Parquet/Iceberg
  archives), `-logs` (the Nearline log-sink destination), and `-applications` (each Project's published client-portal
  bundle, streamed same-origin through `/app/projects/{code}/portal`).
- A **deployment-specific Google service account** with the Secret Manager accessor role, object access on only that
  deployment's five buckets, Workload Identity bindings for the namespace's `navigator-web` and `workflows-service`
  Kubernetes service accounts, and permission to sign its own GCS URLs.
- A separate **Workspace Drive service account** with no runtime GCP roles. An operator creates one JSON key, records it
  in that deployment's `secrets.enc.yaml`, and grants its OAuth client domain-wide delegation in the selected Workspace.

---

No stage generates a credential, so a run prints no secret and there is nothing to record. The five private
[buckets](https://cloud.google.com/storage/docs/creating-buckets) are: `your-project-id-assets` — marketing photography
read by `web` and delivered through `/assets/*`; `your-project-id-documents` — client documents, where `web` writes
content-addressed blobs, kept separate from assets; `your-project-id-exports` — nightly Parquet/Iceberg snapshots
written by `workflows-service`; `your-project-id-logs` — the Nearline destination for the deployment's Cloud Logging
sink; and `your-project-id-applications` — each Project's published client-portal bundle, streamed same-origin through
`/app/projects/{code}/portal`. Setup creates the bucket but not the operator-managed sink; complete the per-deployment
sink recipe in [`docs/gke-prod.md`](/docs/gke-prod). Clients do not get GCS IAM roles or bucket URLs: they see documents
through the portal after Navigator checks their session and Project participation row. Every create call treats an HTTP
**409 Conflict** as success — that is Google's "already exists" response — which is exactly what makes re-running setup
safe rather than destructive.

### How a Project portal reaches a client

Each Project has its own source repository — `neon-law/acme`, say — holding a React application under `portal/`. That
bundle is never committed anywhere in Navigator, and it never touches git on the way to a client. It is built in the
Project repository's own CI, published to the deployment's private `-applications` bucket, and streamed from there by
`web` — same-origin, and only after the session and Project participation row are checked.

```mermaid
flowchart LR
  subgraph repo["Project repo — neon-law/acme"]
    src["portal/ — React + Vite"]
    ci["CI on push to main:<br/>validate + application-publish"]
    src --> ci
  end
  subgraph gcp["Deployment project — neon-law"]
    bucket[("<deployment>-applications<br/>acme/portal/ — private, UBLA")]
  end
  subgraph nav["Navigator — neon-server"]
    web["web streams the bundle at<br/>/app/projects/acme/portal/"]
  end
  client(["Client browser"])

  ci -- "keyless WIF (navigator-app-publisher):<br/>upload dist/ — hashed assets first,<br/>index.html last, never delete" --> bucket
  client -- "GET /app/projects/acme/portal/" --> web
  web -- "check session + participation" --> web
  web -- "stream objects, same-origin" --> bucket
```

---

The **repository name is the Project code**, so the Vite base baked at build time (`/app/projects/acme/portal/`) and the
object prefix in the bucket (`acme/portal/`) both derive from it — nothing declares its own name. Publish order is
load-bearing: hashed assets are uploaded first and `index.html` last, and nothing is ever deleted, so a client mid-load
never resolves an asset the running `index.html` has not published yet. The 30-day
[lifecycle](#network-and-five-buckets) reaches only assets a later build orphaned, because every publish rewrites the
live set unconditionally.

The publisher is a per-deployment `navigator-app-publisher` service account reached by keyless Workload Identity, so no
key leaves GCP. It holds a custom role carrying exactly `storage.objects.create`, `storage.objects.get` and
`storage.objects.update` — `objectCreator` is create-only and refuses every republish. The applications bucket is shared
across Projects, so the bucket is not the boundary: that grant carries an IAM condition naming this Project's own
`acme/portal/` prefix, and it is the condition that stops a sibling Project's repo publishing over this one. Serving is
the mirror of that trust: `web` holds `objectAdmin` on the bucket but streams the bytes itself rather than handing out a
signed URL, so a bundle's own `/app/api` reads carry the same session cookie and stay inside the participation gate.

### One matter's document never backs another matter's

Inside the documents bucket, two matters never share an object:

- Content addressing dedupes **within a matter**, never across matters. A governed expunge deletes an object only when
  no asset row on another matter still points at it, and says so in the log when it declines.

---

An object key in the documents bucket is `blobs/<sha256>` — the address _is_ the content hash, so filing the same bytes
twice costs one object instead of two. That is the right instinct for storage and the wrong one for a law firm, because
the identical PDF is exactly the case you should expect: a blank government form, a recorded deed, a protective order,
an exhibit filed on two related matters. Dedup on the hash alone means one object backs two clients' documents, and the
matters are now coupled through a bucket key that nothing in either file mentions.

That coupling only shows its teeth on deletion. A governed expunge — a privilege clawback, a sealing order, a lawful
deletion request — rewrites the matter's repository history and deletes the document's bytes. If the object were shared,
deleting it on the sealed matter would also empty an unrelated client's file, on the authority of an order that never
named them. If that second matter were under a preservation duty, the firm would have destroyed evidence in a case that
had nothing to do with the one it was acting on, and it would have no record of doing so.

So dedup is scoped to the matter: `store::documents::ingest_bytes` reuses an object only when an asset row **on the same
project** already points at it. Two matters holding the same exhibit get two objects. The storage cost is a rounding
error against a single spoliation finding.

Scoping ingest fixes what gets written from here on; it does not unshare what was already written. So expunge carries
the matching guard: before deleting a key it asks whether an asset row on another matter still references it, and if one
does it keeps the object and emits a warning naming the key. That is deliberately loud rather than silent — an order
that requires the bytes actually destroyed now needs a person to reach the other matter and deal with it there, which is
a decision for a lawyer and not for a delete loop. An asset row carrying no project at all does not block the deletion:
it is unattached rather than another matter's, and letting a stray row veto a privilege clawback would defeat the one
thing this primitive exists to do.

Together the two rules give you the property that matters when you are the one answering for the file: no matter's
documents are held hostage to another matter's, and no order deletes bytes it did not name.

When multiple deployments share a project, pass their explicit bucket, SQL, VPC, subnet, cluster, namespace, gateway-IP,
and service-account variables. The single-project `<project>-suffix` defaults remain convenient for an independent OSS
install, but the three-row Navigator matrix never relies on them.

### A private image registry

With storage in place, the provisioner creates the one private [Artifact
Registry](https://cloud.google.com/artifact-registry/docs) repository that every navigator container image lives in, and
the identities that push to and pull from it:

- A **Docker-format repository** (default `navigator`) at `your-region-docker.pkg.dev/your-project-id/navigator`.
- A **keep-the-last-10-versions cleanup policy** — a `KEEP` rule retaining the last 10 versions of each image plus a
  `DELETE` rule for everything else. Retention is a count rather than an age on purpose: an age-based rule is only safe
  while releases outrun it, and releases are tag-driven, so a quiet fortnight under the old 7-day rule would have let
  the registry delete the versions production was running. A count cannot expire. Keep policies take precedence over
  delete policies, which is what makes the pair mean "keep ten, delete the rest" — the delete half matches every
  version, so it is never applied alone.
- A **CI push identity** (`navigator-ci-pusher` service account) with a repo-scoped `roles/artifactregistry.writer`
  binding, plus a **GitHub Workload Identity federation** pool and provider so CI authenticates keyless — no downloaded
  service-account key — pinned to this one repository and to the refs allowed to publish.
- A repo-scoped `roles/artifactregistry.reader` binding for the GKE Autopilot node identity, so the cluster can pull.

Two values decide whether that federation works at all, and both are easy to get wrong in a way nothing reports:

```text
issuerUri           https://token.actions.githubusercontent.com
                    (on your own GHE tenant: https://token.actions.<your-tenant>.ghe.com)
attributeCondition  assertion.repository == '<owner>/<repo>'
                        && (assertion.ref == 'refs/heads/main' || assertion.ref.startsWith('refs/tags/'))
```

Navigator itself runs on github.com, so the public issuer is the one this workshop's own deployment uses. The tenant
issuer above is for readers who do not.

---

The registry is **private**: only principals inside this project may pull, which is why the cluster's node service
account gets an explicit reader binding rather than relying on a public image. The repository create is a long-running
operation that treats a **409 Conflict** as already-exists, and the cleanup policy is a PATCH; both are safe to
re-apply, keeping this step as idempotent as the rest.

Push access is keyless, and the two values above are where a day disappears. **A GitHub Enterprise data-residency tenant
issues its own OIDC tokens.** If your repository lives on `<tenant>.ghe.com`, the pool must trust that tenant's issuer;
every public federation tutorial assumes github.com, and a pool pinned to `token.actions.githubusercontent.com` is
accepted at create time, reports `ACTIVE`, and then fails every single token exchange. Nothing warns you, because
nothing is wrong until a workflow asks for a token. Read the issuer off the tenant instead of copying one:

```bash
curl https://token.actions.<your-tenant>.ghe.com/.well-known/openid-configuration
```

The condition pins the **full `owner/repo`**, not the owner alone. Owner-scoping sounds equivalent and is not: it admits
every repository in the org, so any one of them can mint a token that pushes your production images. Pinning the whole
slug also refuses a fork for free, because a fork carries its own `repository` claim. The ref clause is the second half
— only `main` and release tags may publish, so an arbitrary branch is refused too.

Both refusals happen at the token exchange, which is the property worth internalising: a run that should not publish
fails at the **authenticate** step and never reaches the push. When you verify this, check _which step_ failed. A run
that fails at `docker push` instead means the condition let the token through and the IAM binding caught it — the right
outcome by luck rather than by design, and a condition still to fix.

One last trap, and the reason the provisioner is worth re-running rather than hand-fixing: an `ensure` that only ever
_creates_ cannot repair any of this. A provider built against the wrong issuer answers the create with 409, so a
create-only provisioner reports "already exists" over a resource that can never authenticate. `ops gcp hub setup`
converges instead — it PATCHes the existing provider onto the current issuer and condition, and it replaces the
impersonation binding rather than appending to it, so a principal left behind by an org rename actually loses access. An
additive binding can never revoke, and a rename is exactly when you need it to.

### The cluster comes up

The cluster is the one part driven through `gcloud` rather than REST. In order, the provisioner reserves a static IP,
creates the [GKE Autopilot](https://cloud.google.com/kubernetes-engine/docs/concepts/autopilot-overview) cluster, and
registers it as a Fleet member:

```bash
cargo run -p cli -- ops gcp setup --project-id your-project-id --region us-west4
```

---

The Container API spec is roughly two hundred lines of JSON, while the Autopilot one-liner does the same job with sound
defaults. The provisioner reserves a global static IP for the Gateway (so your DNS A record survives a cluster
re-create), creates the Autopilot cluster on the `rapid` release channel with the Secret Manager add-on, then registers
the cluster as a Fleet member. If you point `--config-sync-repo` at your fork, it also applies a [Config
Sync](https://cloud.google.com/kubernetes-engine/enterprise/config-sync/docs/overview) `RootSync` so the cluster pulls
its manifests from Git. All three Navigator configs omit that flag: deployment-rendered `navigator ops ship` is their
sole manifest owner, so a `RootSync` cannot revert one site's environment-specific render.

## Environment Matrix

### Three operating modes, two deployment profiles

| Operating mode | Selector | Runtime and data posture |
| --- | --- | --- |
| **Test** | `dev` + `NAVIGATOR_CI_HARNESS=1` | Schemas/KIND; sample matters; canonical seed + test fixtures |
| **Dev** | `dev`; harness normally unset | KIND/cloud namespace; sandbox vendors; canonical seed + sample matters |
| **Production** | `production`, empty, or unset | Hosted services; production vendors; canonical seed + live data |

---

Navigator has three operating modes but only two parsed deployment profiles. “Dev” is the local KIND lane, including its
disposable `navigator dev staging` integration surface; the wire value is `dev`. All three managed GKE rows use
`production`, even when a config, namespace, or hostname ends in `-staging`. “Test” is the `dev` profile with
`NAVIGATOR_CI_HARNESS=1`, not a third parsed value. Exact means exact: `staging`, `test`, `Dev`, `Production`, or any
value with surrounding whitespace is an error. Production is selected by exact `production`, empty, or unset, so an
absent selector is production-safe.

The `-staging` suffix remains a deployment identity and release ring in `deployments/` directory names, Kubernetes
namespaces, data planes, and public hosts. It never becomes a weaker runtime profile. Set both `NAVIGATOR_ENVIRONMENT`
and `NAVIGATOR_CREDENTIAL_ENVIRONMENT` to `production` in `deployments/neon-law-stg/config.toml`; the runtime also
fences provider endpoints, so the proving ring keeps production boot checks while its isolated data plane holds sample
matters.

The data rule is small but not flat: every boot applies the same embedded, environment-blind canonical seed —
jurisdictions above all, the reference table every entity and licensure record points at — and a boot carrying sample
matters additionally and idempotently applies the compiled fixture (three synthetic matters, their participants, and the
rows used by the portal walkthrough). Tests may add rows inside their isolated schemas.

### The deployment that says its matters are sample

`NAVIGATOR_SIMULATED_MATTERS` is the second selector, and it answers a different question from the first. The profile
above decides which runtime wiring a boot gets; this decides whether the matters in front of a visitor are invented.

Left unset or empty it follows the profile: a `dev` boot carries sample matters because that is all a `dev` boot has,
and a `production` boot does not, because production is where the real files are. Exactly `true` or `false` overrides
that in both directions, and every other value — `TRUE`, `1`, `yes`, a case or whitespace variant — is rejected rather
than resolved to the permissive answer. That exactness is the point: a typo that quietly read as `true` would seed
invented clients into a database of real ones.

The override that matters is `true` under a `production` profile, which is exactly what `neon-law-stg` is. Both of its
selectors are `production` on purpose, so nothing in the running process can tell it apart from the row holding real
client files. It therefore says so itself, in `deployments/neon-law-stg/config.toml`:

```toml
NAVIGATOR_SIMULATED_MATTERS = "true"
```

Two things follow from that value. `store::seed` applies the sample-matter fixture, so the row carries
`sample-litigation`, `sample-transactional`, and `sample-estate` rather than an empty portfolio. And every page
publishes a site-wide banner saying the matters are sample — which is why `staging.neonlaw.com` is a link worth handing
to somebody, because a demo matter cannot be mistaken for a client's file.

It is a **coordinate, not a credential**, so it lives in `config.toml` beside the buckets and hostnames rather than in
that deployment's `secrets.enc.yaml`. Adding it needs no SOPS re-encryption on either row.

`neon-law-prod` states `"false"` for documentation rather than for behaviour. The code already reads a missing value as
`false` under a `production` profile, and `ops ship` renders `false` into the web env when a deployment's config omits
the key, so the substitution is deliberately optional: a config that never mentions it still renders and still ships. A
key that could halt a production rollout by being deleted would be a worse failure than the one it guards against.

---

Two selectors, two questions. Ask the room which one decides whether a visitor sees invented clients — the answer is not
the one whose name contains "environment". Then point out that staging's runtime profile is `production`, and let the
implication land: nothing in the process can tell staging from production, so staging has to say so itself. That is why
there is a second selector at all rather than a third value on the first one.

The banner is the part worth dwelling on. It is not a developer convenience — it is what makes `staging.neonlaw.com` a
link you can hand to somebody without them mistaking a demo for a client's file.

### Configuration precedence: the first source wins

| Priority | Source | Who owns it | Typical contents |
| --- | --- | --- | --- |
| 1 | **process environment** | shell, Kubernetes, CI | Explicit one-run or deployed values |
| 2 | **`.env`** | developer/operator; gitignored | Optional sandbox credentials and local overrides |
| 3 | **`.devx/env`** | generated by the dev CLI | Local endpoints, ports, harness, session key |
| 4 | code defaults | each typed config loader | Ports, content directories, optional feature fallbacks |

---

Both `web` and `workflows-service` load `.env` before `.devx/env`, and dotenv never overwrites an existing process
value. That makes the precedence `process environment` → `.env` → `.devx/env` → defaults. It is useful for deliberate
sandbox overrides, but it is also why copying `.env.example` wholesale is not the local setup: a blank
`NAVIGATOR_ENVIRONMENT=` in `.env` wins over the generated `dev` value and selects production checks. Start local
development with no `.env`; add only the sandbox values you actually intend to override.

### Local dev controls: inputs read by `navigator dev`

| Concern | Environment variables | Defaults / effect |
| --- | --- | --- |
| Topology | `NAVIGATOR_KIND_CLUSTER`, `NAVIGATOR_K8S_NAMESPACE` | `navigator`, `navigator` |
| Dependency overlay | `NAVIGATOR_KIND_DEPS_OVERLAY` | deps-only KIND |
| Full KIND overlay | `NAVIGATOR_KIND_OVERLAY` | full KIND |
| GKE overlay | `NAVIGATOR_GKE_OVERLAY` | example GKE manifests |
| Private mode gateway | `NAVIGATOR_PRIVATE_MODE` | off; on puts Pingora network + basic auth before `web` |
| Second store port | `NAVIGATOR_KIND_SURREAL_PORT` | `18000` |
| Restate ports | `NAVIGATOR_KIND_RESTATE_INGRESS_PORT`, `NAVIGATOR_KIND_RESTATE_ADMIN_PORT` | `9080`, `9070` |
| Identity port | `NAVIGATOR_KIND_RAUTHY_PORT` | `30080` |
| Storage port | `NAVIGATOR_KIND_GARAGE_S3_PORT` | `30900` |
| Web port | `NAVIGATOR_KIND_WEB_PORT` | `3001` |
| Observability | `NAVIGATOR_KIND_OPENOBSERVE_PORT`, `NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT` | `5080`, `5081` |
| Documents key | `NAVIGATOR_GARAGE_ACCESS_KEY` | deterministic KIND-only default |
| Documents secret | `NAVIGATOR_GARAGE_SECRET_KEY` | deterministic KIND-only default |
| Assets key | `NAVIGATOR_GARAGE_ASSETS_ACCESS_KEY` | deterministic KIND-only default |
| Assets secret | `NAVIGATOR_GARAGE_ASSETS_SECRET_KEY` | deterministic KIND-only default |
| Applications key | `NAVIGATOR_GARAGE_APPLICATIONS_ACCESS_KEY` | deterministic KIND-only default |
| Applications secret | `NAVIGATOR_GARAGE_APPLICATIONS_SECRET_KEY` | deterministic KIND-only default |
| LFS key | `NAVIGATOR_GARAGE_LFS_ACCESS_KEY` | deterministic KIND-only default |
| LFS secret | `NAVIGATOR_GARAGE_LFS_SECRET_KEY` | deterministic KIND-only default |
| Published demo image | `NAVIGATOR_IMAGE_TAG` | latest dated `YY.M.D` tag when `dev deploy` pulls images |

---

These variables configure the CLI that creates or reconnects the local KIND fixture. They are inputs to orchestration,
not the application runtime. Every one is optional; unset or empty values use the defaults shown. Invalid local port
values also fall back to their defaults. The Garage values are development credentials, written into the generated
runtime environment and the KIND secrets; they are never suitable for a public deployment.

### Local runtime: what `.devx/env` generates

| Concern | Generated environment variables |
| --- | --- |
| Profile and listener | `PORT`, `NAVIGATOR_ENVIRONMENT`, `NAVIGATOR_CI_HARNESS` |
| Repository writer | `NAVIGATOR_GIT_REPO_ROOT` |
| Store | `NAVIGATOR_SURREAL_ENDPOINT`, `NAVIGATOR_SURREAL_NAMESPACE`, `NAVIGATOR_SURREAL_DATABASE` |
| Store credentials | `NAVIGATOR_SURREAL_USER`, `NAVIGATOR_SURREAL_PASSWORD` |
| Storage driver | `NAVIGATOR_STORAGE_BACKEND`, `NAVIGATOR_STORAGE_ENDPOINT` |
| Storage buckets | `NAVIGATOR_STORAGE_BUCKET`, `NAVIGATOR_ASSETS_BUCKET`, `NAVIGATOR_LFS_BUCKET` |
| Applications bucket | `NAVIGATOR_APPLICATIONS_BUCKET` |
| Archive buckets | `NAVIGATOR_ICEBERG_BUCKET`, `NAVIGATOR_TELEMETRY_BUCKET` |
| Storage region | `NAVIGATOR_STORAGE_REGION` |
| Documents credentials | `NAVIGATOR_STORAGE_ACCESS_KEY`, `NAVIGATOR_STORAGE_SECRET_KEY` |
| Assets credentials | `NAVIGATOR_ASSETS_ACCESS_KEY`, `NAVIGATOR_ASSETS_SECRET_KEY` |
| Applications credentials | `NAVIGATOR_APPLICATIONS_ACCESS_KEY`, `NAVIGATOR_APPLICATIONS_SECRET_KEY` |
| LFS credentials | `NAVIGATOR_LFS_ACCESS_KEY`, `NAVIGATOR_LFS_SECRET_KEY` |
| Browser OIDC | `OAUTH_ISSUER_URL`, `OAUTH_CLIENT_ID`, `OAUTH_CLIENT_SECRET`, `OAUTH_REDIRECT_URI`, `SESSION_SECRET` |
| Sign in with Microsoft | `OAUTH_MICROSOFT_CLIENT_ID`, `OAUTH_MICROSOFT_CLIENT_SECRET` |
| Microsoft tenant gate | `OAUTH_MICROSOFT_ALLOWED_TENANTS`, `OAUTH_MICROSOFT_ISSUER_URL` |
| Policy and workflows | `RESTATE_BROKER_URL` |
| Attachment scanner | `NAVIGATOR_CLAMD_ADDR` |
| Harness-only integration placeholders | `SENDGRID_API_KEY`, `SENDGRID_INBOUND_SECRET` |
| Harness sink URL | `SENDGRID_BASE_URL`, `NAVIGATOR_SENDGRID_HARNESS_SECRET` |
| Telemetry endpoint and UI | `OTEL_EXPORTER_OTLP_ENDPOINT`, `NAVIGATOR_OPENOBSERVE_URL` |
| Telemetry credentials | `NAVIGATOR_OPENOBSERVE_USERNAME`, `NAVIGATOR_OPENOBSERVE_PASSWORD` |
| Telemetry routing | `NAVIGATOR_OPENOBSERVE_ORGANIZATION`, `NAVIGATOR_OPENOBSERVE_STREAM` |

---

Do not edit `.devx/env`; the next `navigator dev up` overwrites it. The file is a connection descriptor for the
host-side `web`, not a secret store. In the default loop the host `web` and in-cluster worker share the `navigator`
database. A worktree environment can use another database and port, but Restate-backed flows require the worker and web
database coordinates to agree; `navigator dev worktree-env status` reports a mismatch.

### The store: SurrealDB

| Question | SurrealDB |
| --- | --- |
| Owns the data | Yes — every table |
| Local shape | KIND pod, `surreal start memory` |
| Schema | One idempotent `DEFINE` file plus `schema_version` |
| Tests | Embedded `kv-mem` engine per test |
| Deployed | Surreal Cloud |
| Row-level permissions | `PERMISSIONS NONE` on every table, deliberately |

---

Three choices are worth knowing because they change what you operate. **The schema is applied, not migrated:** every
boot runs one idempotent `DEFINE` file and records a version record, so a process notices it is looking at a database
some other build prepared instead of discovering it one confusing query at a time. Backfills are explicit one-shot jobs
rather than migration steps. **The local engine is memory-backed:** its data resets with the pod, deliberately — boot
re-applies the schema and re-runs the canonical seed. **Tests use an engine inside the test process:** no container, no
port, no shared server, so two tests cannot collide and there is nothing to reclaim afterwards.

Per-worktree isolation comes from the cluster boundary, not from anything extra: `dev worktree-env up` already keys a
KIND cluster to the worktree path, so its Surreal pod is private by construction and `worktree-env down` removes it with
the cluster.

### Where SurrealDB authorization lives

| Layer | Decides | During and after the port |
| --- | --- | --- |
| `persons.role` | The tier: owner, admin, lawyer, clerk, client | Unchanged — the sole tier answer |
| `person_project_roles.participation` | Per-matter scope | Unchanged — the sole scope answer |
| Embedded Rego | Renders the decision per request | Compiled at web-process boot |
| Surreal `PERMISSIONS` | Per-row engine enforcement | Explicit `NONE`; every process signs in as root |

---

The schema carries `PERMISSIONS NONE`, so Navigator's authorization lives above the database. SurrealDB does have an
opinion — every table carries a `PERMISSIONS` clause the engine evaluates per row against the authenticated session — so
adopting it forced a decision rather than inheriting one.

The decision is that authorization stays above the database. Every table says `PERMISSIONS NONE` out loud even though
that is also the engine's default, because an omitted clause and a deliberate one are otherwise indistinguishable, and a
test fails if a table ever lands without one. Be precise about what `NONE` buys: it denies every non-root session
outright, and root bypasses it — so the security property comes from the connection, and the clause is a fail-closed
backstop for a session that does not exist yet.

Mirroring the role-and-participation model into per-table `PERMISSIONS FOR select` expressions was the rejected
alternative, and the conflict check is why. That traversal walks from a proposed client across matters the requesting
person has no part in — which is exactly what imputed firm-wide conflict checking under Model Rule 1.10 means — so a
mirrored policy would need a carve-out at its first real query. Two languages that must agree is also the expensive
failure: a rule that drifts is either over-disclosure, a confidentiality breach, or under-disclosure, which in a
conflict check means a _missed_ conflict. Neither shows up as an error.

What reopens it: the first surface that hands query power closer to a user — a folder read daemon, an MCP tool issuing
SurrealQL, a debugging console — does not get the root credential. It lands its own scoped non-root session and settles
that credential in each deployment's `secrets.enc.yaml` at the same time.

### Deployed runtime: core web and worker wiring

| Concern | Environment variables |
| --- | --- |
| Profile fence | `NAVIGATOR_ENVIRONMENT`, `NAVIGATOR_CI_HARNESS`, `NAVIGATOR_CREDENTIAL_ENVIRONMENT` |
| Sample matters | `NAVIGATOR_SIMULATED_MATTERS` |
| HTTP identity | `PORT`, `NAV_BASE_URL`, `CANONICAL_HOST`, `NAVIGATOR_RATE_LIMIT_PER_MIN` |
| Branding and public assets | `NAVIGATOR_CUSTOM_BRANDING`, `NAVIGATOR_ASSET_BASE_URL` |
| Support chat | `NAVIGATOR_CHATWOOT_WEBSITE_TOKEN`, `NAVIGATOR_CHATWOOT_BASE_URL` |
| Store | `NAVIGATOR_SURREAL_ENDPOINT`, `NAVIGATOR_SURREAL_NAMESPACE`, `NAVIGATOR_SURREAL_DATABASE` |
| Storage driver | `NAVIGATOR_STORAGE_BACKEND`, `NAVIGATOR_STORAGE_ENDPOINT`, `NAVIGATOR_STORAGE_REGION` |
| Documents/exports | `NAVIGATOR_DOCUMENTS_BUCKET`, `NAVIGATOR_STORAGE_BUCKET`, `NAVIGATOR_EXPORTS_BUCKET` |
| Other buckets | `NAVIGATOR_ASSETS_BUCKET`, `NAVIGATOR_LFS_BUCKET` |
| Filesystem storage | `NAVIGATOR_STORAGE_FS_ROOT` |
| Entity Workspace Drive | Per-Workspace Drive coordinates listed below |
| Generic S3 key | `NAVIGATOR_STORAGE_ACCESS_KEY`, `NAVIGATOR_STORAGE_SECRET_KEY` |
| Temporary S3 token | `NAVIGATOR_STORAGE_SESSION_TOKEN` |
| Assets S3 key | `NAVIGATOR_ASSETS_ACCESS_KEY`, `NAVIGATOR_ASSETS_SECRET_KEY` |
| LFS S3 key | `NAVIGATOR_LFS_ACCESS_KEY`, `NAVIGATOR_LFS_SECRET_KEY` |
| Exports S3 key | `NAVIGATOR_EXPORTS_ACCESS_KEY`, `NAVIGATOR_EXPORTS_SECRET_KEY` |
| Sessions | `SESSION_SECRET` |
| Restate client | `RESTATE_BROKER_URL`, `RESTATE_AUTH_TOKEN`, `RESTATE_SERVICE` |
| Trigger ingress | `RESTATE_INGRESS_URL` |
| Worker listener | `WORKFLOWS_SERVICE_LISTEN` |
| Telemetry and log filtering | `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `RUST_LOG` |
| Image-baked release identity | `NAVIGATOR_RELEASE_TAG`, `NAVIGATOR_GIT_SHA`, `NAVIGATOR_BUILD_TIME` |

---

These are the process-level coordinates shared by Kubernetes manifests and non-Kubernetes installs. Production requires
GCS and rejects `NAVIGATOR_STORAGE_ENDPOINT`; the dev profile may use disposable storage, while every hosted
deployment's GCS also rejects an emulator endpoint. `SESSION_SECRET` must contain at least 32 bytes. The worker reads
the same database, document bucket, email backend, branding bundle, and deployment profile as web so a journaled step
never crosses environments. Published images bake the release identity in; local source builds leave it `unknown`.

The Neon Law-controlled Workspace service account has domain-wide delegation. Keep its shared Drive coordinate and
credential together: `NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID`, `NAVIGATOR_DRIVE_NEON_LAW_DELEGATED_USER`, and
`NAVIGATOR_DRIVE_NEON_LAW_SERVICE_ACCOUNT_JSON`.

The typed workspace map selects one root by deployment: `neon-law-stg` uses
`NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID` and `neon-law-prod` uses
`NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID`. Each root is distinct; an unknown deployment fails closed
rather than borrowing another root. `NAVIGATOR_PROJECTS_DRIVE_MOUNT` is an optional machine-local override, never a
deployed credential. The regional GCP command enables the Drive and Admin SDK APIs, but a Workspace administrator must
still grant domain-wide delegation and create the selected Drive root.

For each deployment, setup creates the identity named by `NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID` with no runtime GCP
roles. Complete the global Workspace attachment once:

1. Read that service account's OAuth client ID:

   ```bash
   gcloud iam service-accounts describe \
     "$NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID@$NAVIGATOR_GCP_PROJECT_ID.iam.gserviceaccount.com" \
     --project "$NAVIGATOR_GCP_PROJECT_ID" --format 'value(oauth2ClientId)'
   ```

2. In the selected Workspace Admin console, authorize that client ID for `https://www.googleapis.com/auth/drive`. Do not
   grant a broader Admin SDK scope merely because the API is enabled.
3. Create that deployment's otherwise-empty Projects shared drive and record its ID. Staging creates its own drive,
   separate from production's.
4. Create one JSON key for the dedicated Drive account and place the complete JSON value directly into the selected
   deployment's `secrets.enc.yaml` under the `*_SERVICE_ACCOUNT_JSON` key (`sops set` — never an editor buffer for a key
   this size). Never paste it into Slack, a ticket, shell history, or this repository in plaintext; revoke the key
   immediately after its encrypted replacement is proven during rotation.
5. Set the selected `*_PROJECTS_DRIVE_ID` and `*_DELEGATED_USER` coordinates in the same deployment's `config.toml`, run
   `navigator ops secrets apply --deployment <name>`, and verify the key names with its `--dry-run` plan — see
   [`docs/deployment-secrets.md`](/docs/deployment-secrets).

### Deployed runtime: identity and access

| Concern | Environment variables | Behavior when absent |
| --- | --- | --- |
| Browser OIDC issuer | `OAUTH_ISSUER_URL` | Browser login is not configured |
| Browser OIDC client | `OAUTH_CLIENT_ID`, `OAUTH_CLIENT_SECRET` | Browser login is not configured |
| Browser redirect | `OAUTH_REDIRECT_URI` | Browser login is not configured |
| Microsoft door | `OAUTH_MICROSOFT_CLIENT_ID`, `OAUTH_MICROSOFT_CLIENT_SECRET` | Off; one provider, one button |
| Microsoft tenants | `OAUTH_MICROSOFT_ALLOWED_TENANTS` | **Boot fails** when the client id is set |
| Microsoft authority | `OAUTH_MICROSOFT_ISSUER_URL` | The `organizations` authority: work or school, any tenant |
| Bearer JWKS | `OIDC_JWKS_URL`, `OIDC_AUDIENCE`, `OIDC_ISSUER` | Deployed JWT verification |
| Bearer HMAC | `OIDC_HS256_SECRET` | Local/test verifier path |
| Dev bypass | `OIDC_DISABLED` | Off; the dev profile and production reject `true` / `1` |
| Bootstrap Owner | `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` | No identity is JIT-created |
| Protected firm Entity | `NAVIGATOR_BOOTSTRAP_COMPANY` | `Shook Law PLLC` is protected either way |
| Self-signup | `NAVIGATOR_SELF_SIGNUP_ENABLED` | Off; an unknown email is refused (403) |
| Google token policy | `GOOGLE_OAUTH_CLIENT_IDS`, `GOOGLE_OAUTH_REQUIRED_HD` | No client/domain pin |
| Google token endpoint | `GOOGLE_TOKENINFO_URL` | Google default |
| Password door | `NAVIGATOR_IDENTITY_PLATFORM_API_KEY` | OIDC-only login |
| Password endpoint | `NAVIGATOR_IDENTITY_PLATFORM_ENDPOINT` | Google default |
| Identity Platform admin calls | `NAVIGATOR_GCP_METADATA_ENDPOINT` | GCE metadata server default |

---

Authentication and application authorization stay separate. OIDC proves identity; `persons.role` and Project
participation decide access. `OIDC_DISABLED` exists for host-side diagnosis but the deployment invariant rejects it in
both parsed profiles. `NAVIGATOR_GCP_METADATA_ENDPOINT`, `GOOGLE_TOKENINFO_URL`, and
`NAVIGATOR_IDENTITY_PLATFORM_ENDPOINT` are controlled test seams; a normal deploy leaves them on their secure defaults.
`NAVIGATOR_BOOTSTRAP_COMPANY` names a firm Entity that lawyer and admin may re-type or re-domicile but never rename or
delete. It adds to rather than replaces the protected set: the canonical seed re-creates `Shook Law PLLC` by exact name
on every boot, so that row is protected in every deployment and a white-label operator's own firm Entity is protected
alongside it.

### Deployed runtime: email, signatures, and billing

| Capability | Environment variables | Dev / production rule |
| --- | --- | --- |
| Email backend | `NAVIGATOR_EMAIL_BACKEND` | Must be `sendgrid` outside the harness |
| Outbound SendGrid | `SENDGRID_API_KEY`, `SENDGRID_FROM_EMAIL` | Required outside the harness |
| SendGrid base URL | `SENDGRID_BASE_URL` | Official hosts only outside the harness |
| Inbound SendGrid | `SENDGRID_INBOUND_SECRET` | Required outside the harness |
| Attachment scanner | `NAVIGATOR_CLAMD_ADDR` | Required in every deployed profile; private `clamd` only |
| Event webhook | `SENDGRID_EVENTS_SECRET`, `SENDGRID_EVENTS_PUBLIC_KEY` | Required outside the harness |
| Threaded mail | `NAVIGATOR_PARSE_HOST`, `NAVIGATOR_LAWYER_NOTIFY_EMAIL` | Both values enable it |
| DKIM fence | `NAVIGATOR_DKIM_REQUIRE_DOMAIN` | Optional domain pin |
| Internal ops notices | `SLACK_WEBHOOK_URL` | Optional; otherwise captured in memory |
| DocuSign endpoint | `DOCUSIGN_BASE_URL` | Declares DocuSign; demo in dev, live in production |
| DocuSign account | `DOCUSIGN_ACCOUNT_ID` | Environment-specific account |
| DocuSign JWT IDs | `DOCUSIGN_INTEGRATION_KEY`, `DOCUSIGN_USER_ID` | Preferred auth path |
| DocuSign JWT proof | `DOCUSIGN_PRIVATE_KEY`, `DOCUSIGN_OAUTH_BASE` | Preferred auth path |
| DocuSign static auth | `DOCUSIGN_ACCESS_TOKEN` | Short-lived fallback |
| DocuSign signer | `DOCUSIGN_SIGNER_EMAIL`, `DOCUSIGN_SIGNER_NAME` | Required signer identity |
| DocuSign webhook | `DOCUSIGN_HMAC_KEY`, `DOCUSIGN_WEBHOOK_SECRET` | Required once `DOCUSIGN_BASE_URL` is set |
| Xero tenant | `XERO_TENANT_ID`, `XERO_BASE_URL` | All Xero values select real billing |
| Xero OAuth client | `XERO_CLIENT_ID`, `XERO_CLIENT_SECRET` | Otherwise stub billing |
| Xero OAuth token | `XERO_TOKEN_URL`, `XERO_SCOPE`, `XERO_ACCESS_TOKEN` | Otherwise stub billing |

---

`NAVIGATOR_CREDENTIAL_ENVIRONMENT` must exactly match `dev` or `production` outside the harness. A normal dev deployment
therefore sends real email from a non-production SendGrid account and creates non-binding envelopes in DocuSign demo.
Xero is different today: its variables are not part of the deployment invariant, so an incomplete production Xero set
still boots and selects `StubBillingProvider`. Treat that as an explicit capability choice, not evidence that an invoice
reached the ledger.

### Deployed runtime: repositories, content, AI, and scheduled work

| Capability | Environment variables |
| --- | --- |
| Mounted Git writer | `NAVIGATOR_GIT_REPO_ROOT` |
| Forge coordinate, both required | `NAVIGATOR_GIT_HOST` (default `github.com`), `NAVIGATOR_GITHUB_ORG` (no default) |
| GitHub App identity | `NAVIGATOR_GITHUB_APP_ID` |
| GitHub App proof | `NAVIGATOR_GITHUB_APP_PRIVATE_KEY`, `NAVIGATOR_GITHUB_INSTALLATION_ID` |
| GitHub endpoint | `NAVIGATOR_GITHUB_API_BASE` |
| GitHub webhook receiver | `NAVIGATOR_GITHUB_WEBHOOK_SECRET`, `NAVIGATOR_GITHUB_CANONICAL_REPOSITORY` |
| Receiver identity and Restate submit | `NAVIGATOR_GITHUB_APP_LOGIN`, `RESTATE_INGRESS_URL`, `RESTATE_AUTH_TOKEN` |
| GitHub concurrency cap | `NAVIGATOR_GITHUB_MAX_CONCURRENT` |
| GitHub revision cap | `NAVIGATOR_GITHUB_MAX_REVISE_ROUNDS` |
| GitHub daily token cap | `NAVIGATOR_GITHUB_MAX_DAILY_TOKENS` |
| DevX Slack worker (in `workflows-service`) | `SLACK_WEBHOOK_URL` |
| Content roots | `NAVIGATOR_PUBLIC_DIR`, `NAVIGATOR_BLOG_DIR`, `NAVIGATOR_WORKSHOPS_DIR` |
| CLI login file | `NAVIGATOR_CREDENTIALS_FILE`, `NAVIGATOR_CONFIG_DIR` |
| CLI live inquiry | `NAVIGATOR_NOTATION_TEMPLATE`, `NAVIGATOR_SPEECH_BACKEND` |
| Harness worktree/cache | `NAVIGATOR_WORKTREE_PATH`, `NAVIGATOR_CHROME_CACHE_DIR` |
| Vertex coordinates | `NAVIGATOR_GCP_PROJECT_ID`, `NAVIGATOR_GCP_LOCATION`, `GOOGLE_METADATA_URL` |
| AIDA router | `NAVIGATOR_ROUTER_MODEL` |
| Contract reviewer | `NAVIGATOR_CONTRACT_REVIEW_MODEL` plus the same GCP project, location, and metadata variables |
| On-chain attestation | `NAVIGATOR_ONCHAIN_BACKEND`, `SOLANA_RPC_URL`, `SOLANA_PROGRAM_ID`, `SOLANA_SIGNER_SECRET` |
| Billing export | `BILLING_EXPORT_TABLE`, `BIGQUERY_PROJECT` |
| Billing notices | `BILLING_CANARY_NOTIFY_EMAIL` |
| Billing digest | `BILLING_DIGEST_NOTIFY_EMAIL`, `BILLING_DIGEST_WINDOW_DAYS` |

---

Matter creation must have a real Git writer even if the higher-level forge is local. The content variables only relocate
the read-only files baked into the image. AIDA uses `NullRouter` without the GCP project; contract review uses
`StubContractReviewer` without the same Vertex coordinates. The on-chain backend defaults to `null`, recording no
transaction. Scheduled cost and billing diagnostics stay disabled when their table or recipient variables are absent.

The GitHub webhook receiver is the exception to that degrade-quietly pattern, and where it runs is deliberate. It is
served at `POST workflows.<domain>/webhooks/github/{secret}` by `workflows-service`, on its own Axum listener
(`WORKFLOWS_WEBHOOK_LISTEN`, `9082`) beside the worker's Restate endpoint; the Envoy sidecar routes `/webhooks/github/*`
to that listener and every other path to the Restate leg. It runs on the `workflows` host, not on `www`, because `www`
goes entirely behind the firm's Tailscale tailnet — and GitHub, an external sender that cannot join a tailnet, can only
reach a public host. That split is the rule worth remembering: the VPN protects the human surface (`www`, the portal,
the workbench), while a machine caller's endpoint stays public and is authenticated by the signature it carries, not by
the network it arrives from. GitHub signs each delivery, so the receiver verifies `X-Hub-Signature-256` against the raw
body — which is why the Envoy leg to it stays HTTP/1.1 end to end and forwards the bytes unaltered.

Startup still requires the webhook secret, canonical repository, GitHub org, and app login, and `RESTATE_INGRESS_URL`
plus `RESTATE_AUTH_TOKEN`: a receiver that cannot verify a delivery or reach the Restate ingress has no safe reduced
mode. It is still one worker process — there is no separate receiver container — but it now owns a second listener port
that Envoy fronts, rather than sharing `web`'s. The DevX Slack services `DevxIssueTriage` and `devx-pr` fold into
`workflows-service` alongside the legal workflows and the receiver; they alone read `SLACK_WEBHOOK_URL` and fail closed
when Slack is absent so Restate can retry instead of acknowledging a lost engineering notice. Only `neon-law-stg` is
allowed to mount the receiver or bind the GitHub services. It also requires positive `NAVIGATOR_GITHUB_MAX_CONCURRENT`,
`NAVIGATOR_GITHUB_MAX_REVISE_ROUNDS`, and `NAVIGATOR_GITHUB_MAX_DAILY_TOKENS` values: its singleton
`devx-guardrails/global` object serializes all reservations, defers work at the concurrency cap, and pauses new token
reservations through the UTC-day reset. Other deployments do not bind that object or read these limits.

The receiver watches two GitHub owners. The product code lives at `github.com/neon-law-source-code/navigator` and is
always watched; the firm's private per-Project repos live under `github.com/neon-law-firm/<projects.code>`, where
`neon-law-firm` is `NAVIGATOR_GITHUB_ORG`. A delivery is accepted when it comes from the canonical code repository or
any repo owned by that org.

### Provision and ship: variables read by the operator CLI

| Concern | Environment variables |
| --- | --- |
| GCP target | `NAVIGATOR_GCP_PROJECT_ID`, `NAVIGATOR_GCP_LOCATION` |
| GKE target | `NAVIGATOR_GKE_CLUSTER_NAME`, `NAVIGATOR_GKE_CONTEXT`, `NAVIGATOR_K8S_NAMESPACE` |
| VPC, subnet | `NAVIGATOR_VPC_NAME`, `NAVIGATOR_SUBNETWORK_NAME` |
| Gateway IP | `NAVIGATOR_GATEWAY_IP_NAME` |
| Runtime identities | `NAVIGATOR_GCP_SERVICE_ACCOUNT_ID`, `NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID` |
| Assets/documents | `NAVIGATOR_ASSETS_BUCKET`, `NAVIGATOR_DOCUMENTS_BUCKET` |
| Exports/logs | `NAVIGATOR_EXPORTS_BUCKET`, `NAVIGATOR_LOGS_BUCKET` |
| Optional fork Config Sync | `NAVIGATOR_CONFIG_SYNC_REPO`, `NAVIGATOR_CONFIG_SYNC_DIR` (unset for Navigator's three) |
| Image registry | `NAVIGATOR_IMAGE_REGISTRY`, `NAVIGATOR_WEB_IMAGE` |
| Manifest source | `NAVIGATOR_GKE_OVERLAY` |
| Public OAuth clients | `NAVIGATOR_OAUTH_CLIENT_ID_BROWSER` (required), `NAVIGATOR_OAUTH_CLIENT_ID_GEMINI` |
| Brand and base URL | `NAVIGATOR_CUSTOM_BRANDING`, `NAVIGATOR_PRIMARY_DOMAIN`, `NAV_BASE_URL` |
| Public hosts | `NAVIGATOR_PUBLIC_HOST`, `NAVIGATOR_WORKFLOWS_HOST`, `GOOGLE_OAUTH_REQUIRED_HD` |
| Runtime Secret | `NAVIGATOR_WEB_SECRET_NAME` |
| Worker registration | `NAVIGATOR_WORKFLOWS_URL` |
| Restate admin | `RESTATE_ADMIN_URL`, `RESTATE_ADMIN_TOKEN` |

---

These values belong to the deployment operator, not an application admin. Provisioning consumes the cloud and cluster
coordinates; shipping consumes the already-provisioned target plus image, public identity, Secret, and Restate wiring.
Keep the distinction visible when debugging: changing an admin role cannot repair a missing GKE context or Secret. The
Gemini client ID is the one nullable entry: it stays unset until that deployment's data store assigns it.

### Ancillary operations and opt-in test controls

| Capability | Environment variables |
| --- | --- |
| Operator-local DNSimple CLI | See the operator-shell list below; never store it in a deployment config |
| GitHub release authentication | `GITHUB_TOKEN` |
| Xero sandbox OAuth | `XERO_SANDBOX_CLIENT_ID`, `XERO_SANDBOX_CLIENT_SECRET` |
| Engineering Slack routing | `SLACK_OPS_MENTION` |
| Browser-driver override | `WEBDRIVER_URL`, `WEBDRIVER_HEADED` |
| Deterministic GCP test token | `DEVX_GCP_FAKE_TOKEN`, `ARCHIVES_FAKE_TOKEN` |
| Browser gate | `NAV_REQUIRE_HARNESS`, `NAV_REASK_SHOTS` |
| Server-mode store lane | `NAV_REQUIRE_SURREAL` |

The DNSimple CLI transaction uses operator-shell variables `DNS_SIMPLE` (or legacy `DNSIMPLE_API_TOKEN`),
`DNSIMPLE_TOKEN`, `DNS_ACCT`, and `DNS_ZONE`. None belongs in a deployment config.

---

`navigator ops gcp setup` consumes the target and resource names; `navigator ops ship` consumes the target, public
identity, immutable brand image, OAuth client IDs, Secret name, and Restate coordinates. `ops gcp setup` flags override
their clap-bound environment variables; `ops ship` reads nothing from the environment — its required `--deployment` flag
selects one site's `deployments/<name>/config.toml` before it renders, diffs, and applies the GKE manifests. `ops ship`
accepts exact `production` (or empty/unset) for every persistent hosted deployment. Exact `dev` belongs to the
disposable `navigator dev staging` lane, not `neon-law-stg`.

### When sample data appears

| Surface | Local dev | Disposable `dev staging` | Persistent hosted rows |
| --- | --- | --- | --- |
| **Canonical database seed** | Always runs | Always runs | Always runs |
| **Sample-matter fixture** | Always runs (`dev`) | Always runs (`dev`) | `neon-law-stg` only |
| **Test-local database fixtures** | Browser/E2E harness only | Integration harness only | Never |
| Email | `CapturingEmail` by default | Non-production SendGrid | Production SendGrid |
| E-signature | Stub IDs and documents | Non-binding DocuSign demo | Live DocuSign |
| Billing | `StubBillingProvider` when Xero is absent | Same fallback | Same fallback today |
| Contract review | Stub without Vertex | Same fallback | Reference uses Vertex |
| AIDA free-form routing | `NullRouter` without Vertex | Same fallback | Reference uses Vertex |

---

The canonical seed is not gated by environment: every `web` boot applies the SurrealDB schema and idempotently inserts
the bundled catalog and firm-owned baseline rows in every deployment, production included. Jurisdictions are the
clearest example of the distinction this table encodes. The full reference set — all 248 rows of
`store/seeds/Jurisdiction.yaml`, every US state plus DC and every sovereign a matter can touch — is seeded on **every**
boot in **every** environment, because an entity's domicile and an attorney's licensure must resolve wherever the
application runs; since ENG-20 those rows live in SurrealDB, but the rule is engine-blind. The sample-matter fixture —
three synthetic Projects, their participants, and their walkthrough rows — is the second, idempotent layer. It is
applied wherever `NAVIGATOR_SIMULATED_MATTERS` resolves true: every `dev` boot, whether local KIND or the disposable
staging lane, plus `neon-law-stg`, which says so explicitly because its own runtime profile is `production`. So
`staging.neonlaw.com` serves the three demo matters under the sample-matter banner, and `www.neonlaw.com` holds live
matters and no fixture; both receive the same canonical reference data. Both layers are applied by one environment-aware
orchestration call (`store::seed::seed_environment`), so a reset or recreate restores the same baseline automatically.
Provider simulation is a separate axis: the test harness permits fakes, while non-harness dev uses sandbox integrations.
Test suites add further records inside isolated test schemas.

## Configure the Trust Boundaries

### Secrets: the invariants that gate the boot

`web` fails **loudly** rather than degrading silently: a missing required value crashes startup with a structured
`enforce_deployment_invariants` error naming exactly what is absent. So a `CrashLoopBackOff` is almost always a missing
secret:

```bash
kubectl logs deploy/navigator-web -n navigator
```

---

The boot-invariant set includes `NAVIGATOR_SURREAL_ENDPOINT`, `RESTATE_BROKER_URL`, storage configuration,
`SESSION_SECRET`, the `SENDGRID_*` keys, and `DOCUSIGN_HMAC_KEY`. The Environment Matrix above lists every startup key
by owning process and lifecycle; `.env.example` remains the per-variable contract with defaults, secret classification,
and provider-specific notes. See [`docs/oss-install.md`](/docs/oss-install) §4 and `.env.example`.

Two rules keep client data safe, and the deploy will not let you skip them. First, **plaintext secrets never live in the
manifest tree** — key material is SOPS-encrypted per value in `deployments/<name>/secrets.enc.yaml`, decrypted only by
`navigator ops secrets apply` into that deployment's own Secret Manager, so no readable credential enters Git. Second,
**one interface, your choice of source** — the Secret Manager CSI driver projects each deployment's Secret into the pod
(`neon-law-stg` is live on it). See [`docs/deployment-secrets.md`](/docs/deployment-secrets). Local KIND uses its
generated `.devx/env`, and optional sandbox integrations use a gitignored `.env`. The env-var interface is identical
across them.

### Sign-in: bring an OIDC provider; passwords live there, not here

Passwords matter — Neon Law Navigator's own database never stores a password. There is no password column and no hashing
crate; the credential lives with an **OIDC provider you bring**, never in our database. Identity is delegated via the
standard Authorization Code + PKCE flow. Four env vars wire it:

```bash
OAUTH_ISSUER_URL=...        # the provider's issuer; discovery hangs off /.well-known/openid-configuration
OAUTH_CLIENT_ID=...
OAUTH_CLIENT_SECRET=...
OAUTH_REDIRECT_URI=https://www.your-domain.example/auth/callback
```

Navigator speaks that flow against the provider (`/auth/login` → `/auth/callback`) and discovers every endpoint from
`<issuer>/.well-known/openid-configuration`, so no provider URL is hard-coded. Worked examples for Rauthy, Google,
Auth0, and Okta live in `.env.example`. The provider asserts only _who you are_ (a stable `sub` and an `email`); your
`persons` row owns _what you can do_ (the single `role`), so granting or revoking access is one SQL statement — see
[`docs/oidc.md`](/docs/oidc) for the full model.

For this workshop, that row should make you the `owner`. Navigator has five stored authorization roles, plus the
anonymous public visitor:

- `owner` — the system owner, highest in authority, inheriting every Admin and Lawyer capability. Only Owner may govern
  another Owner identity.
- `admin` — a licensed lawyer with installation-wide administration authority. Admin cannot manage an Owner.
- `lawyer` — a **licensed lawyer** working assigned matters and supervising any Clerk capability the application grants.
- `clerk` — a supervised **non-lawyer** worker. Clerk's `/clerk` surface is a read-only list of firm-assigned Projects
  whose disclosed `is_lawyer_dri` row is a lawyer. Clerk has no legal-advice, approval, Git, MCP, or
  `/app/lawyer` authority by inheritance; upload and preparation work still need their own narrow, supervised
  routes.
- `client` — represented people using the portal for their own matters.

Owner is the deployer's role because this class touches billing, secrets, OIDC, release state, and every Project. The
role still lives in the database, not the IdP token; your OIDC provider proves identity, then Navigator reads the
`persons.role` value to decide the tier.

One environment variable answers which person is the protected bootstrap Owner:

```bash
NAVIGATOR_BOOTSTRAP_OWNER_EMAIL=owner@example.com
```

Read `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` from the environment or secret source that supplies your deployment to determine
the protected identity. Do not copy the deployed value into Git. An unset, empty, or whitespace-only value disables the
bootstrap carve-out, so every person must already have a row before signing in. On a fresh installation, the first
successful OIDC login with the configured email JIT-creates its `persons` row as `owner`; later sign-ins restore that
role if the database has drifted. Its entire Person record is immutable in Navigator so an administrator cannot rename,
demote, or delete the installation's recovery identity by accident.

After signing in as Owner or Admin, open `/app/admin/people` to manage the directory and change another Person's
system-wide role among `owner`, `admin`, `lawyer`, `clerk`, and `client`. Owner appears first because it owns the
deployed system. Only an Owner can assign or modify Owner. Admin can manage Admin and every lower tier. The
bootstrap-Owner row remains read-only, and the command boundary rejects a hand-written update or delete as well. The
Lawyer workbench does not grant role-management power. These values are `persons.role`; Project assignments such as
attorney, paralegal, client, and co-counsel are Participation records and do not create another authorization tier.

**We recommend Google.** Verifying that a person is who they claim is real work — risk signals, step-up challenges, and
hardware-key (passkey / security-key) verification — and Google invests far more in it than we could. A Google-backed
sign-in is stronger than any password we would host ourselves. That is exactly the path NeonLaw's own prod takes: **Sign
in with Google**, wired by the four env vars above with `OAUTH_ISSUER_URL=https://accounts.google.com` — the same
standard redirect flow, no password anywhere. (Google-hosted _email/password_ is a separate, opt-in door: set
`NAVIGATOR_IDENTITY_PLATFORM_API_KEY` and `/auth/login` also renders a password form whose `POST /auth/password` checks
the credential against **GCP Identity Platform** — Google still owns the password and lockout, never us. Reset and email
confirmation need the admin door too: set `NAVIGATOR_GCP_PROJECT_ID`, run `web` where the GCE metadata server can mint a
service-account bearer token, and grant that service account `roles/identitytoolkit.admin`. Leave the key unset and
sign-in stays a pure Google redirect. Config lives in `.env.example`, not the redirect swap above.)

**No-Google path.** "Sign in with Google" cannot be the only front door of a public legal-services portal — the person a
clinic serves may have no Google account. Any standards-compliant OIDC provider — discovery, Authorization Code + PKCE,
RS256-signed ID tokens with RSA keys in JWKS — that hosts its own email/password login works with the env-swap above and
**zero Neon Law Navigator code changes**. **Rauthy** — the same open-source IdP the local KIND loop already runs —
serves email/password, self-registration, reset, and verification from its own pages, in your cluster with no per-user
fee. **Auth0 / Okta** are hosted SaaS equivalents: same four env vars, same redirect flow.

---

The point of this slide is the ownership boundary. Navigator still treats passwords as serious, but it refuses to own
them in its own database. The ordinary production recommendation is Google Sign-In through the standard OIDC redirect
flow. If an operator wants email/password, they either bring an OIDC provider with hosted login pages, or they
deliberately enable the separate Identity Platform password door with `NAVIGATOR_IDENTITY_PLATFORM_API_KEY`.

### Role rings: who can do what

<figure class="role-rings" aria-labelledby="role-rings-caption">
  <span id="role-rings-caption" class="visually-hidden">
    Outside to centre: Client; Clerk (supervised non-lawyer); Lawyer (licensed to practice); Admin (licensed lawyer with
    system administration); Owner (system owner). Anonymous is outside every ring and sees public pages only.
  </span>

  <div class="role-ring role-ring-client"><span>Client<br><small>own matter</small></span></div>

  <div class="role-ring role-ring-clerk"><span>Clerk<br><small>supervised non-lawyer</small></span></div>

  <div class="role-ring role-ring-lawyer"><span>Lawyer<br><small>licensed to practice</small></span></div>

  <div class="role-ring role-ring-admin"><span>Admin<br><small>lawyer + system administration</small></span></div>

  <div class="role-ring role-ring-owner"><span>Owner<br><small>owns the system</small></span></div>

</figure>

The rings display the five stored roles in their authority order: `owner > admin > lawyer > clerk > client`. Owner
inherits Admin and Lawyer capability; Admin inherits Lawyer capability but cannot govern Owner. Clerk is deliberately
not a weakened Lawyer account. Anonymous is outside every ring and sees public pages only. Clients use the portal for
their own matters. Clerks reach `/app/projects` like everyone else and get a read-only rendering of their firm-assigned
Projects and the disclosed lawyer DRI; they never give legal advice. Owner, Admin, and Lawyer are lawyers, and MCP, Git,
drafting, approval, and administration surfaces stay lawyer-only.

---

Use this picture when you add a person. Start with the person's real relationship to the firm, not the task you hope to
delegate: Client for a represented person; Clerk for a supervised non-lawyer; Lawyer for a licensed practitioner; Admin
for a licensed lawyer who also runs the installation. Then add Project participation separately. `/clerk` confirms which
Projects a Clerk may coordinate and names the supervising lawyer. A later Clerk task such as uploading mail from the
mailroom or preparing a contract review must name both the Clerk actor and the Project's disclosed lawyer DRI—it never
turns the Clerk into a legal adviser.

### Provider signup and parity across the deployments

The infrastructure command can create GCP resources, but it cannot accept vendor contracts, prove domain ownership,
choose paid plans, create globally owned GitHub organizations, or grant Workspace-wide authority. Those are explicit
operator gates. One deployment is one provider attachment; the same key names appear in every row, but credentials,
webhook secrets, signing keys, sender identities, Drive roots, Restate journals, and GitHub organizations never cross
rows.

| Deployment | Google browser callback | GitHub organization |
| --- | --- | --- |
| `neon-law-stg` | `https://staging.neonlaw.com/auth/callback` | `neon-law` |
| `neon-law-prod` | `https://www.neonlaw.com/auth/callback` | `neon-law-source-code` |
| `neon-law-prod` | `https://www.neonlaw.com/auth/callback` | `neon-law` |

The three GitHub organizations use GitHub Free, the engineering contact mailbox recorded in
`docs/provider-environment-parity.md`, and `Shook Law PLLC` as the controlling business. The bootstrap creates no
additional invitations: the authenticated operator remains the sole initial owner until an explicit access review adds
another person.

All three organizations and private Apps were created on GitHub Free with those exact slugs. Each App is installed only
on the organization in its row, selects all current and future repositories, and grants repository Administration and
Contents read/write. Webhook delivery is disabled. Each deployment's `config.toml` contains that row's App ID and
installation ID, and its `secrets.enc.yaml` the distinct private key. The complete value-by-value worksheet lives in
[`docs/provider-environment-parity.md`](/docs/provider-environment-parity).

| Config | GitHub App |
| --- | --- |
| `neon-law-stg` | `navigator-neon-law-stg` |
| `neon-law-prod` | `navigator-neon-law-prod` |
| `neon-law-prod` | `navigator-neon-law-prod` |

#### Updating a provider credential

Rotation is two-sided and ordered: **at the provider first, in the repository second.** Re-encrypting alone revokes
nothing — anyone holding repository history and the KMS key can still read every prior ciphertext, so the old value
stays valid until the provider stops honouring it.

Two of these credentials cannot be re-read once created, which makes "rotate" mean _replace_, not _look up_:

- **Google OAuth client secret.** Google removed retrieval entirely; the console shows only a masked suffix. Replace it
  with **Add secret**, capture the value from the one-time dialog, then delete the old secret.
- **GitHub App private key.** GitHub hands over the `.pem` once, at generation. Replace it by generating a new key, then
  deleting the superseded one.

Capture the value at the moment it is shown. A secret minted without capturing it is not recoverable — it is only
deletable, and it leaves a second live credential behind until you remove it.

Write the new value straight into the deployment's encrypted file. `sops` reads its key from the `.sops.yaml` creation
rule for that path, encrypts per value on save, and never writes plaintext to disk:

```bash
sops deployments/neon-law-stg/secrets.enc.yaml
```

A PEM needs a YAML block scalar so its newlines survive:

```yaml
NAVIGATOR_GITHUB_APP_PRIVATE_KEY: |
  -----BEGIN RSA PRIVATE KEY-----
  ...
  -----END RSA PRIVATE KEY-----
```

Then push it to that deployment's own Secret Manager. The CSI driver projects the new version into the pods:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir . --dry-run
navigator ops secrets apply --deployment <row> --deployments-dir .
```

The dry run prints the target project and object names without decrypting anything, so it needs no KMS permission — run
it first to confirm you are aimed at the deployment you meant.

Public coordinates are not key material and do not belong in this file. A GitHub App ID, an installation ID, and an
OAuth **client** ID are greppable, diffable, reviewable values: they live in `config.toml`. Only the App private key and
the OAuth client **secret** cross into `secrets.enc.yaml`.

#### Google OAuth: six clients, not three shared secrets

Create two OAuth clients per deployment in its row's GCP project:

- a Web application client for browser sign-in, with exactly the callback in the table;
- a Gemini Enterprise MCP client for that deployment's data store.

That is six clients total. The staging pair lives in the `neon-law-stg` GCP project's Google Auth Platform consent
configuration. The three production pairs live in their matching projects. Store the browser ID in
`NAVIGATOR_OAUTH_CLIENT_ID_BROWSER`, its secret in `OAUTH_CLIENT_SECRET`, and the Gemini ID in
`NAVIGATOR_OAUTH_CLIENT_ID_GEMINI`. The Gemini client secret belongs in that deployment's Gemini data-store setup.
Google matches a browser redirect exactly, so do not put several sites' callbacks on one client. An Internal audience
requires the project to belong to the matching Workspace organization; an External audience needs test users or the
applicable verification and domain-ownership work.

The staging browser client exists with the exact name and callback in this section. Its consent configuration is
External/Testing, and the authenticated operator is its initial test user. Its deployment config carries only that
browser ID and secret. The Gemini ID remains absent until the data store assigns it; `ops ship` temporarily renders a
browser-only allowlist, and [#1126](https://github.com/neon-law-source-code/navigator/issues/1126) removes that seam
after the authenticated staging AIDA smoke test.

These clients are configured in **Google Auth Platform → Clients**. They are general OAuth clients, not IAP or Workforce
Identity Federation clients. Google does not permit creating or modifying them programmatically, so neither ordinary
`gcloud services enable`/service-account commands nor `gcloud iam oauth-clients` replace this console step.

##### Create a browser OAuth client and save it safely

Do this once for each row that does not already have its browser client. The Google Auth Platform page shown below is
safe to include in an operating record because it contains the client name only—never capture the following creation
dialog, which reveals the secret once.

![Google Auth Platform browser client form](/public/workshops/navigator/google-oauth-browser-client-form.jpg)

1. In the Google Cloud console, switch to the **GCP project** in the deployment matrix for the target deployment. This
   is `neon-law` for `neon-law-prod`, `neon-law-prod` for `neon-law-prod`, and `neon-law-stg` for `neon-law-stg`. Do not
   create a client's credentials in a different project and copy them across.
2. Open **Google Auth Platform → Clients**, then select **Create client**. If Google first opens the Auth Platform setup
   screen, complete the organization-approved branding, support contact, audience, and contact-email setup before
   continuing. That one-time configuration is project-scoped; it is not a replacement for the client below.
3. Choose **OAuth client ID**, set application type to **Web application**, and name it
   `navigator-<deployment>-browser`. For example, Neon Law production is `navigator-neon-law-prod-browser`.
4. Leave **Authorized JavaScript origins** empty. Under **Authorized redirect URIs**, add exactly the single callback
   for that config from the preceding table. Do not add a wildcard, a second deployment's callback, or a guessed local
   URL.
5. Select **Create**. Google displays a dialog with two values: Client ID and Client secret. Copy both before closing
   it. The secret cannot be recovered from this dialog later; create a replacement client if it is lost.
6. In that deployment's `deployments/<name>/` tree, save the values under these exact names:

   | Google creation-dialog value | Where it lives |
   | --- | --- |
   | Client ID | `NAVIGATOR_OAUTH_CLIENT_ID_BROWSER` in `config.toml` |
   | Client secret | `OAUTH_CLIENT_SECRET` in `secrets.enc.yaml` (`sops set`) |

   Save them only in their matching deployment. The Client ID is public metadata but is still deployment-specific; the
   client secret is a credential and must never go in a terminal transcript, Slack, this repository in plaintext, or a
   screenshot.
7. Reopen Google Auth Platform → Clients and verify the client name and its single redirect URI. Then run the normal
   secret synchronization and ship dry run. The deployment operator preflight is deliberately non-interactive: it
   refuses a row whose `NAVIGATOR_OAUTH_CLIENT_ID_BROWSER` or `OAUTH_CLIENT_SECRET` is absent and prints that row's
   project, client name, and exact callback. It verifies presence, not that a non-empty value is a usable Google
   credential.

Do not recycle a browser client, callback, or secret across rows. The Gemini client ID, when a deployment's data store
assigns one, remains `NAVIGATOR_OAUTH_CLIENT_ID_GEMINI`; it is not a substitute for either browser value above.

Delete the obsolete `navigator-neon-law-stg-browser`, `navigator-neon-law-stg-gemini`, `navigator-neon-staging-browser`,
and `navigator-neon-staging-gemini` registrations from the `neon-law-stg` project. Their retired configs are not proof
that the Google registrations are gone, and no surviving deployment may reuse one of those client IDs, callbacks, or
secrets.

#### GitHub: four organizations and four private Apps

For each organization, a GitHub owner must:

1. confirm the existing GitHub Free organization, contact email, controlling business, and current sole owner; add no
   bootstrap invitation, then handle two-factor enforcement and any approved recovery owner in a separate access review;
2. confirm the private, organization-owned GitHub App in the table is installed only in that organization;
3. grant repository Contents and Issues read/write;
4. put that row's organization, App ID, and private key in `NAVIGATOR_GITHUB_ORG`, `NAVIGATOR_GITHUB_APP_ID`,
   `NAVIGATOR_GITHUB_APP_PRIVATE_KEY`, and optionally pin the discovered `NAVIGATOR_GITHUB_INSTALLATION_ID`.

The Apps own no Project repositories: Navigator provisions none. What is left is the `neon-law-source-code/navigator`
webhook and the DevX Restate services, which are the `neon-law-stg` singleton.

#### Provider signups that require a human account owner

- **DocuSign:** create Developer/demo attachments for staging. Production needs production eSignature accounts and
  Go-Live-approved integrations. The deployment's `secrets.enc.yaml` receives the account/base/OAuth IDs, JWT
  app/user/private key, signer, Connect HMAC, and path secret. Prove a completed demo or deliberate production envelope
  and its verified Connect delivery.
- **Twilio SendGrid:** create account or subuser boundaries with four separately revocable keys and webhook
  configurations, and authenticate each sender domain. The deployment's `secrets.enc.yaml` receives the mail API key,
  From address, inbound and event secrets, and signed-event public key. Prove outbound delivery, inbound parse, and a
  signed event callback.
- **Google Workspace Drive:** create three Shared Drives and service accounts. A Super Admin grants Drive domain-wide
  delegation to each service-account OAuth client. The deployment's tree receives the selected Drive ID and delegated
  user (`config.toml`) and service-account JSON (`secrets.enc.yaml`). Prove a synthetic file can be created, read, and
  archived only in the matching Drive.
- **Restate Cloud:** arrange an account and plan supporting at least three environments; the free tier is insufficient.
  The deployment's `secrets.enc.yaml` receives that row's broker, ingress, admin URL, and API key. Prove the worker
  registers and completes a durable workflow in that environment.
- **Production contracts:** an API-capable DocuSign plan and a SendGrid plan supporting the required webhook count may
  be paid subscriptions. A production deployment must not contain a demo DocuSign host, demo account, or staging sender.

For SendGrid, create a restricted mail-send API key rather than a billing-capable key and enable signed webhook
verification. For Drive, authorize only `https://www.googleapis.com/auth/drive`. For Restate, never point two rows at
the same environment in the target state: its journal is state, not a stateless endpoint.

#### Keep provider attachments deployment-local

Do not copy a production provider bundle from staging or another brand. Each deployment's `deployments/<name>/` tree
owns its GitHub App, Restate journal, DocuSign attachment, SendGrid credentials and webhooks, OAuth clients, Drive root,
database, session key, and application-signing keys. Run the names-only parity gate (it runs inside the workspace test
suite on every pull request) and the provider smoke tests per deployment, then let a `--dry-run` ship enforce the actual
boot-key contract.

`ops ship --dry-run` detects a missing or empty boot key before it applies a workload, but it cannot prove that a
non-empty provider credential is valid. Treat a placeholder such as `SG.test` only as a bounded diagnostic that proves
the next preflight branch; replace it with the deployment's restricted SendGrid key and run the outbound, inbound, and
signed-event smoke tests before a production ship. A placeholder can let a pod boot while the first real email fails,
which is useful evidence during setup but never production readiness.

When a dry-run prints a `kubectl patch secret` remedy, read it as the exact missing-key diagnosis—not as the durable
repair. Add the value only to the named deployment's `secrets.enc.yaml` (or `config.toml` for a coordinate), then run
`navigator ops secrets apply --deployment <name>` and rerun the same `ops ship --dry-run`. A hand-patched Kubernetes
Secret is drift the CSI projection will replace. Work one reported requirement at a time: the guard stops before the
workload apply, so the next dry-run is the authoritative check for the next missing boot dependency.

Before the first ship, write each deployment's Secret Manager objects from its tree:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir . --dry-run
navigator ops secrets apply --deployment <row> --deployments-dir .
```

Repeat per `deployments/` directory. The dry run reads key names only — no KMS call, no decrypted value — and fails
closed listing any object the `SecretProviderClass` projects that the tree does not supply. It never writes DNSimple,
gcloud, or operator-session values into Secret Manager; the tree cannot even express them.

Presence is only the first audit, and it runs in CI: the parity gate in `cli/src/devx/deployments.rs` checks every
deployment's key names against `store::deployment::WEB_REQUIREMENTS` on every PR. Then run `navigator ops ship
--deployment <name> --dry-run` and perform the provider smoke tests above. Never paste a private key, token,
service-account JSON, or decrypted value into Slack or workshop notes — see
[`docs/deployment-secrets.md`](/docs/deployment-secrets).

---

Separate what Navigator can reconcile from what an accountable human must authorize. Walk the table row by row, calling
out that OAuth consent, vendor contracts, domain verification, delegated Workspace authority, and production provider
approval cannot be inferred from a non-empty secret. Finish with the smoke-test evidence required before a row is
considered provider-ready.

### The external surface — every third party, in one place

Neon Law Navigator's runtime external surface has these services, in two kinds — **platform services** (the cloud the
stack runs on) and **feature vendors** (each lights up one capability and stubs out cleanly when unconfigured):

| Service | What it gives you | Kind | At boot |
| --- | --- | --- | --- |
| Google Cloud | Storage, OIDC, archive | platform | required — provisioned by `navigator ops gcp setup` |
| Restate Cloud | Durable workflow execution (`workflows-service`) | platform | required — the workflow broker |
| Vertex AI | The A2A agent-router LLM (Gemini Flash in prod) | platform | optional — `NullRouter` until configured |
| GitHub | Private per-Project repositories | platform | required in the requested cloud topology |
| DocuSign | E-signature | feature | CI-harness stub; required in a normal dev deployment and production |
| Xero | Accounting / billing (`ACCREC` invoices) | feature | `StubBillingProvider` until `XERO_*` is complete |
| SendGrid | Outbound + inbound email | feature | CI-harness capture; otherwise required |

---

The full catalog, with env prefixes and the per-environment account rule, is
[`docs/third-party-integrations.md`](/docs/third-party-integrations) — the table above is the deployer's-eye view of it.
Garage is the default open store for local and on-premises installs, while the S3 contract permits any conforming
endpoint. The single-node Garage shape in KIND is disposable developer infrastructure, not a production HA topology. Two
things worth saying out loud for a copyist:

- **Non-production boots with the canonical catalog; deployed integrations fail closed.** The explicit local CI harness
  needs none of the feature vendors: it captures email and uses in-process stubs, while Garage, SurrealDB, Rauthy, and
  Restate run in KIND. Cloud staging and production instead require configured SendGrid and DocuSign accounts at boot.
  Test-only records stay inside the harness's isolated database schema.
- **One provider attachment per deployment.** Staging attachments point only at sandboxes and production attachments
  point only at live accounts. A provider tenant may hold several attachments when that is its native model, but
  credentials, webhooks, senders, data partitions, and smoke evidence remain per deployment. That convention lives in
  [`docs/provider-environment-parity.md`](/docs/provider-environment-parity),
  [`docs/third-party-integrations.md`](/docs/third-party-integrations), and
  [`docs/docusign-esignature.md`](/docs/docusign-esignature).

One boundary worth naming: **Xero reconciles against the firm's bank (Mercury) inside Xero** — Neon Law Navigator never
speaks to Mercury. Our only integration edge is the Xero API. That is the shape to copy: integrate the system of record,
not everything it in turn connects to.

### The two service deployments

A production install runs one Rust application in two operational roles:

- `navigator-web` — the public portal, AIDA/API routes, webhooks, health probes, embedded Rego authorization, and
  client-facing Documents/Engagements/Invoices views.
- `workflows-service` — the durable Restate worker that renders documents, advances workflows, sends emails, and runs
  the background side effects the portal schedules.

The split keeps the portal stateless: every side effect that needs durable retries belongs to the worker rather than to
a request handler. Lawyer and admin users with Project access work the matter through the firm workbench, over the
participation-scoped list that surface resolves for them. Clients use the portal file surface: they see only Projects
where they have a `person_project_roles` row, and the portal renders reviewed documents, Engagements, and invoices
without exposing storage vocabulary or GCS credentials.

---

Pause here and make the deployment shape concrete. `navigator-web` is horizontally scalable because it stores state in
the store, Restate, and GCS — no role owns a mounted filesystem, so nothing in the serving path is a single writer.
`workflows-service` is separate because side effects need durable retries and clean worker ownership. The private
documents bucket stays private in both roles; client access is checked by Navigator before bytes are streamed back
through the portal.

### Security architecture

```mermaid
flowchart TB
  client["Client browser"]
  lawyer["Lawyer/admin browser"]
  oidc["OIDC provider"]
  edge["HTTPS Gateway"]

  subgraph gke["GKE namespace"]
    web["navigator-web\nportal, AIDA, APIs"]
    worker["workflows-service\nDurable worker"]
  end

  subgraph data["Private data plane"]
    pg["SurrealDB\nroles + project access"]
    docs["GCS documents bucket\nprivate blobs"]
    assets["GCS assets bucket\nprivate marketing objects\nserved through /assets"]
  end

  client --> edge
  lawyer --> edge
  edge --> web
  web --> oidc
  web --> pg
  web --> docs
  web --> assets
  web --> worker
  worker --> pg
  worker --> docs

  client -. "portal session only" .-> web
  lawyer -. "portal session + lawyer role" .-> web
```

Clients never receive GCS IAM, bucket URLs, or object paths. A client request enters through `navigator-web`, resolves
identity through OIDC, reads authorization from `persons` and `person_project_roles`, and streams only the
portal-visible matter files back through the app. Lawyer and admin access is not a second door: it enters through the
same service and the same database access model, and differs only in what the role and participation checks allow.

---

This is the security picture to leave in the room. The public edge is not the trust boundary by itself; the app still
checks identity, role, and Project participation before showing matter data. The private documents bucket is a storage
backend, not a client-facing sharing product. A disclosed client sees reviewed matter files because Navigator streams
them through the portal. A disclosed Lawyer/admin user sees more of the matter because their role allows legal editing
and history — through the same portal, not a separate privileged tier.

## Ship the Instance

### Ship and verify

Provisioning gives you an empty cluster; now pin one deployment to one published release. The `--deployment` flag
selects the `deployments/<name>/config.toml` that supplies the exact project, cluster context, namespace, image name,
hosts, buckets, SQL instance, required browser OAuth client, optional post-registration Gemini client, and runtime
Secret name. First-install order is load-bearing: apply the deployment's Secret Manager objects, install observability
so `navigator-otel-env` exists, render the release, then apply it:

Before shipping from a checkout whose deployment changes have not reached your installed binary, install that checkout's
CLI. A stale global binary may enforce an obsolete ship contract even when the selected deployment's config correctly
carries the current `production` profile:

```bash
cargo install --path cli --force
```

An operator wrapper must make the same guarantee before it changes Kubernetes or GCP: build the selected checkout's
`cli` package, then prove that `navigator ops secrets apply --help` exists. Do not fall back to an arbitrary
pre-existing `target/release/navigator`; a stale binary can lack a subcommand the current runbook requires and stop only
after it has already refreshed cluster credentials. Browser OAuth values belong in the deployment's tree before the
wrapper starts, so preparation is non-interactive. Keep explicit confirmations only for irreversible resource retirement
and the live release roll.

The failed guard runs before manifest rendering or cluster mutation. Install the matching CLI, set both environment and
credential profiles to `production` in the deployment's `config.toml`, then run the complete sequence:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir .

navigator ops observability --deployment <row> --deployments-dir .

navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D --dry-run

navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
```

`ops observability` is safe before the application Deployments exist: it creates the namespace-scoped collector and
`navigator-otel-env` ConfigMap, then skips the optional Deployment patch because `ops ship` renders that wiring into new
Deployments. A cold Autopilot cluster may take several minutes to create its first nodes and start the managed
Prometheus admission webhook. The CLI retries an idempotent IAM binding while a new `navigator-otel` Google service
account propagates. Before it applies `collector-monitoring.yaml`, the Rust CLI uses Google ADC and the Container API to
read the selected GKE cluster endpoint and CA, then queries the managed `gmp-operator` Endpoints object with its
Kubernetes client. It waits only while that object has no ready addresses; a `RUNNING` GKE cluster is not treated as
proof that the admission webhook is ready. The endpoint wait is bounded to three minutes and prints each attempt. If it
expires, inspect the managed operator rather than deleting the collector or patching the Secret:

```bash
kubectl --context "$NAVIGATOR_GKE_CONTEXT" -n gke-gmp-system \
  get deployment,pods,endpoints gmp-operator
```

When the managed operator becomes ready, rerun the exact same `navigator ops observability` or three-deployment operator
command. Namespace creation, Secret reconciliation, Google service-account creation, IAM bindings, and collector
manifests are all idempotent; a partially completed first run is a resume point, not a cleanup instruction.

The staging dogfood run also proved two quota-independent defaults. Autopilot clusters are created with
`--enable-private-nodes`, so a new region does not need one public in-use address per node. The link-out compatibility
writer uses the `standard` persistent-disk class, not an SSD-backed class; it must not consume `SSD_TOTAL_GB` for an
otherwise empty mount.

---

GitHub Actions publishes `neon-server` and `navigator-workflows-service` to `ghcr.io/neon-law-source-code`. The images
are public, so no reader grant is needed on any deployment's node identity — that whole cross-project binding retired
with Artifact Registry. `ops ship` refuses `latest`, verifies every selected image exists, renders the embedded
manifests with no unresolved placeholders, diffs before applying, preflights the runtime Secret, waits for all rollouts,
and registers the worker. The `--dry-run` form performs the checks and diff but never applies.

Then confirm the service is live — and you can do it **from the page itself**. The site footer renders the deployed
release as "Neon Law Navigator YY.M.D", so the moment your new image is serving traffic the footer changes: that is your
end-to-end "it worked." For a scripted check:

```bash
curl -fsS https://www.your-domain.example/readyz
curl -fsS https://www.your-domain.example/version   # {"release":"YY.M.D","commit":"…",…}
```

`web` exposes a readiness endpoint that returns `200 OK` only once it has a database connection and its dependencies in
hand, and a `/version` endpoint whose `release` field is the very same `YY.M.D` the footer shows. A `200` on `/readyz`
means the same stack our firm runs is now answering on your own cloud, and the `release` field tells you which dated
image landed without shelling into a pod.

To verify all three after a release, replace `YY.M.D` with the shipped tag:

```bash
TAG=YY.M.D
for host in \
  staging.neonlaw.com www.neonlaw.com www.neonlaw.com
do
  curl --fail --show-error --silent "https://${host}/readyz" >/dev/null
  curl --fail --show-error --silent "https://${host}/version" \
    | jq --exit-status --arg tag "$TAG" '.release == $tag' >/dev/null
  echo "OK ${host} ${TAG}"
done
```

Completion is three `OK` lines and three websites that open in a browser, not merely three successful Kubernetes
applies.

### Post the verified handoff in `#navigator`

Post only after the verification loop prints all three `OK` lines. Replace `YY.M.D` once with the deployed tag and
preserve the config/host mapping exactly. The message body is:

:white_check_mark: **Navigator `YY.M.D` is live on three deployment stacks**

Verified `/readyz`, `/version.release == "YY.M.D"`, and a browser visit:

- [staging](https://staging.neonlaw.com)
- [Neon production](https://www.neonlaw.com)
- [Neon Law production](https://www.neonlaw.com)

Set exactly one website to one published version:

Choose its `name` from the `deployments/` tree (`host` is that config's `NAVIGATOR_PUBLIC_HOST`), set `tag`, then run:

```bash
name=neon-law-stg tag=YY.M.D
host=$(sed -n 's/^NAVIGATOR_PUBLIC_HOST = "\(.*\)"$/\1/p' "deployments/${name}/config.toml")
gcloud auth login --force --update-adc

set -a; eval "$(grep ' = "' "deployments/${name}/config.toml" | sed 's/ = /=/')"; set +a
gcloud container clusters get-credentials "$NAVIGATOR_GKE_CLUSTER_NAME" \
  --region "$NAVIGATOR_GCP_LOCATION" \
  --project "$NAVIGATOR_GCP_PROJECT_ID"

navigator ops secrets apply --deployment "$name"
navigator ops observability --deployment "$name"
navigator ops ship --deployment "$name" --tag "$tag" --dry-run
navigator ops ship --deployment "$name" --tag "$tag"

curl --fail --show-error --silent "https://${host}/readyz" >/dev/null
curl --fail --show-error --silent "https://${host}/version" |
  jq --exit-status --arg tag "$tag" '.release == $tag'
```

Change only `name` and `tag`; the host follows from the config. The current tree maps:

- `neon-law-stg` → `staging.neonlaw.com`
- `neon-law-prod` → `www.neonlaw.com`
- `neon-law-prod` → `www.neonlaw.com`

All three hosts resolve to their own deployment's gateway IP, so all three answer `/readyz` and `/version` directly.

---

Roll and verify staging before production. A successful Kubernetes apply is not completion: `/readyz` must succeed,
`/version.release` must equal `tag`, and the website must open in a browser.

---

Do not present the Slack message as a deployment command. It is the evidence handoff after every deployment's health,
version, and browser checks pass. Demonstrate the single-site recipe separately: one deployment, one immutable tag, one
cluster context, and one host verification. Keeping those values paired is what prevents an operator from rolling the
wrong site.

### Point your domain at the instance (optional)

`navigator ops gcp setup` reserves a static gateway IP but deliberately does not touch DNS. Keep this boundary: do not
put the DNSimple token in any deployment's tree, and do not make DNS a side effect of GCP provisioning. Apply this one
reviewed transaction directly with the DNSimple CLI.

The exact one-time transaction below is the three-deployment record set: one public and one workflow address per
deployment. Review current state before applying it and omit any `create` whose exact record already exists. The apex
continues to redirect `neonlaw.com` to `https://www.neonlaw.com`.

> **This block is the pre-cutover `neonlaw.com` record set and has not been rewritten for the host map above.** It
  records
> the single-zone state live in DNSimple today, including the exact record ids its preflight compares against, so it is
> reproduced verbatim rather than machine-edited. Moving the firm's production to `www.neonlaw.com` and Neon
> production to `www.neonlaw.com` splits this one zone into three, and each new zone needs its own registration,
> records, managed certificate, and OAuth redirect URI before any record here is deleted. Treat the block below as the
> state to migrate _from_.

```bash
export DNS_ACCT=174981
export DNSIMPLE_TOKEN="$DNS_SIMPLE"

dnsimple records list neonlaw.com --account "$DNS_ACCT" --json |
  jq --exit-status '
    .data as $records |
      ([$records[] | select(.name == "" and .type == "URL")] == [{
        id: 80303423,
        zone_id: "neonlaw.com",
        type: "URL",
        name: "",
        content: "https://www.neonlaw.com",
        ttl: 300,
        regions: ["global"],
        created_at: "2026-07-23T22:08:37Z",
        updated_at: "2026-07-23T22:08:37Z"
      }]) and
      ([$records[] |
        select(
          .name == "staging" or
          .name == "workflows-staging" or
          .name == "neon" or
          .name == "workflows-neon-law-prod" or
          .name == "www" or
          .name == "workflows"
        )
      ] == [{
        id: 80303569,
        zone_id: "neonlaw.com",
        type: "URL",
        name: "www",
        content: "https://www.neonlaw.com",
        ttl: 300,
        regions: ["global"],
        created_at: "2026-07-23T22:14:28Z",
        updated_at: "2026-07-23T22:14:28Z"
      }])
  '

dnsimple records delete neonlaw.com 80303569 --account "$DNS_ACCT" --yes

dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name staging \
  --content 34.160.169.219 --ttl 300
dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name workflows-staging \
  --content 34.160.169.219 --ttl 300
dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name neon \
  --content 34.149.196.255 --ttl 300
dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name workflows-neon-law-prod \
  --content 34.149.196.255 --ttl 300
dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name www \
  --content 8.233.220.29 --ttl 300
dnsimple records create neonlaw.com --account "$DNS_ACCT" --type A --name workflows \
  --content 8.233.220.29 --ttl 300
```

The provider-side record list and a public resolver must return all six paired addresses after the transaction. The apex
returned `301 https://www.neonlaw.com/`. The temporary token was then removed from the shell and can be revoked in
DNSimple; record serving and propagation do not depend on it.

Run the mail record groups once for the `neonlaw.com` zone—not once per deployment—using `--google-workspace`,
`--sendgrid`, DKIM/link-branding targets, SPF includes, and DMARC settings. The command is additive and never deletes
unrelated records. The full record ownership and Google Workspace forwarding recipe is in [`docs/dns.md`](/docs/dns).

---

Keep DNS visibly outside the deployment configs. The temporary DNSimple token belongs only in the operator shell, and
the preflight must identify the exact historical redirect before deletion. After the six address records and apex
redirect verify, revoke the token at DNSimple; DNS propagation and the running applications do not depend on it.

### Drive it from the CLI

Once your instance answers `/readyz`, the `navigator` CLI runs the firm's whole matter flow against it from your
terminal. It authenticates like `gcloud auth login` and lands a short-lived (~8h) token at `~/.navigator.json`:

```bash
cargo install --path cli          # installs `navigator` on your PATH
# …or skip installing — run it straight from the source tree:
cargo run -p cli -- login --host www.your-domain.example   # `cargo run -p cli -- <args>` == `navigator <args>`
```

---

The login is a browser-loopback OAuth that reuses your instance's existing OIDC session and stores the token `0600` at
`~/.navigator.json` (a single gcloud-style dotfile; set `NAVIGATOR_CONFIG_DIR` to use the legacy
`<dir>/credentials.json` location instead). **The host is your deployment's** — `www.your-domain.example`, a staging
host, or `http://localhost:8080` for the KIND loop — so the same CLI drives whichever instance you point it at, each
keyed separately in the credential file. That is the whole reason `--host` is a flag and nothing about a domain is baked
in: this CLI is for _your_ install, not ours. The crate is `cli`; the binary it builds is `navigator`.

Create a matter in the lawyer UI first, using the service/product picker, then log in and drive its notation workflow
from the CLI using the matter code:

```bash
navigator site login --host www.your-domain.example    # opens the browser → ~8h token, stored 0600 (~/.navigator.json)
navigator site whoami                                  # "you@example.com (admin) — expires in 7h52m"
navigator site projects list                           # GET /app/projects.csv → table (or --json)
navigator site projects open estate-of-doe              # opens the existing matter workbench if visible
navigator site notation create onboarding__letter \
  --project estate-of-doe --client-email jane@example.com
navigator site notation approve <notation-id>          # renders + parks the retainer PDF (no envelope yet)
navigator site notation status <notation-id>           # state + signature request id + document_ready
navigator site logout
```

Use that same sequence to **verify a fresh install end to end** — it is the smallest real exercise of the durable
pipeline after the lawyer UI has opened the matter. Point the client email at an inbox you control (never a third party
— the notation workflow transmits a binding engagement letter), and walk the three assertions: `notation approve` should
leave the notation parked at `generate_pdf__retainer_pdf`; `notation status` should flip to `document_ready:true` once
the worker has rendered and persisted `document.pdf` (cross-check that the rendered object actually landed in your
private documents bucket):

```text
gcloud storage ls gs://your-project-id-documents/notations/<notation-id>/
```

Signature dispatch is handled by the site's notation workflow after the document is ready. When you sign or decline, the
inbound webhook should log a HMAC-verified `esignature webhook: signature event` in the `navigator-web` pod. Decline or
void the test envelope afterward so no live engagement lingers against a real inbox.

After a single `login`, `--host` is optional — the one stored host is used — so the later commands stay short. Every
command is a thin client over a route `web` already serves, sent with `Authorization: Bearer <token>`: your instance
resolves that token back into your session and runs the same handler the browser does, so the `lawyer_review` gate, the
role check, and the `authored_by` provenance all hold unchanged. The send is a durable two-step: `notation approve`
renders + parks the PDF on the worker, and the notation workflow dispatches the envelope only after confirming the PDF
landed (`document_ready:true`). Sending a retainer for signature stays a deliberate authenticated human action — it is
never exposed as an agent-routable tool. The full per-subcommand reference is the `cli` crate's `README.md` in the
source tree.

### Make it yours — white-label under your own brand

Neon Law Navigator runs two brands from one binary. A deployment operator can ship it under another identity without
forking source by describing the organization once in a private `navigator.yaml` bundle:

```bash
cp navigator.example.yaml navigator.yaml   # then edit: names, emails, domain, logos
cargo run -p cli -- ops rebrand build --out .devx/brand-bundle   # deprecated compatibility command
cargo run -p cli -- ops rebrand verify --dir .devx/brand-bundle   # deprecated compatibility command
set -a; source .devx/env; set +a                # object storage and the rest of the runtime env
NAVIGATOR_CUSTOM_BRANDING=.devx/brand-bundle cargo run -p neon
```

---

The bundle sets only **identity** — names, support addresses, postal addresses, the primary domain, the consultation
link, and brand static files. It never machine-generates binding legal text. Kubernetes mounts it read-only at
`/etc/navigator/brand` on the web and workflow-worker pods, and the deployment sets `NAVIGATOR_CUSTOM_BRANDING` to that
path to opt in — without the env var the identity stays Neon Law. An application admin role does not imply authority to
alter that deployment mount.

Most firms already run their own marketing site and have a team for it, so Neon Law Navigator does not need to be your
public website — it can be just the client portal and workflow engine. **`portal_only: true`** in the bundle mounts only
the application surface (`/app`, auth, `/api`, `/mcp`, the git transport, webhooks, the health probes, and the legal
pages) and drops the public marketing site; `/` redirects to `/app/projects`, and your own website links to your Neon
Law Navigator portal. **`terms_url` / `privacy_url`** point the footer's Terms and Privacy links at the legal pages your
own attorney publishes on your own site; bundle validation rejects a portal-only bundle with an empty `terms_url` — so
you never ship NeonLaw's bundled, Nevada-governed terms under your name.

### This is how we set up our production deployment

Everything above is the recipe. This is the log of us following it for our own production deployment, serving
`www.neonlaw.com`. It is written down because the first install into a cold cluster went differently from the happy
path, and the difference is worth knowing before you hit it.

It is the row that holds real matters, so it went last — after the same release had first proven itself on staging.
`navigator ops gcp setup` had already built the project's half of the world: the `neon-law-prod` GKE Autopilot cluster,
the `navigator-secrets` KMS keyring, five storage buckets, and the reserved global address `neon-law-prod-gateway-ip`.

Then the first `ops ship` failed, and kept failing. Three things were in the way, in the order we hit them.

**The object list was a superset.** `ops secrets apply --deployment <row> --deployments-dir . --dry-run` failed closed
naming nine DocuSign objects. That deployment executes no documents: it supplies no `DOCUSIGN_BASE_URL`, declines the
integration, and runs `StubSignatureProvider`. But the shared `SecretProviderClass` referenced all nine anyway, plus the
three engineering-webhook objects scoped to the automation home that this project must never hold. A CSI mount fails the
whole volume on one object it cannot read, so the only way past was a placeholder credential — and a placeholder boots
the _real_ provider, because `DocuSignSignatureProvider::from_env` returns `Some` for any non-empty value. A green
deploy that fails on its first signature request, on the deployment holding real matters.

That was a genuine defect rather than a configuration mistake, and the fix was to render the object list per deployment
so the class references exactly what the deployment writes. It is described in
[`docs/deployment-secrets.md`](/docs/deployment-secrets). If your own deployment declines an integration the manifest
names, this is the machinery that lets it.

**Secret Manager was empty.** With the object list correct, one command filled it:

```bash
navigator ops secrets apply --deployment <row> --deployments-dir .
```

Twenty-five objects written, twelve reported as skipped with their reasons. Read that skipped line — it is also the list
of what this deployment's mount will not ask for.

**A cold cluster has no namespace, and no Secret to preflight against.** This is the one to plan around. `ops ship`
confirms the deployment's Secret satisfies the running binary's boot invariants _before_ it reconciles anything, which
is the right order for every ship after the first — an unsatisfied requirement aborts a ship that has touched nothing.
On a first ship it is a standoff: the namespace is created by the apply in step 5, and the Secret is projected by the
CSI driver only while a pod mounts the volume, so neither exists when step 4 goes looking. The failure reads:

```text
Error: kubectl get secret neon-law-prod-web-secrets failed:
Error from server (NotFound): namespaces "neon-law-prod" not found
```

We broke the standoff by hand: created the namespace, then seeded a plain `neon-law-prod-web-secrets` Secret from the
Secret Manager objects `ops secrets apply` had just written, so the preflight had something real to read. The projected
Secret takes the same name, so once the pods are up and the driver owns it, the seeded one is retired exactly as
["Retire the plain Secret"](/docs/deployment-secrets) describes.

Do not read that paragraph as a runbook. Doing it by hand is off the invariant this whole workshop rests on — the
`navigator` CLI orchestrates every machine-bound flow, and reaching around it with ad hoc commands is precisely what
leaves the next operator without a path. The bootstrap belongs in the CLI, and the shape it should take is a preflight
that falls back to the key set the _rendered_ `SecretProviderClass` will project when no live Secret exists yet: those
objects are already proven to resolve to an `ENABLED` version before the reconcile, so it checks the same property from
the authoritative source instead of from a cluster that has nothing in it. Until that lands, a first install into a cold
cluster needs a human, and you should expect it.

**A cold Autopilot cluster has no nodes, and that stops the reconcile.** With the Secret readable, `ops ship` rendered
and diffed, then failed on an admission webhook:

```text
Error from server (InternalError): failed calling webhook
"validate.rules.gmp-operator.gke-gmp-system.monitoring.googleapis.com":
no endpoints available for service "gmp-operator"
```

The chain is circular and worth recognising on sight. Autopilot provisions nodes for workloads; a cluster nobody has
shipped to has none. With no nodes, the managed Prometheus operator cannot schedule — ours had been `Pending` for 44
hours — so its admission webhook has no endpoints, and the `Rules` object in the exports overlay cannot be validated.
The ship is blocked by the absence of the very workload it is trying to create.

`ops observability` is what breaks it, and this is exactly why the first-install order puts that command **before** `ops
ship` rather than after. It applies the collector Deployment first — a workload, which makes Autopilot provision a node,
which lets the operator schedule — and only then does it wait for the webhook and apply the monitoring manifests. Run it
and watch a node appear:

```bash
navigator ops observability --deployment <row> --deployments-dir .
```

Skipping it, as we did, turns a documented ordering into forty minutes of diagnosis. Run the order as written.

**Then the ship works.** Every manifest applied, `navigator-web` and `workflows-service` both rolled out, and the five
trigger `CronJob`s pinned to the tag. `ops ship` still exited non-zero, at the last step: it re-registers the worker
with Restate and refuses to call a ship complete when that fails, because a stale handler list means webhook submissions
fail silently later. That is the check behaving correctly — `RESTATE_ADMIN_URL` and `RESTATE_ADMIN_TOKEN` live in the
deployment's encrypted tree for the operator, and CI supplies them from secrets.

**The DNS cutover, and the gap it costs.** The Ingress carries the annotation
`kubernetes.io/ingress.global-static-ip-name: neon-law-prod-gateway-ip`, so it claims the address reserved during
provisioning — wait for it to appear before touching DNS, because until GKE finishes the load balancer there is nothing
behind that IP:

```bash
NAVIGATOR_GATEWAY_IP=$(gcloud compute addresses describe neon-law-prod-gateway-ip \
  --global --format='value(address)')
navigator ops dns setup --domain neonlaw.com --gateway-ip "$NAVIGATOR_GATEWAY_IP" --dry-run
```

Two calls: `www` patched off the old address, `workflows` created. The command never deletes. Then the wait described in
[Pointing a hostname at a deployment](/docs/gke-prod#pointing-a-hostname-at-a-deployment) begins, and it is a real
outage — the hostname serves neither the old site nor the new one while Google validates. Ours ran about twelve minutes
for `www`. `FAILED_NOT_VISIBLE` on the way through is normal: it records validation attempts made while DNS still
pointed elsewhere, and clears itself.

Retire the old certificate **after** the cutover has settled, not during it. `www.neonlaw.com` was previously served by
a Certificate Manager certificate authorized through an `_acme-challenge.www` `CNAME`. That record is inert once the
hostname points at GKE — managed certificates validate through the load balancer, not the ACME DNS challenge — but it is
also what keeps the old certificate renewable, and the old certificate is what makes a DNS rollback instant. Retire the
certificate, its authorization, and the record together, once you have decided not to roll back.

The lesson we would give another deployer is the one the release order already encodes: the deployment that carries real
client matters goes last, and it goes last precisely because the first install is where you learn what the recipe
assumes. Ours assumed a cluster that had already been shipped to once — and every gap above is one the two earlier
deployments could never have found, because by the time they shipped, someone had already shipped to them.

---

This slide exists because a workshop that only shows the happy path teaches people to distrust the workshop the first
time it fails them. Every earlier section is the recipe as it should go; this one is the recipe meeting a cluster nobody
had shipped to, told in the order we hit each wall. Walk it slowly if the room is about to do their own first install —
the deadlocks here are the ones that look like broken infrastructure and are actually just ordering.

The through-line worth naming out loud: each gap came from an assumption that was invisible until a _cold_ cluster
tested it. The object list assumed every deployment wanted every integration. The Secret preflight assumed a namespace
already existed. The reconcile assumed an admission webhook was running, which assumed a node, which assumed a workload
— the thing the ship was about to create. None of these could have been found by the two deployments that shipped first,
because by then someone had already shipped to them. That is why the risk-ascending release order is not bureaucracy: it
puts the deployment carrying real client matters behind the ones that can absorb a surprise.

If a participant asks what to do when they hit the namespace standoff themselves, be honest that it currently needs a
human, point at the fix named above, and say the gap is tracked rather than pretending the CLI covers it.

## Wrap Up

### Canonical references

This workshop is the narrative; these docs are the source of truth and stay current — prefer them when they disagree:

- [`docs/oss-install.md`](/docs/oss-install) — the full end-to-end install (env, Secret, overlay, image, verify).
  [`docs/deployment-secrets.md`](/docs/deployment-secrets) — production secret rendering.
  [`docs/third-party-integrations.md`](/docs/third-party-integrations) — the per-environment vendor-account convention.
  [`docs/docusign-esignature.md`](/docs/docusign-esignature) — e-signature setup and the one-app, two-environment model.

---

This is the access-to-justice fight made deployable: the cheaper and more repeatable it is to stand up a grounded legal
harness, the more clinics and small firms can run one. When your instance is live, tell us at
[support@neonlaw.org](mailto:support@neonlaw.org?subject=Deployed+the+Neon+Law+Navigator) so we can point the next
deployer at what you learned. Telling us what you learned is itself a contribution: the [Contributing to Neon Law
Navigator](/workshops/contribute-to-the-navigator) workshop lays out every way to give back, and once your instance is
live, walk your lawyer through [Using Neon Law Navigator](/workshops/use-the-navigator).
