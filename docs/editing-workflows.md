---
publish: true
---

# Editing a legal workflow

Use this guide to change a shipped workflow. For a new matter type, start with [agent workflows](agent-workflows.md) and
[notation authoring](notation-authoring.md). The questionnaire and workflow composition are the tested contract; the
template body is replaceable. `features/` proves the flow even when a template body begins as placeholder prose.

## The four artifacts of one workflow

A workflow `code`, such as `nv__llc_formation`, has four synchronized artifacts:

1. **Template Markdown:** `templates/forms/...` or `templates/neon_law/<area>/...`; frontmatter carries metadata,
   `questionnaire`, and `workflow`, followed by document prose.
2. **Standalone spec:** `workflows/specs/<code>.yaml`; the same questionnaire and workflow, without prose. The
   scaffolder generates it and the runtime resolves it by code.
3. **Seed registration:** `store/src/seed.rs`; register the template in `mod canonical` and `seed_templates`, and update
   the asserted `templates_inserted` count.
4. **Bundled spec:** `workflows/src/specs.rs`; add `BUNDLED_SPEC_YAML` so the walker and journeys can resolve it.

Two DB-free tests enforce this:

- `workflows/tests/spec_coherence.rs`: standalone YAML and template frontmatter parse to the same spec.
- `workflows/tests/workflow_integrity.rs`: `BEGIN` and reachable `END`, declared transition targets, routable prefixes.

> A bundled but unseeded template parses yet cannot open: `start_notation` resolves it from the database. Check both
> registrations.

## Asking more questions

The `questionnaire:` block is a linear state machine of question codes walked one answer per request.

1. Add the state to the `questionnaire:` block in **both** the template frontmatter and the standalone spec, keeping the
   `_` chain intact (`BEGIN: { _: first }`, … `last: { _: END }`).
2. Each state prefix must be a seeded question code. Reuse `store/seeds/Question.yaml`; `N104` rejects unknown codes.
3. When using a custom state, add a sibling `prompts:` map in both files with that English prompt key, for example
   `fundraising_activities: What are the fundraising activities?`. `N104` rejects custom states without this prompt
   entry. The custom family is `custom_text`, `custom_yes_no`, `custom_single_choice`, `custom_multiple_choice`,
   `custom_usd`, and `custom_datetime` — one-off primitives with no reusable question code
   (`custom_<type>__<prompt_key>`).
4. Reference answers as `{{question_code}}`, `{{type__role.field}}`, or `{{#for … }}`.

A questionnaire that reuses only seeded codes needs no other change. New codes are the only reason to touch the seed.

## Updating the template body

The Markdown body renders to HTML and through `pdf::render` to Typst. Two gotchas:

- **No `#` headings.** `#` starts code mode in Typst markup; use bold runs and prose, as letter and form bodies do.
- **Escape `$`.** A bare `$` opens math mode; write `\$5,000` so it renders as a literal dollar in both the HTML and
  the PDF.

Role-scoped signature placeholders (`{{client.signature}}`, `{{firm.signature}}`, `{{client.date}}`) expand after data
placeholders, so notation evaluation cannot collide with signature anchors. Replacing stub prose changes only the body.

## Changing the workflow composition

The `workflow:` block is a state machine whose state-name **prefix** selects the actor and side effect via
`workflows::step::step_kind_for`. The vocabulary you compose from:

| Prefix | `StepKind` | What it means |
| --- | --- | --- |
| `lawyer_review` | `LawyerReview` | a licensed attorney approves before the flow advances |
| `client_review` | `ClientReview` | the client signs off on an attorney-reviewed draft |
| `generate_pdf__*` | `GeneratePdf` | render + persist a PDF (the signal carries a `DocumentPayload`) |
| `document_intake__*` | `DocumentIntake` | ingest an uploaded document (carries an `IntakePayload`) |
| `sent_for_signature__*` | `System` | wait for the e-signature ceremony |
| `firm_signature__*` | `FirmSignature` | the firm signs — on the closing letter, this closes the matter |
| `mailroom_send` / `certified_mail__*` / `e_filing__*` / `filing__*` | submission kinds | record a `filings` row |

The suffix after `__` identifies this template's instance. Prefer a suffixed existing prefix; a new prefix means a new
engine capability, not a legal product.

