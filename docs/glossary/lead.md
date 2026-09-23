---
title: "Lead"
---

A public request for contact. Capture writes a `lead` row: mailbox, optional phone, brand, source path, consent, status,
and submission count. That row is not an identity. The human directory is [Person](person.md). `store::leads::convert`
creates a Client through `store::persons::create` and sets `lead.person_id`. When the mailbox already belongs to a
Person, the queue links that row instead of forking a second one.

Talking to a lead is attorney work under professional ethics (advertising and solicitation), not a sales sequence.

- Schema: [`lead` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::leads`](../../store/src/leads.rs)

```text
┌─ lead ──────────────────────────────────┐
│ id               record                 │
│ brand_key        string                 │
│ consent_version  string                 │
│ consented_at     datetime               │
│ email            string                 │
│ email_lower      string                 │
│ inserted_at      datetime               │
│ person_id        option<record<person>> │
│ phone            option<string>         │
│ source_path      string                 │
│ status           string                 │
│ submissions      int                    │
│ unsubscribed_at  option<datetime>       │
│ updated_at       datetime               │
└─────────────────────────────────────────┘
```
