# Project Skills

A **Project Skill** is one practice playbook in the catalog `navigator project skill ...` reads: a jurisdiction, a
practice area, a name, a version, and the [Notation](notation.md) `code`s it bundles. A Project pins one onto its own
`navigator.yaml` so the Notations an engagement needs are scaffolded onto that Project repository, instead of an
attorney assembling `templates/` by hand.

This is a different noun from an **Agent Skill** — a `.agents/skills/<name>/SKILL.md` instruction file this repository's
own coding agents (Claude Code, Codex) read to drive their own workflow, such as
[`implement-issue`](../.agents/skills/implement-issue/SKILL.md) or [`create-pr`](../.agents/skills/create-pr/SKILL.md).
An Agent Skill teaches an agent how to work on Navigator's own source; a Project Skill teaches a Project how to run one
kind of legal engagement. Neither catalog reads the other, and pinning a Project Skill never touches `.agents/skills/`.

## Catalog layout

Every entry lives at `skills/<jurisdiction_code_lowercase>/<practice_area>.md` in this repository — for example
[`skills/nv/estates.md`](../skills/nv/estates.md) and [`skills/tx/probate.md`](../skills/tx/probate.md), the two seeded
entries. The directory is compiled into the `navigator` binary at build time with `include_dir!`
(`cli/src/projects/skill.rs`), the same way Navigator's bundled Notation catalog is compiled in — so `list`, `show`, and
`use` need no network call and no database connection, and a checkout's gate can never drift against a different catalog
than the one a Project pins against.

## Required frontmatter

Each catalog entry's frontmatter (parsed by `rules::project_skill::parse`, `rules/src/project_skill.rs`) must carry:

| Field | Required | Shape |
| --- | --- | --- |
| `jurisdiction` | Yes | A seeded jurisdiction code (see below); free text like `Nevada` is rejected. |
| `practice_area` | Yes | A lowercase slug naming the file (`estates`, `probate`). |
| `name` | Yes | The human-readable playbook name. |
| `version` | Yes | The version string a Project pins when it runs `navigator project skill use`. |
| `notations` | No | A list of Notation `code`s this skill bundles. Absent means an empty list, not an error. |

The body below the frontmatter is the playbook itself — prose, a checklist, whatever the practice area needs.

A malformed entry (missing/empty/non-scalar required field, an unrecognized `jurisdiction`, or a `notations:` that is
not a list of strings) fails `navigator project gate`/`navigator validate` under rule `N126`. Two entries that declare
the same `(jurisdiction, practice_area)` pair fail under `N127`. See [`docs/gate.md`](gate.md) for the full rule table.

## Jurisdiction codes

`jurisdiction` is validated offline against `rules::f110::JURISDICTIONS`, a list compiled at build time from
[`store/seeds/Jurisdiction.yaml`](../store/seeds/Jurisdiction.yaml) — the same seed `store::jurisdictions::find_by_code`
reads at runtime. The `rules` crate carries no database dependency (it also backs `navigator-lsp` and CI, neither of
which opens a connection), so this static stands in for the runtime lookup rather than a second embed of the seed file.

## Reading the catalog

```bash
navigator project skill list
navigator project skill show nv estates
```

`list` prints one `jurisdiction<TAB>practice_area<TAB>name` line per entry. `show` matches `jurisdiction` and
`practice_area` case-insensitively (`show nv estates` and `show NV estates` resolve the same entry) and prints the
resolved entry's full body. An unresolved pair exits non-zero, naming the catalog entries closest to it by edit distance
rather than dumping the whole catalog — and never echoes the argument back, so a plain-text operator typo cannot be
mistaken for the `CodeQL` secrets false positive `cli/src/projects/skill.rs` documents inline.

## Pinning a Project Skill

Run from a Project repository root:

```bash
navigator project skill use nv estates
```

`use` resolves the entry, writes a `skills:` entry — `{jurisdiction, practice_area, version}` — onto that Project's
`navigator.yaml` (`cli/src/projects/manifest.rs::pin_skill`), and scaffolds each bundled Notation `code` into
`templates/<code>.md` if that file does not already exist. Re-running `use` for a pair already pinned at the same
version is a byte-for-byte no-op; pinning a pair whose catalog version has moved on updates the recorded `version` in
place rather than adding a duplicate entry. `use` never overwrites an attorney's edits to an already-scaffolded
template, and a failed resolution never touches `navigator.yaml`.

Pinning is by **version string**, not a Git commit or ref: the `version` recorded in `navigator.yaml` is compared
against the `version` the compiled-in catalog currently carries for that `(jurisdiction, practice_area)` pair.

```bash
navigator project skill status
```

`status` reads every `skills:` pin from the current Project's `navigator.yaml` and reports, per pin, whether the catalog
still carries that `(jurisdiction, practice_area)` pair at exactly the pinned version (`resolvable`) or not
(`unresolvable` — either the pair is gone from the catalog, or the catalog has moved on to a different version).
`navigator project gate --check` resolves the same pins against the same catalog through the shared
`projects::skill::resolve_pins` function, so the two surfaces can never disagree about the same `navigator.yaml` (rule
`Y015` covers the manifest shape itself — that each entry carries non-empty `jurisdiction`, `practice_area`, and
`version` text — while pin resolution is this separate, catalog-backed check).

## Example catalog entries

[`skills/nv/estates.md`](../skills/nv/estates.md) and [`skills/tx/probate.md`](../skills/tx/probate.md) are synthetic
practice guides seeded to exercise the catalog end to end — parsing, listing, pinning, and gate resolution. Neither
carries a real matter, client, or production identifier; see
[`docs/public-contributor-safety.md`](public-contributor-safety.md).
