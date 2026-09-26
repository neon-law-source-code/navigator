---
title: "Module"
description: "A Module is a per-matter capability enabled by lawyers on a Project."
---

A per-matter **capability**, enabled by lawyers on a Project. Every Project opens as a blank slate; practice-area
capability arrives as modules rather than as a project type, because one engagement can run litigation **and** a cap
table at once and a single type column cannot express that.

The set is closed — `litigation`, `cap_table`, `estate`, `deadlines` — and widening it is a deliberate enum addition
with a migration, never a free-text value invented at a call site.

**Presence of the ledger row is the enabled state.** Disabling deletes the row; there is no enabled flag and no disabled
timestamp. That keeps "is this module on" a single unambiguous question, and it is what makes the client lens
**toggle-blind by construction**: a disabled module has no row for any query to return, so there is no disabled state
for a response to leak. A client must never be able to infer that a module exists but was withheld — not by name, not by
a flag, not by an empty slot.

Disabling hides a capability; it never deletes what the module owns. Every toggle, in both directions, writes a
relationship-log entry naming the module and the actor.

- Commands and schema: [`store::project_modules`](../../store/src/project_modules.rs) ·
  [`store/src/schema/navigator.surql`](../../store/src/schema/navigator.surql)
