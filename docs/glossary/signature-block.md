---
title: "Signature Block"
---

A `{{ signer.field }}` placeholder in a Template body that becomes an e-signature field on the rendered document. The
*dot* is what separates it from an ordinary data placeholder like `{{client_name}}` (no dot, string-substituted with a
questionnaire answer): a signature block is **not** filled with a value — it expands to a visible signature line plus an
invisible anchor token in the PDF text layer that the e-signature provider keys its field off of.

The `signer` is a **role**, never a person's name (`{{firm.signature}}`, not `{{nick.signature}}`) — it resolves to a
real [Person](person.md) when the [Notation](../notation.md#notation) runs. A template declares its signer set with
optional frontmatter `signers:`, a list of lowercase snake_case role names in routing order. When the key is absent the
set is exactly `[client, firm]`. `client` is the respondent; `firm` is the attorney of record (the configured
countersignature). Any other role is a third party backed by the `person__<role>` questionnaire state. `client` and
`firm` may be omitted from an explicit list only when the template has no such party. The `field` is the field type:
`signature`, `initials`, or `date`. Validity is enforced by rule **N107** ([`rules::f107`](../../rules/src/f107.rs)):
the signer must be in the declared set, the field must be known, a Template that draws any signature block must declare
a `sent_for_signature` (or `sent_for_signature__*`) [State](state.md) to collect the signature, and an explicit
`signers:` list must have a `signature` or `initials` placeholder for every listed role. Rule **N115** requires each
declared role other than `firm` to be backed by questionnaire state carrying at least a name and an email.
