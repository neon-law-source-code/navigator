---
name: triage-projects
description: Reconcile the Linear portfolio with shipped work on `main` and report evidence-backed drift.
---

# Triage projects

Read [`docs/agent-workflows.md`](../../../docs/agent-workflows.md),
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md), and current `origin/main`.

1. Page through Linear projects, initiatives, and relevant issues.
2. List merged PRs for every repository in scope; correlate via Linear's GitHub linkage first, then explicit evidence.
3. Build the current Navigator CLI with `cargo build -p cli`. For every functional capability investigated, run relevant
   CLI commands against synthetic or local-safe fixtures where practical and focused tests that cover the capability.
4. Separate observed runtime and test evidence from source-only inference. Report exact commands and results, every
   skipped check, and why it could not be exercised. Do not require live production access; use staging only when a
   running deployment is genuinely necessary under repository policy.
5. Report stale-open issues, untracked merges, correlation gaps, inconsistencies, and non-overlapping worktree lanes.
6. Cite Linear or `main` for every conclusion. Propose mutations; do not apply them without direction.

Keep all reports abstract or synthetic. External planning surfaces never receive client data, legal files, real contact
details, or production identifiers.
