# Working in a Navigator Project repository

This repository holds source-only material for one Project. Project identity and matter coordinates belong in
`navigator.yaml` and Navigator's live Project row, not in this contract.

Run `navigator project gate` before opening a pull request; it is the required gate for every proposed repository
change.

Legal files, uploads, answers, generated documents, secrets, dependencies, and build output never belong in Git.

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

## One contract, one catalog

This file is the whole agent contract here, and `.agents/skills/` is the whole skill catalog.

A `CLAUDE.md`, `.claude/`, or `.codex/` beside them is a retired mirror and fails `navigator project gate`.

Whichever harness you are, read this file: there is no second copy under another name to keep in sync.

When Navigator's CLI is missing or wrong, open a Linear issue on the Lawyers team rather than documenting a CLI
workaround here.
