---
name: author-docs
description: Audit teaching surfaces changed by a branch and write one citation-backed advisory report in `/tmp`.
---

# Author docs

Before naming a table, field, state, or UI concept, follow the **Find the term first** section in `AGENTS.md`.

Read [`docs/agent-workflows.md`](../../../docs/agent-workflows.md) and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md) first. Code is the source of truth;
docs, comments, tests, workshops, and skills follow it.

1. Confirm this is a non-primary worktree and compare the branch with its local `origin/main`.
2. Read the changed teaching surfaces and the source or test they describe.
3. For every finding, distinguish confirmed drift, ambiguity, and not-drift; cite both sides as `path:line`.
4. Write one report at `/tmp/navigator-author-docs/<branch>-<base>.md`, including a zero-finding result.
5. Keep citations in the report, not reader-facing prose. Do not change files as part of this audit.

Use the smallest matching council for an unresolved architecture, client-facing, or legal-copy decision. Do not copy
client data, legal files, real contact details, or production identifiers into the report.
