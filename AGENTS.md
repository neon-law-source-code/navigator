# Neon Law Navigator

**Neon Law Navigator** is copyright **Shook Law PLLC** (the Firm), which trades as **Neon Law**, operates it, owns the
NEON LAW mark, and is the sole Licensor. **This is a public, source-available repository — not open source** — at
[github.com/neon-law-source-code/navigator](https://github.com/neon-law-source-code/navigator): one grant, `BUSL-1.1`,
over the whole tree including the legal prose under `templates/`. Root [`LICENSE`](LICENSE) holds that grant — the
Business Source License 1.1 with its parameters filled in and its terms otherwise unaltered, so every licence scanner
names it; [`NOTICE`](NOTICE) beside it carries the copyright line and the Firm's own statements, and is where any
wording of ours belongs. **The parameter that matters operationally is `Additional Use Grant: None`** — non-production
use is free, and running Navigator to deliver legal services to other people needs a commercial licence from the Firm.
Each version converts to `AGPL-3.0-only` four years after it is published, and § 13 then obliges an operator who
modified that version to offer those users — its own remote users — the corresponding source. Copies distributed under
the AGPL stay AGPL permanently. Outside contributions are **currently closed** — point anyone asking at
`contact@neonlaw.org` — though the work in here assigns to the Firm. The marks are reserved. This monorepo holds one
website — the firm at the root — and the delivery stack for legal services. See
[`docs/licensing.md`](docs/licensing.md).

**Everything you write here is published.** The no-client-data rule below is what stands between a live legal practice
and a public tree, and it is now enforced by a test rather than by the absence of a publication path.

This file is the short operating contract for agents. [`CLAUDE.md`](CLAUDE.md) is a symlink to it. The linked docs are
authoritative: read the narrowest relevant doc before acting and keep durable detail there, not here.

## Architecture invariants

- **Navigator owns machine-bound flows.** The `navigator` CLI orchestrates every machine-bound flow; there are no shell
  scripts or Makefile. Add automation as a Rust binary under `cli`, a CLI subcommand, or a crate. Rust owns rules,
  notation, workflows, forms, billing, storage, authorization, `store`, and the CLI. Rust owns the browser surface
  through Dioxus; generated PDFs use Typst and transactional email uses string templates. There is no Node or pnpm
  workspace. See [`docs/workspace-layout.md`](docs/workspace-layout.md).
- **English only.** Code, comments, `/docs`, portal UI, emails, and legal template bodies are English, and no page
  publishes a translated surface. Copy is written directly in the Rust module that renders it; there is no copy catalog
  or key lookup. A visitor is free to machine-translate marketing copy, but a translated legal *questionnaire* is a
  different risk: Spanish intake would be a questionnaire-level decision with attorney review, never a side effect of
  loosening this invariant.
- **No client data in the repository.** Shipped data contains only firm-owned or synthetic identities. Non-firm email
  addresses use reserved example domains, and phone numbers do not ship. The workspace test suite enforces this on every
  PR. See [`docs/agent-workflows.md`](docs/agent-workflows.md#no-client-data-in-the-repo).
- **No client matter is ever named in writing that leaves the practice.** A real Project code *is* a client identifier:
  it names who retained the firm, and a repository name, an object prefix, a bucket listing, and a portal URL all carry
  it. So no commit message, branch name, code comment, test fixture, document, issue, pull-request title or body, or
  agent transcript names a real client, matter, or Project code — not even in passing, and not even where the surface
  feels internal. Issues and PR descriptions are the easy mistake, because they read as private and are not. Write about
  the *mechanism* instead: "a Project's publish", "four publisher identities", "one matter's prefix". Where an example
  is genuinely needed, use a synthetic code (`acme`, `sample-litigation`) or the seeded sample matters, which are
  invented. Describing live state — how many Projects publish, when they last did — is fine as long as none of them is
  named. If a real code is already written somewhere, do not propagate it; say where it is and let a human decide.
- **Name only staging in this repository.** The published tree documents how Navigator deploys, and one worked example
  is enough. Use the staging deployment — GCP project `neon-law-stg`, host `staging.neonlaw.com` — in every doc,
  comment, fixture, and test. Production project ids, bucket names, hosts, and service-account emails are not secrets,
  but they are a map of where client bytes live, and a public tree is the wrong place to draw it.
- **Live debugging goes to staging, never production.** When an answer needs a running system — reading a bucket,
  inspecting IAM, driving a browser, tailing logs — use the staging deployment. It is provisioned and shipped
  identically, so what reproduces there reproduces in production. `kubectl` and `gcloud` frequently sit on a production
  context by default: check with `kubectl config current-context` and `gcloud config get-value project` before the first
  command, not after. A read against production needs the user to ask for it in that turn; a write to production is
  propose-only, always.
- **Production-shaped local development.** Local development runs the dependency tier in KIND; every cloud deployment is
  persistent and provisioned, shipped, and configured identically, with staging a role in the release order rather than
  a reduced topology. Ephemeral environments are for development only. Do not replace the KIND topology with ad hoc
  local services. Follow [Local KIND development](#local-kind-development).
- **One AIDA catalog.** AIDA exposes the tools in `mcp/src/tools/` through A2A and MCP. A new router implements
  `portal::agent_router::AgentRouter`; it never forks the catalog. See
  [`docs/aida-a2a-interaction.md`](docs/aida-a2a-interaction.md).

## Ground every action

Start with [`docs/glossary.md`](docs/glossary.md), then use [`docs/index.md`](docs/index.md) to find the narrowest
source of truth. Read the relevant issue or PR from its first comment, the current code, and the covering tests. Do not
plan from assumptions, a diff alone, or leftover local state. Choose the smallest change that satisfies the evidence.

## Local KIND development

The Rust CLI owns the complete local lifecycle. Docker, KIND, `kubectl`, Helm, and the Restate CLI must be installed,
and `docker info` must succeed. Do not add shell-script wrappers or substitute one-off local containers for the
Kubernetes topology.

### Worktree-first code changes

Every code change starts in **New Worktree** in Codex or Claude. A worktree is the isolated task checkout; a topic
branch is still the PR's Git reference. They are complementary, not alternatives. Codex starts its worktrees at a
detached `HEAD`; Claude may create a branch with its worktree. That difference is normal.

Before the first edit, inspect `git worktree list --porcelain`. The current `pwd -P` must be a non-primary `worktree`
entry. If it is not, do not create a branch, a worktree, or edit files; stop and say: **“This task was not started in a
New Worktree. Please click New Worktree and start it again.”** Never repair that mistake by manually creating another
checkout.

A worktree opens at whatever `main` pointed to when the app created it, and that tip is usually stale by the time the
work is ready to push. Start current and stay current: fetch and rebase onto `origin/main` before the first edit, and
again before every push.

```bash
git fetch origin
git rebase -S origin/main
```

Rebase rather than merge — PRs squash to one commit and merge commits are disabled, so a merge commit only has to be
unwound later. Sign the rebase: commit verification recognizes the `nick@neonlaw.com` identity, and an unsigned commit
cannot enter the merge queue. A stacked PR rebases onto `origin/main` too, never onto the previous branch's tip; once
its predecessor squash-merges, that tip describes a commit that no longer exists on `main`.

In the app-created worktree, run the CLI once with the PR topic. It attaches or creates that branch **in this
worktree**, including a detached Codex worktree; it does not create a second task checkout. The CLI creates a sibling
`.worktrees/<topic>` checkout only for an intentional command started from the primary checkout outside Codex or Claude.

```bash
cargo run -p cli -- dev worktree-env up --branch <topic>
```

### Default worktree loop

After the task branch exists, use its isolated KIND dependency tier when editing `web`:

```bash
cargo run -p cli -- dev worktree-env up --path "$PWD"
set -a; source .devx/env; set +a
cargo run -p neon
```

`worktree-env up` creates or reuses a KIND cluster keyed to the worktree path, applies the SurrealDB schema, assigns a
stable worktree port slot, and writes `.devx/env` plus `.devx/worktree.json`. The tier includes SurrealDB, Rauthy,
Garage, Restate, `workflows-service`, and telemetry: the host `web` process and its worker therefore share one store and
one Restate journal, while parallel worktrees do not. Source the generated environment before every local command that
must target this checkout. It is the complete local application environment; use a gitignored `.env` only for optional
live third-party sandbox credentials.

Useful lifecycle commands:

```bash
cargo run -p cli -- dev worktree-env status --path "$PWD"
cargo run -p cli -- dev worktree-env down --path "$PWD"
```

`worktree-env down` removes this checkout's port-forwards, KIND cluster, and `.devx` state. It never touches another
worktree's cluster or database. Run it at handoff: a cluster left behind keeps binding its slot's ports, so every
skipped teardown permanently narrows the pool the next worktree can choose from.

### The shared dependency tier

`dev up` owns the dependency tier directly and binds `web` to the default port instead of a derived one:

```bash
cargo run --release -p cli -- dev up
set -a; source .devx/env; set +a
cargo run -p neon
```

`dev up` deploys SurrealDB, Rauthy, Garage, Restate, `workflows-service`, and telemetry in KIND, restores the host
port-forwards, and writes `.devx/env`. Re-run it after sleep or reboot to re-arm dead port-forwards; it reuses the
cluster. `worktree-env up` creates the same topology in its own cluster. Restart the compiled `web` process after
changing routes, handlers, views, or content.

`dev up` uses Restate ingress `9080`, Restate admin `9070`, Rauthy `30080`, Garage `30900`, and SurrealDB `18000`; `web`
binds `3001`. A worktree selects a free slot in the `20000`–`21299` ranges, including slots held by stopped or orphaned
KIND clusters, so worktrees never share a slot. Always read the selected values from that worktree's `.devx/env`.

SurrealDB is the store (#1093). Its connection contract is `NAVIGATOR_SURREAL_ENDPOINT`, `_NAMESPACE`, and `_DATABASE`,
written into `.devx/env`, and its schema is applied rather than migrated: one idempotent `DEFINE` file
(`store/src/schema/navigator.surql`) plus a `schema_version` record. The local engine is memory-backed, so its data
resets with the pod.

### Authentication and lawyer access

The local Rauthy fixture provides five role-named accounts, each with password `password`: `owner@neonlaw.com` (owner),
`admin@neonlaw.com` (admin), `lawyer@neonlaw.com` (lawyer), `clerk@neonlaw.com` (clerk), and `client@neonlaw.com`
(client). Four of the five are seeded onto all three demo matters, so each can be exercised on the same projects.
`admin@neonlaw.com` deliberately is **not**: since ENG-81 the matter surface is participation-scoped for every tier, so
the fixture Admin is what an unassigned administrator looks like — the matters appear in neither their project list nor
their detail view until they grant themselves a row at `/app/admin`. Its administration surface is
`http://localhost:30080/auth/v1/admin`, using `nick@neonlaw.com` / `admin` on the shared local tier or the Rauthy port
printed for a worktree. Rauthy has one full administrator rather than a realm-scoped `manage-users` administrator; these
known credentials are confined to the loopback-only KIND fixture, while the reusable staging layer contains none.
Authentication comes from OIDC, while authorization comes from `persons.role`. Sign-in does not create a Person: an
IdP-authenticated email with no pre-seeded row receives 403, except `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`, which is created
as `owner`. Seed Lawyer in the database used by the running `web`:

```bash
cargo run -p cli -- dev grant-lawyer
```

`grant-lawyer` targets the environment-owned `navigator` database that every local loop shares, so it seeds the same
rows the running `web` reads. Then open the `NAV_BASE_URL` from `.devx/env`, follow `/auth/login`, and sign in through
Rauthy — a firm tier lands on the `/app/team` home, a client on `/app/projects`. Do not hand-write session cookies.
`/app/*` requires authentication; `/lawyer/*` also requires the database role `lawyer` or `admin`. The session carries
the role, so re-login after any role change, including one made from `/admin/person/{id}`.

`GET /auth/logout` clears the app session and, when the provider published an `end_session_endpoint`, bounces the
browser through it (RP-initiated OIDC logout) so Rauthy drops its own SSO session too — the next `/auth/login` then
prompts for credentials instead of silently re-authenticating. No manual step is needed. A provider without an
`end_session_endpoint` falls back to clearing the app session and redirecting home; to force a prompt there, visit the
provider end-session endpoint at `http://localhost:30080/auth/v1/oidc/logout` (substitute the worktree's Rauthy port)
before starting login again.

### The three sample matters

The fixture seeds three matters, each with its own client, its own practice, and its own sample application:

| Code | Matter | Repository |
| --- | --- | --- |
| `sample-litigation` | *Cruller v. Prine* | `neon-law-staging/sample-litigation` |
| `sample-transactional` | *Widget Works — Outside Counsel* | `neon-law-staging/sample-transactional` |
| `sample-estate` | *Estate of Cornelius Montgomery* | `neon-law-staging/sample-estate` |

The three live in the `neon-law-staging` organization, and each repository is named for the Project code it mounts on.
That is the whole naming rule: the repository name *is* the code, so nothing has to map one to the other.

Each client portal is `/app/projects/<code>/portal/`, and every `dev up` / `dev worktree-env up` refreshes all three
before writing `.devx/env`. The clones and `pnpm` builds happen in temporary directories; each built `dist/` and its
`navigator.yml` survive in `.devx/sample-projects/<code>/`, and `NAVIGATOR_SAMPLE_PROJECTS_DIR` points `web` at the
parent. `index.html` publishes last, so a reader mid-refresh keeps a complete prior document until the new assets are
ready.

The explicit command refreshes the same bundles. Name one matter to rebuild only its application — a full refresh is one
`pnpm install` and build per matter, so the narrow form is the loop worth using while iterating:

```bash
cargo run -p cli -- dev sample-project --project sample-litigation
```

Each application declares its own `name:` in `navigator.yml`, and boot validates that code before publishing it under
the matching matter portal — so a bundle staged under the wrong directory is refused rather than published on another
matter's portal. The code uses the same `projects::is_valid_code` rule as every Project, so the URL segment is always
lowercase letters or numbers separated by single hyphens.

Whether these matters are seeded at all is `NAVIGATOR_SIMULATED_MATTERS`, which defaults to following
`NAVIGATOR_ENVIRONMENT`: a `dev` boot carries them, a `production` boot does not. The persistent staging deployment sets
it to `true` explicitly, because its runtime profile is `production` by design. A deployment carrying them publishes a
site-wide banner saying so on every page.

### Verification

Run the exact browser and accessibility gate after starting `web` with the correct `.devx/env`:

```bash
cargo run -p cli -- dev browser-e2e
```

The command downloads and caches the pinned Chrome for Testing build, starts ChromeDriver on a free port, grants Lawyer,
and runs both suites with `NAV_REQUIRE_HARNESS=1`. It reads the base URL from the sourced `.devx/env`; override it when
driving a topology it did not generate:

```bash
cargo run -p cli -- dev browser-e2e --base-url http://localhost:3001
```

The accessibility suite audits the public shell against the one host `browser-e2e` already starts. It needed a second
base URL while the site served two brands from separate deployments; one binary serves one face now, so there is nothing
extra to start.

The Rust test suite needs no database, no container, and no configuration: each test opens its own embedded,
memory-backed SurrealDB. Run it through nextest — the default profile prints failures only, so a green run is the
one-line summary and a red run is just the failing tests in full. The cucumber BDD suites in `features` keep `cargo test
-p features`; their custom harness does not speak nextest's protocol, so the workspace nextest profile excludes that
package:

```bash
cargo nextest run --workspace && cargo test -p features
```

See [`docs/test-database.md`](docs/test-database.md) for the database contract, including the one lane — a test that
spawns the `navigator` binary — that needs a running engine.

For a full in-cluster demo from published images, run `cargo run -p cli -- dev worktree-env up --demo`; optionally pass
`--tag YY.M.D`. The ingress is at `http://localhost:8080`.

### Troubleshooting and cleanup

Inspect the live topology with:

```bash
kubectl --namespace navigator get pods
kubectl --namespace navigator describe pod <name>
kubectl logs --namespace navigator <name> --all-containers --tail=100
```

Leave the ordinary `dev up` KIND dependency tier running between sessions. For a worktree environment, run `navigator
dev worktree-env down` at handoff to remove its task-owned cluster and port-forwards. Never prune Docker volumes without
explicit approval. Use `cargo run --release -p cli -- dev down` only for a deliberate clean rebuild; it deletes the
ordinary cluster. Resource cleanup details live in
[`docs/agent-workflows.md`](docs/agent-workflows.md#resource-cleanup).

On Codex-provisioned macOS worktrees, Gatekeeper may open repeated `Verifying "<binary>"…` windows the first time each
freshly built Cargo executable runs — `navigator`, and hashed integration-test runners such as
`portal_admin_firm_surface-<hash>`. This is a host-side property, not a Navigator defect, and needs no local action. The
sandboxed Codex execution host stamps `com.apple.provenance` on every executable its Cargo/rustc process writes, and
Gatekeeper scans provenance-carrying binaries once on first launch. The attribute is applied per file by the writing
process — it is not inherited from the worktree directory, so relocating `CARGO_TARGET_DIR` does not prevent it, and the
first binary (`navigator` itself) is verified before any Navigator code could run. Do not strip `com.apple.provenance`,
disable Gatekeeper or SIP, notarize ephemeral test binaries, or wrap Cargo in a shell script to suppress the dialogs:
each either weakens macOS security or cannot reach that first binary. The builds are unaffected. Tracked in
[navigator#570](https://github.com/neon-law-source-code/navigator/issues/570) pending an upstream Codex host fix.

Run local, reversible machine-bound commands directly. Production deploys, production database access, irreversible
cloud operations, host maintenance, and dependency bumps stay propose-only. Put scratch files under `/tmp`, use unique
ports, preserve unrelated user changes, and report any check you did not run.

## The five actions

Every codebase task is exactly one of these actions. GitOps, Markdown validation, councils, and domain authoring are
supporting checks inside them.

### 1. Create an issue

- Read the glossary, relevant docs, code, and tests before writing the issue.
- State the observed problem, grounded scope, acceptance criteria, covering tests, and real files in the blast radius.
- If uncertainty remains, make the smallest throwaway Rust spike needed to answer it. Record the result in the issue; do
  not confuse spike code with the implementation.

### 2. Triage an issue

- Read the issue from its opening body through every comment and reproduce the current behavior where practical.
- Reconcile the request with the glossary, docs, source, and tests.
- Comment a test-driven plan with the minimum implementation and the exact files a future worktree should touch.

### 3. Create a pull request

- Start from the issue in a Codex or Claude **New Worktree**. Before the first edit verify that the current path is a
  non-primary `git worktree` entry; otherwise stop and ask the user to restart from New Worktree. Then run the command
  below once to name that checkout's PR branch and create its isolated KIND tier. Never commit directly to `main` or
  create a second worktree for the same task.

  ```bash
  cargo run -p cli -- dev worktree-env up --branch <topic>
  ```

- Rebase onto `origin/main` before the first edit and again before every push, as [Worktree-first code
  changes](#worktree-first-code-changes) describes. A branch opened from a stale tip stalls on a check it never reports.
- Use test-driven development. The covering test lands with the minimal implementation it proves.
- For Rust or runtime changes, run formatting, clippy with warnings denied, and the workspace tests. CI runs the
  coverage gate inside that same test pass, so total workspace line coverage must hold at or above 90.6%. Cover what you
  wrote regardless: the floor is a workspace total, so it can stay green while your change goes uncovered, and the
  covering test is what proves the change.
- Run Markdown validation for Markdown changes. The workspace test suite carries the no-client-data gate, so running it
  is running that gate. Capture and embed a live walkthrough for public or portal UI changes.
- Push and open the PR against `main`; auto-merge lands it after the required checks pass and review threads resolve.

### 4. Address a pull request comment

- Read the PR, the comment thread, the cited source, and the covering test at the PR head. Reproduce behavioral claims.
- Decide whether the comment is valid from evidence. If it is invalid, reply with the concrete rationale.
- If it is valid, make only the change required by that comment. Avoid opportunistic refactors and unrelated cleanup.
- Add or update the covering test when behavior changes, run the affected gate, push, reply with the proof, and resolve
  the handled thread.

### 5. Address a failed GitHub Action

- Read the failing check and its logs before changing code. Identify the first actionable failure, not a downstream
  cancellation or symptom.
- Reproduce the exact failing command locally when possible and trace it to the relevant source and test.
- Make the smallest root-cause fix. Do not bundle unrelated warnings, refactors, dependency updates, or reviewer work.
- Run the failed command and the directly affected gate locally, push, and report the evidence. Leave unrelated failures
  explicit.

The full recipes live in [`docs/agent-workflows.md`](docs/agent-workflows.md). Branching, gates, auto-merge behavior,
releases, and production handoff live in [`docs/gitops.md`](docs/gitops.md).

## Cross-cutting rules

- **Test the real path.** Compilation and visual inspection are not proof. Add or run the covering test and observe the
  relevant running behavior. Use TDD for implementation changes and measure coverage rather than inferring it.
- **Document the present.** Remove superseded code and vestigial history instead of adding compatibility shims or "used
  to" narration. Git history records the past. See [`docs/rust-programming.md`](docs/rust-programming.md).
- **Validate Markdown with the CLI.** Run `cargo run -p cli --quiet -- validate <path>` for every changed Markdown file.
- **Leave a slide's words alone.** A deck under `server/content/workshops/` is a script someone reads aloud, so carry
  its faces and presenter notes verbatim: reflow, lint, and fix shape, and raise any wording, title, or claim with the
  author rather than editing it. See
  [`.claude/skills/authoring-slides/SKILL.md`](.claude/skills/authoring-slides/SKILL.md).
- **Use councils only when earned.** Engineering Council reviews architecture and doc clarity; Legal Council reviews
  legal copy; Client Council reviews client-facing product decisions. Read the source first and use the smallest useful
  bench. See [`docs/agent-decision-councils.md`](docs/agent-decision-councils.md).
- **Clean task-owned resources.** Tear down the worktree's isolated KIND tier and never prune Docker volumes without
  explicit user approval. See [`docs/agent-workflows.md`](docs/agent-workflows.md#resource-cleanup).

## Cursor Cloud specific instructions

A Cursor Cloud Agent boots from [`.cursor/environment.json`](.cursor/environment.json), whose `install` runs
[`.cursor/install.sh`](.cursor/install.sh): it materializes the pinned toolchain, installs `cargo-nextest`, provisions
the system packages the test build needs (`libssl-dev`/`pkg-config` for `fantoccini`'s `openssl-sys`, `lld` for linking
the test binaries, and `kubectl` for the `cli::devx::ship` `kubectl kustomize` tests), and warms the build cache.

The Cloud VM runs the zero-infrastructure loop above — build, `cargo fmt`, `cargo clippy`, the test gate, `navigator`,
and editing. It does **not** run the KIND dependency tier: nested Docker + Kubernetes is a developer-machine flow, so
`dev up` and `dev worktree-env up` are out of scope there. To exercise the running site in the Cloud VM, boot `neon`
against a standalone SurrealDB server (`surreal start --user root --pass root memory`) with the `NAVIGATOR_SURREAL_*`,
`fs` storage, `SESSION_SECRET`, and placeholder `RESTATE_BROKER_URL`/`NAVIGATOR_CLAMD_ADDR` the boot invariants require;
the latter two are read lazily, so the pages render without those services.

Run the full workspace suite with the same knobs CI uses (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)),
because the Cloud disk cannot hold the ~40 test binaries at the default `debuginfo=2`:

```bash
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export RUSTFLAGS="-C link-arg=-fuse-ld=lld -C strip=symbols"
cargo nextest run --workspace --test-threads 4 && cargo test -p features
```

Start SurrealDB and set `NAVIGATOR_SURREAL_*` (root/root) to include the server-mode lane; otherwise it self-skips.

## Where to start

- [`docs/index.md`](docs/index.md) — documentation map.
- [`docs/glossary.md`](docs/glossary.md) — canonical vocabulary.
- [`docs/agent-workflows.md`](docs/agent-workflows.md) — the five action recipes.
- [`docs/gitops.md`](docs/gitops.md) — branch, PR, release, and deploy flow.
- [Local KIND development](#local-kind-development) — local environment, authentication, verification, and cleanup.
- [`README.md`](README.md) and [`cli/README.md`](cli/README.md) — workspace and CLI entry points.
