---
title: "Lawyer Review"
description: >-
  The workflow prefix lawyer_review is the mandatory human attorney/lawyer gate before a document is
  sent for binding signature, certified mail, e-filing, or another outbound submission.
---

The workflow prefix `lawyer_review` is the mandatory human attorney/lawyer gate before a document is sent for binding
signature, certified mail, e-filing, or another outbound submission. A rejected review does not dead-end — it routes
`changes_requested → reask__client` to re-collect only the flagged answers (see [Re-ask](re-ask.md)), and reserves
`rejected → END` for a genuine withdrawal. See
[`notation-authoring`](../notation-authoring.md#changing-the-workflow-composition),
[`workflows::guardrail`](../../workflows/src/guardrail.rs), and
[`workflows::step::STEP_PREFIXES`](../../workflows/src/step.rs).
