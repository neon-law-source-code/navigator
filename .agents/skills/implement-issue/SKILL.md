---
name: implement-issue
description: >
  Ground and implement one Linear issue in its own Navigator worktree. Use when the user asks to implement, pick up,
  start, or build ENG-NN, whether or not it has already been triaged. Refresh from origin/main, read the full issue,
  verify it against shipped code and tests, adjudicate it, then make the smallest test-driven change. Do not use for
  portfolio triage, a plan-only request, or opening the pull request itself.
---

# `/implement-issue` — ground and implement one issue

Turn one issue identifier into one grounded, minimal, proven implementation in one session. This skill owns both the
grounding and implementation loop. Use the deprecated [`triage-issue`](../triage-issue/SKILL.md) compatibility command
only when the user explicitly wants a Linear plan without code, and [`create-pr`](../create-pr/SKILL.md) when a ready
working tree needs to ship.

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
`docs/index.md`. Fetch the Linear issue with `get_issue` using `includeRelations: true`, then use `list_comments` and
read from the opening body through the last comment. Treat an existing triage comment as evidence, not authority:
refresh every claim against `origin/main` before editing.

Check Linear's GitHub linkage first, then correlated merged pull requests and the source itself, to determine whether
the issue already shipped. Derive the repository from `origin`, list its merged pull requests, and call Linear's
`get_diff` for each candidate GitHub URL. A returned issue is the canonical association. If no diff resolves, use an
issue reference in the pull-request body and corroborate branch-name or title matches with explicit Linear evidence.
Record when only a heuristic is available.

A pull-request search proves whether an issue was linked to merged work; it does not prove the requested capability is
absent. Search the relevant code, configuration, public API, and tests at `origin/main` before calling it missing. For
proposed new structure, also follow the source checks in
[`author-linear-issue`](../author-linear-issue/SKILL.md).

Reproduce the current behavior where practical. If an unknown still prevents a grounded scope, run the smallest
throwaway Rust spike that answers it and record the command, observation, and conclusion. Inspect active worktrees and
open pull requests for overlapping files.

## Adjudicate before editing

Reach exactly one verdict from the evidence:

- **Still valid:** name the smallest behavior, covering test, exact blast-radius files, and collisions, then continue.
- **Already shipped:** cite the correlated merge and source or test evidence, then stop without editing.
- **Duplicate or superseded:** name the surviving issue and evidence, then stop without editing.
- **Blocked on a decision or dependency:** name the blocker, owner, and smallest next action, then stop without editing.

Do not post a separate triage-plan comment during an implementation request. The grounding is the first phase of this
same session, and its result directly controls whether implementation begins. Do not infer a missing legal, product,
or operator decision.

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
