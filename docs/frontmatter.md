---
publish: true
---

# Frontmatter: the cover sheet on every file

This page is for the attorney who is about to write or edit a file in Neon Law Navigator — a notation template, a blog
post, or a workshop — and wants to know what the little block at the top is for. You do not need to be a programmer to
read it. You need to know which label goes on which document, and what each line means.

## You cannot quietly ship a broken document

Start here, because it is the part that protects you. Every file is checked as you type, in your editor, against the
same rules the project enforces everywhere else. If you leave out something a document needs — a title, the attorney
review step, the second half of a pair — the editor underlines it in **red** before the file ever leaves your screen.
You are caught at your desk, not in production and not in front of a client. The rest of this page is just *what* the
checker is looking for.

## What frontmatter is

Most files in Neon Law Navigator are plain text, and many of them begin with a small block fenced top and bottom by a
line of three dashes (`---`). The block holds a few `key: value` lines, like this:

```yaml
title: Retainer Agreement
code: onboarding__retainer
```

That block is the **frontmatter** (the real file has a `---` line above and below it). Think of it as the caption on a
pleading: a short, structured label that tells the system *what kind of document this is* and the handful of facts it
needs to file it correctly. Everything below the block is the document itself — the prose you write and, in the end,
sign.

The format is called YAML, but it is nothing more than `key: value`, one per line. There is no programming. Spell the
key correctly on the left, put a valid value on the right, and keep the indentation the examples show. When something is
wrong, the editor underlines it — the same way a word processor underlines a misspelling.

## The kinds of file, and what each one declares

You say what a file is by **declaring it**: a `kind:` key names the file's kind outright, and that declaration is the
*only* classifier — the system never guesses the kind from a file's structure or its path. Its value is one of a small,
fixed vocabulary. Most values name notation-template kinds — `retainer`, `letter`, `filing`, `will`, `trust`,
`directive`, `agreement`, `onboarding`, `memo` — some name content pages — `post`, `workshop` — and one, `github`, names
the engineering intake notations. A further ten name **matter dashboards**, the page types an attorney composes.
Anything else is a blocking error (`S103`). The vocabulary grows as the firm's practice areas do.

A file that declares no `kind:` is ordinary prose, held only to general writing rules. Because classification is
declaration-only, a file that carries notation *structure* — a `questionnaire:`/`workflow:` block — but forgets its
`kind:` is a blocking error (`S104`): it would otherwise lint silently as prose and skip its whole rule family. Each
kind and the keys it must carry:

- **Notation template** — one of the notation kinds (`retainer`, `letter`, `filing`, `will`, `trust`, `directive`,
  `agreement`, `onboarding`, `memo`). A complete one carries **both** the `questionnaire:` and `workflow:` machines plus
  `title`, `code`, `respondent_type`, `jurisdiction`, and `confidential`, and the missing ones are flagged. Lives under
  `templates/forms/` or `templates/neon_law/`; a `templates/` file with no `kind:` is just prose until it declares one.
- **Blog post** — `kind: post`. Lives under `server/content/blog/`. Needs `title` and `description`, in a file named
  `YYYYMMDD_slug.md`.
- **Workshop page** — `kind: workshop`. Lives under `server/content/workshops/`. Needs `title` and `description`.
- **GitHub notation** — `kind: github`. Lives at `templates/github/create_issue.md` or
  `templates/github/create_pull_request.md`, and nowhere else. Needs `title` and a `questionnaire:`, and its body
  renders the answers into the issue or pull request text. It declares **no** `workflow:`, `code`, `jurisdiction`,
  `respondent_type`, or `confidential` — it is engineering intake, not legal work, so the legal contract has nothing to
  check. `N119` pins the whole shelf; see [the templates README](../templates/README.md).
- **Matter dashboard** — one of the dashboard kinds (`review_queue_workbench`, `verifier_split_view`,
  `matter_status_console`, `docket_deadline_board`, `document_workbench`, `authority_library`, `discovery_cockpit`,
  `hearing_console`, `deliverable_package`, `engagement_billing_records`). Lives in the matter's forge repository, not
  in this repo. Needs `title` and a `lenses:` mapping. It declares **no** `questionnaire:`, `workflow:`, `code`,
  `jurisdiction`, `respondent_type`, or `confidential` — a dashboard composes registered sections, it does not become an
  instrument.
- **Everything else** — ordinary prose (READMEs, docs). No frontmatter is required.

Workshop and presentation bodies use one shared outline contract. A top-level `#` names the material, each `##` starts a
chapter, and each `###` starts one section/slide inside that chapter. Prose between a chapter heading and its first
section is the chapter preamble, rendered on the public overview without becoming a numbered slide. Put the slide face
above a `---` thematic break and its presenter notes below. Every deck begins with an `## Intro` chapter and ends with
an `## Wrap Up` chapter; the public overview and light table group sections by those chapter headings while playback
keeps one stable section order.

