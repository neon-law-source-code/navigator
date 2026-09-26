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
