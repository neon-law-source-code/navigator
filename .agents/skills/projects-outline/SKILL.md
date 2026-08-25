---
name: projects-outline
description: Explain or plan the Project repository and client-portal source contract.
---

# Project source outline

Read [`docs/project-repositories.md`](../../../docs/project-repositories.md),
[`docs/vibe-coding.md`](../../../docs/vibe-coding.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- A Project repository contains source only: portal code, templates, and checked-in configuration.
- Navigator owns data, authorization, and writes. Use its read and command boundaries; do not add a matter-data backend.
- Build and test fast with synthetic or firm-owned fixtures. Never place legal files, client data, real contact details,
  or production identifiers in repository source or planning notes.
