---
title: "Revision"
---

One `assets` row under a [Document Identity](document-identity.md) — a single version of a living document. Revisions
accumulate by convention rather than an append-only trigger, because a governed expunge must be able to delete every
copy; an expunge removes the **whole slug chain**, never one revision.

Which revision is *operative* is derived from insertion order, not a stored flag:

- **Current for a lawyer** — the latest row under the slug (ids are UUIDv7, so id order is insertion order).
- **Current for a client** — the latest row that is both published (`published_at IS NOT NULL`) and
  `visibility = 'client'`.

One rule gives both behaviours a flag would have had to keep in sync by hand: an unpublished redline sitting above the
executed agreement changes nothing for the client, while the client keeps seeing v2 as lawyers iterate on v3.
`published_at` is back-datable to a court's file stamp and is display and sort metadata only — it never reorders a
chain.

Redaction is **two documents, not two revisions**: a redacted public slug beside a sealed unredacted one, the unanimous
CM/ECF practice (Fed. R. Civ. P. 5.2(f)). Per-revision visibility is never a redaction seam.

- Read rule: [`store::assets::current`](../../store/src/assets.rs) (history via `store::assets::revisions`)
