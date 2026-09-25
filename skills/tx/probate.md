---
jurisdiction: TX
practice_area: probate
name: Texas Probate Playbook
version: "1"
notations:
  - onboarding__letter
  - offboarding__letter
---

# Texas Probate Playbook

This playbook is a synthetic practice guide for a decedent's-estate probate engagement under Texas law. It carries no
real matter, client, or production identifier — see `docs/public-contributor-safety.md`. It exists to exercise the
Project Skill catalog end to end: parsing, listing, pinning, and gate resolution.

## Scope

A Texas probate engagement under this playbook typically opens with an application to probate a will (or, absent a will,
an application for letters of administration) under Texas Estates Code Title 2, and closes with a final accounting and
an order of discharge.

## Intake

Pin this skill with:

```bash
navigator project skill use tx probate
```

Pinning bundles the `onboarding__letter` and `offboarding__letter` Notations onto the Project, so an attorney can open
and later close the engagement with the synthetic client described in
[`sample-estate`](https://github.com/neon-law-staging/sample-estate) once the fixture is seeded.

## Checklist

1. Confirm the county of proper venue — the decedent's domicile, or the county where the decedent's principal Texas
   property sat if domiciled elsewhere.
2. Confirm whether the estate qualifies for independent administration, which most Texas wills provide for.
3. Route every filing through `lawyer_review` before submission, per
   [`docs/notation-authoring.md`](../../docs/notation-authoring.md).
4. Close the engagement with `offboarding__letter` once the court discharges the personal representative.

## Out of scope

This playbook does not cover pre-death estate planning — see `skills/nv/estates.md` for the sibling checklist a planning
engagement in another jurisdiction follows, and open a distinct Project Skill for a Texas estate-planning practice area
rather than overloading this one.
