---
name: erd-visualization
description: >
  Render the workspace ERD as SVG (the canonical `docs/erd.svg`) using the navigator CLI's deterministic, pure-Rust
  renderer. Trigger when prepping a slide, screenshot, design-doc, or external review — or when someone asks "can you
  show me the database structure?" Also trigger when a migration has just landed: regenerate both `docs/erd.md`
  (mermaid) and `docs/erd.svg` (rendered) so picture and schema stay in sync; the `cli::tests::erd_svg` integration test
  will otherwise fail.
---

# ERD visualization

The workspace ships two ERD artifacts — `docs/erd.md` (a GitHub-rendered Mermaid block) and `docs/erd.svg` (a
deterministic standalone SVG) — both from one introspection in `cli/src/erd.rs`. The full recipe lives in the doc; read
it and keep it, not this skill, authoritative: [`docs/erd.md`](../../../docs/erd.md) — the two artifacts, the regen
commands (`navigator db erd --format mermaid` / `--format svg`), the `cli/tests/erd_svg.rs` idempotency guard, the
prod-diff and SVG-open recipes, and the `cli/src/erd.rs` layout constants.

## How to treat it (the load-bearing rules)

- **Use the CLI's deterministic renderer; never hand-draw.** `navigator db erd --format svg` introspects
  `pg_catalog` and emits byte-stable SVG — same schema in → byte-identical SVG out. A hand-edited or external-tool
  diagram drifts and breaks the guard test.
- **A migration just landed? Refresh both files in the same PR.** Regenerate `docs/erd.md` (mermaid) **and**
  `docs/erd.svg` (svg); `cli::tests::erd_svg` fails until the committed SVG matches the freshly rendered schema.
- **Scratch SVGs go to `/tmp`, not the tree.** Only the two canonical files (`docs/erd.md`, `docs/erd.svg`) are
  committed.

## Boundaries

- Local store connection: [[kind-local-dev]]. Changing the schema: [[rust]].
