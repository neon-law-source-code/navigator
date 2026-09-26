---
title: "Presentation"
description: "A Presentation is a repo-authored deck of teaching or speaking material."
---

A repo-authored deck of teaching or speaking material. Presentations live only at the top-level `/presentations`
catalog; workshops live only at the top-level `/workshops` catalog. There is no prefixed aggregate or umbrella program.

**Presentations are anonymous.** The talks catalog renders at `/presentations` and each talk reads beneath it, under the
firm's chrome, with no rule in `navigator.rego` at all.

**Workshops are anonymous too.** The `/workshops` catalog page and the class material beneath `/workshops/{slug}` both
mount under the firm's chrome with no session boundary, alongside the talks. The repository is open source and the
classes teach the software it publishes, so gating them would put a login door in front of the one document explaining
how to run what anyone can already clone. The certificate `POST` keeps its own gate — who may *claim* a completion
certificate stays an authorization question even when the material is free to read.

Presentation and workshop material is **repo-authored, and stays that way**. The markdown under
[`server/content/workshops/`](../../server/content/workshops/) is indexed by a hard-coded manifest in
[`portal::workshops::loader`](../../portal/src/workshops/loader.rs): a file the manifest does not name is not published
material, and frontmatter is stripped rather than read. That is deliberate rather than incidental, because two guards
assert the published material agrees with the repository and neither one survives a move into the database — one holds
every code slide byte-identical to the workspace file it cites, and one asserts the Operating workshop's Environment
Matrix names every key in `.env.example`
([`server/tests/deploy_workshop_environment.rs`](../../server/tests/deploy_workshop_environment.rs)). A slide is a claim
*about this repository*, so the repository is what checks it.

A [Workshop](workshop.md) is the matter someone enrols in, which may teach from the repo-authored material — two nouns,
deliberately.
