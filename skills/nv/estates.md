---
jurisdiction: NV
practice_area: estates
name: Nevada Estates Playbook
version: "1"
notations:
  - onboarding__letter
---

# Nevada Estates Playbook

This playbook is a synthetic practice guide for a general estate-planning engagement under Nevada law. It carries no
real matter, client, or production identifier — see `docs/public-contributor-safety.md`. It exists to exercise the
Project Skill catalog end to end: parsing, listing, pinning, and gate resolution.

## Scope

A Nevada estates engagement under this playbook typically opens with a revocable living trust, a pour-over will, and a
durable power of attorney, reviewed against Nevada Revised Statutes Chapter 132 (trusts) and Chapter 133 (wills).

## Intake

Pin this skill with:

```bash
navigator project skill use nv estates
```

Pinning bundles the `onboarding__letter` Notation onto the Project, so an attorney can open engagement with the
synthetic client described in [`sample-estate`](https://github.com/neon-law-staging/sample-estate) once the fixture is
seeded.

## Checklist

1. Confirm the client's Nevada residency and the situs of the estate's principal assets.
2. Confirm whether a prior trust or will exists that this engagement amends or restates.
3. Route the drafted instruments through `lawyer_review` before any signature step, per
   [`docs/notation-authoring.md`](../../docs/notation-authoring.md).
4. Record the engagement's governing law as Nevada, absent an explicit client instruction otherwise.

## Out of scope

This playbook does not cover probate administration — see `skills/tx/probate.md` for the sibling checklist a probate
engagement in another jurisdiction follows, and open a distinct Project Skill for a Nevada probate practice area rather
than overloading this one.
