---
title: "Relationship Edge"
---

A typed graph edge with a [Person](person.md) or [Entity](entity.md) on **each** end — the canonical two-sided
relationship the [Conflict-Check Graph](conflict-check-graph.md) traverses. Distinct from the [Relationship
Log](relationship-log.md), which is a one-sided audit trail (one actor, one subject); a Relationship Edge instead
asserts "A is `adverse_to` B" or "A is a `related_party` of B."

Each edge carries provenance (`source_kind` ∈ `manual` / `disclosure` / `relationship_log` / `llm`) and a
`confidence_pct` (0–100). Human-asserted edges are full confidence; edges an LLM parses out of a Relationship Log's
free-form detail land lower, and the conflict check multiplies confidence along a path so a chain of weak guesses cannot
raise a finding on its own.

Both endpoints are native `record<person|entity>` links, so an endpoint-kind typo cannot be written at all.

- Schema and queries: [`store::relationships`](../../store/src/relationships.rs) (SurrealDB; ENG-120) — Lives in the
  `relationship` relation

```text
┌─ relationship ──────────────────────────────────────────────┐
│ id              record                                      │
│ confidence_pct  int                                         │
│ detail          option<string>                              │
│ inserted_at     datetime                                    │
│ kind            string                                      │
│ source_id       option<record<relationship_log|disclosure>> │
│ source_kind     string                                      │
│ updated_at      datetime                                    │
└─────────────────────────────────────────────────────────────┘
```
