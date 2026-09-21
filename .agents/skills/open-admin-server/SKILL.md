---
name: open-admin-server
description: Prepare a local authenticated administrator session for Navigator development.
---

# Open local admin

Read [`kind-local-dev`](../kind-local-dev/SKILL.md), [`docs/access-model.md`](../../../docs/access-model.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

## Granting lawyer and signing in

Authentication comes from OIDC; authorization comes from `persons.role`. Signing in does not create a Person, so an
IdP-authenticated email with no pre-seeded row receives 403 — except `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`, which is created
as `owner`. Seed Lawyer into the database the running `web` reads:

```bash
cargo run -p cli -- dev grant-lawyer
```

`grant-lawyer` targets the environment-owned `navigator` database that every local loop shares. Then open the
`NAV_BASE_URL` from `.devx/env`, follow `/auth/login`, and sign in through Rauthy — a firm tier lands on `/app/team`, a
client on `/app/projects`. `/app/*` requires authentication and `/app/lawyer/*` additionally requires the database role
`lawyer` or `admin`. The session carries the role, so sign in again after any role change.

The local Rauthy fixture's five role-named accounts are listed in
[`docs/access-model.md`](../../../docs/access-model.md); its administration surface is
`http://localhost:30080/auth/v1/admin` on the shared tier, or the Rauthy port printed for a worktree. These credentials
are confined to the loopback-only KIND fixture and the reusable staging layer contains none.

`GET /auth/logout` clears the app session and, when the provider published an `end_session_endpoint`, bounces the
browser through it so Rauthy drops its own SSO session too; the next `/auth/login` then prompts for credentials instead
of silently re-authenticating.

- Use only the documented local KIND environment and fixture identities; never hand-write a session cookie.
- Confirm the active context is local or staging before inspecting it. Do not access or change production.
- Treat all screenshots, logs, and examples as public-safe source material: no client data, legal files, real contact
  details, or production identifiers.
