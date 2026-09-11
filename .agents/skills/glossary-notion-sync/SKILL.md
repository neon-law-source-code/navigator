---
name: glossary-notion-sync
description: >
  Sync [`docs/glossary.md`](../../../docs/glossary.md) with its mirror page in the Notion "✏️ Writing" database, in
  either direction, and keep the page's alphabetical index derived rather than hand-written. Trigger when asked to push
  the glossary to Notion, pull a Notion glossary edit back into the repository, refresh the Notion copy after a term
  changes, or regenerate the index. The repository is the source of truth; Notion is the place a colleague can read and
  propose. A pull is a pull request, never a paste.
---

# Syncing the glossary with Notion

Two copies of one vocabulary, and they are not peers:

- **[`docs/glossary.md`](../../../docs/glossary.md) is the source of truth.** It is reviewed in pull requests, compiled
  into the binary through `store::glossary::GLOSSARY_MD`, materialized into `glossary_term` rows on every boot, and
  published at `/documents/glossary`. A definition is not real until it lands here.
- **The Notion page is the reading and proposing surface** — a colleague without a checkout can open it, comment, and
  type. Its page URL is `https://app.notion.com/p/3d8c909308608139829dff990512a174`, titled **Glossary** in the **✏️
  Writing** database (`Type: Guideline`). Every push rewrites it wholesale, so an edit made there survives only until
  the next push unless somebody pulls it back into the repository first.

Say that out loud when handing someone the page. A colleague who believes they are editing the glossary, and is actually
editing a copy that gets overwritten, has been misled by us.

## The mechanical contract

`store::glossary::parse` is the parser of record for both directions:

- **Every `##` heading opens a term**, and the body runs to the next one. There is no other level of heading in the
  document, and nothing above the first `##` is a term.
- **The alphabetical index lives in the preamble**, above the first `##`, and is therefore invisible to the parser. A
  letter group is navigation, not vocabulary; giving `A` its own heading would file it in `glossary_term` beside
  Participation.
- **A heading's anchor is a public API.** `store::glossary::slugify` turns `Lawyer Review` into `lawyer-review`, and
  roughly forty places — `docs/*.md`, `store/src/projects.rs`, `mcp/src/tools/*.rs`, `portal/src/api.rs` — link to those
  anchors. Renaming a heading breaks every one of them silently, because a Markdown anchor that no longer exists is a
  link to the top of the page, not an error. Grep before renaming, and fix the callers in the same commit.
- **Headings are alphabetical by slug**, which `cli::docs::tests::glossary_headings_are_alphabetical` pins.

## The index is derived, never typed

Both sides regenerate the index from the document's own headings. Nobody maintains a list of ninety-nine links by hand.

```bash
cargo run -p cli --quiet -- dev docs glossary-index
```

Exit 0 means the committed index matches the headings. On drift it names the file and exits 1; add `--write` to rewrite
the block in place. `cli::docs::tests::the_committed_index_is_what_the_renderer_produces` runs the same comparison in
the test suite, so a term added without refreshing the index fails the gate rather than shipping a half-index.

The renderer wraps greedily at 120 characters and breaks on spaces, including inside a link label, because that is what
`S102` expects of every other paragraph in the file. CommonMark reads the newline as a space, so the link survives.

## Pushing: repository → Notion

The push artifact is a command's output, not a hand-edit, so the page is a pure function of the repository.

```bash
cargo run -p cli --quiet -- dev docs glossary-notion > /tmp/glossary-notion.md
```

That resolves the three link shapes Notion cannot follow — `../store/src/persons.rs` and `../store/` become `blob` and
`tree` URLs on `main`, `notation.md#template` resolves into `docs/`, and an in-page `#participation` anchor is unlinked
down to its label because Notion has no heading anchors to point at. It also drops the frontmatter and the `# Glossary`
H1, undoes the repository's 120-character hard wrap so paragraphs flow, and prepends the provenance blockquote that
tells a reader on the page which way the sync runs.

Then replace the page with it:

1. Split the output at `##` heading boundaries into chunks of roughly 24 KB — the whole document is about 120 KB and
   will not go through in one call.
2. `notion-update-page` with `command: replace_content` and the first chunk.
3. `notion-update-page` with `command: insert_content` and `position: {"type": "end"}` for each remaining chunk, **in
   order**. Send them one at a time; a parallel insert scrambles the page.

Verify with `notion-fetch` on the page URL. The heading count it comes back with must match the number of terms that
`cargo run -p cli --quiet -- dev docs glossary` prints, and the last term must be Workshop.

## Pulling: Notion → repository

**A pull is a pull request.** Fetch, read, and hand-apply the change to `docs/glossary.md`; never paste the Notion page
over the file. The Notion copy has absolute GitHub URLs where the repository has relative paths, flowed paragraphs where
the repository hard-wraps at 120, and no in-page links at all, so pasting it back would rewrite the whole document to
say the same thing worse.

1. `notion-fetch` the page and diff it against `cargo run -p cli --quiet -- dev docs glossary-notion` output. What
   differs is what somebody typed — minus what Notion itself re-serializes. It returns a Markdown table as a native
   `<table>` block and drops the blank lines between blocks, so those differences are the round trip, not an edit.
2. **Read every difference before applying it.** This is the step that matters: Notion is where the firm's real matters
   live, and this repository is public. A definition that arrived through Notion may name a client, a matter, or a
   Project code. The no-client-data rule in [`CLAUDE.md`](../../../CLAUDE.md) governs the destination, not the source —
   rewrite the example as a mechanism ("a Project's publish") or use a synthetic code, and if a real code is already in
   the Notion text, say where it is and let a human decide.
3. Apply the wording to `docs/glossary.md` in its own idiom: relative links, hard wrap at 120, the term in alphabetical
   position.
4. Refresh the index and run the gate:

   ```bash
   cargo run -p cli --quiet -- dev docs glossary-index --write
   cargo run -p cli --quiet -- validate docs/glossary.md
   cargo nextest run -p cli -p store
   ```

   `store::glossary::GLOSSARY_MD` is `include_str!`, so a glossary edit changes a compiled constant and the rows every
   boot materializes — the store tests are not optional here.
5. Open the pull request, then push the merged text back to Notion so the two copies agree again.

## What not to do

- **Don't hand-edit the index block.** Run `glossary-index --write`. The test compares bytes.
- **Don't rename a heading to fix its wording** without grepping `glossary.md#<slug>` across the tree first. Changing
  the body is cheap; changing the anchor is a cross-cutting rename.
- **Don't add a second Notion page** for part of the vocabulary. One page mirrors one file; a split copy is a copy that
  drifts.
- **Don't paste Notion's text into the file.** See the pull procedure above.
- **Don't push from a dirty or unrebased worktree.** The page claims to be generated from `main`; generate it from a
  checkout that is current with `origin/main`.
