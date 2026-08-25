--- name: bulk-contact-upload description: > Turn a raw list of organizations and the people who work at them into a
valid Neon Law Navigator bulk-import payload, then load it through the shared `import` engine: the `cli import-contacts`
command, the `aida_bulk_import` MCP tool, or the web upload route. Use this when someone hands you a contact list
(names, emails, titles, phone numbers, org names) and wants those people and their organizations created as `persons`
and `entities` with the links between them. Trigger on "bulk import these contacts", "load this list of people and
orgs", or "create these as clients", or when AIDA is asked to add several people at once. The contract and rules live in
`docs/bulk-contact-import.md`; this skill is the LLM-facing recipe for producing the JSON and choosing the surface. Skip
for a single person (use `aida_create_person`) or for opening a matter/Project (the per-engagement onboarding flow, not
this importer). ---

# Bulk contact upload

You have a raw contact list. Produce a **version-1 bulk-import payload** and load it. The full contract is in
[`docs/bulk-contact-import.md`](../../../docs/bulk-contact-import.md) — read it if anything here is ambiguous.

## 1. Emit the payload

Output a single JSON object with `organizations` and `people`. Use stable, human-readable `key`s (kebab-case) to wire
each person to their organization; keys live only in the file.

```json
{
  "version": 1,
  "source": "<short provenance slug, e.g. partner-outreach-2026-06>",
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

## 2. Rules for filling it in

- **One organization row per distinct organization.** Several people at the same org share one `organizations` entry
  and reference it by `key`.
- **`email` is required and must be unique** across the payload — it's the dedupe key. Drop or flag any contact with no
  email; they can't be imported.
- **`entity_type`** is almost always `501(c)(3) Non-Profit` for legal-aid and nonprofit orgs. For a company, use the
  matching type name (e.g. `Multi Member LLC`). The type must already exist as reference data — don't invent new ones.
- **`jurisdiction`** is the two-letter state code of the org's home/incorporation state. Infer it from the address or
  area code when not stated (206 → WA, 612 → MN, 312 → IL, 646 → NY), but prefer an explicit signal.
- **`url`** must be a full `https://` URL and should be the organization's canonical website (derive it from the email
  domain when not given: `acounsel@justice.example` → `https://justice.example`). The engine canonicalizes it anyway,
  but emit it clean: no tracking query params, no trailing slash.
- **`title`** and **`phone`** are optional; include them when you have them. A shared org switchboard number can appear
  both as the org `phone` and each person's `phone`.
- **Leave `entity_role` off** unless told otherwise — it defaults to `client_contact`.

## 3. Load it

Pick the surface:

- **AIDA (chat / MCP):** call `aida_bulk_import` with the payload as the arguments. Lawyer/admin only; a client-tier or
  anonymous caller is refused. It returns a per-row created/updated/unchanged/failed report.
- **CLI (operator):** save the JSON to a file and run `cargo run -p cli -- db import-contacts contacts.json` (add
  `--dry-run` to validate without writing). `NAVIGATOR_SURREAL_ENDPOINT` must be set.

## 4. Read the report

Every surface reports each row as `created`, `updated`, `unchanged`, or `failed`. Re-running the same payload is safe —
it reports `unchanged`. A `failed` row carries a reason (commonly an unknown `entity_type` or `jurisdiction` that needs
seeding first); fix and re-run — only the failed rows change.

## What this skill does not do

It loads contacts and their organizations. It does **not** open a matter/Project, send welcome emails, or provision
repositories — those are deliberate, separate steps in the per-engagement onboarding flow. Importing is a pure data
upsert with no outbound side effects.
