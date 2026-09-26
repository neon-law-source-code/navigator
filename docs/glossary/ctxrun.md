---
title: "`ctx.run`"
description: "ctx.run is Restate's journaled side-effect primitive."
---

The journaled **side-effect primitive**. Wraps any non-deterministic operation — a store write, an outbound HTTP call,
reading the wall clock — so its result is recorded in the invocation journal the first time and **reused from the cache
on replay** instead of re-executed.

```rust
// workflows-service::notation_service::questionnaire_signal
ctx.run(|| async move {
    let recorded_at = chrono::Utc::now().to_rfc3339();
    append_event(db.as_ref(), TransitionRecord { … })
        .await
        .map(|_| ())
        .map_err(|e| HandlerError::from(TerminalError::new(format!("journal: {e}"))))
})
.name("append-questionnaire-event")
.await?;
```

What this buys, concretely:

- **No double-writes on crash.** If the worker dies after the `INSERT` commits but before the handler returns, Restate
  replays the handler from the journal. The replay hits this `ctx.run`, sees a cached result, **skips the `INSERT`
  entirely**, and returns the original value.
- **Idempotent in spite of retries.** Restate retries failed invocations until they terminate. Without `ctx.run`, every
  retry would re-run the side effect; with it, only the first attempt that committed a journal entry actually runs.
- **The stable identifier matters.** `.name("append-…")` is how Restate matches a journal entry to a `ctx.run` site
  across handler versions. Rename it and a replay loses the cache hit.

If the handler does **not** wrap a side effect in `ctx.run`, the side effect runs once per replay — that's the
"double-row in `notation_events`" failure mode the design carefully avoids.
