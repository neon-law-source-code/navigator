---
title: "Statement of the present"
---

How SurrealDB's schema is kept, in contrast to a migration chain. `store/src/schema/navigator.surql` is one idempotent
file describing the tables and fields that should exist, applied whole on every boot and by every test, with a single
`schema_version` record recording which build applied it. A migration chain's shape is whatever replaying its ordered
steps leaves behind; this one is written down. Applying the file converges definitions. `store::schema::apply()` may
also run a narrow, idempotent, guarded backfill when a historical row needs an unambiguous write-time default and the
operation is cheap enough to run on every boot and in every test. Expensive or destructive backfills, and repairs that
require human judgment about an old value, remain operator-approved one-shot jobs. The version record is what lets a
process notice it is looking at a database some other build prepared.

- Schema: [`store::schema`](../../store/src/schema/mod.rs)
