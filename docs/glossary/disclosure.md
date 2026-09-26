---
title: "Disclosure"
description: "A Disclosure records a conflict or related-party fact attached to an Entity or Project."
---

A formal disclosure attached to an Entity or a Project (conflicts, related-party, etc.). A `conflict` / `related_party`
disclosure on an entity is read by the [Conflict-Check Graph](conflict-check-graph.md) and surfaced as a review-level
finding when a new matter reaches that entity.

- Commands: [`store::disclosures`](../../store/src/disclosures.rs) · Schema:
  [`disclosure`](../../store/src/schema/navigator.surql)
