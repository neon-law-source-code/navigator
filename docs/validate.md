---
publish: true
---

# Validate

`navigator validate <dir>` (default `.`) is the single command every editor, CI gate, and this repository's `AGENTS.md`
point at. This page is its canonical reference: what it runs, its flag, the error/warning split, and one row per rule
code. `cli/tests/validate_docs_coverage.rs` fails the build when a code exists in `rules/src/` or `cli/src/main.rs` with
no entry here, so this table cannot go stale.

## Usage

```bash
cargo run -p cli --quiet -- validate [dir]
```

`dir` defaults to `.` and is always a directory, not a single file — `validate` walks the whole tree under it. Run it
from the repository root to check everything, or point it at a narrower directory (e.g. `docs`, `templates`) to check
just that subtree.

The walk covers authored content only. It descends into everything except the trees nobody authors: `.git/`, `target/`,
`.worktrees/`, `node_modules/`, and `dist/`, plus — for the Markdown passes — every other hidden directory apart from
the canonical `.agents/` skill catalog. The match is on a whole directory name, so `distributions/` and
`node_modules_policy/` are authored trees and stay in the gate. The one pass that still reads a build is the origin pass
below: `Y009` opens each application's `dist/` directly, because a built bundle is exactly what it exists to check.

This is also the exact command every Project repository's generated CI gate runs against its own tree — see
[`project-repositories.md`](project-repositories.md) for how `navigator site projects repository scaffold` wires it up.

## What it runs

Nine normal validation passes happen in this order:

1. **The classified rule engine** (`rules::ClassifiedRuleEngine::lint_directory`) walks every `.md` file, classifies
   each one by its declared `kind:` (notation template, event, blog post, workshop, GitHub notation, matter dashboard,
   or plain prose), and lints it against that kind's rule set — the S, N, E, C, D, and M families below.
2. **Cross-file code uniqueness** (`rules::code_uniqueness_violations`, rule `N111`) walks the same tree a second time
   and fails if two notation templates declare the same `code:`. This runs after step 1 because it needs every
   template's `code` in hand before it can compare them.
3. **A YAML syntax pass** parses every `.yaml`/`.yml` file under `dir` and fails on a parse error. This has no rule
   code — it is a raw syntax check, not a lint — and it is not limited to notation templates or seed documents; any
   malformed YAML anywhere under `dir` fails it.
4. **A seed-document pass** (rule `Y001`) additionally validates every YAML file whose parent directory is literally
   named `seeds/` against `store::seed::validate_yaml`, the same shape check `navigator site import` enforces at write
   time. A seed document names real people and entities for a production write, so this pass exists to catch a malformed
   seed before it ever reaches `site import`.
5. **A locale-catalog pass** (rule `Y002`) additionally validates every YAML file under a `locales/<locale>/` directory
   against the typed marketing-copy schema in `views::locales`. The site publishes English only: a directory other than
   `en`, an unknown page stem, an unknown brand-key directory, or a document that does not deserialize as the page it
   names fails the gate. A house-of-brands tree uses `locales/en/<brand-key>/<page>.yaml`; a fixture may still use the
   flat `locales/en/<page>.yaml` layout. This is what lets a copy-only edit stay a YAML change without landing a catalog
   the brand crate cannot load.
6. **A document-pointer pass** (rule `Y003`) validates `documents/**/*.yml` only when the validation root is a Project
   repository declared by `navigator.yaml`. It checks the closed asset kind and visibility vocabularies, current
   revision metadata, revision-chain linkage, and the retained document extension without reading the network or bytes.
7. **A Project-manifest pass** (rules `Y004`–`Y008`) runs when the walked root carries `navigator.yaml` or the retired
   `navigator.yml` spelling. It holds `host` to a hostname shape, `project` to `store::projects::is_valid_code`,
   `no_live_row` to a non-empty reason string, refuses an unknown top-level key by naming the accepted set, and tells a
   `navigator.yml` file to rename to `navigator.yaml`.
