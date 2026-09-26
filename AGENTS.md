# Neon Law Navigator

This is a general purpose agentic lawyering engine.

Critical: No client data, this should work for everyone.

This is a Rust monorepo, use Cargo to build and run it. Apply best practices to writing Rust, including TDD, DRY, and
simple docs grounded in our ontology — where nouns become database tables.

Read the [Skills](.agents/skills) and the [glossary](https://www.neonlaw.com/glossary)

When developing, ground every decision in our [ontology](https://www.neonlaw.com/glossary). Use tests, create worktrees
to isolate development, and optionally use Kind for a full e2e parity check like production in GCP.

## Find the term first

Before naming a table, field, state, or UI concept, search the glossary. Run `navigator glossary list` for term titles
and plain-language descriptions. Then run `navigator glossary show "<Term>"` for a term's full definition. Use the
glossary's term when one fits; when none fits, add a glossary entry in `docs/glossary/` in the same change rather than
inventing a synonym.

## Helpful commands

### Reading the ontology

```bash
cargo run -p cli -- glossary list
cargo run -p cli -- glossary show "Lawyer Review"
```

### Reading the Project Skill catalog

A Project Skill is a jurisdiction/practice-area playbook a Project pins onto its own `navigator.yaml` — distinct from
the Agent Skills under `.agents/skills` this file just pointed you at. See
[`docs/project-skills.md`](docs/project-skills.md) and `glossary show "Project Skill"`.

```bash
cargo run -p cli -- project skill list
cargo run -p cli -- project skill show nv estates
```

### Starting web

```bash
cargo run -p cli -- dev up --path "$PWD"
set -a; source .devx/env; set +a
cargo run -p neon
```

### Lifecycle commands

```bash
cargo run -p cli -- dev down --path "$PWD"
```

### e2e Testing

Run if doing UI changes.

```bash
cargo run -p cli -- dev browser-e2e
```

### Testing

Run the focused test while writing the change. Before pushing a pull request, run `navigator validate`, or the CLI crate
when `navigator` is not on `PATH`. Do not re-run the workspace suite at that step. CI runs it.

```bash
navigator validate
cargo run -p cli --quiet -- validate
```

### Local K8 Development

If we spin up local Kind, inspect workloads with:

```bash
kubectl --namespace navigator get pods
kubectl --namespace navigator describe pod <name>
kubectl logs --namespace navigator <name> --all-containers --tail=100
```

## The three sample matters

The fixture seeds three matters, each with its own client, its own practice, and its own sample application:

| Code | Matter | Repository |
| --- | --- | --- |
| `sample-litigation` | *Cruller v. Prine* | `neon-law-staging/sample-litigation` |
| `sample-transactional` | *Widget Works — Outside Counsel* | `neon-law-staging/sample-transactional` |
| `sample-estate` | *Estate of Cornelius Montgomery* | `neon-law-staging/sample-estate` |

The three live in the `neon-law-staging` organization, and each repository is named for the Project code it mounts on.

## Use CLIs and MCPs over Computer Use

- `navigator` — our own app or the CLI target
- `gh` — GitHub CLI for PRs

## Cursor Cloud specific instructions

A Cursor Cloud Agent boots from [`.cursor/environment.json`](.cursor/environment.json), whose `install` runs
[`.cursor/install.sh`](.cursor/install.sh).

```bash
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export RUSTFLAGS="-C link-arg=-fuse-ld=lld -C strip=symbols"
navigator validate
```

When `navigator` is not on `PATH`, run the same check through the CLI crate: `cargo run -p cli --quiet -- validate`.

Start SurrealDB and set `NAVIGATOR_SURREAL_*` (root/root) to include the server-mode lane; otherwise it self-skips.
