# ENG-347 — `navigator projects drift`

Working notes for the drift command. Written to stand alone: the thread that produced it is gone.

- **Worktree** `C:\Users\jaska\navigator-wt-eng-347` (non-primary, verified via `git worktree list --porcelain`)
- **Branch** `jask/eng-347-projects-drift-command`, branched from `origin/main` at `b73038f`
- **Issue** ENG-347 (High), project *Seed documents reach a deployment from CI, under a scoped token*
- **Date** 2026-08-25

## Bottom line, read this first

**Nothing is pushed and there is no PR.** The work is committed locally. Three of the four gate
checks are green; the fourth never completed, so the gate cannot be called green and a push was not
justified.

The command's own tests are green: **22 of 22 `projects::drift::tests` pass.** One of them caught a
real bug, which is fixed (see *The bug the tests caught*).

The blockers to finishing are environmental, not defects:

- `cargo nextest run --workspace` was **killed three times mid-build** by something outside this
  session. Not one workspace test ever executed.
- `cargo test -p features` never ran.
- The **live-host run was impossible from this machine**: no stored credentials, and no Project
  repository checkout root to scan.

## Scope

The **build half only** — the command that reports drift. Out of scope and untouched: reconciling
the thirteen repositories, creating or patching any Project row, anything touching a client entity.
Nothing was written to production; in the end production was not even read.

## Gate results — exactly what was run

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | **green** (after applying `cargo fmt --all`; the diff was pure formatting) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **green**, zero warnings |
| `navigator validate docs/project-repositories.md` | **green** (after fixing 4 real errors) |
| `cargo nextest run -p cli --bin navigator -E 'test(/projects::drift::/)'` | **green, 22/22** |
| `cargo nextest run -p cli --bin navigator` (all 891) | **6 failures, all pre-existing host issues** — see below |
| `cargo nextest run --workspace` | **never completed** — killed 3× mid-build |
| `cargo test -p features` | **not run** |

This is a **narrowed gate, not the workspace gate.** Do not report it as complete.

### The Markdown errors were real

```
docs/project-repositories.md:405 S101: Line is 144 characters (max 120)
docs/project-repositories.md:409 S101: Line is 136 characters (max 120)
docs/project-repositories.md:397 S102: Line is 118 characters; could absorb "a" from line 398 to reach 120 (max 120)
docs/project-repositories.md:433 S102: Line is 115 characters; could absorb "why." from line 434 to reach 120 (max 120)
```

Two over-long table cells and two under-packed prose lines — the rule is two-sided. Fixed by
shortening the cells and reflowing both paragraphs with a greedy 120-**character** wrap. Use the CLI
validator, never `awk`: `awk` counts bytes, so an em dash makes a correct line read two over.

## Local test failures, and which are yours to care about

The full `cli` binary suite is 891 tests. Run naively it looks alarming; almost all of it is this
machine. The taxonomy, each item **proven** by re-running with the cause removed:

| Failures | Cause | Proof |
| --- | --- | --- |
| 1 — `devx::github_setup::tests::dry_run_never_writes` | ambient `NAVIGATOR_GITHUB_APP_ID` / `NAVIGATOR_GITHUB_API_BASE` in the shell | passes with both unset |
| 10 — all `release_check::*` and `release_default_tag::*` | global `commit.gpgsign = true`; the tests make commits in temp repos and signing fails | all 10 pass with `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` pointed at an empty file |
| 6 — `devx::tests::render_env_*`, `devx::native::garage::*`, `devx::worktree_env::*` | Windows host paths | see below |
| 1 — `projects::drift::tests::a_boolean_rowless_declaration_is_refused` | **a real bug in this change** | fixed; now passes |

To run the `cli` suite cleanly on this machine:

```bash
: > /tmp/empty-gitconfig
env -u NAVIGATOR_GITHUB_APP_ID -u NAVIGATOR_GITHUB_API_BASE \
    GIT_CONFIG_GLOBAL=/tmp/empty-gitconfig GIT_CONFIG_SYSTEM=/tmp/empty-gitconfig \
    cargo nextest run -p cli --bin navigator --no-fail-fast
```

The 6 Windows failures survive that and are **not** caused by this change — nothing in this diff
touches `devx`. Their causes are visible in the assertions:

- `devx::tests::render_env_threads_the_ports` — `assertion failed: env.contains("KUBECONFIG='/ws/.devx/kubeconfig'")`,
  a POSIX-path expectation against a Windows path.
