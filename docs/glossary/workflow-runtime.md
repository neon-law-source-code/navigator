---
title: "Workflow Runtime"
---

The trait abstraction over the durable executor — [`workflows::StateMachineRuntime`](../../workflows/src/runtime.rs).
Two implementations ship today:

- **`InMemoryRuntime`** — non-durable, in-process. Used by tests and by `cargo run -p neon` when no
  Restate broker is configured. Reset on each process start.
- **`RestateRuntime`** — HTTP adapter that talks to a [Restate](restate.md) broker
  ([`workflows/src/runtime_restate.rs`](../../workflows/src/runtime_restate.rs)). Production target. The web binary
  picks one at boot and hands it to `AdminState::workflow_runtime`.

A Workflow Runtime is started once per Notation (`start(notation_id, spec)`) and advanced by external
`signal(notation_id, spec, condition)` calls. Every transition is recorded as a [Notation Event](notation-event.md) so a
crash plus replay terminates in the same state.
