---
name: cut-release
description: Prepare a named Navigator version bump for review and release through `main`.
---

# Cut a release

Read [`docs/gitops.md`](../../../docs/gitops.md), [`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md). A release is a version bump landed
through a PR; merging `main` drives publication.

- **No version given: ask the CLI for today's, before doing anything else.** Run
  `cargo run -p cli --quiet -- ops release-default-tag` in the checkout. It prints the bare `YY.M.D` tag for today's UTC
  date on stdout when that date is releasable, and prints nothing to stdout — only a reason on stderr — when a version
  at or past today's date is already published. An empty stdout means there is nothing to cut: say so and stop, without
  touching the manifest, committing, or opening a PR. `ops release-version` itself is unaffected by this — it still
  requires an explicit `--tag` and derives nothing; this command only supplies the name a human would otherwise have had
  to work out by hand.
- Verify the requested (or defaulted) version and the current manifest before changing it. A `-hotfix.N` suffix is a
  semver prerelease of that core, so it ranks *below* the matching ordinary release. After `26.8.22` is published, the
  next hotfix is `26.8.23-hotfix.1`, not `26.8.22-hotfix.1`. See
  [`docs/gitops.md`](../../../docs/gitops.md#why-a-hotfix-prerelease-ranks-below-its-date).
- Make the smallest version-only commit, run the documented gate, and open the PR against `main`. **No draft PRs**: a
  release PR must open ready for review, not as a draft. Auto-merge only lands a PR that is not a draft, so a release
  cut as a draft sits published-but-unmerged until someone notices and marks it ready — take it out of draft as soon as
  the gate has run, rather than leaving that step for later.
- **A review is always required before it merges.** The `production-review` ruleset requires one codeowner-approving
  review on every PR; green CI and non-draft status alone do not satisfy it. That requirement is bypassed only when the
  codeowner named in `.github/CODEOWNERS` opens the PR under their own account — a release PR opened any other way (an
  agent's own token, a bot) sits at `REVIEW_REQUIRED` until a human approves it, so auto-merge will not just fire on its
  own. See [Review gate: two rulesets with a narrow
  bypass](../../../docs/gitops.md#review-gate-two-rulesets-with-a-narrow-bypass).
- Stop when that PR merges. Report its URL. Do not watch `deploy.yml` for the tag, images, archives, or tap.
- Do not deploy, mutate production, or copy production coordinates into the branch, PR, or release notes. Where release
  notes cite planning, cite the bare Linear issue identifier (`ENG-1234`) — never a `linear.app` URL, issue title, or
  project name. See [Linking a PR to its Linear
  issue](../../../docs/agent-workflows.md#linking-a-pr-to-its-linear-issue).
