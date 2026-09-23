---
title: "Entity Type"
---

The kind of legal Entity (`LLC`, `Trust`, `Corporation`, `Foundation`, …). Reference data, seeded from
[`store/seeds/EntityType.yaml`](../../store/seeds/EntityType.yaml).

- Schema and queries: [`store::entity_types`](../../store/src/entity_types.rs) (SurrealDB; #1093, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ entity_type ─────────┐
│ id           record   │
│ inserted_at  datetime │
│ name         string   │
│ updated_at   datetime │
└───────────────────────┘
```
