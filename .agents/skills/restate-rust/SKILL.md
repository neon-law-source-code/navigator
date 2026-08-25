---
name: restate-rust
description: Implement or review a Restate handler without breaking replay safety.
---

# Restate Rust

Read [`docs/durable-workflows.md`](../../../docs/durable-workflows.md),
[`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- Put every non-deterministic operation behind the documented journaled boundary with a stable name.
- Classify failures correctly, keep workflow steps durable, and add a covering test for changed behavior.
- Operational debugging logs identifiers and status only. Never emit client content, legal files, real contact details,
  or production identifiers.
