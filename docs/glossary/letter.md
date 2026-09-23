---
title: "Letter"
---

One physical piece of mail, incoming or outgoing, scoped to a Mailroom.

- Schema and queries: [`store::letters`](../../store/src/letters.rs) (SurrealDB; #1093, ENG-20) —
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)

```text
┌─ letter ──────────────────────┐
│ id           record           │
│ direction    string           │
│ inserted_at  datetime         │
│ mailroom_id  record<mailroom> │
│ recipient    string           │
│ sender       string           │
│ summary      string           │
│ updated_at   datetime         │
└───────────────────────────────┘
```
