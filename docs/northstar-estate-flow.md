# Northstar — estate-plan flow

Northstar is one product at one flat fee — **$3,333, paid once, the same price for everyone**. A client buys their whole
estate plan from one recorded sitting. This document records the architecture: what Phase A shipped (the comment-only
review surface) and the design for Phase B (the recorded-sitting → draft pipeline).

The plan is exactly three documents: a **will**, a **revocable living trust**, and **health and financial directives**.
That is the whole product at the one price — there are no tiers and no add-on modules inside Northstar. Probate / estate
administration and a special-needs trust are out of scope; if a client needs one, it is a separate engagement, not a
Northstar variant.

The five client-visible steps:

1. **The sitting.** One unhurried, recorded conversation about the client's life, the people they love, and what they
   want to leave. This is the intake — structured, but it should feel like a conversation, not a form.
2. **Generation.** From the recording we prepare the documents: a will, a trust, and health and financial directives.
3. **Attorney review.** A licensed attorney reviews every generated document before the client sees a final draft.
4. **Online review (comment-only).** The client reads each document on the website and can comment but not edit. They
   take the time they need; nothing is final until they have seen it.
5. **Signing.** Documents go to e-signature (the existing DocuSign flow) once the client approves.

The firm publishes no per-service marketing page for Northstar; the offering is priced and engaged through `/contact`.

## Phase A — comment-only review surface (shipped)

A matter (notation) can produce several documents the client must read before signing: a will, a trust, and health and
financial directives. Each is one `review_document` row holding the attorney-reviewed draft as HTML, with a `status`
that gates client visibility:

- `draft` — generated, not yet attorney-approved. **Hidden from the client** (the human-in-the-loop gate).
- `pending_review` — an attorney has advanced it. The client may read and comment.
- `approved` — the client has signed off; ready for signature.

The client reads one draft at `/app/projects/:project_code/review/:doc_id` and leaves comments anchored to a text range.
The surface is read-only — a comment is the only thing the client writes. Comments live in `document_comment`, each
carrying a character-offset range (`anchor_start`/`anchor_end`) into the document text, the `quoted_text` it covered,
the comment `body`, and a `resolved` flag lawyer flip once addressed. Comments anchor to a specific `review_document`
row, never to the bare notation, so the will's thread and the trust's thread stay separate.

### Review viewer

The read surface is a first-party custom element, `<northstar-review>` (`server/public/js/northstar-review.js`). It
upgrades a server-rendered, read-only document into a select-text-and-comment surface using the browser's Selection API
(selected text and offsets) and the CSS Custom Highlight API (painting existing comment ranges) — no heavy editor
framework, no new vendored dependency. Comments round-trip as form-encoded POSTs, so the existing `/app` CSRF middleware
guards them. The document degrades to a plain, readable page with no JavaScript.

The anchor model is engine-independent: anchors are character offsets the read surface computes client-side. If we later
want a richer editor (for example TipTap/ProseMirror, which needs a JavaScript build step this Rust-first workspace does
not yet have), the viewer internals can be swapped without changing the server's comment contract.

### Authorization

The review surface is row-scoped exactly like the rest of `/app`. A request resolves `(project_id, doc_id)` to a
client-visible document only when, in order: the document exists, it belongs to a notation in that project, the caller
may see the project (`store::access::can_see_project`), and the draft has been advanced past `draft`. Any failure
returns `404`, never `403` — the document does not exist from that caller's perspective. See
[`access-model.md`](access-model.md).

## Phase B — recorded-sitting → draft pipeline

Phase B captures the recorded sitting and turns it into the attorney-reviewed drafts Phase A renders. It is
feature-first: the `.feature` (`features/tests/features/estate_intake.feature`) precedes the template, and the spec uses
only the Person / Entity / role nouns from the [glossary](glossary.md). Generation routes through the existing template
and workflow machinery; the workflow is hosted by `workflows-service` (one worker, no per-workflow pod). See
[`notation-authoring.md`](notation-authoring.md) and the `create-legal-workflow` recipe.

### Workflow shape: `onboarding__estate` (engine shipped)

