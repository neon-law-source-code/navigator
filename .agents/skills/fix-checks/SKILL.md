---
name: fix-checks
description: Address one failed check or one inline review comment on an open pull request.
---

# Fix checks

Read the matching action in [`docs/agent-workflows.md`](../../../docs/agent-workflows.md) and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- For CI, read the first actionable failure, reproduce it when possible, and make the smallest root-cause fix.
- For a review comment, read the thread and PR head, decide from evidence, and change only what the thread requires.
- Keep unrelated findings separate. Reply with proof and resolve only handled threads.
- Logs, comments, screenshots, and reports use identifiers and synthetic examples only; never disclose client data,
  legal files, real contact details, or production identifiers.
- Reference planning by bare Linear issue identifier (`ENG-1234`); the roadmap stays private, so no `linear.app` URL,
  issue title, or project name enters a PR body, reply, or commit. See [Linking a PR to its Linear
  issue](../../../docs/agent-workflows.md#linking-a-pr-to-its-linear-issue).
