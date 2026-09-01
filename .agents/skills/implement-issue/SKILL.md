---
name: implement-issue
description: >
  Implement one grounded Linear issue in its own Navigator worktree. Use when the user asks to implement, pick up,
  start, or build ENG-NN after it has been triaged. Refresh from origin/main, verify the issue against the current
  code and tests, then make the smallest test-driven change. Do not use for portfolio triage, issue planning, or
  opening the pull request itself.
---

# `/implement-issue` — implement one grounded issue

Turn one issue identifier into one minimal, proven implementation. This skill owns the implementation loop; use
[`triage-issue`](../triage-issue/SKILL.md) when the task is only to decide whether an issue remains valid, and
[`create-pr`](../create-pr/SKILL.md) when a ready working tree needs to ship.

## Start current

Before editing, confirm that `pwd -P` is a non-primary entry from `git worktree list --porcelain`. Preserve unrelated
changes. Fetch and signed-rebase on the shipped baseline:

```bash
git fetch origin main
git rebase -S origin/main
```

Do not implement from a stale local `main`, and do not merge `origin/main`. If the worktree is dirty or the rebase
cannot complete safely, stop and report the condition rather than combining unrelated work.

## Ground the issue

Read `docs/public-contributor-safety.md`, `docs/glossary.md`, and the narrowest relevant source of truth from
`docs/index.md`. Then read the Linear issue from its opening body through every comment, including relations.

At `origin/main`, verify the requested behavior in the source and its covering tests. Check the issue has not already
shipped through Linear's GitHub linkage first, then explicit merged-PR and source evidence. Inspect active worktrees
and open PRs for overlapping files.

Stop instead of coding when the evidence shows the issue is already shipped, duplicate, blocked by an unfinished
dependency, or requires an unresolved legal, product, or operator decision. Report the evidence and the smallest
next action; do not infer the missing decision.

## Implement with TDD

Name the smallest behavior and the test that proves it before editing. Add or adjust the covering test first and
observe that it fails for the intended missing behavior. Implement only enough to make that test pass, then run the
focused test and the applicable repository gate.

Use the workspace's established seams and specialized skills where applicable. Do not use a test that only exercises
an incidental helper when the issue changes a route, authorization boundary, workflow, or user-visible behavior.

Document the present system only: describe current behavior, contracts, and invariants. Remove superseded code and
instructions when the change replaces them; do not add retrospective decision history, compatibility narration, or
"used to" prose.

## Verify and hand off

Run `cargo run -p cli --quiet -- validate .` after the change, plus the focused test and every gate the changed surface
requires under `docs/agent-workflows.md`. For Rust or runtime changes, run formatting, clippy with warnings denied,
the workspace tests, and coverage as that document requires. Verify user-facing changes through the documented browser
loop.

Before handoff, rebase with `git rebase -S origin/main` again and rerun the affected checks. Keep the issue identifier
out of public prose except the bare identifier permitted by `docs/agent-workflows.md`; do not push, change Linear, or
open a pull request unless the user asks. Hand a green, narrowly scoped working tree to `create-pr` when it is ready to
ship.