The estate matter is one notation, `onboarding__estate`, driven by a workflow from the shared step library. The template
lives at `templates/neon_law/northstar/estate_plan.md` with the mirrored standalone spec at
`workflows/specs/onboarding__estate.yaml`; the shape is pinned by `workflows/tests/estate_intake_spec.rs` and
`features/tests/features/estate_intake.feature`. The states:

- `BEGIN --transcript_uploaded--> document_intake__transcript` — the sitting is recorded offline (phone voice memo,
  Zoom, in person) and transcribed offline by AIDA on the already-paid Google Gemini Enterprise (~$0 marginal cost — the
  access-to-justice lever); the transcript is then **uploaded** through the reusable `document_intake__*` step, which
  files it into the matter (`store::documents::ingest_bytes`). The upload is phone-friendly — a text paste, a file, or a
  link — never "scan a PDF". The shipped path does not require live speech-to-text or real-time streaming: a client may
  be on a park bench with no signal, and the sitting's gravity is the product, not a laptop running captions.
- `document_intake__transcript --transcript_ready--> extract__inputs` — the filed transcript advances to extraction.
- `extract__inputs --inputs_ready--> document_drafts__estate` — structured estate inputs are derived from the
  transcript and written as `answers` (source `extracted`): the testator's name, executor, successor trustee,
  guardianship nomination, residuary beneficiary, and the health-care and financial agents. System step; the extraction
  is done by an `EstateExtractor` seam (a deterministic stub today, AIDA/Gemini Enterprise later) — not a metered API.
- `document_drafts__estate --drafts_persisted--> lawyer_review` — the will, trust, and the health and financial
  directives are rendered from those answers into one `review_document` row each at `status = draft`. This is a System
  wait state driven by `web` (which renders the instrument bodies, the same way the retainer renders its document
  web-side), **not** a worker `generate_pdf` PDF dispatch — the artifact is per-instrument review HTML, not a PDF.
- `lawyer_review --approved--> client_review` (and `--rejected--> END`) — an attorney reviews every generated draft.
  **Required gate**: no client-facing auto-generated legal document without a human in the loop. On approval the
  attorney advances each `review_document` row from `draft` to `pending_review`.
- `client_review --client_approved--> sent_for_signature__pending` — the client reads and comments on each draft via the
  Phase A surface. The matter waits here until the client approves; their sign-off advances each row to `approved`.
- `sent_for_signature__pending --signature_received|signature_declined--> END` — approved documents go to the existing
  DocuSign flow (see the e-signature design), reusing the retainer's signature path; the inbound completion webhook
  advances the matter.

`client_review` is a new, reusable engine primitive (`workflows::StepKind::ClientReview`, a Respondent-driven step — the
demand-side mirror of `lawyer_review`): any matter that needs a comment-only client approval before signing can bind to
it. `document_intake__*` is the reusable provided-artifact step (`workflows::StepKind::DocumentIntake`) that the
transcript is the first instance of; `extract__*` is a System seam.

### Remaining integration (the live seams)

The engine — the workflow, the new step kinds, the template/questionnaire, and the deterministic test coverage — is in
place, and the live edges follow the workspace's seam pattern (the same way e-signature shipped `StubSignatureProvider`
first and swapped in DocuSign). Status:

- **Shipped** — estate-matter creation starts the `onboarding__estate` workflow. `POST /lawyer/retainers/new`
  with the estate template reuses the retainer's create plumbing but, detecting a `transcript_uploaded` edge out of
  `BEGIN`, starts the workflow machine and lands lawyer on the matter page (`portal::retainer_walk::start_post`).
- **Shipped** — the transcript-upload surface. The handler `POST /app/projects/:project_code/notations/:nid/transcript`
  (`portal::transcript_intake`) accepts text/file/link, files it via `document_intake__*`, and the lawyer matter page
  renders the phone-friendly upload form while the workflow is at `BEGIN` (`webapp::lawyer_project_detail`).
- **Shipped (stub)** — extraction behind `extract__inputs`. An `EstateExtractor` seam (`portal::estate`) maps the
  transcript onto `answers` (source `extracted`); a deterministic `StubEstateExtractor` ships now, AIDA/Gemini swaps in
  behind the same trait. A coverage report records which questions the sitting answered.
