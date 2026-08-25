--- name: opa-policy description: > Open Policy Agent (OPA) as the authorization decision point for `web` — sidecar
deployment, REST query API, Rego policy authoring, ConfigMap-bundled policies, hot reload. Trigger when editing
`k8s/base/opa/opa.yaml`, writing or modifying Rego, touching the OPA middleware in `web`
(`portal::policy::require_policy`), setting `NAVIGATOR_OPA_URL`, or adding a new `/app/...` route that needs policy
enforcement. Also trigger before reaching for a role-based authorization library — we route authz through OPA. ---

# OPA for navigator-web authorization

OPA is the **decision point**; `web` is the **enforcement point**. The split lets policy change without redeploying the
binary. Two docs own the detail — read them before acting, and keep them, not this skill, authoritative:

- [`docs/opa-policy.md`](../../../docs/opa-policy.md) — the system: sidecar deployment, the query API, Rego
  authoring/testing/hot-reload, decision logs, and the Rust client (`PolicyClient`, `require_policy`).
- [`docs/access-model.md`](../../../docs/access-model.md#how-opa-decides) — the semantics: the canonical `input`
  document, the allow rules, admin bypass, and project scoping. This is the source of truth for what a rule *decides*.

## Decision rules (the load-bearing ones)

- **`default allow := false`** at the top of every package. Default-deny is the only safe default.
- **Fail closed.** A transport error to OPA denies, except the dev-only passthrough when `NAVIGATOR_OPA_URL` is unset.
- **Authz lives in Rego, not Rust.** No `if path.starts_with(...) && session.role != "lawyer"` in handlers — that is an
  authorization rule. Rego decides "is this allowed"; the store stays the source of truth for the data protected.
- **One middleware, not per-handler calls.** Authz is cross-cutting → `require_policy` applied once with route-shape
  input, never an OPA call sprinkled through handlers.
- **Single `role`, not a `roles` array.** The session carries one `role` (`client`/`lawyer`/`admin`) — see
  [[authorization-model]]. A `roles[…]` shape in `input` or Rego is the collapsed-schema drift to fix.

## Boundaries

- The role + participation model and "who can see what": [[authorization-model]] and `docs/access-model.md`.
- How the session and its `role` get populated at login: the [[rauthy-oidc]] skill and `docs/oidc.md`.
