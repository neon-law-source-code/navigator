# Contributing

**Neon Law Navigator is open source, and it is currently closed to outside contributions.**

The Neon Law Foundation produces this software; Shook Law PLLC, trading as Neon Law, operates it. Issues and pull
requests from outside those two organizations are not being accepted right now. This is a capacity decision rather than
a licensing one: the software runs a live legal practice, every change to it needs review by someone who can weigh the
practice consequences, and there is not review capacity to offer an outside contributor today.

**Write to [contact@neonlaw.org](mailto:contact@neonlaw.org).** Anyone is welcome to — a bug you hit, a security
concern, a fork you are running, a question about the licences, or an interest in contributing when this reopens. The
address is read by people, and a report that never becomes a pull request is still worth sending.

The licence is a separate question, and it is open. Navigator is free software under the GNU Affero General Public
License, version 3 — `AGPL-3.0-only` — over the whole tree, `templates/` included. You may run, fork, modify, and
redistribute it, with no permission to ask for. Section 13 is the obligation to know before you deploy: modify it, let
users reach it over a network, and you owe those users your modified source. See [`LICENSE`](LICENSE) for the grant,
[`NOTICE`](NOTICE) for what the Foundation says about it, and [`docs/licensing.md`](docs/licensing.md).

## How contributions are licensed

The terms are stated here so they are knowable in advance, and so a fork's own authors know where they stand.

Contributions are **inbound = outbound**: anything submitted for inclusion is licensed `AGPL-3.0-only` on the same terms
the project ships under, `templates/` included — one grant over the tree means there is no second answer depending on
which directory you touched. If a change adds a blank government form, note that the Foundation licenses nothing in the
agency's own PDF — only the catalog card, field map, and workflow beside it.

You keep the copyright in what you write. There is no contributor agreement to sign, no copyright assignment, and no bot
standing between an author and a merge.

Work by Shook Law PLLC's personnel and contractors assigns to that firm under the employment or contractor agreement
each of them already holds. That is an arrangement between the firm and its own people; it changes nothing about the
terms above.

## What a contribution to a legal-practice repository is not

Two boundaries hold regardless of the licence, and they are why the review bar is what it is. The Foundation produces
the software, but Neon Law runs a live practice on it, and both facts land on every change.

**Prototype freely. Publish only source.** Shipped material contains only firm-owned or synthetic identities; non-firm
email addresses use reserved example domains and phone numbers do not ship. Never put client or matter data, party
names, legal files, real contact details, or production identifiers in Git, pull requests, issues, or other external
planning surfaces. Client data and legal files belong in Navigator-managed systems and approved file stores. See
[`docs/public-contributor-safety.md`](docs/public-contributor-safety.md). The workspace test suite enforces part of this
boundary on every pull request; the rule applies even where a scanner does not.

**Legal template bodies get attorney review.** A change to anything under `templates/` alters a document a real client
may sign, so a licensed attorney reviews it before it merges regardless of how mechanical the diff looks.

Neither the licence nor a merged pull request creates an attorney-client relationship with Shook Law PLLC, and nothing
in this repository is legal advice.

## Working in the tree

For anyone reading the code or running a fork: follow the [workspace layout](docs/workspace-layout.md). Rust owns the
domain and machine-bound flows, and the browser surface through Dioxus. Generated PDFs use Typst and transactional email
uses string templates. Every change is test-driven — the covering test lands with the minimal implementation it proves —
and `cargo fmt`, `cargo clippy` with warnings denied, and `cargo nextest run --workspace` all have to pass before
review.
