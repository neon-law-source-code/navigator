---
title: "Participation"
---

The `person_project_role.participation` column. It is a property of a [Person–Project Role](personproject-role.md) row,
never a standalone grant, and is derived from `person.role`. Its row's presence and its value answer different access
questions; the [access model](../access-model.md) defines the route-specific checks. Embedded Rego receives the role for
route admission, while the matter surface reads the participation ledger for data scope.

Not to be confused with [Disclosure](disclosure.md), which is the firm's conflicts log, not an access grant.

- See [`docs/access-model`](../access-model.md) for the full role + participation model.
