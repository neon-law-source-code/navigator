---
name: cut-release
description: Prepare a named Navigator version bump for review and release through `main`.
---

# Cut a release

Read [`docs/gitops.md`](../../../docs/gitops.md), [`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md). A release is a version bump PR;
merging `main` publishes.

1. **Name today's cut, or stop.** Run `cargo run -p cli --quiet -- ops cut-release --dry-run`. Stdout is today's
   `YY.M.D`. Exit 2 (reason on stderr) means a version at or past today is already published: stop. Do not touch the
   manifest, commit, or open a PR. A named hotfix or other departure from today's date still goes through `ops release
   version --tag`; that command derives nothing. A `-hotfix.N` suffix is a semver prerelease of that core, so it ranks
   below the matching ordinary release. After `26.8.22` is published, the next hotfix is `26.8.23-hotfix.1`, not
   `26.8.22-hotfix.1`. See [`docs/gitops.md`](../../../docs/gitops.md#why-a-hotfix-prerelease-ranks-below-its-date).
2. **Run the full browser and axe-core suite.** `vendor_assets` only proves stylesheet rules. It cannot see live axe
   failures such as WCAG color-contrast on `/notations`. Those only appear in KIND integration. Example:
   <https://github.com/neon-law-source-code/navigator/actions/runs/35550104432/job/106186927840>. Start the KIND fixture
   and host `web` if they are not running (`dev worktree-env up` or `dev up`), source `.devx/env`, then:

   ```bash
   cargo run -p cli -- dev browser-e2e
   ```

   A harness skip is not a pass. Do not ship the bump until this suite is green. `vendor_assets` may still run first; it
   is not a substitute.
3. **Bump and prove the pin sweep.** `ops cut-release` writes today's UTC version, every absolute tag pin under
   `.github/` and `docs/examples/`, and `Cargo.lock` in one version-only commit. Then run `cargo run -p cli --quiet --
   ops release pins`. Every pin must name the version being cut. Comments and `@YY.M.D` placeholders are excluded; every
   other ref is a pin. A prerelease cut pins the prerelease; the stable cut sweeps it. `ci.yml` runs the same command.
4. **Open a ready PR against `main`.** Smallest version-only commit, documented gate, not a draft. Auto-merge will not
   land a draft. One codeowner-approving review is required unless the CODEOWNERS identity opened the PR. See [Review
   gate](../../../docs/gitops.md#review-gate-two-rulesets-with-a-narrow-bypass).
5. **After the merge publishes the tag, sweep Project repositories.** Pins inside this repository moved in step 3. Each
   Project still names the old tag in `ci.yml`, `cd.yml`, and `navigator.yaml`. Dry-run first:

   ```bash
   navigator ops github setup <owner>/<repo> --action-version <YY.M.D> --dry-run
   navigator ops github setup <owner>/<repo> --action-version <YY.M.D>
   ```

   That writes `ci.yml` and `cd.yml` only. Move `navigator.yaml` `version:` on the same branch before merging, or the
   Project gate fails. The cut is finished when the fleet resolves the published tag.
6. **Stop** when the release PR merges. Report its URL and the fleet sweep still outstanding. Do not watch `deploy.yml`.
   Do not deploy, mutate production, or copy production coordinates. Cite Linear as `ENG-1234` only. See [Linking a PR
   to its Linear issue](../../../docs/agent-workflows.md#linking-a-pr-to-its-linear-issue).
