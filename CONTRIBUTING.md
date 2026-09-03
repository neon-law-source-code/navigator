# Contributing

**Neon Law Navigator is source-available, and it is currently closed to outside contributions.**

Shook Law PLLC, trading as Neon Law, holds the copyright and operates this software. Issues and pull requests from
outside the firm are not being accepted right now. This is a capacity decision rather than a licensing one: the software
runs a live legal practice, every change to it needs review by someone who can weigh the practice consequences, and
there is not review capacity to offer an outside contributor today.

**Write to [contact@neonlaw.org](mailto:contact@neonlaw.org).** Anyone is welcome to — a bug you hit, a security
concern, a fork you are running, a question about the licences, or an interest in contributing when this reopens. The
address is read by people, and a report that never becomes a pull request is still worth sending.

The licence is a separate question. Navigator is source-available under the Business Source License 1.1 — `BUSL-1.1` —
over the whole tree, `templates/` included. You may read, build, fork, modify, redistribute, and make any non-production
use of it with no permission to ask for. **Production use is the obligation to know before you deploy:** the Additional
Use Grant lets you run Navigator anywhere — the cloud included — to evaluate, develop against, test, or demonstrate it,
for so long as it performs no work anybody relies on. Running it where somebody relies on what it does, and marketing to
customers a product or service that relies on it, needs a commercial licence from the firm. The test is reliance rather
than where the software runs. Each version converts to `Apache-2.0` four years after it is published, and the
restriction on production use simply ends — Apache-2.0 asks nothing of a modifier going forward, unlike the
`AGPL-3.0-only` Change License this project carried earlier. See [`LICENSE`](LICENSE) for the grant, [`NOTICE`](NOTICE)
for what the firm says about it, and [`docs/licensing.md`](docs/licensing.md).

## The contributor licence agreement

**A contribution assigns to Shook Law PLLC — all right, title, and interest in it, including every copyright in it,
worldwide and for the full term.** That assignment is the contributor licence agreement, and it sits here so the terms
are knowable before anyone writes a line rather than at a merge.

Inside the firm the instrument is the employment or contractor agreement each author signed before their first commit,
so the work is the firm's on creation. An outside contributor signs a contributor licence agreement on the same footing
before a contribution merges, and that is worth saying in advance.

Assignment is the mechanism; the grant to you is the result. Everything an author writes reaches you under `BUSL-1.1` on
the same terms as the rest of the tree, `templates/` included — one grant over the tree means there is no second answer
depending on which directory you touched. What assignment buys is a single holder able to grant the whole work, which is
what lets one licence cover the tree and one party set its parameters.

If a change adds a blank government form, note that nobody here licenses anything in the agency's own PDF — only the
catalog card, field map, and workflow beside it.

## What a contribution to a legal-practice repository is not

Two boundaries hold regardless of the licence, and they are why the review bar is what it is. This tree is published in
full, and Neon Law runs a live practice on it; both facts land on every change.

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
