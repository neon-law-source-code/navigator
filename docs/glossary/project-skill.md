---
title: "Project Skill"
---

One practice playbook in the catalog at `skills/<jurisdiction_code_lowercase>/<practice_area>.md`: a jurisdiction, a
practice area, a name, a version, and the [`Notation`](../notation.md#notation) `code`s it bundles. Running `navigator
project skill use` pins one onto a [Project](project.md)'s `navigator.yaml`, scaffolding each bundled Notation `code`
into `templates/` so an attorney does not assemble the shelf by hand.

**Not an Agent Skill.** An Agent Skill is a `.agents/skills/<name>/SKILL.md` instruction file this repository's own
coding agents (Claude Code, Codex) read to drive their own development workflow — how to implement an issue, how to open
a pull request. A Project Skill is legal-practice reference data: what an engagement in one jurisdiction and practice
area needs. The two catalogs never read each other, and pinning a Project Skill never touches `.agents/skills/`.

The catalog directory is compiled into the `navigator` binary at build time, so `navigator project skill list/show` need
no network call or database connection. `jurisdiction` is validated offline against the seeded codes in
[`store/seeds/Jurisdiction.yaml`](../../store/seeds/Jurisdiction.yaml) — a free-text value or an unrecognized code fails
to parse. Pinning is by version string, not a Git commit: a Project's `navigator.yaml` records `{jurisdiction,
practice_area, version}`, and `navigator project skill status` (and `navigator project gate --check`, through the same
shared resolver) reports whether that pin still resolves against the compiled-in catalog.

- Reference: [`docs/project-skills.md`](../project-skills.md)
