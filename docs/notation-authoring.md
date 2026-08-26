---
publish: true
---

# Authoring notations

This is the how-to companion to [`notation.md`](notation.md). That doc defines the *vocabulary* (Template, Notation,
Questionnaire, Question, Answer, Rule); this one is the *procedure* — how you write a notation, what the toolchain
enforces, what runs after a client finishes intake, and what is still on the roadmap. If a word here is unfamiliar,
[`notation.md`](notation.md) is the source of truth for what it means.

## What a notation is, in one paragraph

A **Template** is a static blueprint: one markdown file with YAML frontmatter, checked into `templates/`. A **Notation**
is that Template come to life — one running instance bound to a [Person](glossary.md#person) (the respondent), exactly
one [Project](glossary.md#project), and optionally an [Entity](glossary.md#entity) — advancing through two state
machines the Template declares. In client English a Notation-in-a-Project is the **Engagement** (or **Retainer**). The
Template *declares*; Restate *runs*. Everything below is about writing good Templates and growing what their workflows
can do.

## Where templates live

Every legal template lives under exactly one of three shelves:

- `templates/forms/` — government form-backed templates. Paths mirror the public assets bucket, so
  `templates/forms/united_states/nevada/state/nv__llc_formation.md` corresponds to
  `gs://<assets-bucket>/forms/united_states/nevada/state/nv__llc_formation.pdf`.
- `templates/neon_law/` — Neon Law product work: product retainers, engagement letters, product-specific
  documents, and shared firm documents such as the closing letter.

Every template declares `jurisdiction:` using a seeded jurisdiction code from `store/seeds/Jurisdiction.yaml`. Form
templates are stricter: their filename stem, `code`, and `form` binding must match, and that code starts with the
jurisdiction prefix (`nv__llc_formation`, `us__form_990`, `ca__...`). A retainer may keep a runtime code such as
`onboarding__retainer` while living at an author-friendly path such as `templates/neon_law/shared/retainer.md`.

## Anatomy of a template file

> For a plain-English, attorney-facing tour of every frontmatter key — across notation templates, events, blog posts,
> and workshop pages — see <frontmatter.md>. This section is the authoring-focused version.

Every template has two parts: YAML frontmatter (the contract) and a markdown body (the document, with
`{{question_code}}` placeholders). Here is the shared retainer frontmatter from `templates/neon_law/shared/retainer.md`
(the real file wraps this block in `---` fences, then the prose body follows):

```yaml
title: Retainer Agreement
respondent_type: person_and_entity
jurisdiction: NV
code: onboarding__retainer
confidential: true
questionnaire:            # the intake Q&A — what we ask the client
  BEGIN:                { _: person__client }
  person__client:       { _: project__engagement }
  project__engagement:  { _: END }
  END: {}
workflow:                 # what happens after intake — render, review, sign
  BEGIN:                       { intake_submitted: intake_persisted__client }
  intake_persisted__client:    { retainer_rendered: lawyer_review }
  lawyer_review:                { approved: generate_pdf__retainer_pdf, changes_requested: reask__client, rejected: END }
  reask__client:               { intake_resubmitted: lawyer_review }   # re-collect flagged answers, loop back
  generate_pdf__retainer_pdf: { pdf_persisted: sent_for_signature__pending }
  sent_for_signature__pending: { signature_received: END, signature_declined: END }
  END: {}
```

The body below the frontmatter is plain prose carrying placeholders for declared answers. At render time each is
replaced with the client's answer — `{{person__client.name}}` becomes the actual name and `{{project__engagement.name}}`
the matter.

Frontmatter fields:

- `title` — the human document title (N101 requires it non-empty).
- `respondent_type` — one of `person`, `entity`, `person_and_entity` (N102).
- `jurisdiction` — a seeded jurisdiction code (`NV`, `CA`, `US`); required even for product templates.
- `code` — the stable, unique identifier (`onboarding__retainer`, `nv__llc_formation`); how every surface refers to it.
- `confidential` — an explicit `true`/`false` decision, never defaulted (N105).
- `origin_url` — forms only; the HTTPS government page where the blank form can be obtained.
- `form` — forms only; the bundled form code, matching `code` and the filename stem.
- `questionnaire:` — the intake state machine: `BEGIN → typed_state → … → END`. Each step's `_:` is the "answered"
  transition — and the **only** condition a questionnaire may use: the block must be one linear `_` chain from `BEGIN`
  to `END` covering every declared state. `QuestionnaireSpec` rejects anything else at parse time, and `N118` flags it
  at authoring time. State names are `<type>__<role>`; the prefix before `__` must be a registered Question Type.
- `workflow:` — the post-intake state machine: render, lawyer review, signature, filing. Transitions fire on named
  signals (`approved`, `pdf_persisted`, `signature_received`).

Two machines, one journal: questionnaire and workflow are hosted on a single Restate virtual object keyed by the
notation's id, so their signals serialize and can never interleave. State is **append-only** — every transition writes a
`notation_events` row, and the current state is the latest row's `to_state`. Nothing is ever updated in place, so the
full history of a matter is replayable for audit.

## The typed question grammar

A questionnaire state name is `<type>__<role>` — a **question type** and the **role** it plays in this matter, e.g.
`person__trustor`, `entity__company`, `address__for_company`. The `<type>` half is a closed set (the registry in
[`store::question_registry`](../store/src/question_registry.rs)); `N113` rejects a typed state whose `<type>` is not
registered. The `<role>` suffix is free naming that keeps two answers of one type distinct — `entity__company` and
`entity__subsidiary` are two separate entities, not one.

Every type is one of three kinds:

- **Record** — the answer creates or links a `store::entity` row: `person`, `entity`, `address`, `role`, `filing`,
  `credential`, `disclosure`, `issuance`, `signature`, `notarization`.
- **Reference** — the answer selects an existing seeded row: `jurisdiction`, `country` (the
  `jurisdiction_type = 'country'` subset, rendered as a country picker), `entity_type`, `product`, `project`.
- **Custom** — a primitive value living in the answer JSON, no SQL grounding: `custom_text`, `custom_phone`,
  `custom_yes_no`, `custom_single_choice`, `custom_multiple_choice`, `custom_usd`, `custom_datetime`. A custom state
  reads `custom_<type>__<prompt_key>` and needs a matching `prompts:` entry (`N104`).

**Prefer the glossary — both its models *and their fields* — and reach for `custom_*` only when nothing in the glossary
fits.** The registry exists so a questionnaire is composed of real domain nouns, not free text. Two checks, in order,
before any `custom_`:

1. **Is the answer a glossary model?** Use the type — a trustee is `person__trustee`, the company `entity__company`, its
   office `address__for_company`, its members `people__members`, its state of licensure `jurisdiction__licensure`.
2. **Is the answer a *field* of a glossary model?** Use the model and read the field with a dotted placeholder, never a
   custom string: a trustee's name is `{{person__trustee.name}}`, the entity's legal name `{{entity__company.name}}`,
   the office city `{{address__for_company.city}}`, a bar number `{{credential__bar.license_number}}`. Each glossary
   term's **Schema** link (`store::entity::…`) is the list of fields a type exposes. `custom_text__trustee_name` is the
   anti-pattern — a name is a field of a Person, not free text.

Only a value that is **neither a glossary model nor a field of one** is custom — a fee status
(`custom_single_choice__fee_status`), a one-off date (`custom_datetime__filed_on`), a consent flag
(`custom_yes_no__recording_consent`). This keeps answers queryable, linkable, and glossary-grounded.

`custom_text` has one extra guardrail: `N117` is a strict allowlist. Every `custom_text__<role>` must appear in the
rules crate's `ALLOWED_CUSTOM_TEXT_ROLES` (each entry carries its rationale in a comment), so a new free-text state
fails closed until it is either typed or adjudicated onto the allowlist. Roles that name a glossary-backed noun —
`custom_text__client_name`, `custom_text__client_email`, `custom_text__healthcare_agent`,
`custom_text__country_of_birth`, `custom_text__daytime_phone`, `custom_text__product_description` — can never be
allowlisted (a meta-test bars the tokens). Use `person__client`, `person__healthcare_agent`, `country__of_birth`,
`custom_phone__daytime_phone`, and their dotted fields, or product/project render context such as the retainer's
`{{custom_clauses}}` slot. The allowlist exists to keep *client* facts in typed states, so it applies to the legal
shelves only — the `kind: github` engineering notations under `templates/github/` hold no client facts and are not held
to `N117`.

**Singular and plural.** Each record/reference type has a singular form (one row) and an explicit aggregate form (an
array of the singular's shape) collected under one question — `person`→`people`, `entity`→`entities`,
`address`→`addresses`, and so on. Add a plural only where a matter actually collects several.

**`__for_` children.** A `<type>__for_<role>` state creates a child row that FKs to an earlier role-addressed parent:
`address__for_trustor` follows `person__trustor` (or `entity__trustor`) and hangs off that answer. `N114` requires the
parent to precede the child in the questionnaire graph and bars aggregates from `__for_` (their children are inline).

**Body placeholders.** Prose uses `{{code}}` for a bare answer, `{{type__role.field}}` for a field off a typed answer,
and `{{#for x in <aggregate>}} … {{x.field}} … {{/for}}` to walk a plural answer. `N115` resolves every path and
iterator against the questionnaire's declared states, and `N120` resolves the bare `{{type__role}}` token the same way;
signature blocks (`{{client.signature}}`) stay `N107`'s specialty. A token nothing declares is never substituted, so it
reaches the rendered page verbatim — which is why both rules ground the body against the questionnaire beside it.

## How to create one — the five-step recipe

New legal matters follow a fixed order (see [`agent-workflows.md`](agent-workflows.md) for the long form).
Feature-first, so the composition is specified before the prose exists:

1. **Write the composition `.feature` first.** Describe the matter as a sequence or branching graph of reusable workflow
   steps, using only Person / Entity nouns from [`glossary.md`](glossary.md). The feature is the product-level spec; the
   template satisfies it by composing already-known steps.
2. **Write the template + questionnaire.** Create the markdown file under `templates/forms/...` for a government form,
   or `templates/neon_law/<product>/...` for a product template. Declare the `questionnaire:` walk and the `workflow:`
   states. Body prose uses `{{question_code}}` placeholders. If a questionnaire state uses a `custom_*__prompt_key`
   code, add a sibling `prompts:` map with that English prompt key and the exact prompt to ask.
3. **Seed the questions.** Add each new question `code` to `store/seeds/Question.yaml` (prompt, `question_type`,
   help text). The questionnaire's state prefixes must resolve to these codes or N104 fails.
4. **Declare the workflow YAML.** Compose the post-intake flow from the shared step registry (below) — never a one-off
   handler. Reuse `lawyer_review`, signature, and document steps so the flow stays auditable.
5. **Wire the durable handlers.** Bind new workflow steps onto the existing `workflows-service` worker. Never stand up a
   per-workflow pod — one worker hosts every flow.

A template is not legally usable until an attorney has reviewed the body copy. The `lawyer_review` state is mandatory
(N106) precisely so a licensed human is always in the loop before anything is sent or filed.

Question codes should stay minimal and reusable. A Question is the stable prompt/fact type (`citizenship_status`,
`passport_expiration_date`, `registered_agent`); an Answer is the respondent's time-bound value for that question. If a
future workflow needs to know whether an answer is still fresh, add freshness/expiration metadata to the answer side
rather than minting a new one-off question code. That keeps questionnaires short: reuse a still-valid answer when it can
lawfully answer the question, and ask again only when the recorded answer has expired or is no longer adequate for the
matter.

## The validation contract

Three rule families guard every template, enforced identically in your editor, in `cli validate`, and in CI — because
all three call the same `rules` crate. A template that is clean on your laptop is clean in the merge gate.

- **N-family (notation template shape, structural).** N101 title present; N102 valid `respondent_type`; N103 snake_case
  filename; N104 both machines declare `BEGIN`, reach `END`, questionnaire states resolve to real Question codes, and
  workflow states resolve to known workflow-step prefixes; N105 `confidential` is an explicit bool; N106 the `workflow:`
  has a bare `lawyer_review` state (the suffix form `lawyer_review__for_grantor` does **not** satisfy it — the
  human-review gate must be unconditional); N108 `code` is the stable Template identifier; N110/N111/F110 enforce the
  `forms/` or `neon_law/` shelf, seeded `jurisdiction`, and jurisdiction-first form codes. N-family rules are
  diagnostic-only: a human must resolve them, the tool will not auto-rewrite legal structure.
- **M-family (markdown hygiene, ~50 rules).** Headings, lists, fences, tables, spacing. Most carry a safe autofix.
- **S101 (line length).** 120 Unicode scalars per line, every `.md`. Frontmatter is linted too; folded YAML scalars let
  a long value wrap and still pass.

Run it before committing any `.md` change:

```bash
cargo run -p cli --quiet -- validate <path>
```

## Authoring in markdown with the LSP

`navigator-lsp` is a single Rust binary speaking LSP over stdio. It shares the exact rules engine the CLI uses, so the
editor and CI can never disagree. Supported editors ship copy-paste configs under [`lsp/`](../lsp) docs: VS Code,
Neovim, Helix, Emacs, Zed. The authoring loop for a non-engineer legal author:

1. **Type.** Open `templates/neon_law/northstar/nv__simple_will.md` in your editor. Write
   legal prose and frontmatter — no proprietary tool, no markup beyond markdown.
2. **Live diagnostics.** On every keystroke the LSP lints the buffer and shows squiggles: N101 if `title:` is missing,
   N104 if the questionnaire/workflow shape is broken or a questionnaire state is not in the canonical question seed
   list, S101 past 120 chars, M-rules on shape. Hover any squiggle for a plain-English explanation of the rule.
3. **Fix-all on save.** `source.fixAll` rewrites every mechanical issue — tabs, trailing whitespace, blank-line spacing,
   heading spacing — automatically. What remains is the *semantic* work only a human can do (an unmade `confidential`
   decision, a workflow that never reaches `END`).
4. **Open a PR.** The clean `.md` is committed as a plain-text diff. CI runs the identical engine. An attorney reviews
   readable prose; the linter has already signed off on structure.

### Why markdown + frontmatter + git, not a proprietary tool

- **Ergonomics.** One free binary attaches to whatever editor the author already knows. Fix-on-save removes the entire
  class of formatting fiddling; hover tooltips teach the rules in context, lowering the floor for a non-engineer.
- **Correctness.** A single rules engine is the authority — editor, CLI, and CI cannot diverge. Invariants that matter
  legally (every workflow has a `lawyer_review` gate, `confidential` is an explicit choice, every workflow code
  resolves) are machine-enforced *before merge*, not left to reviewer vigilance.
- **Auditability.** The template is plain text under git: every change is an attributable, reviewable diff, gated by PR.
  The rules themselves are versioned Rust with snapshot tests. A proprietary document-automation tool hides the document
  in an opaque format with no line-level diff and no enforceable structural contract.

## What runs after intake — the step registry

Once the questionnaire reaches `END`, the workflow machine takes over. Steps are resolved from a state-name prefix to a
`StepKind` and an actor class (System / Lawyer / Respondent) in `workflows/src/step.rs`. Honest status of what is wired
today:

| Step | Status | Notes |
| --- | --- | --- |
| `email_send__<slug>` | Implemented | Durable SendGrid send via two `ctx.run` journals; only `welcome` renders today. |
| `intake_persisted__*` | Implemented | Pass-through wait state recorded on the journal. |
| `lawyer_review` | State-only | Mandatory gate; dev auto-approves. No prod review UI wired to the worker. |
| `client_review` | State-only | Respondent approves attorney-reviewed drafts on the Phase A review surface. |
| `reask__*` | State-only | Re-collects flagged answers after `changes_requested`, then loops back to `lawyer_review`. |
| `document_intake__<slug>` | Implemented | Worker files a provided artifact (text/file/link) via `ingest_bytes`. |
| `extract__*` | Seam | Northstar: estate inputs mined from the transcript by AIDA/Gemini; advanced on completion. |
| `analysis__*` | Seam | Contract review: web (Vertex Gemini) flags playbook deviations; System wait state. |
| `document_drafts__*` | Implemented | Northstar: web renders drafts into review_documents rows (System wait state). |
| `generate_pdf__retainer_pdf` | Implemented | Worker-dispatched: render + storage persist wrapped in `ctx.run`. |
| `sent_for_signature__pending` | Implemented | Wait state; e-signature webhook signals `signature_received` → END. |
| `notarization`, `_signature` | State-only | Trust/will signing states; a human act, no worker side effect. |
| `firm_signature` | State-only | Firm (lawyer) signs the closing letter ending a matter; a human act, no side effect. |
| `mailroom_send` | Implemented | Worker records a `filings` row in `ctx.run`; reached only after `lawyer_review`. |
| `certified_mail`, `e_filing`, `filing__*` | Implemented | Worker submission steps; record `filings` post-review. |
| `onchain__*` | Scaffolded | Node attestation → durable `attestations` row; `null` attestor keeps it `pending`. |
| `github_issue__*` | Implemented | Opens a GitHub issue over the REST API (no `gh`); no token → opens nothing. |
| `mailroom_receive` | State-only | Inbound mail logged by the SendGrid webhook, not a workflow step. |
| `witnesses` | State-only | Respondent's witnesses sign (will); resolves to the Signature step kind. |

Durability is Restate's: each side effect is wrapped in `ctx.run`, so a replay reuses the cached result instead of
re-emailing or double-inserting. In prod the worker dials Restate Cloud; in KIND it dials the in-cluster Operator. The
"State-only" rows are the contract for steps with no worker side effect yet. The drift-guard test
`workflows::step::tests::drift_guard_every_step_prefix_is_documented` fails if `step_kind_for` gains a prefix
(`STEP_PREFIXES`) this table never mentions, so the status here cannot silently rot.

The `onchain__*` row is "Scaffolded": the step kind, the dispatch arm, and the durable `attestations` table are
implemented and tested, but the on-chain write itself is deferred. The chain is isolated behind the
`workflows::attest::Attestor` trait exactly as GCS is isolated behind `cloud::StorageService` — selecting Solana (or a
second chain) is a new `impl Attestor`, never a workflow edit. The default `NullAttestor` records no transaction, so the
row stays `pending` and no live retainer can claim an on-chain record that does not exist. The step is therefore not yet
wired into the binding `onboarding__retainer` workflow; that one-line YAML edge lands together with the `SolanaAttestor`
(whose open questions — firm key custody, the client wallet, public-chain confidentiality of the hash, and finality —
are decisions, not code). See `workflows::attest`.

`github_issue__*` is the one step in the registry that is not a legal act. It belongs to the engineering intake shelf
(`templates/github/`), so it is the only worker step that does not sit behind `lawyer_review` — nothing it does binds a
client. GitHub is isolated behind the `workflows::github::IssueOpener` trait the same way the chain is isolated behind
`Attestor`: `RestIssueOpener` calls `POST /repos/{owner}/{repo}/issues` directly with `reqwest`, and the runtime shells
out to the `gh` CLI nowhere — an unpinned external binary has no place inside a durable step. Configure it with
`NAVIGATOR_GITHUB_TOKEN` (or `GITHUB_TOKEN`) plus `NAVIGATOR_GITHUB_REPO`; with no token the default `NullIssueOpener`
opens nothing and says so, so a KIND run or a test never reaches github.com. The opened issue's number and URL are
journaled on the transition; a skipped open journals nothing rather than an issue that does not exist.

The registry is deliberately small. Template authors should compose these prefixes with discriminators
(`generate_pdf__articles_pdf`, `mailroom_send__notice_of_representation`) rather than creating per-product verbs. If a
workflow needs a genuinely new act, add a reusable `StepKind` first, document it here, cover the mechanics in Rust, and
then compose it from a feature spec.

### Adding a reusable step — the recipe

A "reusable step" is one `StepKind` that many notations bind to by naming a `<prefix>__<slug>` state — `email_send__*`,
`generate_pdf__*`, `document_intake__*`. Two reference implementations show the shape; the next one is a single registry
entry, not a second dispatch match.

- **Signature** is the *seam* reference: `portal::signature::SignatureProvider` is a trait with a stub for KIND/tests
  and a concrete `DocuSignSignatureProvider` for prod, selected from `AppState`. Reach for a trait when the step calls
  an external system that has more than one real implementation you swap at runtime.
- **Document-intake** is the *registry* reference: `document_intake__<slug>` files a provided artifact (a transcript, an
  executed PDF, an ID scan) into the matter through `store::documents::ingest_bytes`. It has exactly one implementation,
  so it is a plain dispatch fn behind one `StepKind`, not a trait.

The step layer routes through one registry, `workflows::dispatch_step`, keyed by `StepKind`. Both callers — the
`workflows-service` worker (`notation_service::workflow_signal`, which wraps the call in `ctx.run`) and the in-process
dev/BDD runtime (`DispatchingRuntime::maybe_dispatch`, which calls it inline) — share that one arm, so a new step is
added once, not twice. To add a step kind with a worker side effect:

1. **Name the prefix + kind.** Add a `StepKind` variant and its `(prefix, StepKind)` row to `STEP_PREFIXES` in
   `workflows/src/step.rs`, plus the actor class in `StepKind::actor`. Document it in the status table above (the
   drift-guard test enforces this).
2. **Write the dispatch fn + payload.** Add `<kind>.rs` with a serde payload (internally tagged on `kind`, like
   `DocumentPayload` / `IntakeArtifact`) and an `async fn dispatch_<kind>(deps…, payload) -> Result<_, _>` that performs
   the *one* side effect and returns — no `ctx.run`, no journaling; durability is the caller's.
3. **Register one arm.** Add the `StepKind` to `dispatches_side_effect` and one match arm to `dispatch_step` in
   `workflows/src/dispatch.rs`, decoding the payload from the signal `value` and calling your dispatch fn with the
   `StepDeps` providers (`email`, `storage`, optional `db`). The worker and the in-process runtime pick it up for free.
4. **Thread the payload from the trigger.** The surface that fires the transition into the step (a `web` handler) builds
   the payload, JSON-serializes it, and passes it as the signal `value`. The artifact for an intake step is
   phone-friendly: a text paste, a file, or a link — never "scan a PDF".

Keep the `ctx.run` boundary in the worker, never inside `dispatch_step`: a registry that journaled its own side effect
would reintroduce the duplicate-effect bug on replay.

## Documents and PDFs

**What we have.** A dedicated `pdf` crate renders a Typst document to PDF bytes in pure Rust (no shell-out), in the firm
typeface Noto Serif, with a redaction helper. Preview, offline rendering, form-fill, and final document generation share
one notation evaluator for bare placeholders, dotted fields, and aggregate loops; final signed documents evaluate data
placeholders first, then expand signature anchors into Typst blocks and the e-signature manifest. The retainer flow
threads the evaluated Typst source to the **worker** as a `DocumentPayload` on the `approved` signal; the
`generate_pdf__retainer_pdf` step calls `pdf::render` and persists the bytes through the `cloud::StorageService` seam
(`FsStorage` in dev, GCS in prod) at `notations/<id>/retainer.pdf`, wrapped in `ctx.run` for replay-idempotent
durability. `web` reads the PDF back from storage to hand to the signature provider. This is one-directional: template →
fresh PDF.

**Rendering a template to PDF offline — `navigator template render`.** For an ad-hoc PDF outside the durable workflow (a
demand letter to send by hand, a draft for review), `navigator template render <template.md> --out <file.pdf>` takes any
validation-passing notation template and compiles it in pure Rust. Because templates are authored in **Markdown** but
the `pdf` crate compiles **Typst**, the body is converted by `pdf::markdown::to_typst` (headings, emphasis, lists, block
quotes, inline code, links) before rendering. The command validates the file against the same rule set as `navigator
validate`, refuses to render a template with any violation, and fills placeholders through the same notation evaluator
used by preview and final PDF generation.

**Output formats — the letterhead seam.** How the document is dressed is an `OutputFormat` (`pdf::format`): `plain`
(page geometry + firm typeface), `letter` (the firm letterhead — mark, letterspaced wordmark, a rule across the page,
and a contact line carrying the firm mailbox and website rather than a street address, with every page numbered), or
`agreement` (that same letterhead over an executed contract, typeset curtly and holding every table unbreakable so a
signature block never splits across a page break). A template declares its default in an optional `output:` frontmatter
field (validated by rule `N109`); `--format` overrides per render. New forms — pleading paper, a fax cover — are a new
`OutputFormat` variant plus its Typst chrome preamble; the conversion, embedded logo, and font stack are shared, and
`agreement` is the worked example. Fill `{{placeholder}}` tokens with repeated `--answer code=value` flags; unfilled
tokens render verbatim. The full command, letterhead, and font reference is [`pdf/README.md`](../pdf/README.md) —
including how the licensed GORP Serif faces reach the renderer without their bytes entering the repository.

**Filling fillable government PDFs — done.** `pdf::fill_acroform(blank_pdf, fields)` opens an existing fillable PDF (a
Nevada SoS articles form, an IRS Form 990) via `lopdf`, walks its AcroForm `/Fields`, sets each `/V`, and sets
`/NeedAppearances` so a viewer regenerates the field appearances — a read-modify-write path distinct from the Typst
`render` path. Blank forms live as templates in the `cloud::StorageService` seam (`forms/<slug>.pdf`); a
`generate_pdf__<form>` sub-slug dispatches the fill through the same worker step as the retainer PDF (via
`DocumentPayload::Acroform`). The output is **attorney-review-ready, never auto-filed**: the workflow spec parks it at
`lawyer_review` before any filing step, enforced by `workflows::lawyer_review_gates_filing` (a spec-graph check + test).
Two loud-failure guardrails — XFA-based forms (Adobe's XML form layer, unsupported by any Rust crate) are detected and
rejected rather than silently emitting a blank, and a field name that matches no form field errors rather than being
silently dropped. Hierarchical (kids / dotted `/T`) field names remain out of scope.

## External integrations

- **Email — implemented.** `email_send__*` → SendGrid via `reqwest`, durable, with the message id captured for the event
  webhook join. This is the one integration wired end-to-end.
- **E-signature — implemented (the production dead-end is closed).** The `SignatureProvider` trait now ships a concrete
  `DocuSignSignatureProvider` (DocuSign eSignature REST via `reqwest`, `.env`-driven; the stub stays for KIND / tests).
  At send time the provider's `envelopeId` is persisted on `notations.signature_request_id`. The inbound webhook at
  `/webhook/esignature/:secret` (`portal::esignature_webhook`) **verifies an HMAC-SHA256 signature over the raw body
  before parsing it** — fail-closed when `DOCUSIGN_HMAC_KEY` is configured (a prod invariant) — then resolves the
  envelope id back to its notation and signals `signature_received` → END. Only a `completed` event advances state;
  other events ack with 200. The engagement terms are attorney-reviewed at `lawyer_review` *before* the document is
  sent, so signature receipt is a ministerial transition with no second human gate. Covered by a `.feature` (happy +
  forgery) and an end-to-end integration test through the real provider against a mocked endpoint.
- **Google Drive — retained.** Each Project keeps a private Google Drive folder for firm working files, including large
  documents the firm intends to ingest. Navigator stores that folder as a firm-only Project resource; legal files and
  client material stay out of the Project repository, which holds only templates and client-portal source. The old
  direct Drive synchronization workflow and Drive API/CLI surfaces remain removed: retaining the folder does not restore
  a second document store.

## Roadmap

Ordered by value, each item independently shippable. Reliability fixes are split out from the features they ride with so
neither blocks the other.

**Recently shipped.** The full ten-template catalog is now bundled into the canonical seed, so a fresh cluster carries
every template (LLC, trust, will, annual report, dissolution, three nonprofit forms, NV MBT) without an import pass.
**The signature loop is closed** — `DocuSignSignatureProvider` plus the HMAC-verified `/webhook/esignature/:secret`
route advance a signed retainer to END (see External integrations above).

1. ~~**Close the signature loop.**~~ **Shipped.** A real `DocuSignSignatureProvider` plus an inbound webhook that
   verifies the provider's HMAC signature over the raw body before signaling `signature_received`; the provider
   request-id is persisted on the notation for correlation. This ended the production dead-end at
   `sent_for_signature__pending`.
2. ~~**AcroForm form-filling.**~~ **Shipped.** `pdf::fill_acroform(blank_pdf, fields)` (lopdf) fills a fillable
   government form; a `generate_pdf__<form>` sub-slug dispatches it through the worker step, with blank forms held in
   `cloud::StorageService`. Output is **attorney-review-ready, never auto-filed** — the spec-graph guardrail
   `lawyer_review_gates_filing` proves no fill→file path skips `lawyer_review`. XFA forms and unmatched field names fail
   loudly rather than emitting a silent blank.
3. ~~**Promote the planned filing/mail steps to real handlers.**~~ **Shipped.** `mailroom_send`, `certified_mail`,
   `e_filing`, and `filing__*` are worker-dispatched steps that record a durable `filings` row (the firm's proof of what
   was filed) in `ctx.run`; compliance flows (e.g. the Nevada annual report) run end-to-end to END instead of parking.
   `lawyer_review_precedes_submission` proves — on every bundled spec — that no submission side effect fires before the
   review gate. (`notarization` stays a human act; `mailroom_receive` is inbound.)
4. ~~**Make language access explicit in intake.**~~ **Dropped.** Navigator is English-only, so intake records no
   language preference and `notation_session` renders the base question prompt directly. See
   [`AGENTS.md`](../AGENTS.md).
5. ~~**Template storage and scoping.**~~ **Shipped.** Template bodies moved from the inline `templates.body` TEXT column
   to blob storage (`templates.blob_id` → a Blob via `cloud::StorageService`); `templates.project_id` plus two partial
   unique indexes add project-scoped templates alongside the shared catalog, resolved by `store::templates::resolve`
   (prefer Project, fall back to shared). The seed + `navigator db catalog-seed` paths ingest bodies into blobs; render
   paths read them back via `store::templates::body`. See [`notation.md`](notation.md).

## Why this matters — access to justice

The whole point of the notation system is to make routine legal work cheap, fast, and repeatable without removing the
attorney. Each design choice traces back to that mission:

- **One template, many matters.** A lawyer encodes a matter once; every future client walks the same validated
  questionnaire. The marginal cost of the next LLC, trust, or annual report trends toward zero, which is what lets the
  firm serve people a billable-hour model prices out.
- **Faster resolution, lower cost.** A guided questionnaire plus automatic document generation collapses what used to be
  multiple back-and-forth meetings into a single self-serve intake the client finishes in minutes — answered in their
  own words through AIDA, on whichever surface they already use.
- **The human stays in the loop.** `lawyer_review` is mandatory by rule, not by convention. Automation does the
  repetitive assembly; a licensed attorney signs off on the substance. Faster *and* accountable, not faster *instead of*
  accountable.
- **Auditable by construction.** Append-only `notation_events` means every matter has a complete, replayable history —
  the transparency a public-interest practice owes the people it serves, and the record that lets one attorney safely
  oversee far more matters than a paper process ever could.

Speed here is not a convenience feature; it is the access-to-justice mechanism. Every minute and dollar the notation
system removes from a routine matter is one that a person who could not otherwise afford a lawyer gets to keep.
