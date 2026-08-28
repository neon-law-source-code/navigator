# Agent workflows

Codebase work has five actions: **create an issue**, **triage an issue**, **create a PR**, **address a PR comment**, or
**address a failed GitHub Action**. GitOps, validation, Restate authoring, and council review support those actions.

Every action starts from the same evidence: [`glossary.md`](glossary.md), the narrowest docs from
[`index.md`](index.md), the current source and tests, and the complete issue or PR conversation. Choose the smallest
change that satisfies that evidence.

## Create an issue

Planning lives in Linear, on the Engineering team and inside a Linear project. GitHub holds code, pull requests, review,
and CI; neither surface duplicates the other. After reading the relevant docs, code, and tests, state the observed
problem, grounded scope, acceptance criteria, test-driven steps, and blast-radius files.

When an unknown blocks a grounded scope, write the smallest throwaway Rust spike that can answer it. Record the command,
observation, and conclusion in the issue, then discard the spike. A spike proves a fact; it is not the implementation.

Persist durable decisions in code, docs, or the glossary when the PR lands.

## Triage an issue

Read the issue from the opening body through every comment and follow how the request evolved. Reproduce the current
behavior where practical, reconcile the ask with the glossary, docs, source, and tests, then comment a test-driven plan
that names the minimum implementation and exact blast-radius files a future worktree should start from.

## Create a PR

Follow [`gitops.md`](gitops.md): branch, push, open a PR, then let squash auto-merge land it. Never commit to `main` or
bundle work outside the issue's acceptance criteria.

Before changing files:

1. Start the task with **New Worktree** in Codex or Claude. Confirm that `pwd -P` appears as a non-primary entry in `git
   worktree list --porcelain`. A worktree is the task's isolated checkout; its topic branch is the PR reference. If this
   check fails, stop before changing anything and say: **“This task was not started in a New Worktree. Please click New
   Worktree and start it again.”** Do not create a second worktree to repair it.
2. Name the PR branch and prepare this task's environment once:

   ```bash
   cargo run -p cli -- dev worktree-env up --branch <topic-branch>
   ```

   The CLI fetches `origin/main` and attaches or creates the topic branch in the current linked worktree — including
   Codex's normal detached `HEAD`. It creates `.worktrees/<topic>` only when deliberately run from the primary checkout
   outside the app workflow. Continue in this one task checkout; never create a nested worktree by hand.
3. Read [`CLAUDE.md`](../CLAUDE.md), [`AGENTS.md`](../AGENTS.md), the narrowest docs from [`index.md`](index.md), and
   [`glossary.md`](glossary.md). Read [`access-model.md`](access-model.md) before touching roles, participation,
   embedded Rego, sessions, or visibility.
4. Run `git status --short --branch`; preserve user changes.

If the decision is architectural, legal-copy, or client-facing, use the relevant council in
[`agent-decision-councils.md`](agent-decision-councils.md) after reading the facts.

When a dirty tree is ready to land:

1. Survey `git status --porcelain`, `git diff`, `git diff --staged`, and untracked files.
2. Group paths by concern: one blast radius per commit.
3. Run the matching gate. Markdown changes require the workspace pass:

   ```bash
   cargo run -p cli --quiet -- validate .
   ```

   The client-data gate rides along with the workspace test suite in step 4; there is no separate command.

4. If the PR changes Rust files or build/runtime configuration, run the full Rust gate:

   ```bash
   cargo fmt
   cargo clippy --workspace --all-targets -- -D warnings
   cargo nextest run --workspace
   cargo test -p features
   ```

   Verify coverage locally; green tests do not prove the 90.6% workspace line floor. Match CI's coverage topology: Each
   test opens its own embedded store; tests requiring KIND, Rauthy, Garage, Restate, or a browser skip and contribute no
   coverage:

   ```bash
   cargo llvm-cov --workspace --fail-under-lines 90.6 \
     --ignore-filename-regex '(cli/src/devx/(browser_e2e|chrome|e2e|garage|orchestrate|staging)|features/src/webdriver)\.rs$'
   ```

   This local number reads below CI's real number, so treat it as a differential sign check, not proof against the
   floor: it skips doctests, which CI covers with a separate `cargo test --workspace --doc` step, and it excludes the
   `features` crate's cucumber suites, which CI runs with `cargo test -p features` and folds into the same coverage
   counters.

   The floor covers the whole workspace, not the diff, and may pass uncovered additions. Give each handler, route, and
   branch a non-gated router test against an embedded store; use browser e2e only as live proof. Explain genuinely
   unreachable lines in the PR. See [`test-database.md`](test-database.md).

