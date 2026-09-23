---
title: "Workflow"
---

The state machine that drives a [Notation](../notation.md#notation) from initial submission to final disposition. Two
layers share the name:

- **The declared workflow** — what a lawyer writes in the template's `workflow:` block. Plain YAML: a set of named
  States, transitions keyed by event, a `BEGIN` and an `END`. Lives next to the questionnaire under
  [`templates/`](../../templates/).
- **The executed workflow** — how the declared workflow actually runs. The [`workflows`](../../workflows/) crate owns
  this layer: the [Workflow Spec](workflow-spec.md) parser, the [Workflow Runtime](workflow-runtime.md) trait, and the
  [Restate](restate.md) adapter that drives it durably in production.

The same YAML is the contract between the two: a lawyer reads it as a flowchart; the engine reads it as a state-machine
spec. **The Template declares; Restate runs.**
