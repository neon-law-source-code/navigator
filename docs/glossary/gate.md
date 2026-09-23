---
title: "Gate"
---

`navigator project gate`, the command that checks one recognised repository — every Markdown, YAML, and seed document in
it — against the Neon Law Navigator rule set, the same engine `navigator-lsp` runs on every keystroke. It takes no path:
it identifies the repository root by the `README` and `.git` beside it and refuses to run anywhere else. Safe-by-
construction fixes land as it goes; errors fail the run and warnings print but pass. `--ci` writes nothing, holds the
origin pass to a built tree, and on a push to `main` checks the manifest against the live row.

`navigator validate [DIR]` is the same rule set aimed at an arbitrary directory. It defaults to `.`, takes `--fix`,
`--errors-only`, and `--ci`, and makes no assumption about the surrounding repository.

- Reference: [`gate`](../gate.md)
