---
title: "Address"
---

A postal address attached to a Person, to an Entity, or to neither (the mailroom placeholder). The person/entity
exclusivity is enforced by the application, not the schema.

- Schema and queries: [`store::addresses`](../../store/src/addresses.rs) (SurrealDB; #1093, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ address ───────────────────────────┐
│ id           record                 │
│ city         string                 │
│ country      string                 │
│ entity_id    option<record<entity>> │
│ inserted_at  datetime               │
│ line1        string                 │
│ line2        option<string>         │
│ person_id    option<record<person>> │
│ postal_code  string                 │
│ region       string                 │
│ updated_at   datetime               │
└─────────────────────────────────────┘
```
