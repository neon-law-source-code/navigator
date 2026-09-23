---
title: "Signature"
---

The workflow suffix family `_signature` / `_signatures`, plus the `witnesses` prefix, records respondent-side signing.
See [`notation-authoring`](../notation-authoring.md#changing-the-workflow-composition) and
[`workflows::step::STEP_PREFIXES`](../../workflows/src/step.rs).

```text
┌─ signature ──────────────────────────────┐
│ id                record                 │
│ field             option<string>         │
│ inserted_at       datetime               │
│ notation_id       record<notation>       │
│ provider          string                 │
│ provider_id       string                 │
│ signed_at         option<string>         │
│ signer_person_id  option<record<person>> │
│ state             string                 │
│ updated_at        datetime               │
└──────────────────────────────────────────┘
```