5. Stage explicit paths for each group, not `git add -A`.
6. Use Conventional Commit subjects; use the PR title as the squash-merge commit title.
7. For public or portal UI, capture the running app with headless Chrome and embed the artifact from
   `/tmp/navigator-screenshots/` in the PR description through `pr-image-upload`; never commit it, self-host it, or use
   a raw `/tmp` URL. Rendering tests are not live proof. For authenticated pages, follow
   [`AGENTS.md`](../AGENTS.md#authentication-and-lawyer-access): grant lawyer against `web`'s database and authenticate
   through Rauthy, never a hand-written cookie.
8. Push and open a PR against `main`, linking its Linear issue with a bare `Closes ENG-NN` trailer in the body — see
   [Linking a PR to its Linear issue](#linking-a-pr-to-its-linear-issue). Auto-merge lands it once the required checks
   pass and its review threads are resolved. CI enables auto-merge on open — do not run `gh pr merge` yourself; let
   auto-merge land it.
9. Clean up task-owned local resources before ending the session. See [Resource cleanup](#resource-cleanup).

If the work should become multiple PRs, decide that before committing. Use the Engineering Council for real sequencing
questions.

### Linking a PR to its Linear issue

Link the PR to the issue it satisfies, and link it by identifier alone. Linear holds the product roadmap, and this
repository is public, so a branch name, PR title, PR body, or commit message is published permanently. `ENG-1234` is
opaque — it names a number and nothing else. An issue title, a project or initiative name, or a `linear.app` URL is not:
the URL path carries the title as a slug, so pasting one publishes that title even though opening the link needs an
account.

Put one magic-word trailer in the PR **body**, carrying nothing but the identifier:

```text
Closes ENG-1234
```

Linear links the PR on open and completes the issue on merge. The closing magic words are `close`, `fix`, `resolve`,
`complete`, and `implement` with their inflections; to link a PR without moving the issue on merge, use a contributing
word instead — `ref`, `references`, `part of`, `related to`, `contributes to`, `towards`. Several issues take one list,
`Closes ENG-1234, ENG-1235`. Linear reads the PR title and body but never comments or commit messages, so the trailer
belongs in the body the PR opens with. Keep the identifier out of the title: that title becomes the squash-merge
subject, and it stays a clean Conventional Commit.

Nothing else about the roadmap goes into a public surface:

- no `linear.app` URL, in a PR body, commit message, review reply, code comment, or doc — its slug is the issue title;
- no issue title, project name, initiative name, milestone, or cycle, quoted or paraphrased;
- no branch name copied from Linear's **Copy git branch name**, which appends that title slug. Name the branch
  yourself as `<initials>/eng-1234-<short-neutral-topic>`: the identifier is the part Linear matches, and the rest is
  yours to keep bland.

Write the PR for a reader with no Linear account — the change and its reasoning in full, the roadmap it belongs to
nowhere. A PR whose justification lives only behind its issue link is unreviewable in public anyway.

Linear can write onto the PR too, and that half is not the agent's to control: where linkbacks are enabled, the GitHub
integration comments on the linked PR with the issue title and description, withholding them — identifier and link alone
— only for a private team. It posts none on this repository today: a PR that did link and did auto-complete its issue
carries no Linear comment at all. So the trailer is safe as long as that holds, which is a workspace setting rather than
anything this tree gates. Re-verify it rather than assuming it, and raise a change in it rather than routing around it.

### Grouping changes into commits

Keep one concern and its proof together: handler + view + test, migration + dependent entity, or generated artifact +
source (`docs/erd.svg` + `docs/erd.md`). Split different blast radii, unrelated fixes, tooling from product code, and
docs from code unless they describe that exact change. Remove superseded paths and history narration.

Write each subject as a Conventional Commit: `<type>(<scope>): <subject>`, imperative mood, lower-case start, no
trailing period, ≤72 chars. `<scope>` is the crate or area (`web`, `store`, `cli`, `views`, `deps`, `mcp`). Append `!`
after the type/scope or add a `BREAKING CHANGE:` body trailer for a breaking change. The PR title is the squash-merge
commit subject, so write it as the Conventional Commit you want in `main`'s history.

| type | when |
| --- | --- |
| `feat` | a new capability or user-visible behavior |
| `fix` | a bug fix |
| `refactor` | behavior-preserving restructuring (rename, move, extract) |
| `docs` | docs, prose, or README only |
| `test` | tests added or changed in isolation (usually folded into `feat`) |
| `chore` | tooling, deps, skills, housekeeping |
| `ci` | `.github/workflows/` and CI plumbing |
| `perf` | a performance improvement |
| `style` | formatting only, no code change |
| `build` | build system, Containerfiles, or Cargo manifests (non-dep) |

### GitHub CLI authentication

Navigator is public at [`neon-law-source-code/navigator`](https://github.com/neon-law-source-code/navigator) on
github.com. `gh` defaults to that host, so no `--hostname` and no `GH_HOST` is needed, and a command that names a host
should be reviewed rather than copied.

`gh` drives PR creation, review, and image uploads. Check auth before remote work:

```bash
gh auth status
```

If auth is missing, use the browser device-code flow with SSH as the git protocol:

```bash
gh auth login --git-protocol ssh --web
```

Relay the printed one-time code verbatim, keep the process alive through browser login, then recheck auth.

GitHub policy reconciliation runs one repository at a time: `navigator ops github setup [repository]` takes an
`owner/name` slug, falling back to `GITHUB_REPOSITORY` and then this checkout's `origin`. There is no `--all` mode —
reconcile deliberately, one at a time. Start with `--dry-run`, and rerun it after applying: a second dry run reporting
no drift is the only proof the reconcile converged. See [`gitops.md`](gitops.md#codified-merge-gate) for the policy each
repository receives.

## Address a PR comment

Against the PR head, adjudicate the requested claim, make and prove only its minimum valid fix, then reply and resolve
that thread.

1. **Identify** the PR and requested comment.
2. **Read** the PR, the thread, the cited source, and the covering test at the head commit.
3. **Reproduce** behavioral claims against the right KIND-backed topology when practical.
4. **Adjudicate** the comment as valid, invalid, or valid-but-not-worth-changing, with evidence.
5. **Apply** only the valid comment's minimum fix and covering test.
6. **Prove** the fix with the affected gate and running behavior.
7. **Reply and resolve** the handled thread through GitHub.
8. **Report** the verdict, commit, proof, and anything explicitly left open.

### Identify the PR and comment

Use a named PR; otherwise resolve the PR for the current branch:

```bash
gh repo view --json nameWithOwner -q .nameWithOwner   # the {owner}/{repo} for this checkout
gh pr view --json number -q .number                   # the PR whose head is the CURRENT branch
```

If lookup is ambiguous, run `gh pr list --json number,title,headRefName` before asking. Pass the resolved slug to every
call as `--repo <owner>/<repo>`.

### Read the PR head

```bash
gh pr view <N> --repo <slug> \
  --json title,body,state,author,baseRefName,headRefName,additions,deletions,changedFiles,mergeable,reviewDecision
gh pr diff <N> --repo <slug>     # full diff; scope to the files you care about if it is large
```

The diff is the claim; complete files, callers, and tests at the head commit are the evidence.

### Bring up the PR worktree and right KIND environment

Fetch the PR ref and prepare its dedicated worktree:

```bash
git fetch origin pull/<N>/head:pr-<N>
cargo run -p cli -- dev worktree-env up --branch pr-<N>
```

Continue in the printed checkout. Source its `.devx/env`, boot `web` as in
[`AGENTS.md`](../AGENTS.md#default-worktree-loop), and restart the compiled process after a fix. Host `web` and the
in-cluster worker share that worktree's database and Restate journal; use `--demo` only when published images are the
subject.

### Read the requested thread in context

Read the finding and replies; paginate inline comments:

```bash
gh api --paginate repos/<slug>/pulls/<N>/comments \
  --jq '.[] | {id, user: .user.login, path, line, original_line, diff_hunk, in_reply_to_id, body}'
gh api --paginate repos/<slug>/issues/<N>/comments --jq '.[] | {id, user: .user.login, body}'
gh pr view <N> --repo <slug> --json reviews -q '.reviews[] | {author: .author.login, state, body}'
```

### Adjudicate the comment against the running code

An inline root has no `in_reply_to_id`; non-null values are replies. Reproduce behavioral claims and classify:

- **Valid** — the claim holds against the real code (and reproduces when run). Confirm severity; note the exact fix.
- **Invalid / false positive** — the claim is wrong (missed context, an intentional codebase-wide pattern, a "bug" that
  can't occur). Note *why*, with file:line evidence.
- **Valid but won't-fix** — real but not worth changing (matches a file-wide pattern, guarded elsewhere). Note the
  rationale.

### Define the minimum fix

Name the minimum fix and covering test before editing. Ask before expanding scope. Reply to invalid and won't-fix
findings with evidence, without changing code.

### Apply and prove the fix

Implement only the defined fix:

- Add and run its covering test in the same change. A lying test must exercise the named path and assert evidence unique
  to that path.
- Restart and click through UI or behavior changes. Embed requested captures through GitHub user attachments, never a
  raw `/tmp` path.
- Run the workspace gate. Coverage findings require the CI-equivalent `cargo llvm-cov` pass and a non-gated router test,
  not harness-gated e2e alone; see [Create a PR](#create-a-pr).
- Commit on the branch as a Conventional Commit referencing the finding, and push so CI re-runs:

```bash
git add <paths> && git commit -m "test(web): exercise the real client-DRI guard (Greptile P2 on #<N>)"
git push origin HEAD:<headRefName>
```

### Reply and resolve the handled thread

Reply to the finding, then resolve only its thread. For inline comments:

```bash
gh api repos/<slug>/pulls/<N>/comments/<comment_id>/replies -f body='Fixed in <sha> — <one line>.'
# or, for a won't-fix / false positive:
gh api repos/<slug>/pulls/<N>/comments/<comment_id>/replies \
  -f body='Acknowledged, not fixing — <rationale with file:line evidence>.'
```

For summary-only findings:

```bash
gh pr comment <N> --repo <slug> --body 'Fixed the P2 "<finding title>" from the summary in <sha>: <what changed>.'
```

REST replies do not resolve review threads. List GraphQL thread ids, then resolve each answered thread:

```bash
gh api graphql -f query='
query($owner:String!,$repo:String!,$pr:Int!){
  repository(owner:$owner,name:$repo){ pullRequest(number:$pr){
    reviewThreads(first:100){
      pageInfo{ hasNextPage endCursor }
      nodes{ id isResolved comments(first:1){ nodes{ databaseId author{login} path line } } } } } }
}' -F owner=<owner> -F repo=<repo> -F pr=<N>

gh api graphql -f query='mutation($id:ID!){ resolveReviewThread(input:{threadId:$id}){ thread{ isResolved } } }' \
  -F id=<threadId>
```

Match the first comment's `databaseId`, not author + path. Follow `pageInfo.endCursor` while `hasNextPage` is true.

### Keep the scope narrow

Do not update from `main`, fix another thread or check, or refactor adjacent code unless the requested comment requires
it. Report separate work under its matching action.

### Report

Report the verdict, change and commit, proof, reply, resolution, and untouched blockers.

## Address a failed GitHub Action

Treat a failed Action as a narrow task and start from its first actionable failure.

1. Identify the PR and list its checks with `gh pr checks <N> --repo <slug>`.
2. Read the failed job and step logs with `gh run view <run-id> --log-failed`. The first actionable error is the lead;
   later cancellations and cascaded errors are symptoms until proven otherwise.
3. Read the workflow step, the source it invokes, and its covering test. Reproduce the exact command locally when the
   environment permits it.
4. Make the smallest root-cause fix. Do not combine a CI repair with reviewer comments, dependency refreshes, broad
   formatting, or unrelated warnings.
5. When the failure exposes a behavior gap, add the non-gated covering test that would have caught it. Do not add a test
   merely to restate an infrastructure outage.
6. Re-run the failed command and the directly affected gate. For a coverage failure, run the `cargo llvm-cov` pass from
   [Create a PR](#create-a-pr) — CI exports no lcov, so the per-file table it prints is the diagnostic — and read it for
   the file that regressed.
7. Commit, push, and report the root cause, minimum fix, local proof, and any unrelated failing checks left untouched.

`.github/workflows/ci.yml` owns PR commands; [`gitops.md`](gitops.md) maps release and integration workflows. Propose,
but do not run, production or irreversible cloud operations.

## Supporting checks

### Markdown lint

Use the workspace CLI, not a separate Markdown linter:

```bash
cargo run -p cli --quiet -- validate <path>
```

`validate` applies prose rules to ordinary Markdown, N-family rules to notation templates, typed-event checks, and YAML
parsing. It walks directories and defaults to `.`. CI runs:

```bash
cargo build -p cli --quiet
./target/debug/navigator validate .
```

### No client data in the repo

Read [`public-contributor-safety.md`](public-contributor-safety.md) before using an example, fixture, issue, or planning
surface. Only firm-owned or synthetic data may ship. Non-firm email addresses must use `example.com` or a reserved
`.example`, `.invalid`, or `.test` domain. Phone numbers may not ship. Client or matter data, legal files, and
production identifiers belong in Navigator-managed systems, never Git or external planning surfaces.

The gate scans `store/seeds`, `templates`, and `server/content`; source and test fixtures remain human-reviewed. It is a
test, not a command — `cli/tests/no_client_data.rs` runs the scan over the real tree, so the required workspace test job
enforces it:

```bash
cargo nextest run -p cli -E 'binary(no_client_data)'
```

A failure names every scanned leak as `path:line NCD-EMAIL/NCD-PHONE "value"`. The test is a guard for its scan scope,
not permission to place sensitive material in a path it does not inspect.

### Legal workflow authoring

For a new matter type or workflow extension, prefer a template + questionnaire + workflow over a one-off router:

1. Write the composition `.feature` first in `features/tests/features/`.
2. Create or edit the template under `templates/forms/...` or `templates/neon_law/<product>/...`.
3. Add new questions to `store/seeds/Question.yaml`.
4. Compose the workflow from documented step prefixes in [`notation-authoring.md`](notation-authoring.md).
5. Add reusable `StepKind` and dispatch code only when the existing step registry cannot express the work.
6. Put every external or non-deterministic side effect behind Restate durability.
7. Add tests in the same commit as the implementation.

The Template declares; Restate runs.

### Restate handler authoring

See [`durable-workflows.md`](durable-workflows.md). The replay-safety rule is:

> Every non-deterministic act belongs inside `ctx.run(...).name("stable-name")`.

This includes clocks, randomness, UUIDs, database writes, storage, network calls, and third-party APIs.

Use terminal errors for invalid input that can never succeed later. Use retryable errors for infrastructure failures. Do
not use native `tokio::spawn`, `join_all`, or channels for journaled steps inside a Restate handler; use Restate SDK
sequencing/combinators or keep the steps sequential.

### GitOps and deploy

Read [`gitops.md`](gitops.md), [`gke-prod.md`](gke-prod.md), and [`cloud-operations.md`](cloud-operations.md) before
changing CI, releases, deploys, clusters, or production secrets. Roll `navigator-web` and `workflows-service` together;
version skew is a production risk.

### Resource cleanup

Clean task-owned resources before ending a create-PR, address-comment, or failed-Action session.

For Cargo builds:

- For Markdown-only tasks, avoid Cargo commands that create a worktree `target/`.
- If Rust checks or e2e tests created build artifacts in the task worktree, run `cargo clean` in that worktree after
  pushing the branch or updating the PR.
- Clean a task-specific `CARGO_TARGET_DIR`; never delete shared Cargo caches or targets.

For Docker, KIND, and browser e2e:

- At handoff, run `navigator dev worktree-env down`, then stop task-owned `web` and browser processes. Do not use `dev
  down` for routine cleanup; it deletes the reusable shared cluster. See
  [`AGENTS.md`](../AGENTS.md#troubleshooting-and-cleanup).
- A deleted worktree can leave its KIND cluster, ports, and Docker memory behind. Find and reclaim only such orphans:

  ```bash
  cargo run -p cli -- dev worktree-env sweep
  cargo run -p cli -- dev worktree-env sweep --apply
  ```

  `sweep` is dry-run; `--apply` deletes only clusters without a live checkout, never the shared cluster, live worktree
  clusters, or volumes. Prefer `worktree-env down` while the checkout exists. Ownership comes from the host registry
  that `up` writes and `down` clears, with git worktrees as fallback.
- Remove task-created containers and images. After image-heavy work, use the narrowest matching build-cache prune.
- Run `docker system df` before broad cleanup. `docker system prune` removes stopped containers, unused networks,
  dangling images, and build cache; `-a` also removes unused images.
- Never prune Docker volumes without explicit approval.

Measure before and after cleanup when disk pressure is part of the task (`df -h .`, `docker system df`, or both), and
report anything left running or left on disk.

### Maintenance support

- Dependency refresh: follow the Rust crate and web asset sections in [`rust-programming.md`](rust-programming.md) and
  the vendored asset rules in `server/public/VENDOR.toml`.
- ERD refresh: regenerate [`erd.md`](erd.md) and `docs/erd.svg` together after schema changes. Government forms use
  canonical issuing-authority sources and keep provenance in [`gov-forms.md`](gov-forms.md). For disk cleanup, measure
  first and never delete Docker volumes without approval.
