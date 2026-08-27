---
publish: true
---

# Safe public experimentation

Navigator is public so people can read it, run it, fork it, prototype quickly, and build portal experiences without
turning a code repository into a matter system. Fast iteration is welcome because the boundary is clear.

> **Prototype freely. Publish only source.**

## What may be public

Commit source code, documentation, tests, design experiments, and fixtures that are either firm-owned or synthetic. For
any non-firm email address, use `example.com` or a reserved `.example`, `.invalid`, or `.test` domain. Do not include
phone numbers.

## What may never be public

Never put any of the following in Git, pull requests, commit messages, issues, planning tools, agent transcripts, or
other external planning surfaces:

- client or matter data, including party names, matter codes, addresses, contact details, answers, or work product;
- legal files, document bodies, uploads, generated documents, or screenshots containing their contents;
- production identifiers, credentials, hosts, project names, buckets, service accounts, or operational coordinates; or
- the product roadmap — Linear issue titles, project, initiative, milestone, or cycle names, and `linear.app` URLs,
  whose path carries the issue title as a slug. The roadmap lives in Linear; a published branch, PR, or commit
  references it by bare issue identifier (`ENG-1234`) and nothing more. See [Linking a PR to its Linear
  issue](agent-workflows.md#linking-a-pr-to-its-linear-issue).

Do not work around this rule by redacting a copy, using a realistic placeholder, or moving the material to another
third-party service. Client data and legal files belong in Navigator-managed systems and the firm's approved file
stores, never source control or external planning surfaces.

## A safe fast loop

1. Start from a synthetic fixture or a blank design.
2. Build and iterate locally; use the repository's local-development and test guidance.
3. Keep examples abstract or synthetic when describing a scenario to another contributor.
4. Validate the changed files and run the relevant tests before sharing the source.

For the Project-repository contract, including Navigator-managed reads and writes, see
[`project-repositories.md`](project-repositories.md). For the repository workflow and its no-client-data gate, see
[`agent-workflows.md`](agent-workflows.md#no-client-data-in-the-repo). The gate is proof for its scanned paths, not
permission to place sensitive material somewhere it does not scan.