- **Shipped** — `document_drafts__estate` renders the will, trust, and the two directives from those answers into one
  `review_document` row each at `status = draft`, web-side via `store::review_documents::create`.
- **Shipped** — the review gates. The attorney releases the drafts (the `release-drafts` route under
  `/lawyer/notations/:id`), which fires `approved` (lawyer_review → client_review) and flips each draft to
  `pending_review`; the client then approves (the `approve-plan` route under `/app/projects/:project_code`), which fires
  `client_approved` (client_review → sent_for_signature__pending) and flips each draft to `approved`. Both are scoped to
  the matter (404 otherwise), with a dedicated embedded Rego rule for the client path; `portal::estate` enforces that
  all drafts are released and the client can approve only once.

### Recording — storage, retention, transcription

- **Storage.** The recording is stored through the `cloud::StorageService` seam — GCS in production (the
  `<project>-assets` bucket), the filesystem backend in dev — like every other client blob. Object-key convention:
  `projects/<project_id>/northstar/sitting-<recording_id>.<ext>`, so the recording lives under the matter's prefix and
  inherits the same project ACL that gates everything else in the matter.
- **Retention / deletion.** The recording is part of the privileged client file. It is kept for the life of the matter
  plus the firm's standard file-retention window, then deleted on a scheduled policy. The exact window is set by the
  signed engagement letter and the firm's file-retention policy, not by code; the workflow records the deletion when it
  runs. Retention is a legal decision, so it lives in the engagement letter — the pipeline only enforces whatever window
  that decision sets.
- **Transcription.** The shipped lane is offline-first. The sitting is recorded offline and transcribed by AIDA on the
  already-paid Google Gemini Enterprise (memory `project_gemini_enterprise_mcp`), keeping the data inside the same GCP
  trust boundary as the rest of client data (GCP-only, per the workspace cloud rule) at ~$0 marginal cost. That
  transcript is then uploaded through the reusable `document_intake__transcript` step and stored as a document; the
  recording stays the source of truth until the drafts are approved.

Live transcript coverage is a planned adjunct, not a replacement for the offline lane — and it is **off by default,
behind the `live-transcription` feature flag**, so a default Northstar sitting is unchanged unless a deployment opts in.
The generic design is [`live-inquiry-coverage.md`](live-inquiry-coverage.md): when enabled, Northstar can seed a Live
Inquiry Session from this Template's questionnaire, persist transcript segments immediately, and show lawyer which
estate-plan Inquiries still need follow-up.

## Legal and ethics constraints

These carry into the build and must not be designed away:

- **Recording consent.** California is a two-party-consent state — the sitting must capture explicit recorded consent at
  the start of the recording.
- **Attorney-in-the-loop.** Every generated document is attorney-reviewed before the client sees a final draft. Phase A
  enforces this with the `draft` → `pending_review` gate; the marketing copy already promises it.
- **Engagement letter governs.** Scope, the $3,333 fee, the out-of-scope rate, and disengagement terms are confirmed in
  a signed engagement letter before work begins.
- **Same product at one price.** No tiered or budget version of the plan — one plan, one fee. See the uniform flat
  pricing decision.

## Settled decisions

- **Scope** is fixed: will + revocable trust + health/financial directives only. Probate / administration and a
  special-needs trust are separate engagements, not Northstar variants (see the top of this document).
- **Recording** storage, retention, and the offline transcription lane are settled above: `cloud::StorageService` under
  the matter prefix, matter-life-plus-retention-window deletion governed by the engagement letter, and transcription by
  AIDA/Gemini Enterprise in-project, with the transcript uploaded through `document_intake__*`.
- **Review viewer** stays the first-party `<northstar-review>` element. TipTap was considered and declined to avoid a
  JavaScript build step; the anchor model is engine-independent, so a richer editor can be swapped in later (through
  `/council`, as a new front-end dependency) without changing the server's comment contract.

## Open — set at build time

- The exact retention window in the engagement letter (a firm-policy number, not a code constant).
- The recording-capture front end for the sitting (browser capture vs. an attorney-side upload of an externally made
  recording) — a Phase B implementation choice that does not change the workflow shape above.
