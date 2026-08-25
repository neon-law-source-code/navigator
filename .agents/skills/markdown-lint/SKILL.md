--- name: markdown-lint description: > Lint every `.md` file in the workspace with the navigator CLI (M-family rules +
S101 120-char line limit). Trigger when adding or editing any Markdown file (READMEs, `docs/`, `CLAUDE.md`, blog posts
under `server/content/`) and before committing `.md` changes. Dogfood the workspace's own binary; never hand-roll a
different linter. ---

# Markdown linting via the navigator CLI

Lint is not a confidentiality classifier. Before validating a public example, read
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md) and remove client data, legal files,
real contact details, and production identifiers rather than relying on a green lint result.

Every `.md` file in this repo must pass the navigator CLI's markdown rule set. We dogfood our own linter so the rule
definitions, exit codes, and CI behavior stay coherent.

## The canonical command

```bash
cargo run -p cli --quiet -- validate <path>
```

What each piece does:

- `validate` classifies each file by its content and path — prose markdown gets the M-family rules + S101/S102, a
  notation template under `templates/` (or any file declaring `questionnaire:`/`workflow:`) also gets the N-family,
  events get the E-family, and blog posts get the C-family. A plain README classifies as prose on its
  own, so it never trips bogus N101/N102/N103. It walks the whole tree — README/`CLAUDE.md` and every directory are in
  scope (only hidden dirs and `target/` are skipped) — and folds in the typed event pass and a `.yaml`/`.yml` parse over
  the same walk. The per-kind frontmatter keys are documented for attorneys in `docs/frontmatter.md`.
- `<path>` — either a file or a directory (default `.`). The walker recurses.

## Lint every workspace README in one pass

```bash
for d in rules store views workflows cloud web cli mcp; do
  cargo run -p cli --quiet -- validate "$d"
done
```

Exit `0` on every iteration means clean. Otherwise the violating file, line, rule code, and message print to stdout.

## Common rules that fire

- **S101** — line longer than 120 characters. Reflow the paragraph; don't fight the limit. **M026** — heading ends with
  trailing punctuation `.`. Drop the period from `## Headings.` (watch for false positives: bash `# comment.` lines
  inside fenced code blocks trip the same rule).
- **M038** — inline code span has leading or trailing whitespace.
  Usually means the span got broken across two lines; keep code spans on a single line.
- **M040** — fenced code block is missing a language tag. Add one
  (`bash`, `rust`, `text`, `yaml`, …) right after the opening fence.
- **M031** — fenced code block must have a blank line before it.
  Common when a code block is nested inside a list item.
- **M060** — table column alignment is inconsistent within a table.

## When to run it

- Before committing any change that touches a `.md` file or creating a new README. CI also validates the content tree.

## What NOT to do

- Don't reach for `markdownlint`, `mdformat`, or any non-Rust linter. We standardize on the in-house `cli` — that's the
  whole point of dogfooding. See [[rust]] for the Rust-only stance.
- Don't disable a rule by editing `cli/src/main.rs`. If a rule is
  wrong, fix it in `rules/src/<code>.rs` with a test.