Rules to hold when editing:

- **Add the prefix to `step_kind_for` first** if it is genuinely new, or `workflow_integrity` fails with "unrouted".
- **`lawyer_review` gates every government submission** (`N106` + `workflows::lawyer_review_precedes_submission`): no
  `filing__*` / `mailroom_send` / `e_filing__*` state may be reachable without first crossing a bare `lawyer_review`.
- **`END` must stay reachable** from `BEGIN`, and every branch target must be a declared state.

Feature files prove composition; Rust tests prove routing, payloads, side effects, and replay-safe durability.

### Two ways a workflow is driven

**Walker-driven (signed templates).** The admin walker at `/app/lawyer/notations/:id/step` auto-drives this exact shape
on questionnaire completion:

```text
intake_submitted → intake_persisted__<respondent> → <doc>_rendered → lawyer_review →
approved → generate_pdf__<doc>_pdf → pdf_persisted → sent_for_signature__pending
```

Walker-driven templates match this shape and may append a tail such as `filing__nv_sos` after `signature_received`.

**Worker-driven:** branching or differently shaped machines signal the runtime through `workflows::DispatchingRuntime`
in dev/tests and `workflows-service` in production. Journeys call `worker().signal(...)`.

## Pricing a matter

Nothing here prices or bills a matter. Every matter is priced bespoke: lawyers agree the figure with the client and
raise the invoice in Xero directly, which is where all accounting originates (see [Xero billing](xero-billing.md)). A
closing letter closes the matter and raises nothing.

## Verifying a change

Run the cheap structural tests first, then the journey that exercises the flow end to end:

```bash
cargo test -p workflows --test workflow_integrity --test spec_coherence
cargo test -p features --test <journey>          # e.g. nest_formation, legal_workflow_shapes
cargo run -p cli --quiet -- validate templates/forms/united_states/nevada/state/nv__llc_formation.md
```

Cucumber's `.run()` is non-fatal: failed or skipped scenarios can exit `0`. Require `N steps (N passed)` and scan for
`Step failed` and `Step skipped`; a drifted matcher can silently skip its assertion.

## Where the journeys live

`features/` carries product/surface journeys and grouped composition specs. Shared fixtures and drivers live in
`features/src/journey.rs`. The matching journey is both proof and worked example.

## Mutating a notation at runtime

At runtime, a notation is fillable from both sides, editable in the small, reviewed, then signed without a per-product
fork.

**Two-sided answers.** `answers.source` (`lawyer` | `client`) and `authored_by_person_id` identify the typist;
`answers.person_id` remains the respondent. These non-null, low-cardinality dimensions feed the nightly archive.

**Question audience.** `questions.audience` (`lawyer` | `client` | `both`, default `both`) marks which side sees a
question. It is data, not code — set it in `store/seeds/Question.yaml`, never branch per product.

**Client intake.** `GET/POST /app/projects/:project_code/intake/:notation_id` uses the ordinary cookie session and
project ACL, not another token. Clients edit client-facing answers without moving lawyer's runtime pointer;
latest-per-code wins at render. The walker emails this URL.

**Custom clauses.** `notation_clauses` stores ordered, analyzable per-matter prose. The lawyer editor writes rows and
`store::notation_clauses::splice` inserts them at `{{custom_clauses}}`.

**The review gate (non-negotiable).** Any custom content — a clause **or** a client-entered answer — forces the notation
back through `lawyer_review` before signature. `drive_post_questionnaire_workflow` parks at `lawyer_review` instead of
auto-approving; the attorney's approval is now two deliberate steps. `approve_send_post` renders the document **once**
and persists it, then parks the workflow at `generate_pdf__*` — it does not send. A separate command, `send_post`
(`navigator retainer send`), confirms that persisted PDF is present and then sends *that exact PDF*, so a real Restate
worker's render never races the send. The bytes the attorney approved are the bytes that get signed. The invariant is
locked structurally by `workflows::guardrail::lawyer_review_precedes_signature` (every engagement template is tested),
and behaviourally by the `mutable_intake_docusign` journey.

`features/tests/mutable_intake_docusign.rs` proves lawyer/client intake, clauses, attorney review, and client-then-firm
DocuSign delivery of the approved bytes.
