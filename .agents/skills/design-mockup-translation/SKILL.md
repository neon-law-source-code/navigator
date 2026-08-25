---
name: design-mockup-translation
description: Translate an approved external design into Navigator's Dioxus surface.
---

# Translate a design mockup

Read `docs/design-mockups.md`, `docs/access-model.md`, and `docs/public-contributor-safety.md`.

- Port the intended layout, states, and copy through the existing Dioxus components; do not merge external prototype
  markup or add a client-side authorization decision.
- Build every named state, retain server-side authorization, add an SSR test, and use a browser check when needed.
- Design references and captures must be synthetic or firm-owned and contain no client data, legal files, real contact
  details, or production identifiers.
