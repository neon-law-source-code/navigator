---
title: "Brand Seed"
description: "A Brand Seed is the data layer a Site applies on every boot of its binary, including production."
---

The seed layer a [`Site`](brand.md) owns, applied on every boot of that binary **including production**. Keyed to the
serving binary, not to a request's resolved [`BrandKey`](brand.md): it carries the data one binary holds and another
must not — `neon` seeds the Firm's own entities and postal identities regardless of which house brand a given request
resolves to, and `tenant` seeds none of ours at all ([`store::seed::BrandSeed`](../../store/src/seed.rs)).

The canonical layer keeps the *shared registry* — the firm anchor and the identities every deployment resolves by name.
An entity no deployment of ours does business as belongs nowhere in these layers at all, which is what keeps a `tenant`
boot carrying none of our corporate records.

It is the middle of three layers in [`store::seed`](../../store/src/seed.rs), and the distinction that matters is which
reach production:

1. **Canonical** — the shared identities, reference data, and catalog. Every brand, every environment.
2. **Brand** — this layer. The booting brand only, every environment.
3. **[Sample matter fixture](sample-matter-fixture.md)** — three synthetic matters, their local participants, and
   their supporting rows. Applied only where the matters are sample, so the shared examples are ready whenever local
   development starts and never reach a deployment holding real files.

A brand crate declares its `BrandSeed` in the `Site` value it hands to the shared run loop, so the seed set is chosen by
which binary is running rather than by configuration. The Firm's mailboxes sit alongside other entities' at one mail
center, sharing a street, a suite, and a ZIP and differing only in the box number — which is why "seed them all
everywhere" reads as correct in any test that merely counts rows, and why the layer exists.
