---
title: "Jurisdiction"
---

A US state, federal jurisdiction, or foreign jurisdiction that an Entity can be organized under, or that a
[Credential](credential.md) is issued by. Identified by short code (`NV`, `CA`, `US`, …).

- Queries: [`store::jurisdictions`](../../store/src/jurisdictions.rs) (SurrealDB; #1093, ENG-20) Schema:
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql) Seed:
  [`store/seeds/Jurisdiction.yaml`](../../store/seeds/Jurisdiction.yaml)

```text
┌─ jurisdiction ──────────────┐
│ id                 record   │
│ code               string   │
│ inserted_at        datetime │
│ jurisdiction_type  string   │
│ name               string   │
│ updated_at         datetime │
└─────────────────────────────┘
```
