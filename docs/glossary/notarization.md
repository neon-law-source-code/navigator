---
title: "Notarization"
---

The workflow prefix `notarization` records a respondent signing or refusing in front of a notary. See
[`notation-authoring`](../notation-authoring.md#changing-the-workflow-composition) and
[`workflows::step::STEP_PREFIXES`](../../workflows/src/step.rs).

```text
┌─ notarization ───────────────────────────┐
│ id                record                 │
│ asset_id          option<record<asset>>  │
│ inserted_at       datetime               │
│ notarized_at      option<string>         │
│ notary_person_id  option<record<person>> │
│ notation_id       record<notation>       │
│ provider          string                 │
│ provider_id       string                 │
│ updated_at        datetime               │
└──────────────────────────────────────────┘
```