- `devx::worktree_env::*` — `fatal: could not create leading directories of
  '//?/C:/Users/.../linked/.git': Invalid argument`. `tempfile::tempdir()` hands back a `\\?\`
  extended-length path and git cannot use it.

**Caveat, stated plainly:** these 6 were *not* proven pre-existing by building the parent commit —
that needs a full rebuild, which kept getting killed. The causal explanation is strong and the diff
cannot reach `devx`, but it is inference, not observation.

## The bug the tests caught

`a_boolean_rowless_declaration_is_refused` failed on the first honest run, and it was right to.

The design says the suppression key carries a **reason**, not a boolean, so nobody can silence a
finding without recording why. The implementation deserialized it straight into `Option<String>` —
and **`serde_yaml` is lenient: `no_live_row: true` coerces to the string `"true"`**, which read as a
perfectly good reason. The rule the whole design rests on was not actually enforced.

Fixed by holding the value untyped and requiring a genuine string:

```rust
no_live_row: Option<serde_yaml::Value>,
```

with `scan_repository` accepting only `Value::String`, refusing an empty one, and reporting anything
else as a manifest error. This is worth remembering: **a permissive deserializer will quietly
undo a validation rule expressed only in the type.**

## Verified — read from source at `b73038f`

| Claim | Evidence |
| --- | --- |
| `navigator.yaml` is an allowed repository root and **nothing in the workspace parses it** | `cli/src/projects/repository.rs:71`, `:89` |
| Its shape is `host:` + `project:` | fixture at `cli/src/projects/repository.rs:990` |
| `navigator.yml` is a *different* manifest, keyed `name:`, for sample bundles | `store/src/sample_project.rs:33`, `:64` |
| **`GET /app/projects.csv` omits `repository_url`** — headers are `id, code, name, status, entity_name` | `portal/src/admin.rs:2189`–`2223` |
| `GET /app/api/projects` returns the whole `store::projects::Project` | `portal/src/api.rs:106`, `:636` |
| `Project.repository_url` is `Option<String>` | `store/src/projects.rs:45` |
| `projects doctor` already defines an `Ok`/`Warn`/`Fail` vocabulary | `cli/src/projects/doctor.rs:32`–`40` |
| Exemptions are centralized *on purpose* for the allowed-root list | `cli/src/projects/repository.rs:41`–`48` |
| `origin` (`neon-law-foundation/navigator`) redirects to `neon-law-source-code/navigator` | `gh api repos/neon-law-foundation/navigator --jq .full_name` |

**The consequence that shaped the design:** the reverse direction is *unreachable* through the CSV
route the existing `site projects list` uses. It carries no `repository_url`, so it can answer only
"does this repository have a row?" and never "does this row have a repository?". The command reads
`GET /app/api/projects` instead. **No server change is needed** — checked, not assumed.

## Inferred — not proven by a run

- **Matching a row's `repository_url` to a checkout by the URL's last path segment against the
  directory name.** Consistent with `cli/src/projects/repository.rs:26` ("the repository name *is*
  the code"). A checkout cloned into a differently named directory would read as drift.
- **That the bearer flow works against `/app/api/projects`.** `AuthedSession`
  (`portal/src/api.rs:554`) reads `SessionData` from request extensions — the same middleware that
  serves `/app/projects.csv`, which `cli/src/remote.rs` already drives with a bearer. **Never
  exercised.** If the live run 401s, look here first.
- **That the 6 Windows `devx` failures are pre-existing** (see caveat above).

## The design decision, and why

**How does the command tell a deliberately row-less repository from a real gap?** Eight of the
thirteen are closed matters left without rows on purpose. A tool that reports eight known-good
repositories as failures is a tool nobody runs twice.

Decided via the repository's Engineering Council (`/council`). Four shapes considered:

1. **A constant list of codes to skip, in Navigator's source.** *Barred by an invariant, not taste.*
   A Project code **is** a client identifier — it names who retained the firm — and this tree is
   public. `AGENTS.md` forbids naming a real client matter in anything that leaves the practice. The
   eight codes cannot be written here at all.
2. **A `--ignore` flag or file.** Rejected: does not remove the identifier, relocates it to a
   runbook or CI invocation — written down just the same, reviewed less.
3. **Infer from repository shape** (no `portal/`, no `seeds/` ⇒ nothing load-bearing). Rejected, and
   this is the one worth understanding: **an empty repository is also exactly what a brand-new
   *unreconciled* Project looks like.** That rule would go silent about precisely the gaps the
   command exists to find. Shape is evidence of impact, never of intent.
4. **The repository declares its own absence.** Chosen.

```yaml
# navigator.yaml
project: <project-code>
no_live_row: the matter closed in <month>; no row was opened
```

The value is a reason, not a boolean — see *The bug the tests caught* for how nearly that failed.
**Suppressed is not silent:** declared row-less repositories are counted in the footer and listed by
`--all`.

**A tension a reviewer will notice:** `cli/src/projects/repository.rs:41`–`48` says exemptions live
centrally, *not* per repository, because a per-repository exemption file makes a gate advisory. This
change goes the other way. The distinguishing fact is that the allowed-root list governs a rule
**identical for every repository**, so centralizing costs nothing; whether one matter is meant to
have a row is a **per-matter fact only that matter knows**. Called out in the module doc, the doc
section, and the commit message so it does not read as inconsistency.

## What was built

`navigator projects drift --host <h> --dir <d> [--all] [--json]`

| File | Change |
| --- | --- |
| `cli/src/projects/drift.rs` | **new**, ~1100 lines including 22 tests |
| `cli/src/projects/mod.rs` | registers the module |
| `cli/src/main.rs` | `ProjectsCmd::Drift` variant + dispatch arm |
| `cli/tests/help.rs` | verb list now `["doctor", "repository", "drift", "help"]` |
| `docs/project-repositories.md` | new section *Reconciling repositories against live rows* |

Pure `analyze()` over two already-read inputs with a thin IO shell, mirroring `doctor.rs`, so every
asymmetry is testable without network or filesystem. Severity reuses `doctor::Status`.

| kind | status |
| --- | --- |
| `repository-has-no-row` | fail |
| `row-has-no-repository-url` | fail |
| `row-repository-absent` | fail |
| `code-mismatch` | fail |
| `duplicate-code` | fail |
| `unreadable-manifest` | fail |
| `manifest-disagrees-with-name` | warn |
| `no-manifest` | warn |
| `rowless-by-declaration` | ok, counted |

`row-has-no-repository-url` is its own category, not folded into the dangling-URL case: it was the
failure that hit an entire fleet silently, and *never recorded* is a different problem from
*recorded wrong*.

## Deliberately not done

### The layout gate — deferred, not overlooked

The issue's third ask — the layout gate refusing a repository whose declared code names no live row
— is deliberately excluded. It needs a host **and a bearer token** inside a Project repository's own
CI run, and minting one from that repository's GitHub Actions OIDC identity is the whole subject of
**ENG-345**. Stacking it here would make a small, read-only command change depend on an unresolved
authentication story, and land a gate that cannot run green until that story finishes.
`cli/src/projects/repository.rs::validate` is untouched.

### The thirteen repositories

Not reconciled, by instruction. Two wait on client entity facts, one on ENG-351 (a Florida P.A. has
no representable entity type). Eight are closed matters left row-less on purpose — which is what
`no_live_row:` exists to record. Adding that key is **eight one-line PRs in those repositories**, not
work in this one.

### The live-host run

**Could not be done from this machine.** `navigator projects drift --host https://www.neonlaw.com
--dir .` returns:

```
navigator: not logged in to https://www.neonlaw.com — run `navigator login --host …`
```

There is no stored credentials file, and `navigator login` is an interactive browser OIDC flow that
was not started unattended. Separately, **there is no Project repository checkout root on this
machine** (`~/neon-law` and siblings do not exist), so even with a login the scan would have had
nothing to compare and every row would have reported `row-repository-absent`.

Whoever picks this up needs both: a `navigator login --host <h>`, and the fleet cloned under one
directory as [`docs/project-repositories.md` § Local checkouts](docs/project-repositories.md)
describes.

## What the next person should pick up, in order

1. **Finish the gate.** `target/` is warm. Run the full `cargo nextest run --workspace` and
   `cargo test -p features`, using the env-neutralising invocation above. Expect the 6 Windows
   `devx` failures; confirm they also fail on `origin/main` before dismissing them.
2. **Rebase onto `origin/main`** (`git fetch origin && git rebase -S origin/main`), then re-read any
   cited line number rather than trusting it across the rebase.
3. **`git rm --cached ENG-347-DRIFT-FINDINGS.md`** so this file does not ride into the diff. It is
   currently **committed**, deliberately, so a stray checkout cannot lose it — but it is a working
   note, not part of the change.
4. **Push and open a normal PR** (not draft). `gh pr create` **requires `--repo
   neon-law-source-code/navigator` and `--head jask/eng-347-projects-drift-command`**: the git remote
   still names the pre-migration org `neon-law-foundation`, which redirects for `git push` but makes
   `gh pr create` fail. Arm auto-merge; do not merge, do not approve. A PR body is drafted in the
   session scratchpad — **its Test-plan boxes are ticked and must be corrected to match whatever was
   actually run.**
5. **Then run the command against the live host** and paste the output into the PR thread. The
   concrete thing to confirm is the reverse direction: a row whose `repository_url` still names a
   repository under its pre-rename name should surface as `row-repository-absent`. A forge rename
   leaves a redirect, so that URL keeps resolving over HTTP — which is exactly why nothing noticed
   before.

## Worktree state at handoff

See the final commit on the branch. `ENG-347-DRIFT-FINDINGS.md` is committed on purpose (durability);
remove it from the branch at PR time per step 3.

No KIND environment was created, so there is nothing to tear down. No cargo processes were left
running — every killed run was checked for orphans and none were found.

**Machine notes worth keeping:** sibling worktrees share `~/.cargo`, so builds here are slow by
nature. Cargo buffers its entire output and flushes only at exit, so a run at zero bytes for thirty
minutes is normal, not hung. A cold workspace build took **29m52s**; test-profile builds are slower
still because they rebuild the dependency chain again.
