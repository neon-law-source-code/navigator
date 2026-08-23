---
kind: workshop
title: Contributing to Neon Law Navigator
description: Five ways to make the Neon Law Navigator corpus better for the next lawyer — most need no code.
---

# Contributing to Neon Law Navigator

## Intro

### Five ways to contribute

Pick the path that fits the time and craft you have:

1. Improve the product repository (authorized lawyer).
2. Add a fillable government PDF.
3. Tell the team what you learned.
4. Join a workshop or presentation.
5. Use Neon Law Navigator and report what breaks.

---

Neon Law Navigator is copyright the Neon Law Foundation, which produces it and publishes it as free software under the
AGPL-3.0; Neon Law operates it. This workshop is about contributing to the **product**: issues, templates, government
PDF forms, tests, and the shared corpus every installation uses. Contributions are inbound = outbound — you keep your
copyright and license your work on the same terms the project ships under. The mission — putting the rights already
written into law within reach of the people they belong to — only moves if that corpus keeps getting better. There are
five ways to contribute; pick the one that fits the time you have.

Outside pull requests are closed for now, so the ways below run through issues, conversation, and use rather than the
merge queue. Anyone may write to <contact@neonlaw.org>.

## Improve the Corpus

### Contribute to the product repository

The whole stack is written in Rust in one public monorepo. Anyone can open issues and pull requests. The most useful
corpus contributions are specific ones:

- Add a missing legal template. Add a government PDF form. Improve a field map. Add a test that locks a workflow to the
  law it implements.

---

Everything you have seen in these workshops — the `web` app, the `navigator` CLI, the durable workflows — lives in one
public monorepo anyone can read, run, and fork. Start by standing it up yourself with the [Operating Neon Law
Navigator](/workshops/deploy-the-navigator) workshop or the local KIND loop, then file what you find. Contributions are
inbound = outbound: what you submit is licensed on the same terms the project ships under, you keep the copyright in
what you write, and there is no agreement to sign and no acceptance bot to clear.

**Scope is not the gate — the local KIND loop is.** A contribution may be as ambitious as the problem demands: a
sweeping refactor, a new service beside the existing ones, even replacing a subsystem. What every change must do is run
inside the local KIND cluster that `cargo run --release -p cli -- dev up` stands up — the loop that already runs
SurrealDB, Rauthy, Garage, Restate, and OpenObserve in one KIND dependency tier. A change that introduces a new
dependency wires it into that loop in the same pull request, with a KIND manifest beside the existing ones, so the whole
stack still comes up locally.

That rule is what keeps production portable and contribution open. In production, persistent state lives on managed
services — a hosted SurrealDB, Cloud Storage for documents — and each one has a cloud-agnostic stand-in inside KIND, so
the topology you test locally is the topology a firm deploys. And because the full stack runs on a laptop, you never
need a cloud account or someone else's cloud bill to verify your work: clone the repository, run `dev up`, prove the
change against the running stack, and open the pull request.

### View content images locally

The repository deliberately leaves published blog, presentation, and workshop image bytes out of Git. Before running the
site locally, hydrate the images referenced by the content tree from the public asset route:

```bash
cargo run -p cli -- ops assets fetch-referenced \
  --base-url https://www.neonlaw.com/assets
cargo run -p neon
```

This is a read-only HTTP download and needs neither Google Application Default Credentials nor access to the production
bucket. The bucket stays private: the running application reads only its marketing assets with its own workload identity
and publishes those bytes at `/assets/*`. Re-run the command when a post gains or changes an image; the files land in
the ignored `server/public/img/` tree and are served locally from `/public`.

---

Keep the two access boundaries distinct. The production bucket is not public and does not need to become public for a
developer to preview the blog: the public asset route reads only the marketing-image lane. `fetch-referenced` uses that
route, so it fetches the same bytes a visitor's browser can reach while keeping documents, exports, and logs out of the
path. The downloaded bytes remain local and ignored; they are a development cache, not repository content.

When creating a new slide image, keep the full-resolution PNG or JPEG at `server/public/img/<deck-slug>/<filename>` for
local preview. Upload that same relative key to staging first and then to production; publishing one bucket never
publishes the other. Production upload remains an authorized operator action.

### Ship through GitOps

Every change travels through a pull request to `main`. The required checks and resolved review threads protect the
branch; auto-merge lands one squash commit, then the release and deployment flow begins from that reviewed tip.

---

GitOps keeps ordinary contributors out of settings drift. Branch protections and merge behavior live in the `navigator`
CLI, so an authorized operator reviews the planned change before applying it to one named repository:

```bash
navigator ops github setup neon-law-source-code/navigator --dry-run
```

