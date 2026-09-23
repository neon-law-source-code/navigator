---
title: "Standing Data Store"
---

Where Navigator's rows live. SurrealDB holds every table (#1093). Its connection contract is
`NAVIGATOR_SURREAL_ENDPOINT` / `_NAMESPACE` / `_DATABASE`, and nothing defaults: a process that is not configured fails
loudly rather than quietly reaching the wrong engine.

Locally it is a KIND pod, memory-backed, so its data resets with the pod. Tests reach an embedded engine inside the test
process rather than a container — no server, no port, nothing to reclaim. Deployed, it is a hosted SurrealDB. Row-level
`PERMISSIONS` are explicitly `NONE` on every table: authorization stays above the database, in [Role](role.md),
[Participation](participation.md), and embedded Rego — see
[`access-model`](../access-model.md#where-surrealdb-authorization-lives).

- Schema: [`store::surreal`](../../store/src/surreal/mod.rs), and [Statement of the
  present](statement-of-the-present.md)
