---
title: "Person–Entity Role"
description: >-
  A Person–Entity Role records a Person's place in an Entity, such as manager, member, beneficiary, or trustee.
---

A Person's role within an Entity (e.g. `manager`, `member`, `beneficiary`, `trustee`). These are the structural ties the
[Conflict-Check Graph](conflict-check-graph.md) walks at full confidence — the tie *is* the graph edge, `RELATE
person->entity_role->entity`, rather than a row projected into one.

There is no surrogate key: a tie's identity is its two endpoints plus its `role`, which is what the UNIQUE
`entity_role_tie` index says and what makes re-seeding idempotent.

- Schema and queries: [`store::entity_roles`](../../store/src/entity_roles.rs) (SurrealDB; ENG-120) — Lives in the
  `entity_role` relation
