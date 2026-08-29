---
kind: workshop
title: Rust in Peace
description: Rust in Peace tours the Rust monorepo behind a litigation-first law practice.
---

# Rust in Peace

A talk by [Nick](mailto:nick@neonlaw.com) for [Rust NYC](https://www.meetup.com/rust-nyc/events/316056830/)

Rust in Peace is a eulogy to my programming career. Over the past year, teaching coding at Apple, I realized nearly all
my students were vibe coding and had stopped needing me. That was the signal to go back to practicing law.

Our firm runs its entire practice on the [Neon Law Navigator](/navigator), Rust-built software that flexes its great
ecosystem. We built our practice to enable all lawyers to vibe-code custom client experiences and tell our clients'
unique stories. In this presentation, you'll see how this all fits together beginning with how we architected the data,
chose Rust-first vendors, and leverage agentic tooling for coding and lawyering.

Navigator is only possible because the ecosystem is this good, because the LLMs are this good, and because communities
like RustNYC are this good. Please come to my retirement party and let us get crabby together.

## Intro

### May my soul rust in peace

![Ferris the Rust crab in goggles holds a glowing vial, Megadeth Rust in Peace parody](img/rust-in-peace/cover.png)

---

Rust in Peace is a Eulogy to my programming career, as I know I'll never see myself as a full-time developer again.

### Las Vegas Ruby Group

![The Las Vegas Ruby Group's red ruby profile mark](img/lvrug/lvrug.png)

---

Began programming here. Very grateful for Paul, David, Ryan, Alex, Jeremy, Russ, Jason, Rachel, Brian, Dylan and so many
more.

### 5 Years at the Fruit

![A hand holds a kiwi in front of a colorful rainbow sculpture](img/rust-in-peace/kiwi-rainbow.jpg)

---

Twenty days in New York. Thank you to Rob, my friend of twenty years.

### 4 years finance data & platform engineering

![Ten colleagues standing together in an Apple office](img/rust-in-peace/apple-team.jpg)

---

Over 30 petabytes of data, high stress but high performing team.

### 1 year teaching programming

![Colleagues share dinner together at a restaurant](img/rust-in-peace/apple-teaching.jpg)

---

The man in the front is Owen, my beloved boss. Apple is an incredible place.

### What I realized is

![Groovy card: asterisk everyone loves vibing; nearly](img/rust-in-peace/everyone-loves-vibing.png)

---

Nearly everyone loves vibing.

### Decided to be a lawyer in New York

![New York City seen beyond a hillside cemetery](img/rust-in-peace/new-york-lawyer-decision.jpg)

---

I decided to be a lawyer in New York.

### What our firm does

{{firm-product-cards}}

---

Our firm offers Fractional CTO, Litigation, Fractional GC, and one-time services.

### Neon Law Navigator

{{navigator-product}}

---

Neon Law Navigator is source-available software designed to enable all lawyers to be vibe-coding storytellers.

### Agenda

- Why go all-in on Rust
- {Library,Data,DevX,Cloud} tour
- Navigator: Everyone vibes
- Notations: Markdown => PDF with workflows
- Thanks

---

We begin with our experience, we end rusting in peace.

## Why go all-in on rust

### Compiled, correct, and fast

![Ferris the Rust crab racing a hare at tremendous speed](img/rust-in-peace/ferris-races-the-hare.png)

---

If we're vibing, why not just vibe in Rust?

### Amazing Community

What other compiled language has meetups this big? And a discord community of thousands.

![A large Rust community meetup filling a presentation room](img/rust-in-peace/rust-community-meetup.png)

---

Also perhaps other communities like JavaScript and Python are too big and subsequently fragmented.

### Mac, Windows, Linux

Rust works on all three.

![Ferris works across Apple, Windows, and Linux](img/rust-in-peace/ferris-apple-windows-linux.png)

---

You can download our `navigator` CLI on each cloud.

### Non-profit governance

The Rust Foundation > Private Company

![Ferris shares governance with an equal business community](img/rust-in-peace/ferris-shared-governance.png)

---

No need to worry about enterprise fees

### snake_case > camelCase

`rob_balicki` or `robBalicki`

---

I'll die on this hill.

## The tour: {Library,Data,DevX,Cloud}

### Libraries: Some crates

Read our [Cargo.toml](https://github.com/neon-law-source-code/navigator/blob/main/Cargo.toml).

- **HTTP and views** — `axum`, `dioxus`, `tower` / `tower-http`
- **Store & Durable execution** — `surrealdb`, `restate-sdk`
- **Telemetry & Backups** — `opentelemetry`, `arrow`, `parquet`
- **Notations** — `typst`
- **Cloud** — `google-cloud-storage`, `reqwest`

---

It's easier thinking in only one language.

### Libraries: Developer parity

Local `KIND` deployment

- **Rauthy** - OIDC
- **Regorus** - OPA
- **Restate** - Durable Workflows
- **Surreal** - Store

---

Choosing Rust vendors helps us stay pure rust.

### Data: People

A `person` can be one of:

- **Client** - someone with a retainer
- **Clerk** - a supervised worker
- **Lawyer** - member of the bar
- **Admin** - lawyers with admin access to the system
- **Owner** - owner of the system

---

Important to have a single owner or DRI.

### Data: Project

A project or "matter" is a loose and flexible arrangement of legal services.

Each has at least one client and lawyer DRI who can make decisions.

---

Per-project disclosure is important.

### Data: Documents

The abstract deliverable of lawyers.

Contracts, pleadings, forms.

---

The more signed PDFs we acquire the better our business.

### DevX: Monorepo

github.com/neon-law-source-code/navigator

---

A well-tested mono repo keeps things in sync.

### DevX: Glossary & Ontology

www.neonlaw.com/docs/glossary

---

Keeping terms aligned ensures that we're reviewing work together.

### DevX: Tests

Unit & Integration. High code coverage.

![Eight continuous integration checks passed](img/rust-in-peace/tests-checks-passed.png)

---

If we're vibing, vibe high and as much as possible.

### DevX: Multiple explanations

Compare a long string that can't fit in memory? 6 unique spots.

Here, 6 unique explanations. Tests, code, prs, commits, diagrams, workshops, etc.

![Three Ferris crabs point at one another](img/rust-in-peace/ferris-three-way-pointing.png)

---

If an LLM describes the same concept in multiple different ways, chances are we have mutual understanding.

### DevX: Agent skills

Optimize for both Claude Code & Codex.

Adversarial review between the two is amazing.

---

A "tell me what this is prompt" is often illuminating.

### DevX: Works on my machine

Linux, Mac. Local with and w/o Kind. GKE.

Orbstack for local containers

---

Flex Rust's skills to work everywhere.

### DevX: Build for all use cases

Web. CLI. MCP.

![Ferris uses a website, CLI, and MCP tools](img/rust-in-peace/ferris-web-cli-mcp.png)

---

Everyone uses software in different ways.

### DevX: One flow, worktree per PR

One `main` branch. Squash & Merge. Worktree per PR.

Tag only from `main`.

---

Environment creation should be idempotent. The test should always run all the time.

### Cloud: GCP

Omakase. Nicer customer support.

---

The `gcloud` CLI and IAM much easier to reason about.

### Cloud: GKE Autopilot

Kubernetes is awesome, even if maintaining isn't.

![Ferris relaxes while a Kubernetes cloudship runs on autopilot](img/rust-in-peace/ferris-gke-autopilot.png)

---

Benefits of using the same manifests and tech like sidecars don't need to learn something new.

### Cloud: SaaS

SaaS will never die, the same way restaurants wont.

Sendgrid, Docusign, Xero, Slack, Notion, GitHub.

---

We create documents, not rebuild the above.

### Cloud: Infra Surreal DB & Restate

Each proudly Rust and based abroad.

![Ferris presents the SurrealDB and Restate logos](img/rust-in-peace/ferris-surrealdb-restate.png)

---

Each uses the Business Source license, an excellent choice for tech companies.

[Sources]

- <https://surrealdb.com/brand>
- <https://restate.dev/logo.svg>

### Cloud: Iceberg backups

Save everything application {logs,telemetry,data} in Iceberg.

Different GCP account **and** different vendor.

---

Never lose data.

## Navigator: Everyone vibes

### High-Throughput Low-Cost

Our north star is Access to Justice

![Ferris helps every person be seen and heard](img/rust-in-peace/ferris-access-to-justice.png)

---

And all of our actions should be consistent with working towards access to justice sustainably.

### Lifecycle of a project

![Three recycling arrows labeled Intake, Feedback, and Documents](img/rust-in-peace/project-lifecycle.png)

---

A project could be a quick LLC incorporation. It could be managing 10 cases on behalf of one person. Projects are
flexible on purpose.

### Intake, our customer conversion

Collect info. Web forms, chat prompts, API calls (e.g. ID.me, Docusign)

---

Decouple questions from projects to ensure we're not repeating and asking the same things.

### Continous feedback

The best ideas never happen with one person. We work with our teammates and clients.

Custom web apps, client MCPs, phone & video.

---

The more our clients are engaged with the process, the happier they are with our services.

### Vibing for clients

<https://staging.neonlaw.com>

![Groovy card: asterisk everyone loves vibing; nearly](img/rust-in-peace/everyone-loves-vibing.png)

---

Everyone knows is vibing is cool, let's do more of it.

### Create documents

![Ferris signs a document with a quill and the Rust logo](img/rust-in-peace/ferris-signs-rust-document.png)

---

We'll cover in our `Notations` section. High-level PDFs we sign.

Nearly everything else is ephemeral toward that pursuit.

Retention time is for everything that's not our final output. Very rarely is our final output not some sort of PDF.

## Notations: Markdown => PDF with workflows

### Frontmatter

```yaml
title: Retainer Agreement
code: onboarding__retainer
respondent_type: person_and_entity
jurisdiction: NV
confidential: true

questionnaire:
  BEGIN: { _: person__client }
  person__client: { _: project__engagement }
  project__engagement: { _: END }
  END: {}

workflow:
  BEGIN: { intake_submitted: intake_persisted__client }
  intake_persisted__client: { retainer_rendered: lawyer_review }
  lawyer_review:
    approved: generate_pdf__retainer_pdf
    changes_requested: reask__client
    rejected: END
  generate_pdf__retainer_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
```

---

Imbue markdown with a ton of rules. Incremental progress.

### Markdown

```md
{{custom_datetime__engagement_start_date}}

{{person__client.name}}

Re: Retainer Agreement — {{project__engagement.name}}

Dear {{person__client.name}}:

This Retainer Agreement (the "Agreement") is entered into between the Firm and
{{person__client.name}} (the "Client"), reachable at
{{person__client.email}}, for legal services on the matter referred to as
{{project__engagement.name}}.

...
```

---

Imbue markdown with a ton of rules. Incremental progress.

### A Clippy for law

```bash
navigator validate --help
Validate Markdown and YAML files in `<dir>` (default `.`).

Usage: navigator validate [OPTIONS] [DIR]

Arguments:
  [DIR]  Directory to walk [default: .]

Options:
      --fix   Apply every safe-by-construction rule autofix (whitespace, ATX heading spacing, blockquote.
  -h, --help  Print help
```

---

Imbue markdown with a ton of rules. Incremental progress.

### Render the retainer

```bash
navigator template render \
  templates/neon_law/shared/retainer.md \
  --out /tmp/retainer.pdf
```

![First page of the rendered Retainer Agreement PDF](img/rust-in-peace/retainer-agreement-preview.png)

---

Markdown in, PDF out.

## Thanks

### Rust in peace

I still vibe in the background, but the foreground is being your lawyer.

![Ferris rests at Green-Wood Cemetery surrounded by friends](img/rust-in-peace/ferris-green-wood-farewell.png)

---

Imbue markdown with a ton of rules.

Rust in peace, Ferris

Surrounded by friends at Green-Wood Cemetery.
