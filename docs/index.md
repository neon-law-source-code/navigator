---
publish: true
---

# Documentation

Published guides also appear at `/docs` in one alphabetical catalog. This page is the map: every document under `docs/`,
grouped by topic, with a stable place for anything new to land. `cli/tests/docs_index_completeness.rs` fails the build
when a file exists under `docs/` with no entry here, so this list cannot decay the way the old seven-line stub did.

## Start here

- [`glossary.md`](glossary.md) — canonical vocabulary, one alphabetical list of terms.
- [`agent-workflows.md`](agent-workflows.md) — the five action recipes every agent task reduces to.
- [`gitops.md`](gitops.md) — branch, PR, release, and deploy flow.
- [`workspace-layout.md`](workspace-layout.md) — what each crate in the monorepo owns.

## Engineering practice

- [`rust-programming.md`](rust-programming.md) — the workspace's Rust conventions and error-handling rules.
- [`observability.md`](observability.md) — how Navigator emits and reads telemetry.
- [`test-database.md`](test-database.md) — the one-engine-per-test SurrealDB contract.
- [`erd.md`](erd.md) — the entity-relationship diagram and how to regenerate it.
- [`licensing.md`](licensing.md) — the BUSL-1.1 grant and what it means operationally.

## Local development and environments

- [`environments.md`](environments.md) — the deployment environments and what distinguishes them.
- [`env-driven-devx.md`](env-driven-devx.md) — one config surface serving local dev, CI, and production.
- [`cloud-operations.md`](cloud-operations.md) — day-to-day cloud operations against a live deployment.
- [`devx-api.md`](devx-api.md) — the DevX HTTP API the CLI and tooling call.
- [`multi-cloud.md`](multi-cloud.md) — running Navigator on AWS, Azure, or self-hosted Kubernetes.
- [`gke-prod.md`](gke-prod.md) — the GKE production deployment shape.
- [`dns.md`](dns.md) — pointing an instance at a real domain.
- [`deployment-secrets.md`](deployment-secrets.md) — the `deployments/` tree and secret material.
- [`provider-environment-parity.md`](provider-environment-parity.md) — keeping third-party providers consistent
  across deployments.
- [`third-party-integrations.md`](third-party-integrations.md) — one provider attachment per deployment.
- [`deploy/gke-ship-example.md`](deploy/gke-ship-example.md) — a worked GKE ship, the roll-only model.
- [`oss-install.md`](oss-install.md) — installing Navigator on your own cloud.

## Authorization and policy

- [`access-model.md`](access-model.md) — the role and participation model: who can see what.
- [`rego-policy.md`](rego-policy.md) — the embedded Rego policy that decides authorization.
- [`oidc.md`](oidc.md) — OIDC sign-in and database-role authorization.
- [`command-boundary.md`](command-boundary.md) — the REST/OpenAPI command boundary.
- [`public-contributor-safety.md`](public-contributor-safety.md) — safe public experimentation without touching
  client data.

## Authoring notations and Markdown

- [`notation.md`](notation.md) — the notation vocabulary: templates, questionnaires, workflows.
- [`notation-authoring.md`](notation-authoring.md) — authoring a notation template's body and structure.
- [`frontmatter.md`](frontmatter.md) — the frontmatter cover sheet on every file, by document kind.
- [`validate.md`](validate.md) — the canonical reference for `navigator validate`: its passes, flags, and every rule
  code.
- [`editing-workflows.md`](editing-workflows.md) — editing a legal workflow's state machine.
- [`durable-workflows.md`](durable-workflows.md) — Restate-backed durable execution for legal workflows.
- [`agent-decision-councils.md`](agent-decision-councils.md) — the review-council patterns and when to use them.
- [`gov-forms.md`](gov-forms.md) — government forms: vendor, map, fill, file.

## Project repositories and client portals

- [`project-repositories.md`](project-repositories.md) — the Project workspace and repository contract.
- [`vibe-coding.md`](vibe-coding.md) — building a Project's client portal quickly and safely.
- [`design-mockups.md`](design-mockups.md) — translating an approved design into the portal surface.
- [`design.md`](design.md) — the design system.
- [`assets.md`](assets.md) — public asset references and the approved workflow.
- [`bulk-contact-import.md`](bulk-contact-import.md) — turning a raw contact list into seeded Persons.
- [`retainer_intake.md`](retainer_intake.md) — the retainer intake walkthrough.

## AIDA, A2A, and MCP clients

- [`aida-a2a-interaction.md`](aida-a2a-interaction.md) — AIDA over A2A: confirmations and errors.
- [`claude-mcp-client.md`](claude-mcp-client.md) — Claude as an AIDA client over MCP.
- [`gemini-enterprise-mcp.md`](gemini-enterprise-mcp.md) — the Gemini Enterprise MCP server.

## Billing, trust, and e-signature

- [`trust-accounting.md`](trust-accounting.md) — the client trust (IOLTA) ledger.
- [`xero-billing.md`](xero-billing.md) — Xero billing setup, invoice flow, and production cutover.
- [`docusign-esignature.md`](docusign-esignature.md) — DocuSign e-signature setup and signing flow.

## Observability pipelines and data

- [`cronjobs.md`](cronjobs.md) — scheduled jobs (CronJobs) and what runs on them.
- [`surreal-archives.md`](surreal-archives.md) — SurrealDB operational archives.
- [`iceberg-archive.md`](iceberg-archive.md) — the Iceberg archive design.
- [`email-events-pipeline.md`](email-events-pipeline.md) — the email delivery event stream into BigQuery.

## Worked demos

- [`naturalization-intake-demo.md`](naturalization-intake-demo.md) — walking the naturalization intake locally.
<<<<<<< HEAD
- [`nautilus-design.md`](nautilus-design.md) — Neon Law Nautilus screening-shield design.
- [`nautilus-workflows.md`](nautilus-workflows.md) — Nautilus screening-dispute workflows.
- [`northstar-estate-flow.md`](northstar-estate-flow.md) — the Northstar estate-plan flow.
- [`live-inquiry-coverage.md`](live-inquiry-coverage.md) — proposed live transcript coverage during a matter session.
=======
>>>>>>> 732a12e (Slim the notation catalog to public forms plus sample letters.)
- [`solana-attestation.md`](solana-attestation.md) — on-chain attestation on Solana.

## Editor integration

- [`lsp/README.md`](lsp/README.md) — `navigator-lsp`, the Language Server Protocol backend for Navigator's Markdown and
  Notation diagnostics.
