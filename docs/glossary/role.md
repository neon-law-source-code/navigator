---
title: "Role"
description: "A Role is the system-wide authorization tier a Person carries in person.role."
---

The **system-wide authorization tier** a [Person](person.md) carries in `person.role`. There are exactly five tiers and
a person holds exactly one:

- **Owner** — the highest tier: the human who owns the system. Inherits every Admin and Lawyer capability and alone may
  create, edit, or demote an Owner identity. Owner may pass route admission and use the Project directory without a
  participation row, but matter-content routes still apply the participation gate.
- **Admin** — a licensed lawyer with system-administration authority. Admin may pass route admission and use the Project
  directory without a participation row, but matter-content routes still apply the participation gate. Admin cannot
  govern an Owner identity. Person deletion remains client-only for every privileged tier.
- **Lawyer** — a person licensed to practice law. Same per-Project visibility scope as `client`; the tier difference is
  in what the lawyer may *do* on a visible Project (edit, sign, file) and in supervising Clerk work.
- **Clerk** — a supervised non-lawyer firm worker. Clerk's read-only lens under `/app/projects` shows only firm-assigned
  Projects with a disclosed licensed-lawyer `lawyer_dri`; it never receives lawyer-work, advice, Git, MCP, or
  `/app/lawyer` authority by inheritance.
- **Client** — a person the firm represents on at least one matter. Sees only Projects with a matching
  `person_project_role` row.
- **Anonymous** — not signed in; no `person` row at all. The public visitor, who sees only public pages.

`role` is read from the DB row at callback time, never trusted from the OIDC token. A supported verified identity with
an email creates a `client` on first sign-in; the system's configured Owner email instead creates an `owner`.

- Schema: [`store::persons::Role`](../../store/src/persons.rs) — a stored `string` on `person`, defaulting to `client`
  and gated by `ASSERT $value IN ['owner', 'admin', 'lawyer', 'clerk', 'client']` in
  [`navigator.surql`](../../store/src/schema/navigator.surql); Anonymous is the absence of a row.
- See [`docs/access-model`](../access-model.md) for the full role + [Participation](participation.md) model.
