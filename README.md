# Neon Law Navigator

Neon Law Navigator is the open-source monorepo behind [neonlaw.com](https://www.neonlaw.com) — Neon Law's website, the
Neon Law Foundation's, and the software that delivers legal services. It is produced by the **Neon Law Foundation**, a
501(c)(3) nonprofit, and operated by Neon Law, the law firm. It combines versioned legal Notations, durable workflows,
attorney-reviewed automation, client and lawyer portals, the `navigator` CLI, and AIDA's agent tools.

It serves the lawyers, operators, and engineers who turn repeatable legal work into accountable client service. The
system keeps the lawyer as the actor while giving each matter a consistent intake, review, document, filing, and audit
path.

Navigator exists to make high-quality legal services easier to operate and more accessible without separating the public
mission from the delivery system that supports it. Neon Law practises consumer law on published flat fees; this is the
software that makes that economics work, and it is public so that anyone else can run it too.

Start with the [glossary](docs/glossary.md), use the [documentation index](docs/index.md) to find the narrow source of
truth, and follow <AGENTS.md> for local development and contribution workflows.

## Safe experimentation

Fork, prototype, and iterate quickly with synthetic or firm-owned source material. Never commit client or matter data,
legal files, real contact details, or production identifiers, and never put them in planning tools or agent transcripts.
Those belong in Navigator-managed systems and approved file stores. See
[`docs/public-contributor-safety.md`](docs/public-contributor-safety.md).

## License

Navigator is free software under the [GNU Affero General Public License, version 3](LICENSE): `AGPL-3.0-only`. Read it,
build it, fork it, and redistribute it.

**One licence covers everything the Foundation can license** — the Rust workspace, the `navigator` CLI, the build and
deployment tooling, and the notation bodies under `templates/`. There is no second grant to read and no per-tree
exception to look up. The blank government PDFs under `templates/forms/` are the issuing agency's work, and the
Foundation licenses nothing in them, because they were never the Foundation's to license.

**If you run a modified Navigator as a service, section 13 applies to you.** That is the clause that makes this the
Affero licence rather than the ordinary GPL: modify this software, let users reach it over a network, and you owe those
users the corresponding source of your version. Operating a legal-services portal for other people is exactly that
shape, so the obligation attaches to running it and not only to shipping it.

<LICENSE> holds that grant: the Free Software Foundation's text, unaltered, so every tool that reads a licence file
names it correctly. <NOTICE> beside it carries the Foundation's own statements — the copyright line, section 13 in its
own voice, the government forms it cannot license, and the marks it reserves. See
[`docs/licensing.md`](docs/licensing.md) for why this software is published at all, and <CONTRIBUTING.md> for how
contributions are licensed.

Copyright (C) 2026 **Shook Law PLLC**.

## Trademarks

Copyright and trademark both sit with **Shook Law PLLC**, the law firm that operates this software, and holding one does
not enlarge the other. **NEON LAW** is a registered trademark, U.S. Reg. No. 6,325,650, owned by **Shook Law PLLC**. The
licence grants rights in copyright, not in trademarks, so no amount of permission in `LICENSE` reaches the name — that
is the one thing a fork does not inherit.

The **Neon Law Foundation** is the publisher: it holds a perpetual, irrevocable, non-exclusive, royalty-free right to
publish Navigator under `AGPL-3.0-only`, binding the Firm's successors and surviving any change of the Firm's control. A
copyright holder can normally stop publishing whenever it likes; here it cannot, because a separate organization holds
the right to go on publishing and can enforce it.

Run it, fork it, redistribute it, and say your work is built on Neon Law Navigator. Do not present your deployment as
Neon Law: a law firm's mark is how a client identifies who is accountable for their legal work, so a fork trading as
Neon Law would misdirect exactly the person least able to check. Rename your deployment through the brand manifest
(`views::brand_bundle`) rather than by editing sources.

The Neon Law Foundation uses the mark for its charitable, pro bono, and public-education work under separate written
permission from the Firm.

## No legal advice

This repository is not legal advice, and using it does not create an attorney-client relationship.
