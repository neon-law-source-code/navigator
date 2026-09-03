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

## Private mode

`NAVIGATOR_PRIVATE_MODE` toggles whether the Kubernetes setup puts Navigator's Pingora gateway in front of `web`. It is
the one flag both halves of the orchestration read, so a deployment is private the same way locally and in production:

- `navigator dev up` and `navigator dev deploy` apply `k8s/overlays/kind-private` instead of `k8s/overlays/kind`.
- `navigator ops ship` appends the `k8s/components/private-mode` component to the rendered GKE tree before `kubectl
  apply -k`, and says so on stderr.

The component is one sidecar and two patches: the workspace's `gateway` crate joins the `navigator-web` pod, and
`Service/navigator-web` stops targeting the application port and starts targeting the Pingora sidecar, which proxies
over pod loopback. Both the KIND Ingress and the GKE load balancer route through that one Service, so neither needs
editing. `/health` is the single unauthenticated route because it is what the kubelet probes and what the GKE load
balancer derives its health check from. Every other route is checked in order: the explicit client-network allowlist
(403), then the shared basic credential (401), then `web`. The gateway refuses a missing or empty allowlist and trusts
`X-Forwarded-For` only when the component explicitly configures it for the ingress/load-balancer path.

The credential is `go` / `bears`, committed in the component and therefore not a secret. Private mode keeps a staged
deployment from being crawled or wandered into; it is not an authorization boundary. Authentication is still OIDC and
authorization remains `persons.role` plus embedded Rego ([`access-model.md`](access-model.md)). Machine callers that
reach `web` over the public host — SendGrid inbound parse, GitHub webhooks, DocuSign Connect — receive 401 while it is
on. The browser e2e gate does not send the header, so leave it unset for a verification run.

The end state is a VPN rather than a shared password. Issue #1116 will narrow the explicit allowlist to the tailnet
egress ranges; it is deliberately not implemented here.

## Host ports

The host ports split into two categories with very different blast radius:

- **Port-forward ports:** SurrealDB, Restate ingress/admin, Garage S3, OpenObserve UI and OTLP ingest, and the local
  server — which binds one port per registered brand it can reach locally rather than only one. Every host in
  `views::brand::BrandKey::hosts` is a real production/staging domain, so a second brand needs its own local port
  instead of a `Host:` header a developer's machine has no DNS to send. `NAVIGATOR_LOCAL_DELETE_YOUR_DATA_PORT` names
  that second port for the `delete-your-data` house brand (ENG-437); `web` binds it directly, alongside `PORT`.
- **Create-time NodePort mappings:** ingress HTTP/HTTPS and Rauthy, rendered into `k8s/kind-config.yaml`.

The CLI renders a temporary KIND config and changes only requested `hostPort` values. Port-forward changes, including
Garage, require no cluster recreation.

## Testing

Tests in `cli/src/devx/mod.rs` require default/override coverage, ports in generated `.devx/env`, byte-identical default
KIND config, and override diffs limited to `hostPort` lines.

## Related

- `AGENTS.md` — local development contract.
- [`cloud-operations.md`](cloud-operations.md) and `.env.example` — production environment surface.
- [`oss-install.md`](oss-install.md) — GCP provisioning conventions.