8. **An origin pass** (rule `Y009`) scans each built application's `dist/` when the walked root is a Project repository.
   Empty first labels (`.test`) and dots/slashes-only are not hosts. Missing `dist/` is skipped so a source-only tree
   can still validate, and is a finding under `--ci`, where the build has already run and nothing to scan means the pass
   read nothing; a present `dist/` with an off-origin host fails either way. An `href` whose host is in `allowed_links`
   passes only when that anchor carries `rel="noreferrer"`.
9. **A consumed mutable-tag pass** walks YAML files and Containerfiles/Dockerfiles for an image or binary reference
   pinned to a mutable tag (`latest`, a branch name) rather than a digest or release version, and fails on each one
   found. This has no rule code either.
When `--fix` is passed, it replaces those nine passes entirely: it applies every rule's safe-by-construction autofix
across the tree, prints the file it changed, re-lints, and prints whatever the autofix could not resolve. This is the
same fix the `navigator-lsp` `source.fixAll` editor action ships.

## Flags

- **`--fix`** — apply every autofixable rule's fix in place (see the Autofix column below), then re-validate and report
  what remains. Each file is re-scanned until it stops changing, because one fix routinely uncovers another — trimming
  trailing whitespace off a short line hands that line to `S102`, which could not flag it while it still looked like a
  hard break. Exits `0` only if no violation remains after fixing; a remaining violation is always one a human has to
  resolve, never a bug in the fixer.
- **`--errors-only`** — print only the findings that fail the gate, hiding the Warning-severity advisories. The summary
  line still counts both and the exit code is unchanged: this narrows the listing for a CI-triage read, not the gate. It
  is rejected with `--fix`, where a remaining advisory still fails the run and so has to stay on screen.
- **`--ci`** — assert that the origin pass read a real build. A declared application with no `dist/` becomes a `Y009`
  finding instead of a skip. The `verify` job in `.github/workflows/project-gate.yml` is the only caller: it runs each
  application's build and then this command, so a missing `dist/` there means the pass examined nothing rather than that
  the tree has not been built yet. Local runs and the bare-checkout `notation` job omit it.

## Errors versus warnings

A rule's severity is either `Error` or `Warning`. An Error-severity violation, a YAML parse failure, a seed-document
failure, a locale-catalog failure, or a consumed mutable tag all fail the gate (exit code `1`). A Warning-severity
violation prints alongside everything else but never fails the run — it is a heads-up, not a blocker. Only two codes are
`Warning`: `N112` (a workflow step is allowed but its automation is not built yet) and `M061` (a relative docs link the
renderer cannot map onto a site route or GitHub). Every other code, including `Y001` and `Y002`, is `Error`.

Every rule-backed finding in the primary listing opens with `error:` or `warning:`, the way `rustc` and `clippy` write
one, before the `path:line`, the rule code, and the message. The raw YAML-syntax and consumed-tag passes retain their
plain stderr diagnostics; the error recapitulation below renders those failures with `error:` too:

```text
warning: docs/example.md:12 M061: Relative link `lib.rs` renders verbatim on the website …
error: docs/example.md:104 S101: Line is 130 characters (max 120)
```

## The error recapitulation

A run that found any error closes with an errors-only block naming every failing line again, after all nine passes have
printed:

```text
2 error(s) fail this run:
error: docs/example.md:104 S101: Line is 130 characters (max 120)
error: locales/xx/home.yaml:1 Y002: locale directory `xx` is not published; only `en` is allowed
```

It is a separate block rather than a reordering because the standalone passes print *after* the markdown lint's summary
line, so no ordering within a single pass could gather a YAML error and a mutable-tag error together. Being additive, it
also leaves the primary listing in tree order — per pass, per file, per line — so a file's findings stay adjacent.
Reading it is how to answer "which line do I fix"; the summary counts and the exit code say only *how many*.

## Rule codes

Every code below is defined in `rules/src/`, except `Y001`–`Y010`, which live in `cli/src/` because the typed YAML,
Project-manifest, and origin passes run outside the `rules` crate entirely. "Autofix" means `--fix` rewrites the file
for that violation without a human decision; every other code needs a person to resolve it.

