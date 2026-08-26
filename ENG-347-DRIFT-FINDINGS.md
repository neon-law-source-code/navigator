# ENG-347 — `navigator projects drift`

Working notes for the drift command. Written to stand alone: the thread that produced it is gone.

- **Worktree** `C:\Users\jaska\navigator-wt-eng-347` (non-primary, verified against `git worktree list --porcelain`)
- **Branch** `jask/eng-347-projects-drift-command`, at `b73038f` — level with `origin/main`
  (`git rev-list --left-right --count origin/main...HEAD` → `0	0`)
- **Issue** ENG-347, "Thirteen of nineteen Project repositories name a code no live project carries,
  and nothing reports the drift" (High, project *Seed documents reach a deployment from CI, under a
  scoped token*)
- **Date** 2026-08-25

## Bottom line, read this first

The command **compiles and its CLI surface is wired**, but the gate never ran. **There is no PR, and
nothing was pushed.** The branch is intact and the work is on disk but not committed to Git.

`cargo build -p cli --bin navigator` finished **green, exit 0, in 29m52s**, with no warnings emitted.
That landed after the session had already been reported as blocked, so it is the last thing verified.
The rest of the gate — fmt, clippy, the test suites, the Markdown validator — was deliberately **not**
started, because the session was being closed out.

The single most useful next action is to run the gate. Everything below is either read off the
source at `b73038f` or is a design decision with its reasoning attached; none of it depends on the
build.

## Scope

The **build half only** — the command that reports drift.

Explicitly out of scope, and untouched: reconciling the thirteen repositories, creating or patching
any Project row, and anything touching a client entity. Production was read-only throughout; in the
end not even read, because the live-host run never happened.

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

All four citations used in the PR body were re-read line-by-line at `b73038f` after `origin/main`
moved, rather than trusted. They were exact.

**The consequence that shaped the design:** the reverse direction is *unreachable* through the CSV
route that the existing `site projects list` uses. It carries no `repository_url`, so it can answer
only "does this repository have a row?" and never "does this row have a repository?". The drift
command therefore reads `GET /app/api/projects`. **No server change is needed** — this was checked,
not assumed.

## Inferred — not proven by a run

