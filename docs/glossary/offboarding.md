---
title: "Offboarding"
description: "Offboarding is the codebase term for a Notation that closes a matter."
---

The codebase term for the notation that **closes a matter** — `rules::kind::Kind::Offboarding`, classified by
[`Kind::closes_a_matter`](../../rules/src/kind.rs), the mirror of [`Kind::opens_a_matter`](onboarding.md). In
conversation and with clients this is the **closing letter**: the firm-signed letter that confirms the representation is
concluded, seeded as `notations/neon_law/offboarding.md` (`code: offboarding__letter`). A closed matter's repository is
archived afterward as a [Closed Repository](closed-repository.md) — a separate step this close never gates.

`store::projects::matter_lifecycle_sets` keys the matching lifecycle flag off this classifier — never off the template's
`code` — so a bespoke closing letter still clears the badge as long as it declares `kind: offboarding`. The
lawyer-facing lifecycle indicator on the Projects list reads **presence, not execution**: `matter_lifecycle_sets`
matches a notation or asset row by its declared kind and reads no signature state, so the green pill is labelled
`onboarding on file` — a location rather than a status. A status word there (`live`, `in good standing`) would assert
that the matter is properly papered on evidence that only shows a row exists. The Restate step names inside that
template's `workflow:` block (`generate_pdf__closing_letter`, `firm_signature__closing_letter`) and the
`closing_letter_storage_key` object-storage prefix are **deliberately frozen** at their old spelling — a Restate step
name is part of a durable journal, and the storage prefix already has objects filed under it, so renaming either would
orphan an in-flight invocation or an existing document rather than merely rename a word.
