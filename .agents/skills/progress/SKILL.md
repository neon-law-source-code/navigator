---
name: progress
description: >
  Triage Linear project progress against shipped code, implement up to five ready issues as separate commits, and open
  one PR that closes them on merge.
---

# `/progress` — move a project forward

Audit Linear project progress against `origin/main`, choose 1–5 ready issues, implement each in its own commit, and open
one ready PR that closes them on merge. Use [`triage-projects`](../triage-projects/SKILL.md) for evidence and issue
decisions, then continue into implementation instead of stopping at its prompt handoff. Follow
[`docs/agent-workflows.md`](../../../docs/agent-workflows.md), [`implement-issue`](../implement-issue/SKILL.md), and
[`create-pr`](../create-pr/SKILL.md) for grounding, implementation, and ship requirements. Keep client data out of the
repository and fixtures.

## Triage the project

1. Start in a New Worktree and follow the `create-pr` worktree and environment setup. Refresh `origin/main`; preserve
   unrelated changes.
2. Run the audit in [`triage-projects`](../triage-projects/SKILL.md): read `docs/public-contributor-safety.md`,
   `docs/glossary/`, relevant docs, and complete Linear conversations; reconcile project updates and issue states with
   shipped code, tests, merged PRs, open PRs, and active worktrees; adjudicate issues as still valid, shipped,
   duplicate/superseded, or blocked. Carry its evidence and decisions forward, but do not stop to emit its copyable
   prompts.
3. Exclude issues already linked to open PRs and avoid files another active change owns. Select 1–5 ready issues whose
   changes can safely share one PR, with one issue per commit. If fewer than one issue is ready, report the project
   findings and blockers without opening a PR. If no compatible set fits one PR, report the split needed instead of
   forcing the work together. Do not bundle unrelated issues just to reach a count.

## Implement and update

For every selected issue, in the same worktree:

1. Re-ground its full issue and comment history using [`implement-issue`](../implement-issue/SKILL.md). Verify it is
   still valid against the refreshed baseline; already shipped, duplicate, superseded, or blocked issues are not part of
   this PR.
2. State the smallest behavior, covering test, and exact files before editing. Use TDD and relevant repository skills.
   Keep changes within that issue's acceptance criteria. Verify with focused tests and the documented UI workflow when
   applicable.
3. Commit that issue's implementation and proof before starting the next issue. Stage explicit paths and use a
   Conventional Commit subject. Keep issue titles and project names out of commits and all public surfaces.

Update Linear project progress and issue triage fields only from verified findings. Summarize what shipped, what remains
blocked, and which issues the PR addresses; do not claim completion before merge. Do not close selected issues directly:
put their bare identifiers in a single PR body trailer, `Closes ENG-1234, ENG-1235`, so Linear completes them on merge.
Use the appropriate team prefix for every issue. Keep issue titles, project or initiative names, and Linear URLs out of
public material. Follow [`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

## Open one PR

After all selected commits, use [`create-pr`](../create-pr/SKILL.md) for the final rebase, teaching surface audit,
content gate, and PR creation. Open one non-draft PR against `main` with one `Closes` trailer listing every selected
issue. Do not add unrelated changes. Let CI arm auto-merge; do not merge it manually.

Unlike the standalone [`create-pr`](../create-pr/SKILL.md) flow, `/progress` follows the PR after opening it. Wait for
required PR-head checks and auto-merge. If checks fail, diagnose and fix failures within the selected issues, commit the
fixes to their corresponding issue commits where practical (otherwise add a focused follow-up commit), push, and wait
again. Stop and report when the PR merges, or when a review, access, CI, or other external blocker needs a person.
Confirm the merge and Linear close before claiming completion; then update project progress from the merged result.
Report the PR link, merge outcome, issue identifiers, commit subjects, test and CI evidence, project/issue updates, and
blockers. If Linear or GitHub is unavailable, complete possible repository work and name the blocked external action.