- **Matching a row's `repository_url` to a checkout by the URL's last path segment against the
  directory name.** This is the assumption `cli/src/projects/repository.rs:26` already makes ("the
  repository name *is* the code") and is consistent with it. A checkout deliberately cloned into a
  differently named directory would read as drift. Judged acceptable; not observed either way.
- **That the bearer token flow works against `/app/api/projects`.** `AuthedSession`
  (`portal/src/api.rs:554`) reads `SessionData` from request extensions, which is the same
  middleware that populates it for `/app/projects.csv` — and `cli/src/remote.rs` already drives that
  route with a bearer. So it should work. **Never exercised.** If the live run 401s, this is the
  first thing to check.
- **That `--dir` holding only part of the fleet produces self-explaining output** rather than noise.
  The finding text names the searched directory for exactly this reason, but nobody has read real
  output.

## The design decision, and why

**How does the command tell a deliberately row-less repository from a real gap?** Eight of the
thirteen are closed matters left without rows on purpose. A tool that reports eight known-good
repositories as failures is a tool nobody runs twice.

Decided via the repository's Engineering Council (`/council`). Four shapes were considered:

1. **A constant list of the codes to skip, in Navigator's source.** *Barred by an invariant, not by
   taste.* A Project code **is** a client identifier — it names who retained the firm — and this
   tree is public. `AGENTS.md` forbids naming a real client matter in anything that leaves the
   practice. The eight codes cannot be written here at all.
2. **A `--ignore` flag or file.** Rejected: it does not remove the identifier, it relocates it to a
   runbook, a CI invocation, or a pasted command — written down just the same and reviewed less.
3. **Infer from repository shape** (no `portal/`, no `seeds/`, no `templates/` ⇒ nothing
   load-bearing). Rejected, and this is the one worth understanding: **an empty repository is also
   exactly what a brand-new *unreconciled* Project looks like.** That rule would go silent about
   precisely the gaps the command exists to find. Shape is evidence of impact, never of intent.
4. **The repository declares its own absence.** Chosen.

So a repository that is meant to have no row says so in the manifest it already carries:

```yaml
# navigator.yaml
project: <project-code>
no_live_row: the matter closed in <month>; no row was opened
```

The fact lives beside the matter it is about, is reviewed by whoever knows that matter, and dies
with the repository.

**The value is a reason string, not a boolean.** `no_live_row: true` fails to deserialize and is
reported as a manifest error. A boolean would let someone silence a red line without recording why.

**Suppressed is not silent.** Declared row-less repositories are counted in the report footer and
listed by `--all`. A report that hides repositories without saying so fails the same way as one that
cries wolf about them.

**A tension a reviewer will notice, and the answer:** `cli/src/projects/repository.rs:41`–`48` says
in as many words that exemptions live centrally, *not* per repository, because a per-repository
exemption file makes a gate advisory. This PR goes the other way. The distinguishing fact is that
the allowed-root list governs a rule **identical for every repository** — which paths may sit at a
root — so centralizing costs nothing. Whether one matter is meant to have a row is a **per-matter
fact only that matter knows**, and centralizing it costs a client identifier in a public tree. This
is called out explicitly in both the module doc and the PR body so it does not read as
inconsistency.

## What was built

`navigator projects drift --host <h> --dir <d> [--all] [--json]`

| File | Change |
| --- | --- |
| `cli/src/projects/drift.rs` | **new**, ~1090 lines including 20 tests |
| `cli/src/projects/mod.rs` | registers the module |
| `cli/src/main.rs` | `ProjectsCmd::Drift` variant + dispatch arm (+45 lines) |
| `cli/tests/help.rs` | verb list now `["doctor", "repository", "drift", "help"]` |
| `docs/project-repositories.md` | new section *Reconciling repositories against live rows* (+70 lines) |

Structure follows `doctor.rs`: a **pure `analyze()`** over two already-read inputs, with a thin IO
shell around it, so every asymmetry is testable without a network or a filesystem. Severity reuses
`doctor::Status` rather than introducing a second severity vocabulary in a neighbouring file.

Nine finding kinds:

| kind | status | meaning |
| --- | --- | --- |
| `repository-has-no-row` | fail | repository declares a code no live row carries |
| `row-has-no-repository-url` | fail | row records no repository at all |
| `row-repository-absent` | fail | row's URL names a repository not present under `--dir` |
| `code-mismatch` | fail | row and the repository its URL names spell the code differently |
| `duplicate-code` | fail | two checkouts claim one code |
| `unreadable-manifest` | fail | `navigator.yaml` unparsable, or names an invalid code |
| `manifest-disagrees-with-name` | warn | manifest declares a code other than the directory name |
| `no-manifest` | warn | checkout declares no Project, so it cannot be reconciled |
| `rowless-by-declaration` | ok | declared deliberate; counted, never failed |

`row-has-no-repository-url` is deliberately **its own category** rather than folded into the dangling-URL
case. It was the failure that hit the entire fleet without reporting anything, and *never recorded*
is a different problem from *recorded wrong*.

The scan takes immediate subdirectories of `--dir` that contain a `.git`, so the scan root is a
directory of sibling clones. A checkout with no `navigator.yaml` is still reported (`no-manifest`)
rather than skipped — skipping it would make an unmanifested Project repository indistinguishable
from a reconciled one.

Exit is nonzero only on a `Fail`; warnings do not make a fleet drifted.

## Deliberately not done

### The layout gate — deferred, and this is not an oversight

The issue's third ask — "add the constraint that would have caught it", i.e. the layout gate
refusing a repository whose declared code names no live row — is **deliberately not in this work**.
Confirmed as the right call, 2026-08-25.

The reason is structural, not effort. That check needs a host **and a bearer token** available inside
a Project repository's own CI run, and minting one from the repository's GitHub Actions OIDC identity
is the entire subject of **ENG-345**. Stacking it here would make a small, reviewable, read-only
command change depend on an unresolved authentication story, and would land a gate that cannot run
green until that story finishes.

Sequence: this command first (no auth story — it runs from an operator's machine against a login
they already have), then the gate on top of ENG-345. `cli/src/projects/repository.rs::validate` is
untouched.

### The thirteen repositories

Not reconciled, by instruction. Three are blocked elsewhere: two wait on client entity facts, and one
on ENG-351 (a Florida P.A. has no representable entity type). Eight are closed matters left row-less
on purpose — which is what the `no_live_row:` key exists to record. Adding that key to those eight
repositories is **eight separate one-line PRs in those repositories**, not work in this one.

## Not verified — the honest list

Nothing in this list was run. Do not report any of it as passing.

**What *was* verified, and is the only build evidence that exists:**

- `cargo build -p cli --bin navigator` — **green, exit 0, 29m52s, no warnings.** The crate compiles.
- `navigator projects --help` lists `doctor`, `repository`, `drift`, `help` — in that order, which is
  what `cli/tests/help.rs` now asserts. So that test should pass, though it has not been run.
- `navigator projects drift --help` renders `--host`, `--dir` (default `.`), `--all`, `--json`.

That covers compilation and argument wiring only. It says nothing about whether the 20 unit tests
pass, whether clippy is clean under `-D warnings`, or whether the Markdown validates.

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `cargo test -p features`
- `cargo run -p cli --quiet -- validate docs/project-repositories.md` — **the Markdown has never
  been through the validator.** The two-sided line rule (S101 caps at 120 *characters*; S102
  requires prose lines be packed) is easy to violate by hand, and the new doc section is 70 lines of
  prose. Expect fixes here.
- The live-host run. The command has never executed against anything.

The code compiles, so first-compile errors are *not* expected. Clippy under `-D warnings` is the
likelier source of friction, along with the Markdown line rules.

## What the next person should pick up, in order

1. **Run the gate**, pinned to `C:\Users\jaska\navigator-wt-eng-347`. The crate already compiles and
   `target/` is warm, so this should be far quicker than the 30-minute cold build. `cargo fmt` then
   `clippy -D warnings` then `cargo nextest run -p cli` is the fast loop; the full workspace suite
   and `cargo test -p features` before pushing.
2. **Run the Markdown validator** on `docs/project-repositories.md`. Use the CLI, never `awk` — awk
   counts bytes, so em dashes make a correct line read two characters over.
3. **`git fetch origin` and rebase onto `origin/main`** (signed: `git rebase -S origin/main`).
   `origin/main` was `b73038f` at the time of writing and moves often. Re-read any cited line number
   after the rebase rather than trusting it; that has bitten a sibling thread.
4. **Commit, push, open the PR.** Body drafted at
   `C:\Users\jaska\AppData\Local\Temp\claude\C--Users-jaska-navigator-wt-eng-347\47d3fb42-6fc7-4dbb-8494-b0c7cb3ce0ae\scratchpad\pr-body.md`
   — **it currently ticks the Test plan boxes, which is false until the gate is actually green.
   Fix that before posting.** `gh pr create` needs `--repo` and `--head` explicit: the git remote
   still names the pre-migration org and the command fails without them. Normal PR, not a draft. Arm
   auto-merge; do not merge and do not approve.
5. **Then run it against the live host** and paste the real output into the PR thread. The concrete
   thing to confirm is the reverse direction: a row whose `repository_url` still names a repository
   under its pre-rename name should surface as `row-repository-absent`. A forge rename leaves a
   redirect, so that URL keeps resolving over HTTP — which is exactly why nothing noticed before.

## Worktree state at handoff

Nothing is committed. `git status --porcelain`:

```
 M cli/src/main.rs
 M cli/src/projects/mod.rs
 M cli/tests/help.rs
 M docs/project-repositories.md
?? ENG-347-DRIFT-FINDINGS.md
?? cli/src/projects/drift.rs
```

`ENG-347-DRIFT-FINDINGS.md` is this file. It is untracked on purpose — it is a working note, not
part of the change, and should **not** be committed into the PR.

No KIND environment was created, so there is nothing to tear down. The `cargo build` finished and
exited cleanly, so this thread leaves no cargo process holding a lock, and `target/` is warm.

If a later build in this worktree seems stuck, check for orphaned cargo processes from *sibling*
worktrees before assuming breakage — they share `~/.cargo`, and that contention is a known hazard on
this machine rather than a fault. It cost this session a 30-minute build: cargo buffered its entire
output and flushed only at exit, so the run looked dead at zero bytes the whole way through. Slow and
silent is the expected shape here, not a symptom.
