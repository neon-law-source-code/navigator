---
title: "Durable execution"
---

The property the [Workflow Runtime](workflow-runtime.md) gives the application. Once a Notation has emitted a signal
(say, `retainer_rendered`), the transition is recorded somewhere that survives process restarts; replay reaches the same
terminal state even if the worker crashes mid-flight. [Restate](restate.md) provides this property in production;
`InMemoryRuntime` is a non-durable simulation for tests and local dev.