## Notation templates — the legal blueprints

A notation template is the document a client eventually signs, plus the questions that fill it in and the path it walks
to get there. Here is the real frontmatter from the shared retainer, `templates/neon_law/shared/retainer.md` (shown
without its surrounding `---` fences):

```yaml
title: Retainer Agreement
respondent_type: person_and_entity
code: onboarding__retainer
jurisdiction: NV
confidential: true
questionnaire:
  BEGIN:               { _: person__client }
  person__client:      { _: project__engagement }
  project__engagement: { _: END }
  END: {}
workflow:
  BEGIN:                       { intake_submitted: intake_persisted__client }
  intake_persisted__client:    { retainer_rendered: lawyer_review }
  lawyer_review:                { approved: generate_pdf__retainer_pdf, rejected: END }
  generate_pdf__retainer_pdf: { pdf_persisted: sent_for_signature__pending }
  sent_for_signature__pending: { signature_received: END, signature_declined: END }
  END: {}
```

Each key, in plain English:

- **`kind`** — what this notation is: `retainer` (the engagement agreement that opens a matter), `letter` (a letter the
  firm sends on the client's behalf), `filing` (a document filed with a government body), `will`, `trust`, `directive`
  (a health-care or durable financial directive), `agreement` (employment, contractor, or LLC operating), `onboarding`
  (a multi-instrument intake bundle such as the estate plan), or `memo` (an analytical work product like a contract
  review). It is **required** on every notation template — the declared kind is the sole classifier, so a template
  without it lints as prose — and an unrecognized value is a blocking error.
- **`title`** — the human name of the document, e.g. `Retainer Agreement`. It cannot be blank.
- **`code`** — the document's permanent file number, in `snake_case` (e.g. `onboarding__retainer`). It must be unique
  across the whole project, and you do not change it once clients have signed under it. The reason is the record: the
  `code` is how a signed document is traced back to the blueprint it came from, so changing it later would cut the audit
  trail your malpractice carrier may one day need to read.
- **`respondent_type`** — who signs: `person`, `entity`, or `person_and_entity`. Nothing else is accepted.
- **`jurisdiction`** — the state whose law governs: `NV`, `CA`, or `US`.
- **`confidential`** — `true` or `false`. There is no default; you state it on purpose, every time, because the system
  will not guess how to treat a client's document for you.
- **`questionnaire`** — the questions the client answers, written as a simple step-by-step ladder from `BEGIN` to `END`.
- **`workflow`** — the path the document walks from intake to signature. It **must** include a `lawyer_review` step.
  That is not a formality: a licensed attorney reviews the draft before anything is sent — the supervision you owe any
  non-lawyer assistant (ABA Model Rule 5.3). The document is never sent, filed, or signed on its own.

### Where a question's wording comes from

Most questionnaire states are **bank-backed** — a state like `person__client` or `entity__company` reuses a question the
firm has already worded once, in the shared question bank. You write the state and nothing else; the bank supplies the
prompt (your editor shows it when you hover the state). Rewording that prompt for one template is the exception, not the
rule: add a `prompts:` entry keyed by the state's role (e.g. `client`) only when this template genuinely needs different
wording. Improving the bank prompt itself is usually the better fix, because every template inherits it at once.

A **one-off** question — something no bank type covers — uses a `custom_*` type and is defined in `custom_questions:`,
keyed by the part of the state after the **first** `__` (so `custom_single_choice__management_structure` is keyed
`management_structure`). That block is the single home for a custom question's wording, and, for `custom_single_choice`
/ `custom_multiple_choice`, its options:

```yaml
questionnaire:
  BEGIN:                                       { _: custom_single_choice__management_structure }
  custom_single_choice__management_structure:  { _: custom_datetime__formation_date }
  custom_datetime__formation_date:             { _: END }
  END: {}
custom_questions:
  management_structure:
    prompt: How will the company be managed?
    choices:
      members: Managed by its members — the owners
      managers: Managed by appointed managers
  formation_date:
    prompt: When was the formation date?
```

N104 enforces the split: every `custom_*` state needs a matching `custom_questions:` entry with a non-empty `prompt`; a
choice type needs `choices` and every other custom type must not carry them. Options live inside `custom_questions`, so
there is no top-level `choices:` key.

### One rule worth saying twice: `questionnaire` and `workflow` travel together

A notation template has **both** `questionnaire:` and `workflow:`, or neither. If you write one and forget the other,
the checker stops you. A blueprint with questions but no path — or a path but no questions — is half a document, and a
half-built document should never reach a client. This is a guardrail, not a nicety.

The body below the frontmatter is the legal prose, in English, carrying `{{placeholder}}` slots that the questionnaire
answers fill in (`{{person__client.name}}`, `{{project__engagement.name}}`, and so on). Authoring that body, and the
full list of structural checks, is covered in <notation-authoring.md>.

### How the finished document looks: `output`

A notation template may carry an optional **`output`** key. It is the one place a template declares its **render
profile** — what comes out and how it is dressed:

- **omit it** (the default) and the document renders as a plain page — our standard serif, one-inch margins, no
  letterhead. The body's `{{placeholders}}` fill from the questionnaire answers.
- **`output: letter`** renders the same body on Neon Law letterhead: our logo, the firm name and contact line, a rule
  across the top. This is the dressing we use for the documents that go out under the firm's name, such as engagement
  letters and demand letters. It is typeset airily, for a document someone reads once before deciding to sign it.
- **`output: agreement`** puts the same letterhead on an executed contract, typeset curtly: narrower margins, closer
  leading, headings at body size, and no table ever split across a page break so a signature block stays whole. Reach
  for it when the document is navigated by section number rather than read straight through.
- **`output: form`** is a different mode entirely: instead of typesetting prose, it prints the questionnaire answers
  onto an official government form (an AcroForm fill). A `form` template carries no legal prose — its body is the field
  map — so it always rides with the two form keys below (`form:` and `origin_url:`), and the checker (N109) requires
  them. Conversely a typeset profile (`letter`, `agreement`, or no `output:` at all) must **not** carry a `form:` key.

`letter`, `agreement`, and `form` are the values the checker accepts today (N109); leaving the key off gives you the
plain page. As we add court-specific layouts (pleading paper), each becomes one more named value here — so `output`
stays the one place a template says what it should look like.

### Government form templates carry two extra keys

A template backed by an official government form (under `templates/forms/`) declares `output: form` and adds `form:`
(the form's identity) and `origin_url:` (the official `.gov` page the blank form came from), as in
`templates/forms/united_states/nevada/state/nv__llc_formation.md`:

```yaml
title: Neon Law Nest — Nevada Entity Formation
respondent_type: person_and_entity
code: nv__llc_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
confidential: false
output: form
form: nv__llc_formation
```

The three travel together: N109 requires `form:` and `origin_url:` whenever `output: form` is declared, and rejects a
`form:` key on any other profile. So `form:` present and `output: form` always imply each other.

## Blog posts

The simplest kind: a `title` and a `description`, and a filename that follows a fixed shape.

A blog post (`server/content/blog/`) takes its publish date from the filename, so the name **must** be
`YYYYMMDD_slug.md` (e.g. `20260625_going_all_in_on_rust.md`). A name whose date does not parse is silently dropped — the
post never shows up and never errors — so the checker flags a bad name for you.

```yaml
title: Going All-In on Rust
description: Why Neon Law chose one language for fast, safe, local-first access-to-justice software.
```

## Every frontmatter key at a glance

The narrative above covers the keys you reach for daily. This table is the complete set the system knows, grouped by
document kind, so nothing is hidden. The `Checked by` column names which rule catches a missing or malformed key; for
what each code actually checks, its severity, and whether it autofixes, see the canonical reference at
[`validate.md`](validate.md).

### Notation template

| Key | Required | Values | Checked by |
| --- | --- | --- | --- |
| `kind` | yes | a notation kind (`retainer` … `memo`, the nine listed above) | S103, S104 |
| `title` | yes | any non-empty text | N101 |
| `code` | yes | unique `snake_case` | N108 |
| `respondent_type` | yes | `person`, `entity`, `person_and_entity` | N102 |
| `jurisdiction` | yes | `NV`, `CA`, `US` | N110 |
| `confidential` | yes | `true` or `false` | N105 |
| `questionnaire` | yes (paired) | a `BEGIN` → `END` ladder | N104 |
| `workflow` | yes (paired) | a `BEGIN` → `END` path that includes `lawyer_review` | N104, N106 |
| `custom_questions` | with any `custom_*` state | wording (and options) for one-off questions | N104 |
| `prompts` | no | override the bank's wording for a bank-backed state | N104 |
| `output` | no | `letter`, `agreement`, or `form` (omit for a plain page) | N109 |
| `form` | with `output: form` | the bundled form's code | N109 |
| `origin_url` | forms only | the `.gov` page the blank form came from | N109, N110 |

### Event page

| Key | Required | Values | Checked by |
| --- | --- | --- | --- |
| `kind` | yes | `event` | S103, S104 |
| `title` | yes | any non-empty text | C001 |
| `description` | yes | any non-empty text | C002 |
| `starts_at` | yes | an ISO-8601 time | E001 |
| `timezone` | yes | an IANA zone, e.g. `America/Denver` | E001 |
| `luma_url` | yes | the Luma event URL — Luma hosts the event and its RSVPs | E004 |
| `ends_at` | no | an ISO-8601 time | web build |
| `image_url`, `image_alt` | no | the event picture (same as on Luma) and its alt text | web build |
| `public_slug` | no | a custom URL slug | web build |

### Blog post and workshop page

| Key | Required | Values | Checked by |
| --- | --- | --- | --- |
| `kind` | yes | `post` (blog), `workshop` (workshop page) | S103 |
| `title` | yes | any non-empty text | C001 |
| `description` | yes | any non-empty text | C002 |

### Matter dashboard

A dashboard names its page type with `kind:` and its faces with `lenses:`. The client, lawyer, and clerk section lists
live in one file, so a dashboard's faces cannot drift apart across separate documents. Each kind has its own catalog of
sections; picking from another kind's catalog is an error, which is what makes per-kind checking mean anything.

```yaml
kind: review_queue_workbench
title: Document review — Homer v. Flanders
lenses:
  lawyer: [queue_rail, item_detail, item_status_setter, boundary_note, provenance_statement]
  client: [boundary_note, provenance_statement]
```

| Key | Required | Values | Checked by |
| --- | --- | --- | --- |
| `kind` | yes | one of the ten dashboard kinds listed above | S103 |
| `title` | yes | any non-empty text | C001 |
| `lenses` | yes | a mapping of `client`, `lawyer`, or `clerk` to a list of section names | D001, D002, D003, D004 |

Two sections are in every kind's skeleton and must appear in **every** declared lens: `boundary_note` (what this page is
not, and what still requires a human) and `provenance_statement` (as-of date, what was examined, what was not). A client
face without a boundary note is the failure `D003` exists to prevent. The kind's own spine — the sections without which
the page is not that kind — must appear in at least one lens, since a client face legitimately shows less than the
firm's.

### GitHub notation

| Key | Required | Values | Checked by |
| --- | --- | --- | --- |
| `kind` | yes | `github` | S103, S104 |
| `title` | yes | any non-empty text | N101 |
| `questionnaire` | yes | a linear `BEGIN` → `END` ladder | N104, N113, N118, N119 |
| `custom_questions` | yes | wording (and options) for every `custom_*` state | N104, N119 |

Every key the legal contract requires — `code`, `respondent_type`, `jurisdiction`, `confidential`, `workflow`, `output`
— is absent here, and that is the point: a GitHub notation is engineering intake, so there is no respondent to bind, no
jurisdiction to declare, and no workflow to run. `N119` adds the shelf's own contract: the file is one of exactly two
names, and its questionnaire asks the change surface (`web`, `api`, `infrastructure`, `form`), the Engineering Council
question, and at least one free-text question whose answer becomes the body.

Three footnotes. `form` rides along on government-form templates and is bound to `output: form` — N109 requires the two
together and rejects a `form:` key on any other profile, so a stray or orphaned `form:` is now a loud error rather than
a silent one. Finally, on how the required `kind:` is enforced: a template that carries structure but omits `kind:`
trips `S104` right in your editor; a content page (blog, workshop) has no such structural tell, so a repo-wide corpus
test — not a per-file squiggle — fails CI if one ships without its `kind:`.

## The squiggly underline: red versus yellow

Open any of these files in a supported editor and the checker runs as you type:

- a **red** underline is a blocking error — a missing `title`, an unknown `respondent_type`, a workflow with no
  `lawyer_review`, a half-declared template, a blog filename that will not publish. The file is not done until the red
  is gone.
- a **yellow** underline is a non-blocking heads-up — most often a workflow step that is allowed but whose automation is
  not built yet. It is information, not a blocker.

Hover over an underline and it tells you the rule and what to fix. Nothing you type leaves your machine: the checker
reads only the buffer in front of you and sends nothing anywhere, which is the same confidentiality the `confidential`
flag is there to protect. Editor setup is in <lsp/README.md>.

## Checking it yourself from the command line

The editor checks continuously, but you can run the same checker by hand over a file or folder:

```bash
cargo run -p cli --quiet -- validate <path>
```

It classifies each file automatically — a template is held to the template rules, a blog post to the blog rules, prose
to the writing rules — and prints any problem with its file, line, and rule code.

## Where to go next

- <notation-authoring.md> — how to author the body of a notation template and the full validation contract.
- The Neon Law Navigator workshop — a hands-on walk that builds one real notation end to end.
