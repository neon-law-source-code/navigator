# Neon Law Navigator

Neon Law Navigator is the source-available monorepo behind [neonlaw.com](https://www.neonlaw.com) — Neon Law's website
and the software that delivers legal services. It is produced and operated by **Shook Law PLLC**, the law firm that
trades as Neon Law. It combines versioned legal Notations, durable workflows, attorney-reviewed automation, client and
lawyer portals, the `navigator` CLI, and AIDA's agent tools.

It serves the lawyers, operators, and engineers who turn repeatable legal work into accountable client service. The
system keeps the lawyer as the actor while giving each matter a consistent intake, review, document, filing, and audit
path.

Navigator exists to make high-quality legal services easier to operate and more accessible. Neon Law practises consumer
law on published flat fees; this is the software that makes that economics work, and the source is public so that anyone
can read it, learn from it, and build on it.

Start with the [glossary](docs/glossary.md), use the [documentation index](docs/index.md) to find the narrow source of
truth, and follow <AGENTS.md> for local development and contribution workflows.

## Safe experimentation

Fork, prototype, and iterate quickly with synthetic or firm-owned source material. Never commit client or matter data,
legal files, real contact details, or production identifiers, and never put them in planning tools or agent transcripts.
Those belong in Navigator-managed systems and approved file stores. See
[`docs/public-contributor-safety.md`](docs/public-contributor-safety.md).

## License

Navigator is **source-available, not open source**, under the [Business Source License 1.1](LICENSE): `BUSL-1.1`. Read
it, build it, fork it, redistribute it, and make any non-production use of it. **Production use needs a commercial
licence** from Shook Law PLLC. Four years after a version is published, that version converts to `AGPL-3.0-only` and the
restriction ends for it permanently.

Copyright (C) 2026 **Shook Law PLLC**.

## Trademarks

Copyright and trademark both sit with **Shook Law PLLC**, the law firm that operates this software, and holding one does
not enlarge the other. **NEON LAW** is a registered trademark, U.S. Reg. No. 6,325,650, owned by **Shook Law PLLC**. The
licence grants rights in copyright, not in trademarks, so no amount of permission in `LICENSE` reaches the name — that
is the one thing a fork does not inherit. BUSL says as much itself: it grants no right in any trademark or logo of the
Licensor.

Run it, fork it, redistribute it, and say your work is built on Neon Law Navigator. Do not present your deployment as
Neon Law: a law firm's mark is how a client identifies who is accountable for their legal work, so a fork trading as
Neon Law would misdirect exactly the person least able to check. Rename your deployment through the brand manifest
(`views::brand_bundle`) rather than by editing sources.

## No legal advice

This repository is not legal advice, and using it does not create an attorney-client relationship.
