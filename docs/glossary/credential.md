---
title: "Credential"
---

A Person's licensure in a Jurisdiction — pairs a Person with a Jurisdiction and a state-issued `license_number`. The
pair `(person, jurisdiction)` is unique so the same attorney can't be double-listed under one jurisdiction.

- Schema and queries: [`store::credentials`](../../store/src/credentials.rs) (SurrealDB; #1093, ENG-19, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ credential ──────────────────────────┐
│ id               record               │
│ inserted_at      datetime             │
│ jurisdiction_id  record<jurisdiction> │
│ license_number   string               │
│ person_id        record<person>       │
│ updated_at       datetime             │
└───────────────────────────────────────┘
```
