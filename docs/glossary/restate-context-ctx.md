---
title: "Restate context (`ctx`)"
---

The handle Restate passes into every handler invocation. Carries the durable **journal** for this invocation, the
**keyed state** for the virtual object the call landed on, and the primitives the handler uses to interact with Restate
(`ctx.get`, `ctx.set`, `ctx.run`, `ctx.sleep`, …). Each handler in
[`workflows-service::notation_service`](../../workflows-service/src/notation_service.rs) takes a `ctx:
ObjectContext<'_>` (or `SharedObjectContext<'_>` for read-only handlers); that's how the worker reads the stored spec
yaml, advances state, and records side effects atomically with respect to replay.

> **Mental model.** `ctx` is to a Restate handler what a database **transaction handle** is to a store helper — every
  durable thing the handler does flows through it, and the framework treats the sequence of `ctx` calls as the unit of
  replay.
