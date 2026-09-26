---
title: "Entity"
description: "An Entity is a legal organization such as an LLC, trust, corporation, or foundation."
---

A legal organization — an LLC, trust, corporation, foundation, etc. Has a name, an [Entity Type](entity-type.md), and a
[Jurisdiction](jurisdiction.md) it is organized under.

- Schema and queries: [`store::entities`](../../store/src/entities.rs) (SurrealDB; ENG-120) — Lives in: `entity` table.
  Its `entity_type_id` and `jurisdiction_id` are real `record<>` links; the firm's own row is protected from forking by
  a claim in the `firm_anchor` table, whose record id is the anchor key, rather than by an advisory lock. The UNIQUE
  `entity_firm_anchor` index is the backstop behind it — it refuses a fork that is not a race, but racers write no
  shared key for the engine to conflict on, so the claim is what serializes them (ENG-272).
