---
name: rauthy-oidc
description: >
  Rauthy as Navigator's non-production OIDC identity provider — bootstrap fixtures, the single-public-issuer bridge,
  Authorization Code + PKCE, JWKS verification, id_token decoding, and persons-table linking. Trigger when editing
  `k8s/staging/rauthy.yaml`, `k8s/overlays/kind/rauthy/`, the Rauthy loopback sidecar, OIDC client configuration,
  `/auth/callback`, or the `oauth2` and `jsonwebtoken` crates. Identity providers remain pluggable and application code
  stays spec-compliant.
---

# Rauthy and OIDC in the Navigator workspace

Rauthy is the local and staging dependency-tier IdP; production uses Google Identity Services. The contract between them
is OIDC discovery. Provider changes belong in environment configuration and manifests, not application branches.

Read [`docs/oidc.md`](../../../docs/oidc.md) before acting and keep it authoritative for the login sequence, local
fixture, single-public-issuer bridge, role/session boundary, tests, and troubleshooting.

## Load-bearing rules

- **Identity is `sub` + `email`.** The `persons` table owns profile, role, project membership, and billing.
- **The session role comes from the database.** At `/auth/callback`, link the identity to a `persons` row and stamp
  `session.role = row.role`; ignore token-side authorization claims.
- **Use discovery.** Never hardcode authorization, token, or JWKS endpoints in application code.
- **Keep one issuer locally.** Rauthy's `PUB_URL`, `.devx/env`, and the in-cluster loopback bridge must all derive from
  the selected Rauthy port.
- **Verify the id_token fully.** Validate JWKS signature plus `iss`, `aud`, `exp`, and `nbf`. Rauthy and Google use
  RS256; HS256 is accepted only by mocked tests.
- **Keep credentials environment-owned.** `k8s/staging/rauthy.yaml` contains no usable bootstrap or client secret. The
  known lawyer, Virgo, client, and administrator credentials live only in the loopback-bound KIND fixture.

## Anti-patterns

- Reading application profile or authorization from id_token claims.
- Using Rauthy roles for Navigator authorization.
- Giving browser and backchannel requests different issuer URLs.
- Adding provider-specific routes or rewrites to `portal/src/oauth.rs`.
- Storing the access token after extracting the login identity.

## Boundaries

- Role and participation model: [[authorization-model]] and `docs/access-model.md`.
- Rego decision point: [[opa-policy]].
- Local KIND lifecycle: [[kind-local-dev]].
