---
title: "Entity Type"
description: "An Entity Type names the kind of legal Entity, such as an LLC, trust, corporation, or foundation."
---

The kind of legal Entity (`LLC`, `Trust`, `Corporation`, `Foundation`, …). Reference data, seeded from
[`store/seeds/EntityType.yaml`](../../store/seeds/EntityType.yaml).

- Schema and queries: [`store::entity_types`](../../store/src/entity_types.rs) (SurrealDB; #1093, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)
