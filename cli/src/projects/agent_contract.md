# Working in a Navigator Project repository

This repository holds source-only material for one Project. Project identity and matter coordinates belong in
`navigator.yaml` and Navigator's live Project row, not in this contract.

Run `navigator project gate` before opening a pull request; it is the required gate for every proposed repository
change. CI runs `navigator project gate --ci` in the `verify` job (after building, so the origin pass can read a built
`dist/`) and, separately, `navigator project gate --check --ci` in the `documents` job. `--check` (LAW-62) runs only the
live document check — comparing committed pointers with what the deployment holds — and skips every offline pass
`verify` already ran, so `documents` never builds. Locally, `navigator project gate --check` rewrites a drifted pointer,
writes a missing pointer, and writes a missing `documents/.gitignore`. It never writes to the live site. Under `--ci`
any of those fixes fails the job.

## Folders

* `documents/` holds project documents. Only their YAML metadata files (`.yaml`) are stored in Git.
* `portal/` holds the portal, a static React application.
* `seeds/` holds seed data that `navigator site import` creates or updates.
* `templates/` holds notation templates.

## Tools

Reach for the connected MCP server for each job, and the Navigator CLI for anything that touches the matter record.
Never substitute browser or native-app automation for a server that is available.

* **Navigator CLI** — the system of record; run `navigator project gate` before every commit.
* Documents, authorities, and answers go through it, never into Git.
* **Notion** — writing: handbooks, guides, and intake notes.
* **Gmail** — client instructions and correspondence. Read and search; send only when asked.
* **Google Calendar** — meetings and deadlines.
* **Google Drive** — meeting notes, transcripts, and filed documents.
* **Slack** — this matter's channel.
* **Linear** — CLI feedback, named by mechanism and never by matter.
* **CourtListener** — dockets, filings, parties, and judges.
* **Midpage** — statutes and rules.
* **Descrybe** — case law.
* **Exa** — the open web, never primary law.

The last four leave the firm. Keep client identities and matter facts out of those queries.

## Feedback

When Navigator's CLI is missing or wrong, open a Linear issue on the Lawyers team rather than documenting a CLI
workaround here.
