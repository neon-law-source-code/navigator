---
name: stay-in-repo
description: >
  Confines an agent session to the current repository's own checkout — never read, search, or write to a sibling
  checkout, another Project's repository, the home directory, or system temp. Trigger at the start of every session in
  a Navigator Project repository (a client portal or notation-template checkout), and whenever a task seems to need
  something from outside the current tree. Synced verbatim into every Project repository's `.claude/skills/` by
  `navigator projects repository sync-skills`; this canonical copy lives in Navigator's own `.agents/skills/`.
---

# Stay inside this repository

Read, search, and edit only files inside this repository's own checkout. Do not open, read, or search files elsewhere
on the machine — not the home directory, not a sibling checkout, not another Project's repository, not system paths,
and not another worktree of this repository.

`node_modules/` inside this tree is the one exception: reading a dependency's shipped types or stylesheet there is
part of using the toolchain, not reaching outside it.

This is a scope rule, not a security boundary, and the reason for it is concrete rather than procedural: another
folder on this machine may hold a real client matter, a different Project's fixture data invented on purpose to
differ from this one, or a different revision of the Navigator route this bundle mounts under. Copying from any of
those by hand imports a decision, or a fact, without importing the reason for it — and a change justified by a file
nobody else can see is a change the next contributor cannot verify.

If a task genuinely cannot be done without something outside this tree, say so and ask rather than going to find it.
Installing dependencies and fetching public sources — a package registry, a published release, public case law — is
fine; reading another checkout on this machine is not.

Scratch files belong in the session's own scratchpad, never in this repository's working tree — a temporary script
left behind becomes somebody's confusing diff.
