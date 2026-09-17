---
name: random-refactor
description: >
  Pick one tracked Rust file at random and pressure-test it against the workspace Rust skill, The Rust Book, a local
  standard-library clone, and similar patterns in this repository. Trigger for `/random-refactor`, "random refactor", or
  an opportunistic quality pass over a Rust file. Do not use for a named bug, a Linear issue, or a pull-request review.
---

# `/random-refactor`: one file, three comparisons, then the smallest honest edit

Read [`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md),
[`docs/rust-programming.md`](../../../docs/rust-programming.md), and [`.agents/skills/rust/SKILL.md`](../rust/SKILL.md)
first. Code is the source of truth. Comments, docs, tests, Presentations, and Workshops describe today. Git already
holds rejected alternatives, so those do not belong in the file.

One opportunistic pass over one file. Not a sweep, and not a substitute for
[`implement-issue`](../implement-issue/SKILL.md), [`review`](../review/SKILL.md), or
[`author-docs`](../author-docs/SKILL.md). This file must not grow.

## 1. Choose the file

If the user named a tracked `.rs` path, use it. Otherwise pick one tracked Rust file:

```bash
mapfile -t files < <(git ls-files '*.rs')
printf '%s\n' "${files[RANDOM % ${#files[@]}]}"
```

Refuse generated or vendored trees (`target/`, bindgen blobs, `OUT_DIR`). Record the path, crate, and `git log -1
--oneline -- <path>`.

## 2. Read the references that apply

Keep clones out of the workspace. Reuse `/tmp/navigator-rust-library` and `/tmp/navigator-rust-book` when `.git` is
there.

```bash
stdlib=/tmp/navigator-rust-library
[ -d "$stdlib/.git" ] || {
  git clone --depth 1 --filter=blob:none --sparse https://github.com/rust-lang/rust.git "$stdlib"
  git -C "$stdlib" sparse-checkout set library/core library/alloc library/std
}
book=/tmp/navigator-rust-book
[ -d "$book/.git" ] || git clone --depth 1 https://github.com/rust-lang/book.git "$book"
```

If a clone cannot run, fetch the matching rust-lang/book chapter or `library/std` source from
`https://doc.rust-lang.org/` and say so. Do not invent a standard-library API from memory.

Read only what the file uses. Ownership and modules from rust-lang/book; `Result`, `From`, iterators, `BTreeMap`,
channels, `Display` from `library/std` / `library/core`. Then the workspace Rust doc and skill, plus the [API
Guidelines](https://rust-lang.github.io/api-guidelines/checklist.html) and [Style
Guide](https://doc.rust-lang.org/style-guide/) that `docs/rust-programming.md` already names, and Microsoft's [Pragmatic
Rust Guidelines](https://microsoft.github.io/rust-guidelines/) (`M-UPSTREAM-GUIDELINES`: follow those first). Search
`library/std` for the same type before calling a local helper original.

## 3. Compare similar patterns

Identify the file's public types, error enum, traits, Axum extractors, store module, or CLI subcommand. Search the
workspace for the same names, the `thiserror`/`anyhow` split, extractor order, and test layout. Cite at least two
siblings as `path:line`. A one-off that duplicates a helper two crates over is a finding; a one-off that is the helper
is not.

## 4. Answer every review question with evidence

Each answer needs `path:line`. "Seems fine" is not an answer.

- **Is this actually needed?** Delete the unused path and its seam, rather than documenting why it remains.
- **Does it describe only the present?** Remove chronology. Keep only the why behind a live invariant.
- **Is it tested?** Name the covering test, or the missing behavior. A compile is not a test.
- **Is it documented?** Module docs and `docs/` describe today. Do not doc a private helper whose name is the contract.
- **If applicable, is it featured in our Presentations or Workshops?** Search `server/content/workshops/`
  (`RUST_IN_PEACE.md` is the presentations catalog). A cited code slide is the source: update it with the file. Leave
  spoken workshop words alone; see [`authoring-slides`](../authoring-slides/SKILL.md).

Also apply the Rust skill guards: no `unwrap` / `expect` / `panic!` outside `main()` and tests, no `unsafe`, Axum body
extractors last, comments that describe today.

## 5. Write the report, then maybe edit

Write one report at `/tmp/navigator-random-refactor/<file-stem>-<shortsha>.md`, including a zero-finding result. Keep
citations. Do not copy client data, legal files, real contact details, production identifiers, or Linear titles.

Then take exactly one of these actions:

- **Zero findings, or findings that need an author or council decision:** stop. Hand the report back.
- **Clear, small, evidence-backed findings:** smallest present-tense change, covering test first, one file. Run
  `cargo fmt`, the focused test, and `cargo run -p cli --quiet -- project gate`. For a Rust behavior change, also clippy
  with warnings denied and the crate's tests.
- **Teaching-surface drift only:** fix the doc, comment, or test. Do not rewrite workshop spoken words.

A behavior-preserving refactor is still a refactor: keep the covering test green, and do not add chronology.
