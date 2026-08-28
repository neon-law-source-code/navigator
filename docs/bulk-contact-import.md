---
publish: true
---

# Bulk contact import

One engine, three surfaces, for turning a list of organizations and the people who work at them into Neon Law Navigator
`entities`, `persons`, and the links between them.

The motivating case: a list of legal-aid organizations and their executive directors / CIOs, loaded so each org is a
client `entity` and each contact a `person`, ready for an on-prem-install engagement to be opened against later.

## Why a shared engine

Parse, validate, and apply live in the [`import`](../import) crate — the same way validation rules live in
[`rules`](../rules) and are shared by `cli validate`, `navigator-lsp`, and `web`. Three thin surfaces call the one
engine:

- **`aida_bulk_import`** — the AIDA MCP tool; hands the engine a whole document. Lawyer/admin only. **`web` upload
  route** — *(fast-follow)* the same engine behind a browser upload page.

No surface re-implements the logic. Adding the web page later is wiring, not new behavior.

## The contract (version 1)

A submission is a JSON document (YAML is also accepted). Stable `key`s wire people to their organization and exist only
in the file — they are never persisted.

```json
{
  "version": 1,
  "source": "partner-outreach-2026-06",
  "organizations": [
    {
      "key": "ejp",
      "name": "Example Justice Project",
      "entity_type": "501(c)(3) Non-Profit",
      "jurisdiction": "WA",
      "phone": "206-555-0142",
      "url": "https://justice.example"
    }
  ],
  "people": [
    {
      "key": "ada-counsel",
      "name": "Ada Counsel",
      "email": "acounsel@justice.example",
      "title": "Executive Director",
      "phone": "206-555-0142",
      "organization": "ejp"
    }
  ]
}
```

Field notes:

- **`organizations[].entity_type`** resolves to an existing `entity_types` row by name. `501(c)(3) Non-Profit` is one
  global type, reusable for any state — `entity_types` are keyed by name alone, not per jurisdiction.
- **`organizations[].jurisdiction`** is the two-letter code (`WA`, `MN`, `IL`, `NY`), resolved to a `jurisdictions`
  row. The states themselves are all seeded.
- **`organizations[].url`** is canonicalized before storage: `http` upgraded to `https`, host lowercased, query /
  fragment / trailing slash dropped. `http://Justice.example/?ref=x` is stored as `https://justice.example`.
- **`people[].email`** is the unique upsert key. **`people[].organization`** must be a `key` from this same payload's
  `organizations`. **`people[].entity_role`** is the `entity_role` relation's role; it defaults to `client_contact`.

The `projects` block — opening the engagement Project with its retainer and closing-letter flow — is deliberately not in
version 1. Contacts land first; a Project is opened per real install later. The envelope is versioned so that block can
be added without breaking callers.

## Idempotency

Every write is find-or-create, so an import is always safe to re-run. Every table the import touches lives in SurrealDB,
and the link's dedupe is enforced by an index rather than by a hand-rolled read:

| Row | Dedupe key |
| --- | --- |
| Organization → `entity` | `(name, entity_type_id, jurisdiction_id)` |
| Person → `person` | `email` (the unique column) |
| Link → the `entity_role` relation | `(person, entity, role)`, held by the UNIQUE `entity_role_tie` index |

The JSON is authoritative: a re-run overwrites a person's `name`/`title`/`phone` and an org's `phone`/`url` from the
file. Two things are never touched — a person's `role` (a promotion above Client is sticky) and any field the payload
leaves absent (an omitted `phone` never erases a stored one).

## Validation

`import::validate` is pure and database-free, so it runs in a CLI dry-run, an MCP call, or (later) an editor/LSP. It
returns diagnostics, not a hard stop. Any **error** blocks the whole apply — nothing is written; **warnings** (e.g. a
URL rewritten to canonical form) are informational.

Errors: an unsupported `version`; an empty or duplicate `key`; an empty `name`/`entity_type`; a `jurisdiction` that
isn't a two-letter code; a malformed or in-file-duplicate `email`; a `person.organization` that names no organization in
the payload; a non-canonicalizable `url`.

Existence of a referenced `entity_type` or `jurisdiction` is a database fact, so it is not a structural error — it
surfaces at apply time as a per-row **failure** (that one row only; the rest of the batch still applies).

Every diagnostic and per-row failure reason is folded into the human-readable result text via
`ImportReport::problem_lines`, so a text-only client (Gemini Enterprise over A2A) sees *why* an import wrote nothing
instead of a bland tally. The interaction model — confirmations and how errors reach the user — is in
[`aida-a2a-interaction.md`](aida-a2a-interaction.md).

## Telemetry

Each applied row and the final tally are emitted as `tracing`/OTel events under the `import` target (`created` /
`updated` / `unchanged` / `failed`, with the payload `source`). Where an OTLP exporter is configured
(`OTEL_EXPORTER_OTLP_ENDPOINT`), these flow into the same pipeline as the rest of the app's telemetry — the import's
history is queryable there rather than in a provenance column.

## Generating a payload with an LLM

The reusable instructions for turning a raw contact list into a valid payload live in the bulk-contact upload workflow.
