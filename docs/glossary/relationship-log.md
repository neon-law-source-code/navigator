---
title: "Relationship Log"
---

Append-only audit trail of relationship changes — entries like `person joined entity` or `project closed`. The source of
truth for "what changed when" outside of normal table rows.

It is **not** the [Conflict-Check Graph](conflict-check-graph.md): a Relationship Log row is one-sided (an actor acted
on a subject), whereas the graph's edges are two-sided [Relationship Edges](relationship-edge.md). The log *feeds* the
graph — an LLM can parse a row's free-form detail into typed edges — and the graph writes back to the log when lawyers
acknowledge a conflict override.

It moved to SurrealDB with the graph (ENG-120) because its **writers** did: `store::projects` and
`store::project_modules` reached across engines for this one insert, so a matter open was a two-engine write with no
transaction spanning it.

- Schema and queries: [`store::relationship_logs`](../../store/src/relationship_logs.rs) (SurrealDB; ENG-120) — Lives
  in: `relationship_log` table

```text
┌─ relationship_log ──────────────────────┐
│ id               record                 │
│ action           string                 │
│ actor_person_id  option<record<person>> │
│ detail           string                 │
│ inserted_at      datetime               │
│ subject_id       uuid                   │
│ subject_type     string                 │
│ updated_at       datetime               │
└─────────────────────────────────────────┘
```
