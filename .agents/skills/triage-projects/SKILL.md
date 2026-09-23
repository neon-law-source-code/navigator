---
name: triage-projects
description: Audit Linear against shipped code; return 1–4 independent prompts through tested PRs.
---

# Triage projects

Follow [workflows](../../../docs/agent-workflows.md) and [safety](../../../docs/public-contributor-safety.md).

1. Refresh `origin/main`. Page through Linear projects, initiatives, issues, and selected conversations on both teams:
   Engineering (`ENG-`) and the `LAW` team, whose issues are what lawyers file when a matter's Project repository blocks
   them. Correlate merged PRs across scoped repositories, preferring GitHub linkage.
2. Run `rtk cargo build -p cli`, relevant CLI commands against safe fixtures, and focused tests. Use RTK only to compact
   agent-facing Cargo output; it does not improve compile time or replace raw output where exact diagnostics matter.
   Distinguish observations from inference; record commands, results, and explained skips.
3. Report health, stale issues, untracked merges, and correlation gaps with Linear/main citations.
   Propose tracker changes without applying them.
4. Check open PRs and active worktrees. Group issues into 1–4 lanes without overlapping files or cross-lane
   dependencies. Rank open `LAW-` issues first: each one is a lawyer blocked on live work, so it outranks an `ENG-`
   issue of equal priority, and ready `LAW-` work takes the first lane. Where a `LAW-` issue is blocked on an `ENG-`
   issue, the unblocking `ENG-` issue inherits that rank.
5. Ask only material scope, acceptance, or access questions before finalizing prompts. Include answers and defaults.

End with 1–4 copyable prompts for ready work, each containing:

- Issue identifiers, ordered scope, files, acceptance criteria, and tests.
- New Worktree using [implement-issue](../implement-issue/SKILL.md).
- One commit per issue, including its tests.
- Open one PR using [create-pr](../create-pr/SKILL.md). Fix failures until required local tests and
  PR-head CI pass, then end the session.
- Return the PR link, test evidence, or the external blocker that prevented that outcome.
