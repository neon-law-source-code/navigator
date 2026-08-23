---
publish: true
---

# Neon Law Navigator documentation index

Top-level `docs/*.md` with `publish: true` frontmatter appear at `/docs/<slug>`; other docs remain repo-local unless
linked. This task-oriented map is the documentation front door.

Read [`glossary`](glossary.md) before using domain words, [`access-model`](access-model.md) before making authorization
claims, and [`agent-decision-councils`](agent-decision-councils.md) before convening a council.

## How this index works

- Start with the task, confirm its nouns in [Glossary quick links](#glossary-quick-links), and follow the most specific
  doc.
- Link to the page or heading that answers the reader's question.

## Glossary quick links

The full alphabetical reference is [`glossary`](glossary.md); notation vocabulary is in [`notation`](notation.md).

- [AIDA](glossary.md#aida) — domain agent persona and protocol bridge. See
  [`aida-a2a-interaction`](aida-a2a-interaction.md), [`gemini-enterprise-mcp`](gemini-enterprise-mcp.md), and
  [`claude-mcp-client`](claude-mcp-client.md).
- [Asset](glossary.md#asset) — a static byte artifact behind `cloud::StorageService` (merges the former Blob + Document
  split). See [`cloud-operations`](cloud-operations.md) and [`project-repositories`](project-repositories.md).
- [Council](glossary.md#council) / [Counsel](glossary.md#counsel) — decision councils and attorney spelling. See
  [`agent-decision-councils`](agent-decision-councils.md).
- [`ctx.run`](glossary.md#ctxrun) — Restate journaled side-effect primitive. See
  [`durable-workflows`](durable-workflows.md) and [`agent-workflows`](agent-workflows.md).
- [Document](glossary.md#document) — a project-scoped [Asset](glossary.md#asset) with filename/kind metadata. See
  [`gov-forms`](gov-forms.md) and [`docusign-esignature`](docusign-esignature.md).
- [Engagement / Retainer](glossary.md#engagement--retainer) — client-English name for a running Notation. See
  [`retainer_intake`](retainer_intake.md) and [`notation-authoring`](notation-authoring.md).
- [Participation](glossary.md#participation) — per-project scope row, not system role. See
  [`access-model`](access-model.md) and [`oidc`](oidc.md).
- [Person](glossary.md#person) / [Entity](glossary.md#entity) — human and legal-organization nouns. See
  [`bulk-contact-import`](bulk-contact-import.md) and [`access-model`](access-model.md).
- [Project](glossary.md#project) — matter container. See [`project-repositories`](project-repositories.md) and
  [`nautilus-workflows`](nautilus-workflows.md).
- [Workflow Runtime](glossary.md#workflow-runtime) — durable runtime model. See
  [`durable-workflows`](durable-workflows.md) and [`cronjobs`](cronjobs.md).

## Agent operating model

- [`agent-workflows`](agent-workflows.md) — the five codebase actions: create an issue, triage an issue, create a PR,
  address a PR comment, or address a failed GitHub Action. Preparation, GitOps, Markdown validation, Restate, and
  workflow authoring are supporting checks inside those actions.
- [`devx-api`](devx-api.md) — GitHub webhook ingress and durable engineering notices.
- [`agent-decision-councils`](agent-decision-councils.md) — Engineering Council, Legal Council, Client Council.
  [`cloud-operations`](cloud-operations.md) — local dev, GCP setup, deploy, prod DB, spend, observability.
  [`rust-programming`](rust-programming.md) — Rust language conventions, async, Axum, service lifecycle.

## Vocabulary and access

- [`glossary`](glossary.md) — workspace vocabulary. [`notation`](notation.md) — notation-system vocabulary.
  [`access-model`](access-model.md) — role + participation authorization model.
  [`command-boundary`](command-boundary.md) — the one REST/OpenAPI command every user- and tool-initiated write travels,
  and the system carve-outs that do not. [`rego-policy`](rego-policy.md) — embedded Rego authorization: source, Rust
  runtime, and authoring. [`oidc`](oidc.md) — OpenID Connect login and role loading.

## Workspace and development

- [`workspace-layout`](workspace-layout.md) — Cargo map. [`AGENTS`](../AGENTS.md#local-kind-development) — local
  development. [`test-database`](test-database.md) — test store model. [`env-driven-devx`](env-driven-devx.md) —
  env-var-driven dev and deploy surfaces. [`assets`](assets.md) — public image pipeline (build/upload/pull); pull images
  down for local dev. [`deployment-secrets`](deployment-secrets.md) — the in-repo `deployments/` tree: plaintext
  coordinates, SOPS-encrypted key material, and how a rotation actually revokes something.
  [`editing-workflows`](editing-workflows.md) — editing notation templates.
  [`notation-authoring`](notation-authoring.md) — authoring notation templates. [`frontmatter`](frontmatter.md) — the
  attorney-facing guide to every frontmatter key, per document kind (notation template, blog post, workshop, GitHub
  notation). [`lsp/README`](lsp/README.md) — editor integrations for notation diagnostics.
  [`templates/README`](../templates/README.md) — how the notation tree is shelved, including the `github/` engineering
  intake notations that gather what an issue or pull request needs. [`pdf/README`](../pdf/README.md) — rendering a
  template to a PDF with `navigator template render`, the output formats, the letterhead, and the font stack.
  [`design`](design.md) — the Dioxus Components theme: the leaf contract, brand tokens, the two render modes,
  accessibility rules, and the gates.
- [`design-mockups`](design-mockups.md) — the intake for contributors who do not write Rust: prototype outside the
  repository, file a design-mockup issue, and an engineer translates it to Dioxus.
- [`vibe-coding`](vibe-coding.md) — the React lane for a Project's client portal, where the prototype ships as itself:
  build it, write the Linear issue, ground it, land it, keep it green.
- [`public-contributor-safety`](public-contributor-safety.md) — the source-only boundary that makes fast public
  experimentation safe: use synthetic or firm-owned content and keep client data, legal files, and production
  identifiers out of Git and planning surfaces.

## Shipping and operations

- [`environments`](environments.md) — the map of five GCP projects: one image registry hub and four runtime projects
  containing one staging and three production deployments, plus the three per-brand images. [`gitops`](gitops.md) —
  branch, PR, release, and deploy. [`gke-prod`](gke-prod.md) — GKE production architecture.
  [`oss-install`](oss-install.md) — installing Neon Law Navigator on your own cloud. [`dns`](dns.md) — DNS for a public
  deploy: reachability, apex redirect, and Google Workspace and SendGrid mail. [`multi-cloud`](multi-cloud.md) — AWS,
  Azure, and self-hosted sketches. [`observability`](observability.md) — logs, traces, metrics, and the no-content rule.
  [`durable-workflows`](durable-workflows.md) — Restate durable execution and operations. [`cronjobs`](cronjobs.md) —
  scheduled jobs. [`deploy/gke-ship-example`](deploy/gke-ship-example.md) — deploy walkthrough example.
  [`licensing`](licensing.md) — who owns the Software, and why the repository is private.

## Legal workflows and documents

- [`retainer_intake`](retainer_intake.md) — retainer intake state machine.
  [`naturalization-intake-demo`](naturalization-intake-demo.md) — walk the N-400 intake locally with the CLI.
  [`live-inquiry-coverage`](live-inquiry-coverage.md) — transcript coverage and lawyer confirmation.
  [`northstar-estate-flow`](northstar-estate-flow.md) — estate-planning flow. [`nautilus-design`](nautilus-design.md) —
  Nautilus design. [`nautilus-workflows`](nautilus-workflows.md) — Nautilus workflow details.
  [`gov-forms`](gov-forms.md) — government form provenance. [`docusign-esignature`](docusign-esignature.md) — DocuSign
  e-signature. [`solana-attestation`](solana-attestation.md) — on-chain attestation. [`erd`](erd.md) and
  [`erd.svg`](https://www.neonlaw.com/docs/erd.svg) — database relationship diagram.

## Data, billing, and integrations

- [`aida-a2a-interaction`](aida-a2a-interaction.md) — AIDA, A2A, and MCP interaction.
- [`gemini-enterprise-mcp`](gemini-enterprise-mcp.md) — Gemini Enterprise MCP integration.
- [`claude-mcp-client`](claude-mcp-client.md) — Claude as an AIDA client over `navigator site mcp`.
- [`bulk-contact-import`](bulk-contact-import.md) — bulk contact import payloads.
- [`email-events-pipeline`](email-events-pipeline.md) — inbound/outbound email events.
- [`project-repositories`](project-repositories.md) — one repository per Project code: its layout, its scaffold, its CI
  gate, and where its portal mounts.
- [`iceberg-archive`](iceberg-archive.md) — archive export.
  [`surreal-archives`](surreal-archives.md) — operational SurrealDB recovery archive.
- [`third-party-integrations`](third-party-integrations.md) — vendor account convention.
- [`provider-environment-parity`](provider-environment-parity.md) — four-deployment signup, credential, and smoke-test
  matrix for Google OAuth, GitHub, DocuSign, SendGrid, Drive, and Restate.
- [`trust-accounting`](trust-accounting.md) — client trust / IOLTA ledger.
- [`xero-billing`](xero-billing.md) — Xero billing.
