---
title: "Template"
description: "A Template is the authored Markdown file that supplies a Notation’s metadata, questionnaire, workflow, and body."
---

The authored Markdown file under `templates/` that a Notation is created from — the firm's drafted text together with
the machine that gathers what the text needs. It has four parts: **metadata**, **questionnaire**, **workflow**, and
**body**. A lawyer calls the same file a **[Draft](draft.md)**; the two nouns name one thing.

**Metadata is a conceptual grouping, not a literal nested YAML key.** It names the frontmatter keys that classify the
file — `kind:`, `code:`, jurisdiction, respondent — which sit at the top level of the frontmatter block. A template that
declared an actual `metadata:` mapping would be malformed. The full anatomy is documented in
[`notation`](../notation.md).

A Template is versioned by append rather than by edit: `templates.is_current` marks the live revision and a change adds
a row. Every Notation pins the exact `templates` row it was created from, which is what keeps approved text from
silently re-rendering out of a later revision.
