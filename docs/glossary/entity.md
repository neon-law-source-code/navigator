---
title: "Entity"
---

A legal organization — an LLC, trust, corporation, foundation, etc. Has a name, an [Entity Type](entity-type.md), and a
[Jurisdiction](jurisdiction.md) it is organized under.

- Schema and queries: [`store::entities`](../../store/src/entities.rs) (SurrealDB; ENG-120) — Lives in: `entity` table.
  Its `entity_type_id` and `jurisdiction_id` are real `record<>` links; the firm's own row is protected from forking by
  a claim in the `firm_anchor` table, whose record id is the anchor key, rather than by an advisory lock. The UNIQUE
  `entity_firm_anchor` index is the backstop behind it — it refuses a fork that is not a race, but racers write no
  shared key for the engine to conflict on, so the claim is what serializes them (ENG-272).

```text
┌─ entity ──────────────────────────────┐
│ id               record               │
│ entity_type_id   record<entity_type>  │
│ firm_anchor_key  option<string>       │
│ inserted_at      datetime             │
│ jurisdiction_id  record<jurisdiction> │
│ name             string               │
│ phone            option<string>       │
│ updated_at       datetime             │
│ url              option<string>       │
└───────────────────────────────────────┘
```
