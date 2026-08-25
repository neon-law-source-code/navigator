---
name: review-pr
description: >
  Address a requested pull-request comment against the real code at the PR head. Read the thread, source, and covering
  test; reproduce behavioral claims in the right KIND-backed environment; apply only the minimum valid fix; prove it;
  then reply and resolve that thread. Trigger when the user asks to address or review a PR comment, a Greptile finding,
  or a named review thread. For a request covering several comments, repeat this narrow action for each one without
  combining their scopes.
---

# `/review-pr` — address a PR comment

The authoritative procedure is [`Address a PR comment`](../../../docs/agent-workflows.md#address-a-pr-comment). Read it
before acting and keep the detail there.

The action is narrow:

1. Identify the PR and requested comment.
2. Read the PR head, complete thread, cited source, and covering test.
3. Reproduce the claim against the right KIND-backed topology when practical.
4. Adjudicate it as valid, invalid, or valid-but-not-worth-changing, with evidence.
5. Apply only the valid comment's minimum fix and covering test.
6. Run the affected gate and prove the changed behavior.
7. Push, reply with the proof, and resolve only the handled thread.
8. Report unrelated comments, CI failures, and branch state without changing them.

For a dirty tree that needs a new PR, use `create-pr`. For a failed GitHub Action, use the separate failed-Action flow
in [`docs/agent-workflows.md`](../../../docs/agent-workflows.md#address-a-failed-github-action).
