---
title: "Restate"
description: "Restate is the durable execution layer Navigator uses in production."
---

The **durable execution layer** in production — [restate.dev](https://restate.dev). An open-source workflow orchestrator
that records each `signal` as a durable side effect, so a worker that crashes mid-flight can replay to the same terminal
state. Restate is the production target for [Workflow Runtime](workflow-runtime.md); locally,
[`k8s/staging/restate.yaml`](../../k8s/staging/restate.yaml) brings up a broker in staging.

Crucially, **Restate executes the declared workflow verbatim.** The Template's `workflow:` block is the spec; Restate is
the engine. Neither layer needs to know about the other beyond the YAML contract.
