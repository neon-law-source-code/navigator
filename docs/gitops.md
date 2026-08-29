# GitOps: edit → merge → release → deploy

Every change reaches production through an auto-merging PR to `main`. A release is one particular such PR — the one that
bumps `[workspace.package].version` — and merging it is what creates the tag, publishes the images, and hands the
operator a deliberate rollout. This flow supports the actions in [`agent-workflows.md`](agent-workflows.md).

## `main` is sacred and squash-merge-only

- **Never commit directly to `main`.** PRs squash to one commit; merge and rebase-merge are disabled.
- **Production follows `main`.** GKE reconciles `examples/deploy/k8s/gke`, and every release tag is created by
  `deploy.yml` at a commit on `main` — so a PR branch cannot be a release source by construction, rather than by a
  check. See [`gke-prod.md`](gke-prod.md).

## The branch → PR → auto-merge flow

1. **Task worktree + branch.** Start with **New Worktree** in Codex or Claude and verify the current path is a
   non-primary `git worktree` entry. Then run `cargo run -p cli -- dev worktree-env up --branch <kebab-topic>` once. The
   CLI names that worktree's PR branch; it does not create another checkout. See
   [`agent-workflows.md`](agent-workflows.md#create-a-pr) for the stop condition when a task did not start in New
   Worktree.
2. **Push + open a PR.** `git push -u origin <branch>` then `gh pr create`.
3. **Let auto-merge land it.** `ci.yml` enables auto-merge on open and each push. A green required gate plus resolved
   review threads triggers squash; approval is not required. Use draft status to hold a PR. The PR title becomes the
   Conventional Commit subject on `main`.

### Codified merge gate

Navigator is public at [`neon-law-source-code/navigator`](https://github.com/neon-law-source-code/navigator) on
github.com. Nothing here names a host, sets `GH_HOST`, or repoints `NAVIGATOR_GITHUB_API_BASE`: `gh` and every Action
default to the right place.

A public repository gets GitHub-hosted runners at no cost, so CI runs on `ubuntu-latest` rather than on
organization-hosted capacity, and the Marketplace is available.

**Confirm an App id against a live check run before trusting it.** A required status check registered under the wrong id
still *reads* as present in the API while matching an App that never posts a check, so the gate silently enforces
nothing:

```bash
gh api repos/neon-law-source-code/navigator/commits/<sha>/check-runs \
    --jq '.check_runs[] | "\(.name) \(.app.id) \(.app.slug)"'
```

`navigator ops github setup [repository]` reconciles one repository at a time, and it governs **every** repository the
Firm administers rather than a checked-in pair. The target resolves in precedence order — the explicit `owner/name`
argument, then `GITHUB_REPOSITORY`, then the checkout's `origin` remote.

The authorization boundary is a **`(host, organization)` pair**: the public organization holding Navigator, and this
deployment's own `NAVIGATOR_GITHUB_ORG`, on the host `NAVIGATOR_GIT_HOST` names. A repository in neither organization is
refused before a token is read, so an incidental checkout of someone else's fork cannot become a write target by being
the current directory. The host alone was the boundary while every repository the Firm owned sat on a private tenant; on
a public host it admits any repository on GitHub, which is why the organization came back.

Policy stays explicit; it is simply no longer an allowlist. It forks by organization: a repository in the public
organization gets `COMMON_POLICY` and one in the deployment's own gets `CLIENT_POLICY`, which is the same gate with
client-confidential defaults — private visibility, and none of the open-source governance files a published repository
carries. That gate, in either organization, is the `production` branch protections, the CODEOWNERS assertion, and the
merge policy — pull requests only, squash only, auto-merge, automatic head-branch deletion, and squash commits titled
and described from the pull request.

Repository features are reconciled alongside the merge policy, in the same `PATCH`, and all three are off: Issues,
Projects, and the wiki. Issue tracking is Linear's, so a repository-level tracker is a second inbox nobody reads, and a
wiki is documentation outside the review gate every other word in the tree passes through. They were applied by hand
before this command carried them, and the hand-application did not hold, which is the argument for reconciling them
rather than trusting a setting nobody re-checks.

`neon-law-source-code/navigator` alone adds `NAVIGATOR_POLICY`'s three extras — the release-tag ruleset, the DevX
labels, and the App-installation assertion — because it is the only repository that cuts a release or runs that
automation.

There is one lighter tier and one repository in it. A repository the Firm administers on someone else's behalf still
receives the same gate; what earns the exception is not ownership but whether a person writes `main` at all.
`assert_codeowners` sits in the common policy rather than beside the review gate for a reason the tests enforce:
`require_code_owner_review` against an absent or unresolvable CODEOWNERS silently accepts anyone's approval, so the two
ship together or neither means anything.

#### The Homebrew tap carries no gate

`neon-law-source-code/homebrew-navigator` receives `TAP_POLICY`: no `production` ruleset, no review ruleset, and neither
assertion. It still receives the merge settings, which govern its occasional human pull request.

A tap is the published output of a release rather than a repository the Firm develops in. Its `main` holds one
mechanical file and grows by one commit per release, written by the tap's own `bump` workflow *after* that workflow has
computed each sha256 from the published archives, installed the formula, and run `brew test` against the binary it is
about to publish. The verification a reviewer would perform has already run, by machine, against the actual bytes — so a
review gate there has nothing left to read.

Worse, it does not merely add nothing. Every rule in the `production` ruleset refuses that write instead of governing
it: `pull_request` admits no direct push, and `required_signatures` rejects a runner's `git commit`, which GitHub
verifies only for commits made through the API or the web editor. A gated tap reports a stale version to everyone who
installed through it while every check stays green. That is not hypothetical — the ruleset cost three consecutive
releases (`26.8.21-hotfix.10`, `.11`, and `.12`), each dispatched, each dying at the bump's final `git push`, while
`brew install` served `26.8.20-hotfix.4`. `deploy.yml` now reads the formula back and fails the release when it does not
move, so a recurrence is loud rather than silent.

The assertions are off for the same structural reason rather than as a convenience: the tap has no CODEOWNERS to resolve
and no `ci` job to bind, because it has no reviewer and no test gate of that shape. Asserting either would make the
command refuse a repository it is in fact configured for.

**Adding a second exception is a policy decision, not a config change.** The test a repository must pass is that no
human writes its default branch and a machine has already proved each commit before pushing it. If you point this
command at the tap it is a no-op on rulesets — nothing is created, and because the command emits no `DeleteRuleset`,
nothing a human deliberately adds is taken away either.

#### Reconciling generated workflow content

Every Project repository's `.github/workflows/gate.yml` — and `.github/workflows/publish.yml`, if it carries a `portal/`
— pins Navigator's validate action to an exact release tag, the same way [`scaffold`'s generated
gate](project-repositories.md#scaffolding-a-repository) does. Before this, moving that pin forward across the fleet
after a release meant hand-editing it in every one of the 19+ Project repositories that carry it — a manual, unreviewed
in spirit, per-repository chore.

`ops github setup` now reconciles that content too, using the exact same templates `scaffold` writes
(`cli/src/projects/repository.rs`'s `workflow`/`cd_workflow`) rather than a second copy that could drift from them. It
first has to know whether the target repository is one this applies to at all:

- `neon-law-source-code/navigator` and the Homebrew tap carry no generated `gate.yml`/`publish.yml` in this shape, so
  this half of the reconcile is a no-op for both, same as before this feature existed.
- The deploy repository — named by the optional `NAVIGATOR_GITHUB_DEPLOY_REPO` environment variable, never a literal in
  source, the same reason `.github/workflows/deploy.yml` carries its own checkout as the `DEPLOY_REPO` Actions variable
  rather than typing it — carries `ci.yml`/`ship.yml`, not the Project shape, and is excluded the same way.
- Everything else this command is authorized to reconcile is verified, not assumed: it fetches `navigator.yaml` from the
  repository's root over the API. A repository the fleet's own name does not reliably identify — one live example is a
  local checkout named `vaib` for the GitHub repository `vaib-studio` — earns no exception; a repository admitted by the
  `(host, organization)` boundary that carries neither an explicit deploy-repository exemption nor a manifest fails the
  reconcile loudly, naming the repository, rather than being silently skipped or silently treated as a Project
  repository either way.

The pin defaults the same way `scaffold --action-version` does — this binary's own confirmed release, refusing `main`,
`latest`, or an unconfirmed local build — and accepts the same `--action-version` override.

**A drifted file becomes a pull request, never a direct commit to `main`.** Every other reconciled artifact in this
command — a ruleset, a label, repository settings — is API-visible state with no review history of its own, so writing
the desired payload directly *is* the reconciliation. A workflow file is different: it is a tracked file on a repository
whose own ruleset already requires a pull request, a passing `ci`, and a code owner's approval to change `main` at all,
so writing it directly would either be rejected by the very ruleset this command maintains or, on a repository where
that ruleset is not yet applied, bypass it outright — for a binding legal-services practice, that is not an acceptable
trade for one fewer manual step. So when `gate.yml` or `publish.yml` (or both) drift, the command opens a branch off
`main`, commits the regenerated file(s) there, and opens an ordinary pull request back into `main` — the same shape a
human bumping the pin by hand opens today, gated the same way. Re-running before that pull request merges is idempotent:
the branch is named for the exact pin, so a second run finds it already holding the identical, deterministic template
output and only makes sure the pull request is still open, rather than stacking a duplicate.

Run a dry run before applying drift, then rerun without it:

```bash
navigator ops github setup neon-law-source-code/navigator --dry-run
navigator ops github setup neon-law/ui --dry-run
```

A second dry run after applying must report *no drift*. That is the only proof the reconcile actually converged, and it
is worth running: GitHub returns a ruleset's rules in whatever order it first stored them, which differs between a
ruleset this command created and one built by hand through REST. Comparing the rule vectors positionally made every
hand-made ruleset read as permanently drifted — each run wrote a PUT, each following run still saw drift, and "already
matches" was unreachable. `ruleset_matches` normalizes the order away, because order carries no meaning in the API. A
reconcile that never converges is a reconcile whose drift report means nothing.

#### One required check, named `ci` everywhere

Every administered repository terminates its `ci` workflow in a single aggregating job spelled exactly `ci`, and that
one context is what the ruleset requires. The job runs nothing itself: it `needs:` the real jobs and fails unless they
all succeeded. The jobs behind it stay free to differ per repository — this workspace runs `cargo test (workspace)` on a
large runner, `neon-law/ui` runs a `lint`/`verify` pair — and free to be renamed, because the required context never
moves.

The indirection exists because the alternative fails silently. A required status check is matched by string, so renaming
the job renames its check run while the ruleset goes on waiting for the old spelling. Nothing turns red; pull requests
simply sit forever on a check that will never arrive, and the usual fix — dropping the stale rule — leaves the branch
enforcing nothing at all. `ops github setup` therefore reads the repository's CI workflow and refuses to bind the gate
unless a job in it actually reports as `ci`.

It accepts either `.github/workflows/ci.yml` or `.github/workflows/gate.yml`, and looks for them in that order. Two
spellings are live at once and both are correct: a repository the Firm has always administered carries `ci.yml`, while a
Project repository written by `navigator site projects repository scaffold` carries `gate.yml`. What they share is the
invariant the gate is actually matched by — a job whose check run is named `ci` — so the filename is free to differ. A
repository carrying neither file is refused, and so is one whose workflow exists but ends in some other job name; those
are different problems with different fixes, so they are different errors.

#### Adopting a repository that is not yet governed

Host-based resolution removed the allowlist, not the convention. A repository still has to *earn* the gate, and the two
fail-closed assertions above are what it earns it with.

Every other repository the Firm administers must terminate its workflow in a job named `ci`. `assert_required_check_job`
refuses to bind the gate until that job exists. If `.github/CODEOWNERS` is absent, the command plans a one-time creation
with `* @shicholas`; if it exists, every named user or team must resolve and have write access before any ruleset is
written.

Order matters and is not a deadlock. Land the `ci` job while the ruleset still requires `verify`; both jobs run, the
pull request merges on `verify`, and the reconcile afterwards moves the required context from `verify` to `ci`. A
repository holding template content with no workflow at all has nothing for a required status check to bind to, so it
takes the CODEOWNERS half and waits for a real `ci.yml` before it can take the rest.

#### Review gate: two rulesets with a narrow bypass

Every governed source repository carries `production` and `production-review`. `production` has no bypass actors and
therefore binds everyone to signed commits, linear history, no deletion or force-push, squash-only merges, resolved
threads, and the required `ci` check. `production-review` requires one approval from a CODEOWNER, and its bypass actors
are the numeric users or teams resolved from `.github/CODEOWNERS`. The bypass therefore releases only the review
requirement for the people who can own the changed path; it does not release the integrity gate.

The split is required because GitHub scopes bypasses to an entire ruleset rather than to an individual rule. Keeping the
approval requirement separate gives code owners a safe self-merge path while all other production rules remain
universal.

Repository permissions are the outer boundary and are not managed by this command: a collaborator with `read` cannot
push a branch at all, and with forking disabled has no fork path either. `production` governs everyone who *can* push —
today the `write` collaborators.

#### CODEOWNERS owners must resolve

`require_code_owner_review` is only worth anything if the file names an owner GitHub can find. GitHub does not reject an
unresolvable CODEOWNERS entry: it drops the rule and leaves those paths unowned, which is indistinguishable from having
no CODEOWNERS at all. A repository can sit for months with the review gate on, the file committed, and no owner on any
path.

This was not hypothetical while the repository sat on an EMU-provisioned enterprise that shared no account namespace
with github.com: a handle carried over from a github.com checkout resolved to nothing. The public host removed that
particular trap, and left the general one — a misspelled handle, or a person who has left the org — which fails exactly
the same way and just as silently. `ops github setup` resolves every owner named in `.github/CODEOWNERS` against the API
(`@user`, `@org/team`) and fails closed before writing anything. Email owners cannot become numeric ruleset actors and
are rejected when the review gate is enabled.

All assertions run before the first write, so a repository that cannot satisfy the policy is left exactly as it was
rather than half-reconciled.

> **Auto-merge identity.** `enable-automerge` arms auto-merge as a GitHub App with `contents: write` and
> `pull_requests: write`, and as nothing else. It needs `AUTOMERGE_APP_ID` and `AUTOMERGE_APP_PRIVATE_KEY` as Actions
> secrets, **and the same pair in the Dependabot secret store**, which is separate — without them there, Dependabot's
> bumps arm nothing. There is no fallback to `GITHUB_TOKEN`: an absent secret skips arming and leaves the pull request
> visibly waiting for a human, and `cli/tests/automerge_identity.rs` asserts the fallback stays gone. Publishing adds no
> companion configuration — it authenticates with the run's own `GITHUB_TOKEN`, see
> [Keyless pushes to GHCR](#keyless-pushes-to-ghcr) — so the repository sets no Actions *variable* at all.
>
> The App is `navigator-merge-queue` (app id `4158267`), installed on selected repositories with exactly
> `contents: write` and `pull_requests: write`, and deliberately without `workflows`. Both secrets exist, so
> auto-merge arms under that App.
>
> **Which identity arms it is load-bearing, not cosmetic.** GitHub's recursion guard names `GITHUB_TOKEN`
> specifically, so a merge armed with the run's own token produces a push that triggers no workflow at all — not a
> skipped run, not a red one: none. Because a landed version bump is the publish, such a merge loses the release
> silently, with no failing check to read. That is what happened to #95: it bumped the workspace to
> `26.8.22-hotfix.23`, `main` moved, and `deploy` never ran.
>
> The App holds no `workflows` permission on purpose, so a pull request touching `.github/workflows/**` still cannot
> be auto-merged and is merged by hand. Arming also requires at least one *required* status check: with nothing
> required, the mutation is refused with `Pull request is in unstable status`, because auto-merge has nothing to wait
> on.
>
> Dependabot has a separate secret store, so mirror both App secrets there. Forks do not use this path.

### TDD and the pre-commit gate

- Tests share a commit with the implementation. Rust or runtime changes require:

  ```bash
  cargo fmt
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- Prose-only changes require:

  ```bash
  cargo run -p cli -- validate <path>
  ```

- After each PR update, clean task-owned builds, KIND, browser, images, and build cache. Never prune volumes without
  approval.

## CI/CD workflows

Add jobs to the workflow that owns their trigger; do not create a redundant workflow.

The Rust dependency cache is written and read by the merge gate itself, on the branch being tested. Actions caches are
scoped per ref, so the first push of a new branch compiles the third-party graph from zero and every later push of that
branch restores it. One workflow owns the entry it reads, so there is no second file to keep in agreement.

The Actions cache is a single 10 GB budget shared by every cache in the repository, and an entry evicted before the next
push reads it is indistinguishable from one never written. That budget is why `deploy.yml`'s `build` job exports a
`type=gha` cache only on the legs carrying a `ci_cache_scope` — the scopes `publish-service` reads back. A `mode=max`
export of a Rust builder stage carries the whole `target` directory, so one leg nobody reads is enough to starve the
gate. Before adding a `cache-to` anywhere, name the reader.

Read cache health from a job log, not from the API. Every job on this fleet logs `Enabling Blacksmith transparent cache`
and Blacksmith serves the Actions cache protocol from its own store, so `/actions/cache/usage` and `/actions/caches` can
both report zero while every gate run restores a full match. Grep the job for `Cache hit`, `No cache found`, and `full
match` instead — that is the only authoritative signal.

| Workflow | Trigger | Job |
| --- | --- | --- |
| `.github/workflows/ci.yml` | `pull_request` → `main` | Rust quality gate |
| `.github/workflows/deploy.yml` | a push to `main`, or a `kind-ci/**` branch | prove + tag + publish images |
| `.github/workflows/ghcr-retention.yml` | 01:11 UTC nightly, or a dispatch | prune old GHCR versions |
| `.github/workflows/codeql.yml` | `pull_request` → `main`, and `push` → `main` | CodeQL scan |

### CodeQL is enabled

The advanced CodeQL workflow scans pull requests targeting `main` and pushes to `main`. The pull-request scan supplies
early feedback before merge; the post-merge scan refreshes the default-branch alert inventory so fixed findings close
after the fix lands.

The repository is public, so CodeQL code scanning is available without GitHub Code Security. The workflow uses standard
`ubuntu-latest` GitHub-hosted runners, which are free for public repositories.

Keep the workflow enabled. If GitHub reports it as disabled, re-enable it with:

```bash
gh workflow enable CodeQL
```

The CodeQL checks are not *required* — the `production` ruleset requires only `ci` — but a failing check still makes the
pull request's overall status roll up to `FAILURE`, and auto-merge will not fire against a failing rollup. A real CodeQL
finding therefore needs to be resolved before auto-merge can land the pull request.

### One protection system, not two

`main` is governed by **rulesets** alone — `production`, plus `release-tags` on the tag target. A legacy classic
branch-protection rule was configured as well, and the pair did not compose: the classic rule carried
`requiresApprovingReviews: true` with a required count of `0`, which leaves `reviewDecision` at `null` forever and holds
every pull request at `BLOCKED` no matter how green its checks are. It was deleted.

Nothing was given up in the trade. The rulesets are the stricter of the two — they require signed commits and a passing
`ci`, neither of which the classic rule asked for. Keep protections in rulesets, where `ops github setup` can reconcile
them; a classic rule added by hand is invisible to that command and will drift.

`production` keeps its empty `bypass_actors`, so the classic rule's admin enforcement is preserved for everything that
must hold universally. The review-only bypass does not release any of these rules — see [Review gate: two rulesets with
a narrow bypass](#review-gate-two-rulesets-with-a-narrow-bypass).

One caveat learned the hard way: GitHub caches a pull request's merge state. Changing branch protection does **not**
recompute it for pull requests whose checks have already finished — they stay `BLOCKED` until some later event on the
pull request forces a fresh evaluation. Push a commit, or close and reopen, after changing protection.

### When `gh pr merge` refuses but the merge is legal

`gh pr merge --squash` can refuse with *"the base branch policy prohibits the merge"* on a pull request that GitHub will
merge without complaint. The CLI runs its own pre-flight against `mergeStateStatus`, and that field reads `BLOCKED`
whenever the status rollup is failing — including when every failing check is optional. The API applies the real policy
instead:

```bash
gh api -X PUT repos/neon-law-source-code/navigator/pulls/<n>/merge -f merge_method=squash
```

Confirm the required check is genuinely green first, because this bypasses the CLI's guess and nothing else:

```bash
gh pr view <n> --json statusCheckRollup \
    --jq '.statusCheckRollup[] | select(.isRequired) | "\(.name) \(.conclusion)"'
```

### PR flow — `ci.yml`

`ci.yml` runs for PRs to `main`, never pushes. The `rust` job carries the quality gate: formatting, the repository-wide
`navigator` content validation pass, `cargo clippy` with warnings denied, and `cargo test --workspace`. It runs on
`blacksmith-4vcpu-ubuntu-2404`, and every other job on the pull-request path stays on stock `ubuntu-latest`, which is
free for a public repository. Four vCPU is a measured choice rather than a default: Blacksmith bills linearly in cores,
more than half of this job is test execution bounded by `--test-threads 4` rather than by cores, and the workflow's own
header comment carries the two-arm measurement and the arithmetic. The Rust tests need no database service — each opens
its own embedded engine. `ci.yml` is the only workflow that runs on `pull_request`, and it carries no KIND, Docker, or
browser coverage — that proof happens on the release train (and locally, see below), never on a PR. The workflow is the
source of truth for commands, caches, and pinned tool versions.

The `ci` job is the required status check — see [One required check, named `ci`
everywhere](#one-required-check-named-ci-everywhere). It runs nothing, `needs:` the `rust` job, and fails unless it
succeeded. It tests the dependency's result explicitly rather than relying on a bare `needs:`, because a skipped
required check is not a red one: GitHub reports no result at all, so the gate would quietly stop blocking exactly when
the job it guards had failed.

`deploy.yml` no longer has a `pull_request` trigger. It previously ran its KIND integration job against UI-scoped PRs so
Dioxus/browser changes got production-shaped proof before merge; that coupled every PR to the release workflow's script
and image builds. UI and browser changes are instead verified locally before opening a PR — see [Local KIND
development](../CLAUDE.md#local-kind-development) and the `web-preview` / `kind-local-dev` skills — and a tagged release
(or a `kind-ci/**` branch push, below) remains the CI-side KIND proof.

### One workflow owns publishing — `deploy.yml`

Publishing is a deliberate act, and the act is **landing a version bump on `main`**. Bump `[workspace.package].version`
in an ordinary pull request, merge it, and that push proves the workspace in KIND, builds every image, pushes them to
GHCR, creates the immutable release tag, attaches the three `navigator` CLI archives to that tag's GitHub Release, hands
the release to the Homebrew tap, and reports what it published.

**The version in the manifest is the release, and that is what makes a version trustworthy.** Three triggers have owned
this pipeline. A cron and a `workflow_dispatch` each derived a version from the runner clock, so the name an image
carried stood behind no Git ref — which is how `Cargo.toml` sat at `0.1.0` while published images marched on under names
the source had never heard of. A hand-pushed tag fixed the drift but paid for it in ceremony: four validations existed
purely to re-establish facts a bare ref cannot carry, and each of them, failing late, spent an immutable name.

Reading the version out of the merged manifest answers all four by construction:

| The old check | Why it is gone |
| --- | --- |
| SHAPE — a `YY.M.D` regex | `semver::Version::parse` is the shape, and it is stricter than the regex was |
| DATE — the tag equals today's UTC date | a bump merges days after it is authored, so this punished slow review |
| MANIFEST — the tag equals `[workspace.package].version` | the tag is *derived from* it; they are one decision |
| PROVENANCE — the tag's commit is reachable from `main` | a push to `main` **is** the provenance |

What survives is the one question none of them asked, and it is now the whole gate: **is this version newer than every
version already published?** `navigator ops release check` reads the manifest, lists every release tag, and compares
them with semver's own ordering. Three answers:

| Answer | What happens |
| --- | --- |
| newer than every released version | this is a release: build, prove, tag, publish |
| equal to the newest | already released — the run ends in seconds. Almost every merge |
| older than the newest | **the job fails.** A bad bump, or a rebase that resurrected an old manifest |

**`release-version` runs a published `navigator`, not one it compiles.** Answering a yes/no question by building the CLI
cost ~8 minutes of latency on every merge to `main`, at the very front of the train where nothing else can start. The
job downloads the newest release that carries a Linux CLI archive, and runs `ops release check` with it. `ci.yml` still
runs the in-tree command on every pull request, so the rule is proved on the branch that changes it.

Two consequences worth stating rather than discovering. **The checker is release N-1's**, so a change to `ops release
check` itself governs from the release after the one that lands it — tolerable because the binary carries the rule
while the run supplies the data, reading this commit's manifest and the current tag list. And **the binary is
deliberately unpinned**, the one exception to [Pin every consumed image, binary, and
action](#pin-every-consumed-image-binary-and-action): a checker that had to be pinned would freeze at one version and
need a manual bump to ever move. `/releases/latest` is *not* how it is found — that endpoint excludes prereleases and
every release here is one, so it answers 404; the job enumerates releases and takes the newest carrying the archive. A
download that fails falls back to compiling from this tree, because a release lost to a blipped API is the failure this
pipeline has already paid for once.

The version threads into every image build as the `RELEASE_TAG` build-arg, which each Containerfile turns into the
runtime environment variable `NAVIGATOR_RELEASE_TAG`.

**`YY.M.D` is a convention, not a rule.** Nothing validates the calendar any more. The naming convention is still
`YY.M.D`, optionally suffixed with a prerelease, and the [`cut-release`](../.agents/skills/cut-release/SKILL.md) skill
is where it is written down — but a version that departs from it publishes just as well, provided it is newer than the
last one. What the date really bought was uniqueness, and comparing against the tags buys that directly.

**`ops release-default-tag` computes today's name, so nobody has to by hand.** When the operator running `cut-release`
names no version, this command prints today's `YY.M.D` on stdout — or nothing, when a version at or past it is already
published, which is not an error. It sits upstream of `release-version` rather than inside it: `release-version` still
requires an explicit `--tag` and still derives nothing, for the reason above the table — a clock-derived name is a fact
about when a command ran, not an operator decision. This command only supplies the candidate a human would otherwise
have worked out by counting days since the last release; naming the release is still `--tag`'s job.

Three shape facts still hold, because they are semver's:

- **No leading zeros.** August is `8`; `26.08.22` is not a version at all, and neither is `-hotfix.08`.
- **Three components exactly.** Cargo parses `[workspace.package].version` as strict semver, so a fourth component
  (`26.8.22.13`) cannot be written into the manifest — which is why the historical `YY.M.D.H` spelling is retired
  everywhere, including in the registry parser that once ordered it.
- **No build metadata.** `+` is not a legal character in an OCI image tag, and its precedence is not portable: the spec
  says to ignore it, the `semver` crate orders by it. `cli/src/release.rs` refuses it rather than depending on either.

**Ordering replaced the hotfix rules.** A prerelease ranks *below* its own base version (semver §11.3), so the whole "a
hotfix hangs off tomorrow's date" rule was a hand-written description of what `Version::cmp` already computes. Comparing
against the highest released version makes it a consequence instead:

```text
26.8.21  <  26.8.22-hotfix.3  <  26.8.22-hotfix.21  <  26.8.22
```

So `26.8.22-hotfix.3` is admissible after `26.8.21` and **refused** after `26.8.22`. The hyphen is exactly why — see
[Why a hotfix prerelease ranks below its date](#why-a-hotfix-prerelease-ranks-below-its-date).

**This workflow deploys nothing, and holds no cloud credential.** It ends at the registry. Putting a version in front of
real clients' matters is a separate act a person takes from their own machine — see [The deploy is a human
act](#the-deploy-is-a-human-act).

**Run the browser gate locally before you merge the bump.** A green `ci` proves the Rust workspace and says nothing at
all about the browser and accessibility suites: they self-skip when no harness is present, so the only thing that runs
them on CI is `deploy.yml`'s `integration` job. So prove it first:

```bash
cargo run -p cli -- dev browser-e2e
```

A `kind-ci/<topic>` branch push is the CI-side alternative when the change is to the workflow itself rather than to a
page — it runs `integration` alone, creates no tag, and publishes nothing.

**A failed release no longer costs its name.** This is the reason the tag is created *inside* the pipeline rather than
pushed ahead of it. The tag job sits between `integration` and every publisher, so:

- **A failure before the tag** — a red build, a red KIND suite — creates no ref and publishes nothing. Re-run it; the
  version keeps its name.
- **A failure after the tag** — a registry flake, a tap rejection — leaves an immutable ref, but a re-run republishes
  the same name over itself. There is nothing to un-publish, because nothing was deployed.
- **A wrong source** spends that version for good. Bump past it and merge again.

`release check` reports a version whose tag already names *this very commit* as publishable, so re-running the whole
workflow republishes rather than skipping every job and reporting success for having done nothing.

### Releasing twice in one day

Nothing forbids it, and no special spelling is required — the calendar is not a rule. Two ordinary ways to name a second
release the same day, and both are just "a bigger number":

| After releasing | A valid next version | Why |
| --- | --- | --- |
| `26.8.22` | `26.8.23` | tomorrow's date, cut early |
| `26.8.22` | `26.8.23-hotfix.1` | a prerelease of tomorrow's |

Both sort strictly above `26.8.22`, so both are admissible. What is **refused** is a prerelease of a version already
released — `26.8.22-hotfix.1` after `26.8.22` — because semver ranks it below the release it would be fixing, and every
consumer resolving those two versions would read the fix as the older one. `ops release check` says so by name rather
than letting it publish.

### Why a hotfix prerelease ranks below its date

The hyphen starts a **prerelease** identifier. `26.8.22-hotfix.1` is not "26.8.22 plus a fix"; it is an earlier,
unstable form of `26.8.22`, the same construct as `1.0.0-rc.1`. Semver §11.3 gives a prerelease lower precedence than
the matching normal version:

```text
26.8.22-hotfix.1  <  26.8.22  <  26.8.23-hotfix.1  <  26.8.23
```

After `26.8.22` is published, a same-day cut must bump the **core** — `26.8.23` or `26.8.23-hotfix.1`. A fourth numeric
component (`26.8.22.1`) is not a version Cargo can parse, and build metadata (`26.8.22+hotfix.1`) cannot name an image
tag, so neither is an escape. `ops release check` refuses the older spelling as a regression before the pipeline spends
a tag.

`N` in `-hotfix.N` is an unpadded nonnegative integer and it is the operator's to choose: a uniqueness-and-ordering
discriminator, never an hour. The padding is not cosmetic — semver forbids a leading zero in a numeric prerelease
identifier, so `hotfix.08` is not a version. Nothing derives `N`; `ops release version` writes the name it is given.

Write the version the same way as any other release, then land it:

```bash
cargo run -p cli -- ops release version --tag 26.8.23-hotfix.1
```

**A prerelease does not become the default download.** Exactly one thing behaves differently from an ordinary release,
because a prerelease must not present itself as the latest version to someone browsing the releases page:

| Surface | Ordinary release | prerelease |
| --- | --- | --- |
| GHCR images and CLI archives | published | published |
| GitHub Release | latest | flagged `--prerelease` |
| Homebrew tap | bumped | bumped |

Which versions count as prereleases is no longer a spelling rule the workflow knows: `release check` reports it from
`Version::pre`, so `-hotfix.3` and `-rc.1` are both flagged.

**The tap follows every publishable version, prerelease included.** It holds exactly one version and every `brew
install` resolves to it, so the version it holds has to be the newest build that exists — not the newest build of a
particular shape. Excluding prereleases meant the formula could only move when an ordinary release succeeded end to end,
and a run of ordinary releases failing at the KIND gate left `brew install` serving a 404 for days with every check
green, because a skipped job is not a failed one.

What made the exclusion look necessary is real, but it belongs to the tap: **Homebrew's comparator is not semver.** It
orders `26.8.20-hotfix.4` *above* `26.8.20`, the reverse of the §11.3 ranking above. A formula walked from a prerelease
to its own base version therefore looks like a downgrade, and `brew` reports the keg as current instead of upgrading it.
`scripts/bump.sh` in the tap closes that with `version_scheme` — Homebrew's own mechanism for a version series that
stops sorting forward — comparing each new tag to the outgoing one with Homebrew's comparator and incrementing the
scheme whenever the new tag does not sort strictly above. Every bump is an upgrade, whatever the shape of either tag.

A prerelease is still a full release in every way that matters to a deploy: it proves the workspace in KIND, publishes
every image, and hands the operator the same `ops ship` command.

**The bump carries `Cargo.lock` too.** `navigator ops release version --tag <version>` writes
`[workspace.package].version` — the value every crate inherits through `version.workspace = true` and `cli/build.rs`
bakes into `navigator --version` — and refreshes `Cargo.lock` in the same commit. Every workspace crate is pinned there
as well, and the archive jobs build with `--locked`, which refuses a lock the manifest has moved past. `ci.yml` runs
`cargo metadata --locked` on every pull request for exactly that reason: it is the last place that failure is free.

`--tag` is required, because naming a release is the operator's decision and a derived name is only ever a fact about
when the command ran. That commit lands through an ordinary PR — `main` takes no direct commits.

**The release preflight is a required check now, not a habit.** On every pull request `ci.yml` runs `ops release check`,
`ops notices --check`, and the `--locked` lock check. All three lived only in the `cut-release` preflight script, run on
the operator's machine, skippable by forgetting; the merge is what publishes now, so the pull request is the last point
at which any of them is still free to fix.

`ops notices --check` was held back at first, because reading licence texts out of the runner's unpacked
`$CARGO_HOME/registry/src` made the answer a property of the machine: cargo unpacks a crate there only when something
needs it, so a build unpacks the platform it built for, and `notices_for` recorded every crate this runner had never
unpacked exactly like one that ships no licence file. Two changes fixed that. The command now refuses an unread registry
rather than folding it into the no-licence-file list, and the step runs `cargo fetch --locked` before it, which with no
`--target` unpacks every target platform's graph rather than the pair the build steps happen to need. The set it reads
is therefore `Cargo.lock`'s, the same on a macOS laptop and a Linux runner, and the `THIRD-PARTY-NOTICES.txt` shipped
inside every CLI archive names every crate rather than the ones the last build happened to need. That fetch is
load-bearing in both places it appears, the CI step and the `cut-release` preflight script, and is not tidiness to be
dropped.

**One job creates a ref, and it can only create.** `release-tag` holds `contents: write` to create `refs/tags/<version>`
at the merged commit; `release-windows-cli-publish` holds it to create the GitHub Release against that tag. Neither can
move one: the `release-tags` ruleset restricts deletion, update, and non-fast-forward with no bypass actor, and the job
refuses a tag that already exists at a *different* commit rather than forcing it.

**The tag is created inside the same run, and that is a constraint rather than a preference.** A tag created with the
built-in `GITHUB_TOKEN` does not trigger another workflow's `on: push: tags`, so a job that created the tag and handed
off to a second workflow could not work — a separate `release-tag.yml` once cut the tag as the `navigator-release` App
purely to defeat that recursion guard. One workflow, two triggers, no App identity.

### What each stage does — `deploy.yml`

The release run proves the workspace in KIND and publishes all service and trigger images, plus three `navigator` CLI
archives attached to the GitHub Release hanging off the tag the run created: `navigator-<tag>-windows.zip`,
`navigator-<tag>-linux.tar.gz`, and `navigator-<tag>-macos.tar.gz`. Each carries the executable beside `LICENSE`.
Container images are **linux/amd64 only**; GKE Autopilot consumes amd64. The macOS archive is arm64 — `macos-latest` is
Apple silicon — so an Intel Mac still builds the immutable release tag locally with Cargo, and the `#navigator` report
carries that exact command beside the three downloads. Failure at any stage pages `#navigator`.

**Every publishing run builds all three CLI archives, and Project CI depends on them.** `release-windows-cli-build`,
`release-cli-build-linux`, and `release-cli-build-macos` declare the same `needs: [integration, release-version]` and
the same `publishable` gate the two publish jobs do, so they run whenever a run publishes images and skip whenever one
does not (a `kind-ci/**` branch iteration). That puts the CLI in the same stage as the GHCR push rather than in a lane
beside it: the archives compile only once the KIND e2e, interop, and browser/accessibility suite is green. A release
names one version across the image tags, the three archives, `navigator --version`, and the Release page, and nobody
reading that version can tell which of those the e2e run stood behind — so all of them wait for it, and "e2e-proven"
describes the whole release instead of only the images. It also stops occupying three runners, one of them a 90-minute
cold Windows compile, on every release whose integration job then goes red — those runners are free on a public
repository, but the archives they produce have no possible consumer once the Release is never cut. Nothing reaches a
stranger early either, because `release-windows-cli-publish` needs both publish jobs as well as the three archives — the
Release is the first fetchable artifact of the run. This is not only for human downloads: the `.github/actions/validate`
composite action, the gate **every** Project repository runs, downloads `navigator-<version>-<platform>` from the
Release these jobs cut. If they stop running, Project CI breaks everywhere with a download 404 and nothing in this
repository goes red — which is exactly the kind of failure worth stating in prose, because no test here will catch it.
The macOS archive existed nowhere until it was added: `validate` had always mapped a macOS runner to `platform=macos`,
so that download 404'd for every Project repository whose gate ran on one.

**The three archive jobs run on the free GitHub-hosted runners** — `windows-latest`, `ubuntu-latest`, and
`macos-latest`. Public repositories are not billed for any of them, including the macOS and Windows classes a private
repository pays a multiplier for, so all three platforms cost the same as the Linux one: nothing.

### The Homebrew tap

`brew install neon-law-source-code/navigator/navigator` installs the CLI, and
[`neon-law-source-code/homebrew-navigator`](https://github.com/neon-law-source-code/homebrew-navigator) is the tap it
resolves. On a Mac it is the **recommended** path, not a convenience: the released binary is unsigned and unnotarized,
and Gatekeeper blocks an unsigned Mach-O downloaded through a browser outright. Homebrew fetches with `curl`, which sets
no `com.apple.quarantine` attribute, so the same bytes run. Signing remains the right fix; the tap is what stands in
until it lands.

`release-homebrew-tap` is the hand-off. It needs `release-windows-cli-publish`, so it fires only once the Release
actually carries the archives, and it sends a `repository_dispatch` naming the tag **and nothing else**. The tap
computes every `sha256` itself by downloading the artifacts it will then tell readers to download. A payload carrying
digests would let a malformed dispatch pin the formula to bytes nobody verified, and would leave the tap unable to
repair a bad bump from a bare tag — which matters, because `YY.M.D` admits no second ordinary release the same UTC day,
so a bump that went wrong cannot be fixed by re-cutting the release it came from. The tap covers that with a
`workflow_dispatch` that re-runs any tag by hand.

**A separate repository, not a folder here.** A tap is a Git repository Homebrew clones and re-reads on every `brew
update`, and its formula changes once per release, mechanically, with no review to add. Keeping it here would mean
either a bot commit to a protected `main` or a PR nobody reads, and would put a full workspace clone in front of every
`brew update`.

The dispatch authenticates with `HOMEBREW_TAP_TOKEN`, a fine-grained token scoped to `contents: write` on the tap and
nothing else — the run's own `GITHUB_TOKEN` cannot reach another repository, and widening it to one that could would
hand that reach to every job in the workflow. **A missing or rejected token fails the release**, deliberately: a tap
that silently stops updating serves a stale version to everyone who installed through it while nothing anywhere goes
red, which is the Project-CI 404 one channel over. `cli/tests/homebrew_tap_dispatch.rs` holds the contract, because the
two repositories never reference each other.

Two platforms have no prebuilt archive — Intel macOS and arm64 Linux — and the formula compiles the immutable source tag
for them instead. The tap's own CI installs the formula on all four platforms, gating every push on the two prebuilt
ones and running the two source builds weekly, since a cold workspace compile is tens of minutes.

The run narrates itself while it goes when `SLACK_WEBHOOK_URL` is configured. Every forward-path step opens with a
`.github/actions/slack-progress` post to `#navigator` naming the tag, the stage, and the step, so the channel watches
the release advance rather than waiting ~45 minutes for a verdict — and the last line posted names the step a failure
died in, before anyone opens the run. All Slack reporting is advisory: an absent webhook notices and skips, and a
rejected post warns, because a release must not be lost to its own narration. GitHub's run conclusion remains the source
of truth. The posts self-gate on the trigger ref the same way the two terminal reports do, so a `kind-ci/**` branch
iteration stays silent. Steps gated on `failure()` or `always()` are post-mortem diagnostics rather than progress and
are deliberately not narrated; the failure page already covers that moment when Slack is configured.
`cli/tests/deploy_slack_progress.rs` holds the narration complete — a new step added without a post fails that gate,
because nobody notices a *missing* Slack line.

### What detects a broken pipeline

**Nothing on a clock does.** The nightly train doubled as a daily liveness check on the whole release path: images still
build, KIND still stands up, `ops ship` still authenticates. A defect in the publishing stages is invisible until
someone next bumps the version. And the cron was never a reliable signal even while it ran — a silent nightly failure
went unnoticed for four consecutive nights — so the honest statement is that this pipeline has no automatic breakage
detection, not that it has a weaker one.

One part of it did get cheaper. `deploy.yml`'s first job now runs on **every** merge to `main`, so a break in the
release trigger itself — the manifest read, the tag comparison, the guard binary — surfaces at the next merge rather
than at the next release. The build, KIND, tag, and publish stages still wait for a real bump.

What remains, and what each does not cover:

- When `SLACK_WEBHOOK_URL` is configured, `notify-failure` pages `#navigator` when a release fails, reading the trigger
  ref rather than a job output, so a failure anywhere — including the release decision — still pages. Reading the ref
  costs nothing in false positives: a merge carrying no bump has nothing to fail, because the decision job succeeds,
  reports `publishable=false`, and every other job skips. It can only fire on a run that happened; without the optional
  webhook, the GitHub run conclusion is the signal.
- `kind-ci/**` proves a release-workflow change on demand: push a `kind-ci/<topic>` branch to run the KIND integration
  job alone, creating no tag, publishing nothing and shipping nothing. On demand, not on a schedule.
- `ci.yml` proves the Rust workspace on every PR, plus the three release preflight checks — `ops release check`,
  `ops notices --check`, and `cargo metadata --locked`. It still says nothing about images, KIND, or shipping.

Two consequences to plan around rather than discover:

- A `kind-ci/**` push is the cheapest way to exercise the pipeline without releasing. It is the periodic check the cron
  used to be, and it now has to be a habit rather than a trigger.
- **Image retention does not depend on release cadence**, because a count floor sits under its age rule — see [Image
  retention](#image-retention). Age alone would let a quiet fortnight delete the versions production was running;
  keeping the last ten versions of every image removes that failure mode rather than documenting it.
- `ghcr-retention.yml` is on a clock, but it proves nothing about the release path: it prunes the registry and never
  builds, publishes, or stands up KIND. It pages `#navigator` on its own failure, which is a signal about retention, not
  about whether a release would work today.

### Recovering a failed release

Three lanes, cheapest first.

1. **Re-run the failed jobs** from the run's page, or dispatch `deploy.yml` again. The version derives from the same
   UTC day either way, so a re-run republishes that same name over itself. This is the move for a flake: a runner disk,
   a registry timeout, a wedged port-forward. Nothing was deployed, so there is nothing to un-deploy.
2. **`ops ship` the already-published tag.** If the images published green and only a roll failed, rebuild nothing:

   ```bash
   navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
   ```

   This is [The manual deploy](#the-manual-deploy). `ship` builds nothing, refuses a tag absent from the registry, and
   `--dry-run` rehearses it first.
3. **Fix forward the same day with a `-hotfix.N` tag.** If the source is wrong, land the fix on `main`, bump it with
   `ops release version`, passing the `-hotfix.N` name you chose as `--tag`, and tag the merged commit. See [Releasing
   twice in one day](#releasing-twice-in-one-day) for the shape and for the two surfaces a hotfix deliberately does not
   touch.

**The day's `YY.M.D` name is still spent, and the tag still cannot move.** The `release-tags` ruleset restricts
deletion, update, and non-fast-forward with no bypass actor, and no second `YY.M.D` exists for that day — so a same-day
fix takes a *new* name rather than a moved one. Deleting and re-pushing a tag is never the answer: a moved tag makes
every artifact already carrying that version a lie. Rolling back with `ops ship --tag <previous>` remains the right move
while a fix is still being written, since a hotfix has to be proven before it is worth shipping.

### Keyless pushes to GHCR

The publish jobs hold no registry key, no PAT, and no cloud credential. Every image goes to `ghcr.io/<owner>`, and the
login is the run's own `GITHUB_TOKEN`:

```yaml
- name: log in to the image registry
  uses: docker/login-action@v4.6.0
  with:
    registry: ghcr.io
    username: ${{ github.actor }}
    password: ${{ secrets.GITHUB_TOKEN }}
```

That works because the repository, its Actions, and its registry are one product: github.com mints the token and
`ghcr.io` is github.com's own registry. The token is issued per run and expires with it, so there is nothing to
configure, nothing to rotate, and nothing to leak. `packages: write` is the entire grant.

**The grant is per job, not workflow-wide.** The top-level `permissions:` block holds `contents: read`; only
`publish-service` and `publish-triggers` add `packages: write`, so no other job in the release can push an image. A
fork's run receives a read-only token and fails at the push rather than somewhere subtler.

**No Workload Identity Federation is involved, and a test holds that line.** There is no `google-github-actions/auth`
step, no `navigator-ci-pusher` service account, and no attribute condition pinning `assertion.repository` — the pool,
the provider, and the impersonation binding the Artifact Registry path needed all retired with it, along with the issuer
subtlety that made a provider report `ACTIVE` and then fail every exchange.
`cli/tests/deploy_workflow.rs::deploy_workflow_ships_nothing_and_holds_no_cloud_credential` asserts that neither
`google-github-actions/auth` nor `workload_identity_provider` appears anywhere in `deploy.yml`, so the credential path
cannot creep back in unremarked. `publish-service` still requests `id-token: write` and no step consumes it — a leftover
of the retired path, not a second one.

**A fork changes one variable, not three.** `cli::devx::registry::DEFAULT_REGISTRY` is `ghcr.io/neon-law-source-code`
and `NAVIGATOR_IMAGE_REGISTRY` overrides it. The Artifact Registry path needed a region, a hub project, and a repository
name, any two of which could disagree and still render a syntactically valid reference to somewhere no image had ever
been pushed.

**The Google Cloud image hub survives in the CLI and governs nothing this repository publishes.** `ops gcp hub setup`
still provisions a GAR repository, the `navigator-ci-pusher` service account, and a GitHub Workload Identity pool, while
`ops gcp setup` still runs a "container registry access" stage and `--images-project-id` still writes a cross-project
`roles/artifactregistry.reader` binding. Nothing in the publish or pull path reaches any of it: CI pushes to GHCR, and
`ops ship` renders `ghcr.io/<owner>` into every `image:` line. Treat that machinery as unused for images rather than as
a second live lane, and do not reconcile it expecting it to affect a release. Removing it is a scoping exercise rather
than a delete: `artifact_registry.rs` also hosts the WIF helpers that `marketing.rs`, `app_publisher.rs`, `kms.rs`, and
`secret_manager.rs` genuinely use.

### Image retention

Published images are pruned by `.github/workflows/ghcr-retention.yml`, at 01:11 UTC nightly — the slot the release train
held before publishing moved off the nightly clock. GHCR offers no server-side retention rule, so a workflow is the only
place this can live. Its credential is the run's own `GITHUB_TOKEN` with `packages: write`: no PAT, nothing to rotate,
and no cloud provider.

**A version must clear three independent floors to be deleted, and the count floor is the load-bearing one.** Age alone
is only safe while releases outrun it. Under the nightly train every running tag was a day old, so the old
`delete-older-than-7d` rule could never reach one; with releases driven by tags, a quiet month is ordinary and age alone
would delete the exact versions production is running. Serving pods survive that — they already pulled — but a restart,
a reschedule, or a node replacement cannot pull its image, and `ops ship` refuses a tag the registry no longer holds,
which is also the documented rollback. So the sweep deletes a version only when *all three* hold:

| Floor | Rule | Why |
| --- | --- | --- |
| Age | older than `CUTOFF_DAYS` (30) | a version has to be genuinely old to qualify |
| Count | outside its image's newest `RETAINED_VERSIONS` (10) | a count cannot expire, so cadence stops mattering |
| Tag | not the version carrying `latest` | deleting it orphans a published pointer, failing at pull time |

The count is per image, so each keeps its own newest ten rather than ten across the registry. One release pushes one
version per image under two tags (`YY.M.D` and `latest`) — one digest, one version — so ten versions is ten releases.

**The sweep may only touch packages this repository publishes, and it names them rather than discovering them.** A GHCR
package is owned by the *organization*, and the org owns packages other repositories push, so deleting by age across
everything the org holds would prune those too — on a clock, with nothing going red. The workflow therefore carries an
explicit `PACKAGES` list, and `cli/tests/ghcr_retention.rs` holds it equal to the images `deploy.yml` publishes: a new
image joins the sweep in the same commit that starts publishing it, and a retired one cannot linger aimed at a package
this repository no longer owns.

Naming them is also what lets the credential stay `GITHUB_TOKEN`. The discovery call, `GET /orgs/{org}/packages`, is
reachable only by a classic PAT holding `read:packages` — an Actions token is answered 403 however the permissions block
is written, which is why every scheduled sweep failed on that one line before the list replaced it. The per-package
version *listing* is a different lane, and the run's own token reaches it.

**Deleting needs package-admin access.** Every published Containerfile carries the OCI source label for
`neon-law-source-code/navigator`, so GHCR links a newly published package to that repository and its workflow token can
receive the required access without a PAT. **A repository move does not repair packages that already exist:** reconnect
each migrated package to the canonical source repository, then grant it `admin` under Package settings → "Manage Actions
access". A missing link or grant makes every delete answer `404 Package not found`, which is also what a version another
run already removed answers, so the sweep prunes nothing while reading like routine noise. The run's failure branch
separates the two: when *nothing* was deleted it names this grant instead of counting warnings.

**Rehearse a change before a night runs it live.** Dispatch the workflow with `dry_run: true` (the dispatch default) and
it lists every deletion it would make and deletes nothing. That is the only safe way to prove a change to a job whose
mistakes are unrecoverable, and `cli/tests/ghcr_retention.rs` guards the floors, the scope bound, and the `#navigator`
page so none of them can be dropped quietly.

Change retention by changing `CUTOFF_DAYS` or `RETAINED_VERSIONS` in the workflow; the guard test pins both literals, so
a change there is a change the test makes you state.

**Artifact Registry's `cleanupPolicies` are a separate, unused lane.** `navigator ops gcp hub setup` still PATCHes a
count-based `KEEP`/`DELETE` pair onto a GAR repository (`cli::devx::gcp::artifact_registry`, `RETAINED_VERSIONS = 10`),
and GHCR never reads it. Nothing publishes to that registry any more — `cli::devx::registry::DEFAULT_REGISTRY` is
`ghcr.io/neon-law-source-code` — so that policy governs whatever the GAR repository still holds and nothing this
repository ships.

## Pin every consumed image, binary, and action

**Every consumed image, binary, and action is immutable.** Publishing `latest` is allowed; consuming it is not.

Embedded Rego policy is load-bearing: `cli/tests/regorus_policy.rs` compiles the production source and runs every
checked-in policy rule. The Regorus version and policy source ship together in the web binary; upgrading either is a
deliberate, tested change.

- **Images** (`image:`, `FROM`): pin an explicit version tag, never `latest` or another rolling tag, and confirm the tag
  still exists on the registry we pull from before pinning.
- **Installer binaries** (a workflow step's `version:`): pin the version, never `latest` — `latest` also round-trips a
  release API that has 500'd and killed a job. The one exception is `release-version`'s own checker, which must follow
  the newest release to be useful at all; see [One workflow owns publishing](#one-workflow-owns-publishing--deployyml).
- **Third-party GitHub Actions** (`uses:`): pin the full commit SHA with a trailing `# vX.Y.Z` comment, per GitHub's
  guidance — a bare `@v2` resolves to a branch tip upstream can force-push.

`navigator validate` rejects mutable consumption under `k8s/`, `examples/`, `images/`, and `.github/workflows/`;
`deploy.yml` publication sites are exempt.

## Publish vs. roll out

The publish jobs cannot mutate Google Cloud or a cluster, and neither can anything else in `deploy.yml`. Rolling is a
separate act under a separate identity — a person's, not the pipeline's.

Every roll target is a directory in the repository's `deployments/` tree: one `ops ship` run per directory, staging
first, then production, every deployment on the same tag.

Staging is the only gate on the way to production, and it is the only one that earns its place. It runs the same
`neon-server` image over a sample data plane, so a failure there is evidence about the version rather than about real
people's matters — which is exactly what a canary has to be. Nothing rolls it on your behalf any more, so it is a step
the operator takes before the row clients are on rather than one a run reports.

**Publishing and rolling are separated because they answer to different things.** Publishing is mechanical: the
workspace either builds and passes its gates or it does not, which is a question a cron can settle. Rolling puts a
version in front of people whose legal matters are in it, which is a judgement — and the moment a green pipeline made
it, the record of what production runs stopped being a decision anyone took and became a side effect of whoever merged
last.

### The deploy is a human act

**No workflow in this repository can roll a cluster.** `deploy.yml` ends at the registry, holds no Google Cloud
credential, and requests no `id-token: write`. That is a security boundary rather than a preference: a pipeline that can
roll production is a pipeline whose compromise rolls production, and CI's remaining reach is a registry push.
`cli/tests/deploy_workflow.rs` asserts both halves — no job named `ship*`, and no credential exchange step — so
restoring that reach means deleting an assertion that says why it was removed.

When Slack is configured, the handoff is a Slack message. After publication, `notify` posts what was published, CLI
installation instructions, and the `ops ship` command with the version already substituted. Without the optional webhook
those advisory reports skip successfully; the GitHub Release and workflow run remain the durable handoff.

That second message enumerates nothing. It once derived a line per row from the tree at run time (`ls deployments`) and
read each row's public host out of its `config.toml`; the tree moved, so both would now fail inside a Slack step whose
failure nobody reads as "the tree moved". It names no deployment either — a row is rollable because its directory
exists, and this repository cannot see whether one does, so every public instruction takes a placeholder.
`no_public_source_instructs_a_deployment_by_name` in `cli/src/devx/deployments.rs` fails the build if a name drifts back
in.

Nor does it name the repository that holds the tree. The `DEPLOY_REPO` Actions variable carries that, so renaming or
replacing the deploy repository is a variable change rather than a pull request here; unset, the message says "your
deployments checkout" instead of interpolating a blank. The coupling runs one way only — the deploy repository's own
workflow hardcodes this one and derives the release tag from its own clock, and nothing here reaches back.

Then a person runs it, from their own machine, against their own short-lived credentials:

```bash
gcloud auth application-default login
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
```

**Rolling back is the same command with an older version.** `ops ship` neither knows nor cares which direction a version
moves; it reconciles the deployment onto the tag it is given. The only requirement is that the images still exist — see
[Image retention](#image-retention), which keeps the last ten versions for exactly this.

### The manual deploy

To roll outside a release or promotion run — a re-roll, a rehearsal, a rollback, or a deployment neither workflow
reached — run one `ops ship` per directory:

```bash
navigator ops ship --deployment <row> --deployments-dir . --tag YY.M.D
```

`--deployment` is required and reads every coordinate from `deployments/<name>/config.toml` — never from the shell, so a
stale environment cannot select the wrong deployment. `ship` builds nothing. Before a full or image-only roll it checks
every embedded slide-media key directly in that deployment's `NAVIGATOR_ASSETS_BUCKET`. It then validates the secrets,
reconciles manifests, rolls every service and trigger to one tag, and re-registers Restate. After a secret rotation, a
`--restart-only` ship restarts the pods without changing the version; that lane is manual only. See
[`cloud-operations.md`](cloud-operations.md) and [`gke-prod.md`](gke-prod.md#trust-boundary).

Forks that run a GitOps controller (Config Sync, Argo CD, Flux) can let the controller reconcile the manifests instead
of running `ship`. This repository has no controller and no deploying workflow: `ops ship`, run by a person, is the
whole rollout path.
