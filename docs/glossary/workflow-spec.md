---
title: "Workflow Spec"
description: "A Workflow Spec is the parsed workflow graph from a Template, with required BEGIN and END states."
---

The parsed form of a Template's `workflow:` block — a set of named States, transitions keyed by event, with `BEGIN` and
`END` required. Produced once at boot from the template frontmatter; reused for every Notation of that Template. See
[`workflows::spec`](../../workflows/src/spec.rs).
