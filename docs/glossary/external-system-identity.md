---
title: "External System Identity"
---

The identifier a third-party system issues for a [Person](person.md) — a GitHub numeric id, a Slack `U…`, a Google
`sub`, a Linear uuid, or a Notion workspace user id — so Navigator can name them in an API call. Creating a repository
and putting the right people on it means telling GitHub *which* user, and the API wants an id, not an email address;
inviting someone to a Slack channel, or as a Docusign envelope recipient, is the same problem.

Always the provider's **immutable** id, never the handle. Handles are renameable, and a mapping keyed on one breaks
quietly — the provisioning call simply fails to find a user, at exactly the moment a matter is opening. A display
`handle` is kept beside the id and is expected to drift.

**It carries no authorization meaning, and that is the point.** A row is an address, not a key and not a permission:
`person.role` is the authorization tier and `person_project_role.participation` is the scope, and an external identity
is neither. No code may read it to make an access decision. This is what keeps the [Clerk](person.md) rule intact — a
Clerk recorded as GitHub user `12345` receives no Git authority by that record — and what keeps the table from becoming
a back door around the rule that Project participation never grants source-forge access. The vocabulary of systems is
closed and fails closed; the values themselves are unverified, so a wrong id is a data-entry bug and a stale row is
wrong data, not a security incident.

- Schema and queries: [`store::external_identities`](../../store/src/external_identities.rs) (SurrealDB; ENG-85) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)
- Inertness guard: `cli/tests/external_identity_is_inert.rs`
- Access model: [`access-model`](../access-model.md#what-an-external-system-identity-is-not)
