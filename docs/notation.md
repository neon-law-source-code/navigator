---
publish: true
---

# Notation vocabulary

This doc holds the notation-system vocabulary — what the markdown templates produce, how they're filled in, and the
rules that validate them. It is kept in teaching order rather than alphabetically, because Template precedes Notation by
design: you read what a Template *declares* before you read what a Notation *runs*. The rest of the workspace vocabulary
lives in [`glossary.md`](glossary.md).

## Template

A **static blueprint.** A markdown file with a YAML frontmatter block — `title`, `code`, `respondent_type`, and the
`questionnaire:` and `workflow:` specs — plus a body of legal prose with `{{question_code}}` placeholders.

A Template *declares*: which Questions to ask, in what order, what workflow advances the resulting document, who the
respondent is, what the document is titled. It asks nothing on its own. Until a respondent is bound to it (see
[Notation](#notation)), it is inert — a file on disk, useful for linting and preview, but no questions have been asked
and no workflow has run.

Identified by a stable `code` like `nv__llc_formation` or `onboarding__letter`.

**Draft** is the same noun in the lawyer's English: a Draft *is* a Template, and either spelling names this file and its
four parts. The [Draft](glossary.md#draft) glossary entry holds the synonym and separates it from the lowercase `draft`
workflow state a running Notation carries, described below.

### The four parts

Every Template has exactly four parts. Three of them live in the YAML frontmatter block and the fourth is the prose
beneath it.

1. **Metadata** — the flat top-level frontmatter scalars that classify the file: `title`, `code`, `respondent_type`,
   `jurisdiction`, `kind`, and `form`. Read one key at a time through
   [`rules::frontmatter::field`](../rules/src/frontmatter.rs), a `serde_yaml`-backed single-key lookup.

2. **Questionnaire** — the `questionnaire:` block: the states, transitions, and prompts that drive the respondent's
   question-and-answer walk. See [`notation-authoring.md`](notation-authoring.md).

3. **Workflow** — the `workflow:` block: composable `<prefix>__<discriminator>` step states that `workflows-service`
   executes against Restate. See [`durable-workflows.md`](durable-workflows.md) and the Workflow Runtime entry in
   [`glossary.md`](glossary.md).

4. **Body** — the Markdown prose below the frontmatter fence, carrying `{{question_code}}` and `{{type__role.field}}`
   placeholders that are resolved at render and assembly time.

**Metadata is a conceptual grouping, not a literal YAML key.** There is no `metadata:` container in the frontmatter.
Those scalars are siblings of `questionnaire:` and `workflow:`, not children of a shared key, so a template that
declared an actual `metadata:` mapping would not parse as one. The grouping names the *role* those keys play —
classifying what the file is — so the four parts can be discussed as one model.

Nesting them under a real `metadata:` key would be a breaking parser change touching every file under
[`templates/`](../templates/) as well as `rules::frontmatter`, and is deliberately not implied by this model.

- Schema: [`template` in `navigator.surql`](../store/src/schema/navigator.surql) Queries:
  [`store::templates`](../store/src/templates.rs) Files: [`templates/`](../templates/) — three top-level shelves:
  `notations/forms/<country>/<jurisdiction>/<office>/<code>.md` for government forms, `notations/neon_law/<document>.md`
  for Neon Law firm work, and `github/<notation>.md` for engineering intake. Only the first two are Templates in the
  sense this doc means: a `kind: github` file declares a questionnaire and a body but is not a legal instrument, is
  never imported as a `templates` row, and so never becomes a Notation. See
  [`notation-authoring.md`](notation-authoring.md).

> **Storage.** The markdown body lives in [`cloud::StorageService`](../cloud/) like every other artifact: the
  `templates.body` TEXT column is gone; `templates.asset_id` references an [Asset](glossary.md#asset) holding the bytes.
  Read via [`store::templates::body`](../store/src/templates.rs); the seed and `navigator site seed` paths ingest it
  (sha-dedup). `site seed` creates only workspace-shared rows (`project_id IS NULL`) and question catalog rows; it never
  creates a Project or a client-facing Notation. Templates are workspace-scoped code-like assets governed by git, not by
  the per-Project archive.

> **Workspace-shared vs project-scoped.** A Template is workspace-shared (`templates.project_id IS NULL`, the public
  catalog default) or scoped to a single Project. Project-scoped rows are hidden from the public Template list (cli
  `list`, the admin surface) and resolved only under that Project;
  [`store::templates::resolve`](../store/src/templates.rs) prefers the caller's Project, falling back to the shared row.
  Two partial unique indexes on `code` enforce the rule. The shared index keeps workspace-shared codes globally unique
  (`nv__llc_formation`, `onboarding__letter`); the per-Project index on `project_id` and `code` lets each Project reuse
  short codes (`amendment`, `consent`) without colliding with another Project's.

> **Jurisdiction.** Every Template declares a `jurisdiction:` code that resolves to
  [`store/seeds/Jurisdiction.yaml`](../store/seeds/Jurisdiction.yaml). Government form templates also encode the
  jurisdiction in their `code`: `NV` maps to `nv__...`, `US` maps to `us__...`, and the markdown filename stem must
  match that code. The government provenance URL is `origin_url`, not a checked-in checksum or revision field; git
  tracks the vendored bytes.

## Notation

A Template **come to life.** One running instance of a Template, bound to a specific [Person](glossary.md#person) — the
respondent — a [Project](glossary.md#project), and optionally an [Entity](glossary.md#entity), carrying a workflow
`state` such as `draft`, `lawyer_review`, or `signed`.

> **Client English.** A Notation in the context of its Project is what clients call an
  **[Engagement](glossary.md#engagement--retainer)** (or a **Retainer**, when the bound Template is a retainer). The
  schema noun is `Notation`; the marketing noun is Engagement.

The Questions the Template declared are *asked* here; the [Answers](#answer) the respondent gives are stored against
this Notation; the workflow runs against this Notation. Where a Template is static, a Notation has a lifetime — born
when a matter opens, closed when its workflow terminates. **In our legal practice, the unit of work is a Notation:**
opening a new matter creates one; walking a client through engagement, intake, and signing advances it through its
states.

- Schema: [`notation` in `navigator.surql`](../store/src/schema/navigator.surql) Queries:
  [`store::notations`](../store/src/notations.rs) Lives in: the `notation` table in SurrealDB

> **Note — two meanings.** "Notation" is also the umbrella name for Neon Law Navigator's markdown notation format (the
  file format Templates are written in). Templates *are* notations in that sense; each row in the `notations` table is
  one running instance of one. The format name is older than the schema; both meanings stuck. Disambiguate by context:
  capitalized and referring to a row or a matter, it's the runtime instance; referring to the file format, it's the
  lowercase "markdown notation."

### Word Markdown trust boundary

The Word notation projection is a trusted internal format. The public `word::notation::to_markdown` and
`word::CanonicalDocument::to_markdown` producers return an opaque `TrustedNotationMarkdown` value; only that value can
reach the structural parser. Raw Markdown from an editor, upload, or another tool cannot be presented to
`from_markdown`, so directly supplied `navigator-*` comments cannot acquire structural authority. The actual parser
callers are the round-trip tests in `word/src/outline.rs` and `word/src/notation.rs`; the public
`Document::notation_markdown` method is a producer for a governed caller, not an external Markdown ingestion path. Word
text first passes through the managed adapter and the canonical emitter, whose escaping keeps comment-shaped prose as
prose.

## Questionnaire

The ordered list of [Questions](#question) a Template **declares** it will ask. Lives entirely in the template's
frontmatter under `questionnaire:`. Not a separate table — the questionnaire is what you get when you read a Template's
frontmatter and resolve each entry against the `questions` table.

When a [Notation](#notation) runs, *those* are the prompts the respondent sees and the [Answers](#answer) get attached
to. **The Template declares the questionnaire; the Notation asks it.**

> **Status — declared and walked.** The questionnaire state machine is structurally validated by the [`N104` rule
  implementation](../rules/src/f104.rs) **and** executed step-by-step by
  [`portal::retainer_walk`](../portal/src/retainer_walk.rs): one question per request, one [Answer](#answer) per
  advance, one [Notation Event](glossary.md#notation-event) per transition. The walker shares its runtime surface with
  the [Workflow Runtime](glossary.md#workflow-runtime) — both implement `workflows::StateMachineRuntime`, keyed by
  `MachineKind` and `notation_id` — so a single Restate virtual object per Notation hosts both timelines on one logical
  journal. See [`docs/retainer_intake.md`](retainer_intake.md) for the end-to-end walkthrough.

### Reaching END begins the workflow

**A questionnaire that reaches `END` starts its Notation's [Workflow](#template).** That is a property of the
questionnaire completing, not of the door that recorded the last answer, and all three doors reach it through one
function — [`portal::retainer_walk::begin_post_questionnaire_workflow`](../portal/src/retainer_walk.rs):

- the lawyer's form walk (`POST /app/lawyer/notations/{id}/step`),
- the REST command boundary (`POST /app/api/notations/{id}/answers`), and
- Navigator MCP's `answer_notation`, which takes the drive as a
  [`workflows::PostQuestionnaireDrive`](../workflows/src/post_questionnaire.rs) because beginning a closing letter's
  workflow means rendering one, and `workflows` owns no renderer.

What it begins is the bound Template's business rather than the caller's. A signed template advances to the
`lawyer_review` human gate and stops: nothing is sent and no PDF is rendered on the request that completed intake, so a
render failure lands at the lawyer's step rather than on the respondent's last answer, and `lawyer_review` stays a true
human gate (`N116`). A closing letter, which the firm signs and which ends the matter, runs through to `END`.

**The client's own intake is a subset and does not reach `END`.** A client sees only the questions whose `audience` is
`client` or `both` and their answers are written straight to the `answers` table; the questionnaire runtime's pointer is
the *firm's* progress, so the client finishing their portion leaves the rest for the firm and starts nothing. See
[`workflows::notation_session::client_intake_step`](../workflows/src/notation_session.rs).

### Conversational notation (Navigator MCP)

The same questionnaire state machine is also driven from outside the HTML form by two catalog tools: `create_notation`
(start the Notation, get the first question) and `answer_notation` (submit one answer, get the next question or
"complete"). The LLM client is the UI; the server owns the state. Both the form and these tools call the same
[`workflows::notation_session`](../workflows/src/notation_session.rs) service, so changes to the walking logic touch
exactly one codepath.

Creating or answering a Notation is a supervised act, so that walk runs over A2A, where every call pauses in
`input-required` until a firm principal authorizes it. The `/app/mcp` endpoint and the stdio bridge withhold both tools
and refuse one named anyway: neither can collect an approval, and simulating one is worse than declining. See
[`docs/mcp-a2a-interaction.md`](mcp-a2a-interaction.md) for the authorization round trip.

`navigator notations preview` is intentionally a local reading surface: it binds only to `127.0.0.1`, takes an
OS-assigned ephemeral port unless `--port` supplies one, opens no store, and holds no session or persistent state.

`navigator notations run <FILE>` is the complementary terminal walkthrough. It validates one template and runs it in a
new embedded store with a temporary object directory and synthetic identities. It first completes the client-visible
subset (which does not advance the firm's questionnaire), then completes the firm's questionnaire, invokes the shared
post-questionnaire drive at `END`, journals every transition to `notation_events` itself (this workbench never starts
the `workflows-service` worker that journals a production run), and reports the persisted answers, journaled
transitions, and workflow state. The store and objects exist only for that process: the command neither selects a
deployment from ambient configuration nor calls live providers.

## Question

One prompt presented to a respondent during Template traversal. Identified by a stable `code` (e.g. `client_name`,
`organizer_state`). Has an `answer_type` — `string`, `int`, `bool`, `choice`, etc. — that the form layer uses to render
the right input. When a questionnaire state uses the typed grammar `<type>__<role>`, its `<type>` prefix is a [Question
Type](glossary.md#question-type) from `store::question_registry` (record / reference / custom, singular / plural) — the
closed vocabulary `N113`–`N117` and the render/form-fill evaluator all share. Use glossary-backed states and dotted
fields for durable nouns: `person__client` with `{{person__client.name}}`, not `custom_text__client_name`.

- Schema: [`store::questions`](../store/src/questions.rs) Lives in: the `question` table in SurrealDB (ENG-121) Seed:
  [`store/seeds/Question.yaml`](../store/seeds/Question.yaml)

## Answer

One respondent's answer to one Question. **Append-only**: a re-ask or a correction is a new row, never an update, and
the latest row for a `(notation, state)` wins on read. The seed's own fixtures deduplicate on `(question, person,
value)`, so re-seeding the same value is a no-op.

- Schema: [`store::answers`](../store/src/answers.rs) Lives in: the `answer` table in SurrealDB (ENG-121)

## Rule

A validation check applied to markdown notations by the [`rules`](../rules/) crate. Three families:

- **M-family** — generic Markdown hygiene (headings, list spacing, code-fence languages, link targets). **N-family** —
  Neon Law Navigator notation template shape (required keys, question-code resolution, template/workflow
  well-formedness).
- **S101** — the 120-character line-length limit. Applies to every `.md` file in the workspace.

The `cli validate` subcommand runs the relevant subset per file.
