---
title: "Draft"
description: "Draft is the lawyer’s English name for a Template."
---

A **[Template](template.md)** in the lawyer's English. The same authored Markdown file, under the noun said out loud in
the office: reusable source carrying all four parts — **metadata**, **questionnaire**, **workflow**, and **body** — that
a [Notation](../notation.md#notation) is created from. *"Send me the engagement draft"* and *"send me the engagement
Template"* name one file and one row. The full anatomy is taught in [`notation`](../notation.md#template).

A Draft is versioned by append rather than by edit, exactly as a Template is: `templates.is_current` marks the live
revision, a change adds a row, and every Notation pins the row it was created from. That is a different mechanism from
the asset lane's [Document Identity](document-identity.md) slug, where a re-upload adds a revision to a living document;
authored source is never re-uploaded over.

Capitalized Draft is that reusable authored source. Three narrower lowercase uses of "draft" are not this noun and keep
their own meanings:

- The Notation workflow `state` value `draft` — one state in a running Notation's lifetime, beside `lawyer_review` and
  `signed`. A Notation exists through intake and drafting and holds its identity through review and signature; signing
  advances the same Notation rather than creating a second one.
- The [Document Drafts](document-drafts.md) workflow prefix `document_drafts__*`, a system wait state for web-rendered
  review-document rows.
- `review_document.status`, whose default is `draft` — the status of one attorney-reviewed instrument a client reads,
  not of the authored source it was rendered from. Schema: [`review_document` in
  `navigator.surql`](../../store/src/schema/navigator.surql).
