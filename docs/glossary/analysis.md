---
title: "Analysis"
---

The workflow prefix `analysis` is a system wait state for review-in matters: the web app performs the contract analysis,
persists the findings, and signals the workflow when `analysis_ready` is available. See
[`notation-authoring`](../notation-authoring.md#changing-the-workflow-composition) and
[`workflows::step::STEP_PREFIXES`](../../workflows/src/step.rs).
