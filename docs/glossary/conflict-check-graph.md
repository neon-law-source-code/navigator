---
title: "Conflict-Check Graph"
description: >-
  A Conflict-Check Graph is the graph the firm checks before opening a matter to identify conflicts with current
  clients.
---

The graph the firm walks **before opening a matter** to decide whether the new engagement would conflict with a client
it already serves. Every node is a [Person](person.md) or an [Entity](entity.md); every edge is a typed relationship
between two of them.

The graph **is** the store (ENG-120). Its two edge tables are Surreal-resident, and `store::conflicts` traverses them on
the deployment's own connection:

- `entity_role` — structural ties (manages / owns / member-of), always full confidence, written as
  `RELATE person->entity_role->entity`. Owned by [`store::entity_roles`](../../store/src/entity_roles.rs).
- [`relationship`](relationship-edge.md) — the supplemental typed edges: adversity, related-party, and edges an LLM
  later parses out of a [Relationship Log](relationship-log.md)'s free-form detail. Written as `RELATE
  (person|entity)->relationship->(person|entity)`, owned by [`store::relationships`](../../store/src/relationships.rs).

It was once a *transient view*: each check loaded the rows and projected a name-only copy into an embedded in-memory
SurrealDB that was dropped with the check. That projection was deliberately written in the shape the persistent store
would hold, so making the rows resident deleted the projection — and its separate schema file — rather than rewriting
the traversal.

The engine walks — one bounded SurrealQL query collects every edge within three hops of the anchors — and Rust scores:
the confidence product along a path and the review/block floors are conflict judgments, not graph operations. The
traversal is read-only by construction, which matters more now that it runs against the live store than it did against a
throwaway one.

A check anchors on the proposed client and entity and surfaces every distinct firm-served party it reaches. It reads
**across matters, unscoped**, because imputed conflicts under Model Rule 1.10 live on other people's matters; the
containment is that only firm-side create paths call it (see
[`access-model`](../access-model.md#where-surrealdb-authorization-lives)). Findings are **advisory to clear,
authoritative to block**: a confident, direct `adverse_to` link to a current client hard-stops the open; softer
entanglements (a shared entity, a recorded [Disclosure](disclosure.md)) are flagged for authorized lawyers to
acknowledge — recorded to the [Relationship Log](relationship-log.md) when they do. The graph can *raise* a conflict;
only a person can *clear* one, because it is never assumed complete.

It runs on every create path (portal, [Navigator MCP](navigator-mcp.md) MCP tool, CLI); the non-interactive paths have
no acknowledgment seam, so any finding refuses the open and routes lawyer to the portal.

- Engine: [`store::conflicts`](../../store/src/conflicts.rs), which traverses the resident graph. See
  [multi-cloud](../multi-cloud.md) for the deployment shape.
