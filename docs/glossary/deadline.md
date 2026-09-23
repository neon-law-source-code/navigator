---
title: "Deadline"
---

A forward-dated obligation on a [Project](project.md) — a pleading due, a filing due, a statutory window closing. This
is the firm's docket: a missed deadline is a malpractice event, not a backlog item. Deadlines are the one
forward-looking record in the schema. [Notation Event](notation-event.md) is an immutable journal and is past tense by
construction, and [Filing](filing.md) names a workflow step that runs, not a date it is due by.

The table is `project_deadlines`. [Matter](matter.md) is client-English for the same row, and lawyer-facing copy may say
"matter deadline", but the schema speaks `project` without exception — every other table already does.

**Authority.** Every deadline records *why the date binds*: a closed `authority_kind` vocabulary — `statute`,
`court_rule`, `court_order`, `contract`, or `internal` — beside a free-text `authority` citation such as `15 U.S.C. §
1681i(a)(1)` or a court-order reference. "Statutory" stays a queryable distinction rather than collapsing into a general
bucket, and a deadline nobody can justify cannot be written: `statute` and `court_rule` both require a citation.
Distinct from `source`, which records the *producing workflow* — or `lawyer` for a hand-entered date — not the
authority.

**Stored, never derived.** The due date is written down, not recomputed at read time. A rule can change — a statute is
amended, a court rule is revised — and recomputing would silently move a date the firm already docketed and relied on.
The stored date is the one malpractice exposure attaches to; `authority` records the rule that produced it, so the
derivation stays auditable. Computing court days (per jurisdiction, with holiday calendars and service-method
extensions) is deliberately out of scope — a deadline must never *pretend* to have counted court days.

**Two lead times.** A deadline carries separate internal and client lead times, because the firm is warned before the
client is: the internal lead is never shorter than the client lead. Either may be unset, and an unset client lead means
the client is never warned about that date.

**Idempotency is explicit.** A workflow-written deadline carries a `replay_key` so a replayed [`ctx.run`](ctxrun.md)
step updates its row instead of duplicating it. A hand-entered deadline leaves the key unset, so two genuinely different
pleadings sharing a kind and a trigger date stay two rows — an idempotency key, not a natural key that silently merges
malpractice-relevant records.

```text
┌─ statutory_deadline ─────────┐
│ id           record          │
│ due_on       string          │
│ inserted_at  string          │
│ kind         string          │
│ project_id   record<project> │
│ source       string          │
│ status       string          │
│ statute      string          │
│ trigger_on   string          │
│ updated_at   string          │
└──────────────────────────────┘
```
