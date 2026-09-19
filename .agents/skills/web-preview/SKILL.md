---
name: web-preview
description: Run and verify a Navigator page with automatic rebuilds and browser refresh.
---

# Web preview

Read [`AGENTS.md`](../../../AGENTS.md#local-kind-development),
[`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- Reuse the task's configured worktree runtime and source its generated `.devx/env`. Start interactive previews with
  `cargo run -p cli -- dev serve`. This command builds and runs `neon`, watches Rust and catalog changes, and restarts
  the server after a successful build. Static-asset changes refresh the browser without recompiling. Keep the watcher
  running while editing; use the worktree's assigned brand port from `.devx/env`.
- Verify the browser has loaded `/__dev/reload.js`. A source edit should appear after compilation without a manual
  refresh. This is automatic rebuild and page refresh; it does not preserve client state like component hot replacement.
  If a failed build leaves the previous page visible, fix the reported error and save again.
- Authenticate through the documented local OIDC flow when the page requires it. Do not hand-write cookies or touch
  production.
- Keep browser captures under `/tmp`; use a real browser check for behavior string tests cannot prove.
- Captures, logs, and PR material must contain only synthetic or firm-owned content. Never record client data, legal
  files, real contact details, or production identifiers.
