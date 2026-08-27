# Neon Law Nautilus — screening-dispute workflows (build index)

Nautilus is the firm's $66/month consumer-report screening shield. The compliance contract at
[`nautilus-design.md`](nautilus-design.md) has shipped; the firm publishes no per-service marketing page. This doc is
the engineering build index for the Restate-durable workflows that run it: an adverse-action notice or a forwarded
screening report comes to the firm and goes back out as an attorney-signed dispute letter under the client's FCRA
rights.

Read the compliance contract first — it is the source of truth for the scope boundary and the statutory hooks. This file
is the source of truth for *how the workflows are wired*. They are complementary: the design doc says what is allowed,
this index says how it is built.

## Shared context (applies to every numbered workflow)

- **Email engine (live in prod).** `parse.neonlaw.com` MX → SendGrid Inbound Parse → `/webhook/sendgrid/inbound` →
  `.eml` in GCS, then the `web` threading + relay path; outbound goes back through the same relay. The lawyer-reply
  `@approve` command is the attorney-approval gate — reuse it, never reinvent it.
- **One worker.** Every workflow binds onto the existing `workflows-service` Restate endpoint — one worker, never a
  per-workflow pod. This is idiomatic Restate: many handlers, one deployment.
- **Recipe.** Follow [`agent-workflows.md`](agent-workflows.md) — (1) `.feature` first, (2) template + questionnaire,
  (3) seeded questions, (4) workflow YAML from the shared step library, (5) Restate handlers. Use only Person / Entity /
  role nouns from [`glossary.md`](glossary.md).
- **Matter lifecycle.** A Nautilus engagement is a `projects` matter opened by `onboarding__` and closed by
  `offboarding__letter` when the representation ends.

## Guardrails (every outbound letter, every workflow)

These restate the compliance contract so a workflow PR cannot drift from it:

- A licensed attorney reviews and signs **every** outbound letter via the `@approve` gate — modeled in the spec as a
  `lawyer_review` state. No letter auto-sends (no UPL).
- The fee is a flat **$66/month** — never a percentage of anything, never contingent on a report changing.
- **No template, questionnaire, or copy markets credit repair or score improvement** — that would take on the Credit
  Repair Organizations Act analysis the product deliberately stays outside of. Nautilus disputes what is *inaccurate*.
- A lawsuit, a summons, or a viable FCRA damages claim is **litigation** → refer to litigation counsel (Sethi Legal),
  never answered as correspondence.

## The shared step chain

Every Nautilus letter is the same three-state spine drawn from the shared step library in
[`workflows::step`](../workflows/src/step.rs):

1. `generate_pdf__<letter>` — the runtime renders the letter template into a PDF blob and persists it via
   `cloud::StorageService`. No human in the loop yet.
2. `lawyer_review` — the attorney reads the rendered letter and approves or rejects it. This state **is** the `@approve`
   gate; it is the unauthorized-practice-of-law control.
3. `mailroom_send__<letter>` (or `email_send__<letter>`) — the runtime delivers the approved letter through the relay;
   the worker advances only on a 2xx.

The gate is enforced in code, not prose: `lawyer_review_gates_filing` in
[`workflows::guardrail`](../workflows/src/guardrail.rs) proves no path from a `generate_pdf__*` fill state reaches a
submission state without passing a `lawyer_review` state in between. Every Nautilus workflow spec inherits that
invariant, so an auto-send path fails the test rather than reaching a client.

## The template library

The dispute letter carries role-scoped signature anchors so the **attorney** signs, and it rides the step chain above.
It lands under `templates/neon_law/nautilus/` with a paired `workflows/specs/<code>.yaml` registered in
`workflows::specs::BUNDLED_SPEC_YAML` and pinned by `workflows/tests/spec_coherence.rs`:

- `fcra_dispute` — FCRA 15 U.S.C. §1681i — built in workflow 01. Disputes an inaccurate item on a tenant-screening,
  background-check, or other consumer report with the reporting agency.

The FCRA writing requirement makes the render-before-send chain load-bearing: a §1681i reinvestigation starts from a
**written** dispute. Rendering the letter to a durable blob before delivery is what puts the dispute in writing. The
next letter the sequence adds is a furnisher dispute (§1681s-2(b), workflow 04).

- 15 U.S.C. §1681i:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681i&num=0&edition=prelim>
- 15 U.S.C. §1681s-2:
  <https://uscode.house.gov/view.xhtml?req=granuleid:USC-prelim-title15-section1681s-2&num=0&edition=prelim>

Each anchored letter may also be recorded on-chain through Neon Law Node: a SHA-256 fingerprint of the signed letter on
Solana, bound to the firm and client wallets, so the client can prove the exact letter existed. Anchoring is optional
and adds only the network fee (see [`nautilus-design.md`](nautilus-design.md)).

## Build sequence

Build each workflow as one PR, in order — each declares its dependencies:

1. **01 — Intake & consumer-report dispute.** Onboard the client, sign the engagement letter, set the $66/mo billing,
   collect the disputed items, and render `fcra_dispute` for the reporting agency. Depends on nothing.
2. **02 — Inbound triage.** Classify each inbound `.eml` (adverse-action notice, forwarded report, agency
   reinvestigation result) against active matters and route it (`workflows::nautilus::triage`); the deadline-tracking
   spine (the §1681i 30-day reinvestigation timer and the §1681j(b) 60-day free-report window). Depends on workflow 01
   and the email engine.
3. **03 — Reinvestigation review.** The agency's §1681i response, classified corrected/deleted vs verified-unchanged
   (`workflows::nautilus::classify_fcra_result`), surfaced to the client and queued for attorney review. Depends on
   workflow 02.
4. **04 — Furnisher dispute.** The §1681s-2(b) furnisher dispute when the agency verifies an item the furnisher's
   records do not support. Depends on workflow 03.
5. **05 — Referral.** The lawsuit/summons or viable FCRA damages claim → litigation-counsel referral branch
   (`workflows::nautilus::litigation_referral`). Depends on workflow 02.

The client-facing UX contract (one-tap forward, a visible sent-letters timeline with the signing attorney and tracked
deadlines, the unmissable flat-fee trust line) lives in [`nautilus-design.md`](nautilus-design.md) and is built by
workflows 01–02.

Each PR ends with the standard pre-commit gate — `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, plus markdown lint for any `.md`. Tests land in the same commit as the implementation.
