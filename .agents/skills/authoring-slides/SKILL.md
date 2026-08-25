---
name: authoring-slides
description: Create or structurally repair Navigator workshop and presentation Markdown.
---

# Author slides

Read the relevant workshop source, [`docs/agent-workflows.md`](../../../docs/agent-workflows.md), and
[`docs/public-contributor-safety.md`](../../../docs/public-contributor-safety.md).

- Preserve an existing deck's spoken words and notes; change only structure or typography unless its author approves
  the copy change.
- Follow the loader's required frontmatter, chapter, slide, and notes shape; use the CLI validator on the changed file.
- Use repository-owned, public-safe media only. Never put legal files, client imagery or data, real contact details, or
  production identifiers on a slide, in its source, or in a capture.
- For a bucket-lane slide image, keep its ignored local copy at
  `server/public/img/<deck-slug>/<filename>`, then publish and verify staging (`neon-law-stg-assets`) before the
  separate `<production>-assets` handoff.
- If production remains pending, say so; do not describe a staging-only image as published everywhere.
- Keep generated captures in `/tmp`; use the established asset workflow for approved public media.