The repository name is optional — it falls back to `GITHUB_REPOSITORY` and then this checkout's `origin` — but the
command still reconciles one repository at a time and cannot apply to every repository at once. The boundary is a
`(host, organization)` pair: the host from `NAVIGATOR_GIT_HOST`, defaulting to `github.com`, and two admissible
organizations — the public `neon-law-source-code` that holds Navigator itself, plus the deployment's own
`NAVIGATOR_GITHUB_ORG` when one is configured. A repository outside that pair is refused before a token is read. It
reconciles pull-request-only and squash-only policy, gated on one check named `ci`. Read
[GitOps](https://github.com/neon-law-source-code/navigator/blob/main/docs/gitops.md) before changing the real setting;
contributors normally work through a PR and let the gate do its job.

### Work with the store

SurrealDB holds every table. `dev up` writes its whole connection contract into `.devx/env`, and nothing defaults: a
process that is not configured fails loudly instead of quietly connecting to the wrong store.

- Connection: `NAVIGATOR_SURREAL_ENDPOINT`, `_NAMESPACE`, `_DATABASE`.
- Schema: one idempotent `DEFINE` file, applied whole on every boot.
- Tests: an embedded engine per test — no container, no port, nothing to reclaim.

---

The shape worth internalizing is that the schema is a *statement of the present*, not a history.
`store/src/schema/navigator.surql` is one file of `DEFINE` statements describing the tables that should exist, applied
whole on every boot. You change it by editing the file, not by appending a step to a chain — and because applying a file
converges definitions but cannot perform a data change, you bump `SCHEMA_VERSION` in the same commit so a database
prepared by a different build reports as drifted instead of silently disagreeing. The local engine is memory-backed, so
its rows reset when the pod restarts and the schema re-applies at boot. That is deliberate, not a rough edge.

Tests need no database at all. `store::test_support::mem_surreal()` starts an engine inside the test process with the
schema applied, so there is no container, no port, and nothing to reclaim afterwards. Reach for
`store::surreal::record_id` whenever you turn a `Uuid` into a record id — SurrealDB will accept a string that merely
looks like a UUID, and a link written that way resolves to nothing without ever raising an error.

### Add a fillable government PDF

Government forms are one of the best places to contribute because the path is concrete. A form-backed template lives
under `templates/forms/...`, names the official `origin_url`, asks its intake questions from the shared question bank in
`store/seeds/Question.yaml`, and maps answers onto the PDF's AcroForm fields through either a `.fields` manifest or a
`.fields.toml` map. The workflow still passes through `lawyer_review` before the filled packet can be signed or filed.

---

The Nevada LLC formation packet is the model to study. Its template,
`templates/forms/united_states/nevada/state/nv__llc_formation.md`, asks canonical states like `person__client`,
`entity__company`, `person__registered_agent`, `custom_single_choice__management_structure`, and
`people__managing_members`. Its re-authored field layer,
`templates/forms/united_states/nevada/state/nv__llc_formation.fields`, uses those same state paths as PDF field names.
That is the contract: the common question bank gives the questionnaire a stable vocabulary, the PDF layer uses that
vocabulary, and `forms/tests/question_code_contract.rs` fails if the two drift.

When you add a form, start from the real issuing-authority blank, run `navigator template forms fields <code>` to read
the field names off the exact bytes, map only the fields the questionnaire can answer, and leave payment-card or
lawyer-only acceptance fields unmapped. Then run the form tests before you open the issue or PR:

```bash
cargo test -p forms
cargo run -p cli -- validate templates
```

## Share What You Learn

### Tell us what you learned

The most valuable thing you can send is what you learned using it: a template that worked, a checklist item we are
missing, a bug, a kaizen improvement — never a client's data, just the craft. Email
[support@neonlaw.org](mailto:support@neonlaw.org?subject=Navigator+feedback).

---

You do not need a GitHub account or a single line of code to make Neon Law Navigator better. The [Using Neon Law
Navigator](/workshops/use-the-navigator) workshop closes by asking you to send the markdown of the template you built
and the one kaizen improvement you found — that is this contribution. Every template a lawyer shares raises the floor of
competence for the next lawyer who joins. Send the craft, never a client's file: sharing a template or a checklist
grants the Foundation a license to use it, and anything that lands in the repository is the source intellectual property
of Shook Law PLLC.

### Join a presentation

We give talks on how and why we build this. [Rust in Peace](/presentations/rust-in-peace), our Rust NYC talk, dissects
deterministic legal workflows — and every code slide is an exact copy of the shipped repository.

---

Presentations go deeper into the engineering and the argument behind it. [Rust in Peace](/presentations/rust-in-peace)
walks the path from statute to Cucumber feature to template to notation, one attorney-gated step at a time, with a build
test that fails if any slide drifts from the real source. More talks are on the way. Come to one, push back on the
design, and tell us where it breaks — that pressure is how the architecture earns its keep.

## Put It to Work

### Use Neon Law Navigator

The simplest contribution is to use it. Every matter you run and every instance you stand up surfaces the next
improvement. Start with [Using](/workshops/use-the-navigator); run your own with
[Operating](/workshops/deploy-the-navigator).

---

Using the platform is not a lesser contribution — it is the one that generates all the others. A lawyer who runs a real
matter in [Using the Navigator Workshop](/workshops/use-the-navigator) finds the missing checklist item; an operator who
stands up an instance in [Operating Neon Law Navigator](/workshops/deploy-the-navigator) finds the rough edge in the
install. What you learn flows back through the four contributions above.

## Wrap Up

### Why this matters

A corpus the whole community improves is how routine legal work gets cheap enough to reach the people priced out of it
today.

---

Neon Law Navigator is licensed so that no one has to ask permission to run it, and built in the open so that every fix
and every template compounds for the next clinic and the next small firm. That is the access-to-justice fight, and
contributing — in any of these five ways — is how you join it. Read the [Foundation mission](/foundation/mission) for
why it matters.