### S-family — cross-cutting structure

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `S101` | Error | A line exceeds the 120-character limit. | No |
| `S102` | Error | A line could absorb more text from the next line before hitting the limit (prose only). | Yes |
| `S103` | Error | The declared `kind:` must be a recognized document kind. | No |
| `S104` | Error | A file's declared `kind:` must agree with its notation/event structure. | No |

### N-family — notation template shape

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `N101` | Error | Notation template must declare a non-empty `title`. | No |
| `N102` | Error | Notation template must declare a valid `respondent_type`. | No |
| `N103` | Error | Notation template filename must be snake_case. | No |
| `N104` | Error | Questionnaire/workflow state references an unknown registry item. | No |
| `N105` | Error | Notation template must declare `confidential`. | No |
| `N106` | Error | Notation workflow must include a `lawyer_review` step. | No |
| `N107` | Error | Signature placeholders must match the declared signer set and signing state. | No |
| `N108` | Error | Notation template must declare a stable `code`. | No |
| `N109` | Error | `output:` must name a known render format, and its paired keys must travel with it. | No |
| `N110` | Error | Catalog: `notations/` shelves. Project: flat `templates/<code>.md`. | No |
| `N111` | Error | Notation template `code` must be unique across the whole tree. | No |
| `N112` | **Warning** | A workflow step is allowed but its automation is not built yet. | No |
| `N113` | Error | Questionnaire state type must be a registered question type. | No |
| `N114` | Error | A `__for_` child state must follow a role-matched person/entity parent. | No |
| `N115` | Error | Data paths, iterators, and declared signer roles must resolve against questionnaire state. | No |
| `N116` | Error | Notation workflow must gate every outbound submission behind lawyer review. | No |
| `N117` | Error | Every `custom_text__*` state must be an allowlisted free-text primitive. | No |
| `N118` | Error | Questionnaire must be one linear chain from `BEGIN` to `END`. | No |
| `N119` | Error | A `kind: github` notation must be one of the two shelf paths and ask its required questions. | No |
| `N120` | Error | A template body placeholder must name a declared questionnaire state. | No |
| `N121` | Error | A `sent_for_signature` state must be preceded by a `generate_pdf` state. | No |

### E-family — events

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `E001` | Error | Event must declare both a `starts_at` timestamp and a `timezone`. | No |
| `E002` | Error | A file is either an event or a notation template, never both. | No |
| `E004` | Error | Event must declare a `luma_url`. | No |

### C-family — content pages

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `C001` | Error | Content page must declare a non-empty `title`. | No |
| `C002` | Error | Content page must declare a non-empty `description`. | No |
| `C003` | Error | Blog post filename must be `YYYYMMDD_slug.md`. | No |

### D-family — matter dashboards

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `D001` | Error | Matter dashboard section must be a recognized section type. | No |
| `D002` | Error | Matter dashboard section must be in the declared kind's own catalog. | No |
| `D003` | Error | Matter dashboard must carry its required sections in every declared lens. | No |
| `D004` | Error | Matter dashboard must declare a `lenses:` composition of known lenses. | No |

