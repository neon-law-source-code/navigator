---
name: project-pr-delivery
description: >
  Deliver a clean, signed change from a Navigator Project repository through its pull request to a merged or named
  bounded stop verdict.
---

# Deliver a Project pull request

Use Navigator's repository verb for the forge workflow. Do not transcribe `gh` or forge API commands into a Project
repository.

1. Run the Project gate and fix every finding.
2. Commit the complete change on a topic branch with a signed Conventional Commit. Fetch and signed-rebase onto
   `origin/main` immediately before delivery.
3. Put the public pull-request explanation in a Markdown file, then run:

   ```bash
   navigator project repository deliver \
     --branch <topic-branch> \
     --title '<conventional title>' \
     --body-file <path-to-body.md>
   ```

The verb gates the clean tree again, verifies the topic commits are signed, pushes the branch, opens or adopts its pull
request, explicitly arms auto-merge, reads the setting back, and watches for a bounded verdict. Report its URL and exact
verdict. A failed required check, required review, outdated branch, unarmed auto-merge request, or timeout is a stop
condition to resolve; do not describe any of them as merged.
