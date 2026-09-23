---
title: "Person–Project Role"
---

A Person's participation on a Project. The `participation` column records which side of the matter they are on, and it
is **derived, never entered**: `store::projects::participation_for_role` maps `person.role` onto it, so the value is one
of `owner`, `admin`, `lawyer`, `clerk`, or `client`. No write door takes a participation — not the lawyer form, not
`POST /app/api/projects/{id}/participants`, not `link_person_project`.

The row answers two questions, and they are not the same question. Its **presence** gates whether a `client` or `lawyer`
tier principal sees the Project at all. Its **value** decides which side of the matter that principal is on:
`store::projects::client_side_condition` matches the client side, and the firm lens
(`store::projects::firm_side_condition`) is its exact complement. Reaching the client's documents is narrower still,
keyed on `store::projects::client_document_condition`, which admits the natural-person `client` and the client-DRI
marker.

So "the row's presence is the signal" holds only for the first question. It is also true of the Rego layer, which reads
no participation value at all — but a `store` caller that treats presence alone as the signal collapses the firm lens
into the client-side set and hands an adverse party the lawyer workbench.

Values are stored folded — trimmed, lowercased, separators as single underscores — so one kind is one value. See
[`docs/access-model`](../access-model.md).

- Schema: [`store::projects`](../../store/src/projects.rs) ·
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ person_project_role ──────────┐
│ id             record          │
│ inserted_at    string          │
│ is_client_dri  bool            │
│ is_lawyer_dri  bool            │
│ person_id      record<person>  │
│ project_id     record<project> │
│ updated_at     string          │
└────────────────────────────────┘
```
