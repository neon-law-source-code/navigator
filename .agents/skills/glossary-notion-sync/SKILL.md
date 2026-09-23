---
name: glossary-notion-sync
description: >
  Sync [`docs/glossary/`](../../../docs/glossary/README.md) — one Markdown file per term — with its mirror page in the
  Notion "✏️ Writing" database, in either direction. Trigger when asked to push the glossary to Notion, pull a Notion
  glossary edit back into the repository, or refresh the Notion copy after a term changes. The repository is the source
  of truth; Notion is the place a colleague can read and propose. A pull is a pull request, never a paste.
---

# Syncing the glossary with Notion

Two copies of one vocabulary, and they are not peers:

- **[`docs/glossary/`](../../../docs/glossary/README.md) is the source of truth.** One file per term, reviewed in pull
  requests, embedded in the binary through `store::glossary::GLOSSARY`, materialized into `glossary_term` rows on every
  boot, published on one page at `/glossary`, and read by `navigator glossary list` / `navigator glossary show <term>`.
  A definition is not real until it lands here.
- **The Notion page is the reading and proposing surface** — a colleague without a checkout can open it, comment, and
  type. Its page URL is `https://app.notion.com/p/3d8c909308608139829dff990512a174`, titled **Glossary** in the **✏️
  Writing** database (`Type: Guideline`). Every push rewrites it wholesale, so an edit made there survives only until
  the next push unless somebody pulls it back into the repository first.

Say that out loud when handing someone the page. A colleague who believes they are editing the glossary, and is actually
editing a copy that gets overwritten, has been misled by us.

## The mechanical contract

`store::glossary::terms` is the reader of record for both directions:

- **Every `<slug>.md` beside the README is one term.** Its frontmatter carries `title:` (the term as a reader says it),
  and its body is the definition. `store::glossary::parse_entry` refuses a file whose name is not the slug of its title,
  so the file a reader opens and the anchor a link uses cannot disagree.
- **The slug is a public API.** `store::glossary::slugify` turns `Lawyer Review` into `lawyer-review`: the file name,
  the `/glossary#lawyer-review` anchor, and the `glossary_term.slug` row key. Places across `docs/*.md`,
  `store/src/projects.rs`, `mcp/src/tools/*.rs`, and `portal/src/api.rs` link `glossary/<slug>.md`. Renaming a term
  renames its file and breaks every one of them, so grep before renaming and fix the callers in the same commit.
- **Terms link each other as siblings** — `[Matter](matter.md)`. The web page turns that into `#matter`; a repository
  path climbs two levels (`../../store/`), and a contributor doc one (`../notation.md`). `M057` checks every link
  resolves on disk; `M061` checks the page can render it.
- **There is no index to maintain.** The page's side navigation and the CLI's `list` are both derived from the files.

## Pushing: repository → Notion

The push artifact is a command's output, not a hand-edit, so the page is a pure function of the repository.

```bash
cargo run -p cli --quiet -- glossary notion > /tmp/glossary-notion.md
```

That joins every term into one page (`## Title` then body, alphabetical by slug), resolves repository paths to `blob`
and `tree` URLs on `main`, unlinks sibling-term links down to their label because Notion has no heading anchors to point
at, undoes the repository's 120-character hard wrap so paragraphs flow, and prepends the provenance blockquote that
tells a reader on the page which way the sync runs.

Then replace the page with it:

1. Split the output at `##` heading boundaries into chunks of roughly 24 KB — the whole document is about 120 KB and
   will not go through in one call.
2. `notion-update-page` with `command: replace_content` and the first chunk.
3. `notion-update-page` with `command: insert_content` and `position: {"type": "end"}` for each remaining chunk, **in
   order**. Send them one at a time; a parallel insert scrambles the page.

Verify with `notion-fetch` on the page URL. The heading count it comes back with must match the number of lines `cargo
run -p cli --quiet -- glossary list` prints, and the last term must be Workshop.

## Pulling: Notion → repository

**A pull is a pull request.** Fetch, read, and hand-apply the change to the term's own file; never paste the Notion page
over the directory. The Notion copy has absolute GitHub URLs where the repository has relative paths, flowed paragraphs
where the repository hard-wraps at 120, and no links between terms at all, so pasting it back would rewrite every entry
to say the same thing worse.

1. `notion-fetch` the page and diff it against `cargo run -p cli --quiet -- glossary notion` output. What differs is
   what somebody typed — minus what Notion itself re-serializes. It returns a Markdown table as a native `<table>` block
   and drops the blank lines between blocks, so those differences are the round trip, not an edit.
2. **Read every difference before applying it.** This is the step that matters: Notion is where the firm's real matters
   live, and this repository is public. A definition that arrived through Notion may name a client, a matter, or a
   Project code. The no-client-data rule in [`AGENTS.md`](../../../AGENTS.md) governs the destination, not the source —
   rewrite the example as a mechanism ("a Project's publish") or use a synthetic code, and if a real code is already in
   the Notion text, say where it is and let a human decide.
3. Apply the wording to `docs/glossary/<slug>.md` in its own idiom: sibling links, hard wrap at 120. A new term is a new
   file named for the slug of its `title:`.
4. Refresh any schema box and run the gate:

   ```bash
   cargo run -p cli --quiet -- glossary tables --write
   cargo run -p cli --quiet -- project gate
   rtk cargo nextest run -p cli -p store -p portal
   ```

   `store::glossary::GLOSSARY` is `include_dir!`, so a glossary edit changes compiled data and the rows every boot
   materializes — the store tests are not optional here.
5. Open the pull request, then push the merged text back to Notion so the two copies agree again.

## What not to do

- **Don't hand-edit a schema box.** Run `glossary tables --write`. The test compares bytes.
- **Don't rename a term to fix its wording** without grepping `glossary/<slug>.md` across the tree first. Changing the
  body is cheap; changing the slug is a cross-cutting rename.
- **Don't add a second Notion page** for part of the vocabulary. One page mirrors one directory; a split copy is a copy
  that drifts.
- **Don't paste Notion's text into the files.** See the pull procedure above.
- **Don't push from a dirty or unrebased worktree.** The page claims to be generated from `main`; generate it from a
  checkout that is current with `origin/main`.
