---
name: web-preview
description: Verify a Navigator change in the documented local browser loop.
---

# Web preview

Read [`AGENTS.md`](../../../AGENTS.md#local-kind-development),
[`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- Use the task's local KIND environment, source its generated `.devx/env`, and authenticate through the documented
  local OIDC flow. Do not hand-write cookies or touch production.
- Keep browser captures under `/tmp`; use a real browser check for behavior string tests cannot prove.
- Captures, logs, and PR material must contain only synthetic or firm-owned content. Never record client data, legal
  files, real contact details, or production identifiers.
