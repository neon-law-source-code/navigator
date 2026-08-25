---
name: create-legal-workflow
description: Add a legal workflow through the existing template, questionnaire, and durable-runtime seams.
---

# Create a legal workflow

Read [`docs/notation-authoring.md`](../../../docs/notation-authoring.md),
[`docs/durable-workflows.md`](../../../docs/durable-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md) before changing a workflow.

- Start with a composition test, then use existing template and step vocabulary before adding runtime code.
- Put non-deterministic effects behind the documented durable boundary and add covering tests with the implementation.
- Treat template bodies as attorney-reviewed legal material; do not use a real client file, fact pattern, contact
  detail, matter code, or production coordinate as an example.
