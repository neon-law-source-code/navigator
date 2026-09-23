---
title: "Filing"
---

The workflow prefix `filing` records a named government filing. It is an outbound submission step and must sit behind
[Lawyer Review](lawyer-review.md). See the
[`notation-authoring`](../notation-authoring.md#changing-the-workflow-composition) guide and
[`workflows::step::STEP_PREFIXES`](../../workflows/src/step.rs).

```text
┌─ filing ───────────────────────┐
│ id            record           │
│ inserted_at   datetime         │
│ kind          string           │
│ notation_id   record<notation> │
│ office        string           │
│ reference     option<string>   │
│ submitted_at  string           │
│ summary       string           │
│ updated_at    datetime         │
└────────────────────────────────┘
```
