---
name: authorization-model
description: >
  Neon Law Navigator's role + participation authorization model — the **canonical answer** to "who can see what." Every
  person carries exactly one role in `persons.role` (`owner`, `admin`, `lawyer`, `clerk`, or `client`); per-project
  scope lives separately in `person_project_roles.participation`. Owner and Admin bypass project-scoping silently, while
  Owner alone governs Owner identities. Trigger when the user mentions any of `role`, `roles`, `owner`, `lawyer`,
  `client`, `admin`, `participation`, OPA, "who can see", "what does Libra/Nick see", or before adding a new authz check
  anywhere in `web`. Also trigger when reaching for a JSON-array `roles[…]` shape — the schema collapsed to a single
  `role` column in migration `m20260619_collapse_persons_roles_to_role`, and the doc/PR drift to fix is to use the
  singular column.
---

# Authorization model — role + participation

The one-liner that captures the whole thing:

> **Role decides the tier; participation decides the scope.**

*What a person is* (system-wide tier) is separate from *what they see* (per-project scope). Both columns live in the DB,
both flow into OPA, neither lives in the IdP token. **Everything factual lives in the doc** — read
[`docs/access-model.md`](../../../docs/access-model.md) and keep it, not this skill, authoritative: the five tiers and
anonymous, the participation vocabulary, the seeded people, and the OPA allow rules.

## How to treat it (the load-bearing rules)

- **One singular `role` column, never a `roles` array.** A person has exactly one row in `persons` and one `role`
  (`owner`/`admin`/`lawyer`/`clerk`/`client`). Anything saying `roles = '["lawyer"]'` or `session.roles` contains
  `"lawyer"` is the legacy array shape from `m20260528_add_roles_to_persons`, collapsed in
  `m20260619_collapse_persons_roles_to_role` — fix the prose to `role = 'lawyer'` and `session.role == "lawyer"`.
- **`lawyer` INCLUDES attorneys.** Attorney, paralegal, and support are all the `lawyer` tier — there is no separate
  "attorney" role. The tier difference vs `client` shows up in *actions* (edit, sign, file), not visibility.
- **Owner is the highest tier.** The authority order is `owner > admin > lawyer > clerk > client`. Owner inherits every
  Admin and Lawyer capability. Admin cannot create, edit, or demote Owner identities; only Owner can manage an Owner.
  Person deletion remains client-only, so privileged identities are never deleted through that command.
- **Owner and Admin bypass project-scoping silently**, except `/app/owner`, which is Owner only. `session.role in
  {"owner", "admin"}` allows every authenticated request with no per-read audit row, then the Owner-only carve-out
  denies Admin on that path. Each is a separate enum value, not "lawyer + a flag"; visibility-wise both are supersets of
  Lawyer on every other route. Admin people and matter directories additionally filter by `person_firm_role`.
- **`participation` is derived from `persons.role`, never entered.** `store::projects::participation_for_role` is the
  only way one is chosen, so the column holds a tier word and nothing else. No write door takes a participation — not
  the lawyer matter-people form, not `POST /app/api/projects/{id}/participants`, not `aida_link_person_project`. A
  proposal to add a matter-side word (`co_counsel`, `translator`) is the drift to fix: outside counsel is a `lawyer`
  Person, and an adverse party gets no row at all. OPA reads no participation value; for the Rego layer the row's
  *existence* is the signal.
- **Don't conflate `persons.role` with `person_project_roles.participation`** — same English word, different columns,
  different decisions. And `participation` is not the `disclosures` table (conflicts of interest), which flows the other
  direction.

## Boundaries

- The OPA decision point (Rego, sidecar, `require_policy` middleware): [[opa-policy]] and `docs/opa-policy.md`.
- How the `role` enters the session at login: [[rauthy-oidc]] and `docs/oidc.md`.
