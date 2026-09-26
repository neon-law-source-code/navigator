---
title: "Repository"
description: "A Repository is a provenance record for an external Git source used by a Notation."
---

A provenance record for an external git repository that notation content came from. The `git_repositories` row holds a
hash of the remote URL and the last imported commit SHA. One row per external source, shared across Projects. Rows are
written by the canonical seed in [`store::seed`](../../store/src/seed.rs); no command fetches these remotes today.

`navigator site seed` does **not** write here — it walks a local directory given on the command line and never reads a
remote or records a commit SHA. This is the *external imports* flavor. The `git_repositories` table tracks outside
sources the workspace reads notation *from*; it is the only Git this workspace knows about besides its own code
repository, and it is unrelated to Projects (see [Project](project.md)), which have no repository at all.

- Schema and queries: [`store::git_repositories`](../../store/src/git_repositories.rs) (SurrealDB; #1093, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)
