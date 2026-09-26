---
title: "Notation Event"
description: "A Notation Event is one immutable journal row for a Notation's state machine."
---

One immutable journal row for a [Notation](../notation.md#notation)'s state machine. Each row records the fact that a
given pair `(notation_id, machine_kind)` moved from one state to another via some condition, plus an optional opaque
JSON payload (the respondent's answer for questionnaire signals; `None` for workflow signals). The durable runtime
appends these so replay is deterministic, and the "current state" of a pair is the `to_state` of its latest row.

The on-disk shape mirrors the runtime type [`workflows::runtime::WorkflowEvent`](../../workflows/src/runtime.rs); both
layers stay in sync because the worker writes them through `ctx.run`.

- Schema: [`notation_event` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::notation_events`](../../store/src/notation_events.rs) Lives in: the `notation_event` table in SurrealDB
