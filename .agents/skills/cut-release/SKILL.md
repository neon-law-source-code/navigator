---
name: cut-release
description: Prepare a named Navigator version bump for review and release through `main`.
---

# Cut a release

Read [`docs/gitops.md`](../../../docs/gitops.md), [`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md). A release is a version bump landed
through a PR; merging `main` drives publication.

- Verify the requested version and the current manifest before changing it.
- Make the smallest version-only commit, run the documented gate, and open the PR against `main`.
- Stop when that PR merges. Report its URL. Do not watch `deploy.yml` for the tag, images, archives, or tap.
- Do not deploy, mutate production, or copy production coordinates into the branch, PR, or release notes.
