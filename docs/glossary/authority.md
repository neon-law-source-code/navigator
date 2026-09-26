---
title: "Authority"
description: >-
  An Authority is a case, statute, regulation, administrative proceeding, or secondary source held as global reference
  data.
---

One case, statute, regulation, administrative proceeding, or secondary source, as **global reference data**. An
Authority carries its citation, its title and publisher, its canonical URL, and an archived artifact so it survives link
rot. It is deliberately **not case-shaped**: a statute is a first-class Authority, not a case record with its fields
bent to fit.

An Authority carries **no `project_id`**. The same authorities recur across matters, and a matter's *use* of one is a
separate scoped record holding which side relies on it (`ours`, `adverse`, `neutral`) and what the firm did with it.
This is the participation shape — global entity, scoped relationship — and inverting it would leak one matter's
litigation posture into another matter's view of the same case.

The disposition on a matter's use is a closed taxonomy. Several of its values (`reviewed-not-used`,
`record-exhibit-not-relied-on`, `captured-exhibit-not-quoted`, `monitoring-not-relied-on`) record **firm reasoning** —
what the firm considered and chose not to rely on. A client who sees "reviewed, not used" learns the firm's strategic
assessment of their own matter, which discloses work product rather than merely data, so none of them may ever enter a
client-facing allowlist.

A composition references an Authority by id and the server resolves it under the lens gate. Embedding citation prose
instead is the failure mode: it drifts from the record and cannot be re-verified.

- Vocabulary: [`rules::citation`](../../rules/src/citation.rs) · Schema:
  [`authority` in `navigator.surql`](../../store/src/schema/navigator.surql) Queries:
  [`store::authorities`](../../store/src/authorities.rs) Lives in: the `authority` table in SurrealDB
