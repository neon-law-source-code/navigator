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
  touching the manifest, committing, or opening a PR. `ops release version` itself is unaffected by this — it still
  requires an explicit `--tag` and derives nothing; this command only supplies the name a human would otherwise have had
  to work out by hand.
- Verify the requested (or defaulted) version and the current manifest before changing it. A `-hotfix.N` suffix is a
  semver prerelease of that core, so it ranks *below* the matching ordinary release. After `26.8.22` is published, the
  next hotfix is `26.8.23-hotfix.1`, not `26.8.22-hotfix.1`. See
  [`docs/gitops.md`](../../../docs/gitops.md#why-a-hotfix-prerelease-ranks-below-its-date).
- Run `cargo nextest run -p server --test vendor_assets` before the version bump. This is the required KIND-free
  accessibility check: it proves the deterministic stylesheet rules that keep normal-size text and links accessible. If
  this worktree already has a running KIND fixture and host `web`, also source `.devx/env` and run `cargo run -p cli --
  dev browser-e2e`; that is the full browser and axe-core audit. If no fixture is running, do not run the browser suite
  raw — it intentionally self-skips without a harness. Report that browser E2E was skipped; do not describe it as a
  passing accessibility audit.
- **Bump every version this repository names, not just the manifest.** The reusable workflows and composite actions
  under `.github/` reference this repository's own actions by an **absolute tag**, not by the ref the caller used, and
  `docs/examples/sample-portal-publish.yml` names two reusable workflows the same way, so a release moves them only if
  this step does:

  ```bash
  cargo run -p cli --quiet -- ops release pins
  ```

  It walks `.github/` whole and `docs/examples/`, and every literal pin must name the version being cut. The command is
  the rule: `ci.yml` runs it on every pull request too, so a pin left behind is already a red PR before a cut asks. A
  literal tag names a version and has to move, while the `@YY.M.D` occurrences in comments and prose are placeholders
  standing in for one — the command tells them apart so nobody has to read a regex, and rewriting those would destroy
  the example rather than update it. A pin left behind is not cosmetic. `navigator-install` was added after `26.9.16`
  (ENG-671) while its pins still read `@26.9.16`, a tag that does not carry it, so `26.9.17-rc.1` published a gate no
  consumer could run. Every job needing the CLI failed in about five seconds, unable to resolve the action at all, and
  only `read-manifest` — the one job needing no CLI — stayed green. Naming the version being cut is what makes a release
  self-consistent, since a caller only ever resolves a *published* tag, by which point the tag exists. Where that is too
  bold for a given cut, the conservative fallback is the most recent published tag that actually carries the action —
  never a tag predating it.

  The example file is in that sweep because it is the same decision written a third time.
  [`docs/project-repositories.md`](../../../docs/project-repositories.md) calls it the caller the scaffold emits, so a
  reader copies it into a Project repository and inherits whatever tag it names. It sat at `26.9.14` for three releases
  while this step watched only `.github/workflows/`.
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
- **Hand the new tag to the Project repositories, after the merge publishes it.** The pins swept above are the ones
  *inside* this repository. Every Project repository carries its own copy of the same decision — `ci.yml` calls
  `project-gate.yml@YY.M.D`, `cd.yml` calls `project-publish.yml@YY.M.D`, and `navigator.yaml` names the identical tag
  in `version:` — and nothing in `deploy.yml` moves them: the run's own `GITHUB_TOKEN` cannot reach another repository,
  and the one cross-repository grant that exists (`HOMEBREW_TAP_TOKEN`) is scoped to the tap. So the fleet sweep is an
  operator step, run once the tag is published, one repository at a time, dry run first:

  ```bash
  navigator ops github setup <owner>/<repo> --action-version <YY.M.D> --dry-run
  navigator ops github setup <owner>/<repo> --action-version <YY.M.D>
  ```

  It opens a pull request per repository on `ops-github-setup/workflow-templates-<YY.M.D>` rather than writing `main`,
  so each one still needs that repository's own `ci` and a code owner. Rerun the dry run afterwards; no drift is the
  only proof it converged.

  **That reconcile writes `ci.yml` and `cd.yml` only.** `navigator.yaml` is read, never written, so its `version:` stays
  behind and the pull request fails its own gate:

  ```text
  project-gate workflow ref `26.9.17` must equal manifest version `26.9.16`
  ```

  Move `version:` on that same branch before merging it. Until `ops github setup` reconciles the manifest too, a release
  is not finished when its own PR merges — it is finished when the fleet resolves the tag it published.
- Stop when that PR merges, then report its URL and the fleet sweep it leaves outstanding. Do not watch
  `deploy.yml` for the tag, images, archives, or tap.
- Do not deploy, mutate production, or copy production coordinates into the branch, PR, or release notes. Where release
  notes cite planning, cite the bare Linear issue identifier (`ENG-1234`) — never a `linear.app` URL, issue title, or
  project name. See [Linking a PR to its Linear
  issue](../../../docs/agent-workflows.md#linking-a-pr-to-its-linear-issue).
