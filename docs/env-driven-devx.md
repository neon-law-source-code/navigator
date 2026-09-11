# Env-driven orchestration — one config surface, three audiences

`NAVIGATOR_ENVIRONMENT` is the deployment-profile selector shared by the server, `workflows-service`, and shipping
preflight. Exact `dev` selects development; exact `production`, empty, or unset selects production. Any other value,
case variant, or surrounding whitespace is an error. Local KIND uses `dev`; every hosted deployment uses `production`,
the three `*-staging` rows included, because staging names a role in the release order, not an application value.

It changes neither database engine nor authorization. Every boot applies two seed layers that reach production — the
canonical seed every deployment shares, and the booting brand's own seed — and `dev` additionally and idempotently
applies the sample-matter fixture. Local boot also refreshes and stages each sample application before writing
`.devx/env` for `web`.

The brand layer is selected by the binary rather than by this variable: the brand layer seeds the Firm's own data, and a
white-label `tenant` boot seeds none; each brand declares its `BrandSeed` in the `Brand` value it hands to the shared
run loop. That split is why one deployment's postal identities never appear in another's database. All three layers are
idempotent, so a reset and recreate restores the same baseline.

A **deployment operator** owns Kubernetes, cloud accounts, secrets, domains, and these environment values. That person
is distinct from a Navigator application **admin**, whose database role grants application authorization but no
infrastructure access.

`navigator` owns orchestration in `cli/src/devx/mod.rs`; one environment surface serves:

1. **Local dev:** `navigator dev up` against KIND; defaults need no `.env`.
2. **Every cloud deployment:** `navigator ops gcp setup` against that deployment's own project, then `navigator ops
   ship`. All six are provisioned and shipped by the same pipeline; there is no reduced staging substrate.
3. **OSS/multi-cloud:** supply cluster, namespace, overlays, and ports through `.env`, without Rust edits.

## The seam: one `KindConfig`, resolved once

`cli::devx::KindConfig` resolves all KIND/local values once, applies `DEFAULT_*` fallbacks, and is threaded through
`up`, `deploy`, `down`, `status`, environment rendering, and lifecycle helpers. Add each new knob only to this seam.
`.env.example` is the operator-facing list of variables and defaults.

## Naming: role, not provider

Variables are named `NAVIGATOR_<scope>_<thing>` so `.env.example` reads as one coherent table rather than two dialects:

- **Shared concepts get one var.** A Kubernetes namespace is the same idea in KIND and GKE, so it is
  `NAVIGATOR_K8S_NAMESPACE` (no `KIND`/`GKE` prefix).
- **Provider-specific concepts fork by scope.** The cluster name differs by provider — prod already has
  `NAVIGATOR_GKE_CLUSTER_NAME`, so the KIND cluster is `NAVIGATOR_KIND_CLUSTER`.
- **Overlay paths generalize.** `NAVIGATOR_KIND_OVERLAY` (full local stack) and `NAVIGATOR_GKE_OVERLAY` are the same
  idea at two scopes; a fork points either at its own kustomize overlay.

## Host ports

The host ports split into two categories with very different blast radius:

- **Port-forward ports:** SurrealDB, Restate ingress/admin, Garage S3, OpenObserve UI and OTLP ingest, and the local
  server — which binds one port per registered brand it can reach locally rather than only one. Every host in
  `views::brand::BrandKey::hosts` is a real production/staging domain, so every non-default brand needs its own local
  port instead of a `Host:` header a developer's machine has no DNS to send. The house-brand ports are
  `NAVIGATOR_LOCAL_DELETE_YOUR_DATA_PORT` and `NAVIGATOR_LOCAL_LAWYER_SHOOK_PORT`; `web` binds them alongside `PORT`.
- **Create-time NodePort mappings:** ingress HTTP/HTTPS and Rauthy, rendered into `k8s/kind-config.yaml`.

The CLI renders a temporary KIND config and changes only requested `hostPort` values. Port-forward changes, including
Garage, require no cluster recreation.

## Native shared runtime

`navigator dev worktree-env up --runtime native` uses the same host lock and descriptor-based slot reservation as KIND
lane. Restate, workflows-service, and host `web` keep their worktree slot; SurrealDB, Rauthy, and Garage are one shared
host process set. The native registry records each process's PID, complete command, and process-start identity together
with each worktree's private SurrealDB database and Garage bucket/key set. A second worktree adopts a verified listener,
and a recycled PID is treated as stale unless all identity fields still match.

`down` removes only the calling worktree's database, buckets, and claim. Shared processes remain until the final live
claim leaves. `worktree-env sweep` is a dry run by default: it reports native claims whose checkout is gone alongside
KIND orphans; `--apply` removes only those orphaned tenants and task-owned state, and never a shared process claimed by
a live worktree. The registry lives at `~/.navigator/native-runtime.json` by default and may be relocated with
`NAVIGATOR_NATIVE_REGISTRY`; its sibling `native-runtime/` directory holds shared process state.

## Testing

Tests in `cli/src/devx/mod.rs` require default/override coverage, ports in generated `.devx/env`, byte-identical default
KIND config, and override diffs limited to `hostPort` lines.

## Related

- `AGENTS.md` — local development contract.
- [`cloud-operations.md`](cloud-operations.md) and `.env.example` — production environment surface.
- [`oss-install.md`](oss-install.md) — GCP provisioning conventions.
