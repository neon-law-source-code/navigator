---
title: "Sample Matter Fixture"
description: "A Sample Matter Fixture is one of the three synthetic matters applied alongside the canonical seed."
---

The three synthetic matters a boot applies on top of the canonical seed wherever `NAVIGATOR_SIMULATED_MATTERS` resolves
true. Written by [`store::seed::seed_sample_portfolio`](../../store/src/seed.rs), idempotent, and it keeps the local
accounts and all three matters ready for the firm, clerk, and client surfaces.

The three are deliberately different shapes of legal work, because one matter can only demonstrate one:

| Code | Matter | Practice | Client |
| --- | --- | --- | --- |
| `sample-litigation` | *Cruller v. Prine* | trespass and rescission | an individual plaintiff |
| `sample-transactional` | *Widget Works — Outside Counsel* | employment and contract review | a Nevada C-Corp |
| `sample-estate` | *Estate of Cornelius Montgomery* | an estate plan | an individual testator |

Each carries its own companion application, refreshed from its own public repository during local boot and served at
`/app/projects/{code}/portal/`. The project code is the URL slug: lowercase letters and numbers joined by single
hyphens, with no UUID in the project show URL.

A `dev` boot with no built bundle staged publishes a deterministic placeholder document, which is what keeps a portal
serving something while a Vite build is broken. A boot under the **production** deployment profile publishes nothing:
whatever sits in that deployment's applications bucket was published by an operator and is authoritative, so the seed
leaves it alone and an unpublished portal answers 404 rather than a placeholder that looks like a working application.
That is the one place [Deployment Environment](deployment-environment.md) reaches past which rows get seeded and into
what gets written to object storage.

The fixture Client participates in all three, so a signed-in client sees a project list worth looking at. The fixture
Admin participates in none of them — see [Deployment Environment](deployment-environment.md) for which deployments apply
this layer at all.
