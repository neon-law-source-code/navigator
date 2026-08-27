---
name: create-pr
description: >
  Turn a dirty working tree into a clean pull request against `main`: survey every change, group the files into
  Conventional Commits by blast radius (one concern per commit), run the gate, branch off `main`, capture a visual for
  any user-visible change, push, and open the PR. Trigger when the user says "/create-pr", "create a PR", "open a pull
  request", "commit and PR these changes", "group these into commits and ship them", or has a dirty working tree they
  want landed. Stops at "PR open"; building images and deploying to prod is a separate flow.
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
- Run the matching gate first, and open the PR from a green tree. For a Markdown change, validate the file; for Rust,
  run the workspace gate:

  ```bash
  cargo run -p cli -- validate <path>
  cargo fmt
  cargo clippy --workspace --all-targets -- -D warnings
  cargo nextest run --workspace && cargo test -p features
  ```

  Total line coverage stays ≥ 90.6%, and the default nextest profile prints failures only.
- **Measure coverage locally before pushing** — a green `cargo test` reports pass/fail, and coverage is a separate read.
  The floor rides inside the `cargo test (workspace)` check (`cargo llvm-cov --fail-under-lines 90.6`). Harness-gated
  browser/e2e tests (`new_client_or_skip`) skip in CI's coverage pass, so code covered *only* by them counts as
  uncovered; give handlers and routes a non-gated covering test through the router, and spin up just what CI's coverage
  job does — the OPA binary the policy tests use — to measure. A test that needs the full KIND stack skips in that job
  like the e2e, so it counts as uncovered too. The floor measures the whole workspace, so cover what you wrote yourself.
  See the full note in the doc's [Create a PR](../../../docs/agent-workflows.md#create-a-pr) gate.
- Group by blast radius: one reviewable concern per commit, staging each path explicitly.
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
- **Leave auto-merge to CI.** Push, `gh pr create --base main`, report the PR URL, and stop. CI enables auto-merge on
  open, and it lands once the gate is green and its review threads resolve.
