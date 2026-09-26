---
title: "Docket Entry"
---

One typed, numbered entry on a litigation case's docket — the court's own record of what was filed or served. The
generic spine every case record hangs off, mirroring how a court docket actually works: a numbered list of typed entries
rather than a table per instrument. A new niche instrument type is one value in the closed, code-extended `kind` set,
never a migration.

The entry number is **text, not an integer**. Real dockets use attachment sub-numbers such as `29-1`, and an entry list
has to reference them exactly.

An entry with **no document attached is a meaningful state**, not an error: it renders as *source pending*.
Staged-versus-pending is derived from whether the document link exists, never from hand-maintained copy.

A **hearing** or **trial** records when the court set the appearance in `scheduled_on`. That field is required for those
two kinds and refused for every other. A continuance is a new docket entry of the same kind carrying the new date and a
`supersedes` link to the prior entry, so the earlier date stays on the record. The project calendar shows the entry no
later entry supersedes. Existing rows that predate `scheduled_on` are reported by a store read; they are never
backfilled. An appearance is a fact the docket records — it is not a Deadlines obligation.

**Docket belongs to litigation.** The case docket is the court record of filings. The cross-practice surface answering
*what is due* is the Deadlines module — litigators speak of *docketing* and corporate lawyers of a *compliance
calendar*, but each maps to one schema noun, the same way Matter maps to Project. This module records what exists; the
Deadlines module answers what is due.

- Commands: [`store::cases`](../../store/src/cases.rs) · Schema:
  [`case_docket_entry`](../../store/src/schema/navigator.surql)
