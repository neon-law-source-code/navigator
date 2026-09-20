---
name: create-pr
description: >
  Turn a dirty working tree into a clean pull request against `main`: survey every change, group the files into
  Conventional Commits by blast radius (one concern per commit), run the gate, branch off `main`, capture a visual for
  any user-visible change, push, and open the PR ready for review, not as a draft. Trigger when the user says
  "/create-pr", "create a PR", "open a pull request", "commit and PR these changes", "group these into commits and ship
  them", or has a dirty working tree they want landed, and as the ship step of
  [`implement-issue`](../implement-issue/SKILL.md). Stops at "PR open"; building images and deploying to prod is a
  separate flow.
---

# create-pr

Before preparing a public PR, read [`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).
The branch, commit, PR body, capture, and every linked planning surface use only firm-owned or synthetic content; client
data, legal files, real contact details, and production identifiers never leave Navigator-managed systems.

One shared skill for Claude and Codex. The workflow lives in the docs; this file points at it so both tools run the same
steps. Read, in order:

- **[docs/agent-workflows.md → Create a PR](../../../docs/agent-workflows.md#create-a-pr)** — the whole flow: survey,
  group into commits (with the grouping heuristics and Conventional Commit type table), run the gate, branch, commit,
  capture a visual, push, open the PR. Includes the `gh auth` recipe for an unauthenticated shell.
- **[docs/gitops.md](../../../docs/gitops.md)** — branch → PR → auto-merge mechanics: `main` is squash-merge-only, and
  CI arms auto-merge when the PR opens.

Load-bearing rules from those docs:

- Start every change in a Codex or Claude **New Worktree**, then run `navigator dev worktree-env up --branch <topic>`
  once. The CLI names that linked worktree's PR branch in place, and creates a sibling only when deliberately started
  from the primary checkout outside the app workflow.
- Run the matching gate first, and open the PR from a green tree. **The gate follows the diff, not the habit.** Every
  PR owes the tree-wide gate:

  ```bash
  cargo run -p cli -- project gate
  ```

  A PR that touches Rust scope owes the cargo gate on top of it:

  ```bash
  cargo fmt
  cargo clippy --workspace --all-targets -- -D warnings
  cargo nextest run --workspace && cargo test -p features
  ```

  Total line coverage stays ≥ 90.6%, and the default nextest profile prints failures only.
- **Let CI's own scope test decide what "touches Rust" means.** The `changes` job in
  [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml) classifies the diff and skips `cargo test (workspace)`
  outright when nothing matches: `*.rs`, `*.surql`, `*.feature`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  `.cargo/`, `.config/nextest.toml`, `features/`, or `ci.yml` itself. A pure Markdown, YAML, or asset PR therefore never
  runs the Rust suite in CI, so running it locally proves nothing that CI will check — run `project gate` and push. Read
  that job's globs rather than guessing; it fails open, so an unreadable diff runs Rust anyway.
- **A content change can still be a Rust change.** Prose compiled into the binary is asserted by tests —
  `neon/content/*.md` by [`server/tests/host_legal_pages.rs`](../../../server/tests/host_legal_pages.rs), locale
  catalogs by `views::locales`. Before calling a Markdown PR Markdown-only, `git grep` a distinctive phrase you removed;
  a hit in a `.rs` file means the diff now carries Rust and takes the full cargo gate.
- **Measure coverage before pushing** — a green `cargo test` reports pass/fail; coverage is a separate read, taken by
  `cargo llvm-cov --fail-under-lines 90.6` inside the `cargo test (workspace)` check. CI's coverage pass skips
  harness-gated tests (`new_client_or_skip`, anything needing the KIND stack), so code covered *only* by those counts as
  uncovered. Give handlers and routes a non-gated test through the router. The floor is a workspace total and can stay
  green while your change goes uncovered, so cover what you wrote. Full note in the doc's [Create a
  PR](../../../docs/agent-workflows.md#create-a-pr) gate.
- Group by blast radius: one reviewable concern per commit, staging each path explicitly.
- **If the change removes anything, sweep before pushing.** `git grep -n '<removed-name>'` comes back empty apart from
  the test that guards its absence. Manifest entries, fixtures, and doc prose are where a half-removal hides; see
  [[rust]].
- **Link the Linear issue by identifier, and by nothing else.** Put one magic-word trailer in the PR body — `Closes
  ENG-1234` — so Linear links the PR and completes the issue on merge. Keep the identifier out of the PR title, which
  becomes the squash-merge subject. The roadmap stays private even though the code is public, so no `linear.app` URL
  (its path carries the issue title as a slug), no issue title, project, initiative, milestone, or cycle name, and no
  branch name copied from Linear's **Copy git branch name**, which appends that slug — name the branch
  `<initials>/eng-1234-<short-neutral-topic>` yourself. See [Linking a PR to its Linear
  issue](../../../docs/agent-workflows.md#linking-a-pr-to-its-linear-issue), which also carries the one exposure this
  discipline cannot close: Linear's own linkback comment.
- Capture a live walkthrough of any user-visible change into `/tmp/navigator-screenshots/`, look at it yourself, and
  embed it in the PR body via [[pr-image-upload]] (one `curl` to the tenant's `user-attachments` store, authenticated by
  `gh auth token`). The artifact lives in `/tmp` and the PR body links it; reach for `curl` rather than the `gh-image`
  extension, which cannot target this host. **Default to a GIF of the real interaction** ([[web-preview]] §5); use a
  still when the change is genuinely static, with no keypress, click, or state transition to show. A GIF carries the
  input between states that a before/after pair leaves out.
- For authenticated screenshots, follow the worktree login flow in
  [`AGENTS.md`](../../../AGENTS.md#authentication-and-lawyer-access): grant lawyer against the same store as `web`, then
  sign in through Rauthy for a real session cookie.
- **Audit the teaching surfaces (advisory).** Before pushing, run [[author-docs]]. It reads the docs, inline comments,
  tests, and workshops against what this branch changed and reports any that describe something the code has moved past.
  Fix the confirmed drift or escalate per its routing, and update the surface that owns each changed fact in the same
  commits. Findings are advisory.
- **Leave auto-merge to CI.** Push, `gh pr create --base main` with no `--draft`, report the PR URL, and stop. Open it
  ready for review, not as a draft: auto-merge is armed only on a non-draft open, and a draft sits until someone marks
  it ready. Use `--draft` only when the user asks to hold the PR. CI enables auto-merge on a ready open, and it lands
  once the gate is green and its review threads resolve.