### M-family — Markdown hygiene

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `M001` | Error | Heading levels must increment by one. | No |
| `M003` | Error | Headings must use the ATX (`# Heading`) style. | No |
| `M004` | Error | Unordered list markers must be consistent. | No |
| `M005` | Error | List indentation must be consistent. | No |
| `M007` | Error | Unordered list indentation must be a multiple of two. | No |
| `M009` | Error | Lines must not end with trailing whitespace. | Yes |
| `M010` | Error | Hard tabs are not allowed. | Yes |
| `M011` | Error | Link syntax must be `[text](url)`, not the reverse. | No |
| `M012` | Error | Multiple consecutive blank lines are not allowed. | Yes |
| `M018` | Error | ATX headings must have a space after the `#`. | Yes |
| `M019` | Error | ATX headings must have a single space after the `#`. | Yes |
| `M020` | Error | Closed ATX headings must have a space before the closing `#`. | Yes |
| `M021` | Error | Closed ATX headings must have a single space before the closing `#`. | Yes |
| `M022` | Error | Headings must be surrounded by blank lines. | No |
| `M023` | Error | Headings must start at column one. | No |
| `M024` | Error | Headings must not duplicate a prior sibling. | No |
| `M025` | Error | A document must have at most one top-level (H1) heading. | No |
| `M026` | Error | Headings must not end with punctuation. | No |
| `M027` | Error | Blockquote markers must have a single space before their content. | Yes |
| `M028` | Error | Blockquotes must not contain blank lines. | No |
| `M029` | Error | Ordered list items must use the configured prefix. | No |
| `M030` | Error | List markers must have a single space before their content. | Yes |
| `M031` | Error | Fenced code blocks must be surrounded by blank lines. | No |
| `M032` | Error | Lists must be surrounded by blank lines. | No |
| `M034` | Error | Bare URLs must be wrapped in angle brackets. | No |
| `M035` | Error | Horizontal rule style must be consistent. | No |
| `M036` | Error | Emphasis must not stand in for a heading (prose only, not notation templates). | No |
| `M037` | Error | Emphasis markers must not have inner whitespace. | Yes |
| `M038` | Error | Inline code spans must not have inner whitespace. | Yes |
| `M039` | Error | Link text must not have inner whitespace. | Yes |
| `M040` | Error | Fenced code blocks must declare a language. | No |
| `M042` | Error | Links must not be empty. | No |
| `M045` | Error | Images must declare alt text. | No |
| `M046` | Error | Code block style must be consistent. | No |
| `M047` | Error | A file must end with a single trailing newline. | Yes |
| `M048` | Error | Fenced code block markers must be consistent. | No |
| `M049` | Error | Emphasis marker style must be consistent. | No |
| `M050` | Error | Strong-emphasis marker style must be consistent. | No |
| `M051` | Error | Link fragments must reference an existing heading. | No |
| `M052` | Error | Reference-style links and images must define their references. | No |
| `M053` | Error | Reference definitions must be referenced by something. | No |
| `M054` | Error | Link and image style must be consistent. | No |
| `M055` | Error | Table pipe style must be consistent. | No |
| `M056` | Error | Table column counts must match the header row. | No |
| `M057` | Error | A relative link target must resolve to a real file on disk. | No |
| `M058` | Error | Tables must be surrounded by blank lines. | No |
| `M059` | Error | Link text must be descriptive, not `here` or `click`. | No |
| `M060` | Error | Table column styles must be consistent. | No |
| `M061` | **Warning** | A published doc must not keep a relative link the renderer cannot map. | No |

### Y-family — YAML documents

| Code | Severity | Rule | Autofix |
| --- | --- | --- | --- |
| `Y001` | Error | A `seeds/*.yaml` document must be accepted by `navigator site import`. | No |
| `Y002` | Error | An English `locales/` catalog must deserialize as the page its stem names. | No |
| `Y003` | Error | A Project repository's `documents/**/*.yml` pointer must name a valid asset revision. | No |
| `Y004` | Error | A Project manifest `host` must be a hostname (no scheme, port, or path). | No |
| `Y005` | Error | A Project manifest `project` must be a valid Navigator Project code. | No |
| `Y006` | Error | A Project manifest top-level key must be one of the accepted set. | No |
| `Y007` | Error | A Project manifest `no_live_row` must be a non-empty reason string. | No |
| `Y008` | Error | The Project manifest filename is `navigator.yaml`; rename `navigator.yml`. | No |
| `Y009` | Error | Off-origin hosts fail unless listed in `allowed_links` with `rel="noreferrer"`. | No |
| `Y010` | Error | A Project template naming `Neon Law` with a corporate suffix must name the entity of record. | No |

`Y010` runs inside the Project-repository check that `navigator validate` applies when the walked root is a Project
repository. It reads each `templates/<code>.md` and compares any `Neon Law` spelled with a corporate suffix (`, Inc.`,
`LLC`, `PLLC`, and the like) against `store::seed::FIRM_ENTITY_NAME`, the legal person a client engages, so a signature
instrument cannot name a party the firm is not. The bare mark and `Neon Law IP LLC`, the Licensor, are not findings.
