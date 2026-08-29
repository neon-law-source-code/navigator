//! Resolved step kinds derived from a state-name prefix.
//!
//! State-name prefixes (`BEGIN`, `END`, `lawyer_review`,
//! `notarization`, `firm_signature`, `mailroom_send`,
//! `mailroom_receive`) dispatch to the matching step kind here.

use serde::{Deserialize, Serialize};

use crate::spec::{ActorClass, StateName};

/// Concrete kinds of workflow step. Each kind binds a state-name
/// prefix to the actor that drives transitions out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// `BEGIN` / `END`, and explicit wait states (e.g.
    /// `sent_for_signature__pending`) — driven by the runtime or by
    /// an external webhook, never by a human in our UI.
    System,
    /// `lawyer_review*` — a lawyer approves / rejects.
    LawyerReview,
    /// `client_review*` — the client (respondent) reads an
    /// attorney-approved draft and approves it, the mirror of
    /// [`LawyerReview`]. The canonical case is the estate plan:
    /// after `lawyer_review` advances each generated instrument to
    /// `pending_review`, the client comments on and approves the drafts
    /// through the Phase A review surface, and that approval drives the
    /// matter to signing. Respondent-driven, like [`Signature`], but it
    /// records an approval rather than a signature — reusable by any
    /// matter that needs a comment-only client sign-off before signing.
    ClientReview,
    /// `notarization*` — the respondent signs or refuses in front
    /// of a notary.
    Notarization,
    /// `reask__*` — a `lawyer_review` that returns `changes_requested`
    /// parks the notation here so the flagged answers can be re-collected
    /// (the client self-serve, or lawyer on the client's behalf) before the
    /// matter returns to `lawyer_review`. A wait state with no worker side
    /// effect: the human who re-answers drives the `intake_resubmitted`
    /// edge back to review. This is what keeps a rejected review from
    /// dead-ending at `END` — `rejected` is reserved for a genuine
    /// withdrawal or decline. The respondent is the answering actor even
    /// when lawyers enter on their behalf, mirroring the questionnaire.
    Reask,
    /// Respondent-driven signing inside our UI. Matched by states
    /// ending in `_signature` / `_signatures`, or the literal
    /// prefix `witnesses` (a respondent's witnesses sign).
    Signature,
    /// `firm_signature*` — a Neon Law lawyer (the firm) signs an
    /// outbound document; the canonical case is the closing letter that
    /// ends a matter. This is the mirror of [`Signature`]: a matter
    /// opens on the *client's* signature (respondent-driven) and closes
    /// on the *firm's* (lawyer-driven), so the actor class here is Lawyer,
    /// not Respondent. A human act with no worker side effect — the
    /// signature is recorded on the journal, like `notarization`.
    FirmSignature,
    /// `mailroom_send*` — lawyer posts physical mail.
    MailroomSend,
    /// `mailroom_receive*` — lawyer logs physical mail received.
    MailroomReceive,
    /// `generate_pdf*` — runtime renders the template body into a
    /// blob and persists it via `cloud::StorageService`. No human in
    /// the loop; the worker advances out as soon as the blob lands.
    GeneratePdf,
    /// `document_intake__<slug>` — the inbound mirror of
    /// [`GeneratePdf`]: a human or agent *provides* an artifact (a
    /// transcript text, an executed PDF, an ID scan) and the worker
    /// files it into the matter — content-addressed blob + `documents`
    /// row via `store::documents::ingest_bytes`, the same write the
    /// e-sign / Drive / email intake lanes use. The artifact arrives
    /// threaded through the signal `value` (phone-friendly: text paste,
    /// file, or link), so the side effect is the worker's persist — like
    /// `generate_pdf`, the actor class is System. The slug names the
    /// instance (`document_intake__transcript` is the estate
    /// sitting's transcript); the step kind stays generic so future
    /// intakes reuse one state machine.
    DocumentIntake,
    /// `email_send*` — runtime renders an email template and posts it
    /// through the configured `EmailService` (SendGrid in prod,
    /// CapturingEmail in dev). No human in the loop; the worker
    /// advances out as soon as SendGrid 2xx's. Slug after the prefix
    /// (`email_send__welcome`) names a bundled email renderer — keeps the
    /// step kind generic so future flows (engagement signed,
    /// certified-mail mailed, etc.) reuse one state machine.
    EmailSend,
    /// `certified_mail*` — lawyer sends a document by USPS certified
    /// mail; the worker records the outbound submission durably in
    /// `filings` (proof of mailing). Reached only after `lawyer_review`.
    CertifiedMail,
    /// `e_filing*` — an electronic filing with a government office; the
    /// worker records the submission in `filings`. After `lawyer_review`.
    EFiling,
    /// `filing*` (e.g. `filing__nv_sos`) — a filing with a named
    /// government office; the worker records it in `filings`. After
    /// `lawyer_review`.
    Filing,
    /// `onchain__*` (e.g. `onchain__record_attestation`) — the worker
    /// records an on-chain attorney attestation (the Neon Law Node
    /// product): it hashes the attested document, writes the durable
    /// `attestations` row, and — when a real chain backend is configured
    /// — records the hash on Solana. The chain is isolated behind the
    /// `workflows::attest::Attestor` trait; the default `NullAttestor`
    /// records no transaction, leaving the row `pending`. System-driven
    /// (no human in the loop), like `generate_pdf`.
    OnChainRecord,
    /// `github_issue__*` — the worker opens a GitHub issue from the
    /// rendered `kind: github` notation, calling the GitHub REST API
    /// directly with `reqwest`. GitHub is isolated behind the
    /// `workflows::github::IssueOpener` trait; the default
    /// `NullIssueOpener` (no token configured) opens nothing and reports
    /// that it did, so a workflow can never claim an issue that does not
    /// exist. System-driven, like `generate_pdf`.
    ///
    /// This is the one step that is *not* a legal act — it belongs to the
    /// engineering intake shelf (`templates/github/`), which is why it
    /// never sits behind `lawyer_review`.
    GithubIssue,
}

impl StepKind {
    /// Actor class that drives transitions out of this step.
    #[must_use]
    pub const fn actor(&self) -> ActorClass {
        match self {
            Self::System
            | Self::GeneratePdf
            | Self::DocumentIntake
            | Self::EmailSend
            | Self::OnChainRecord
            | Self::GithubIssue => ActorClass::System,
            Self::LawyerReview
            | Self::FirmSignature
            | Self::MailroomSend
            | Self::MailroomReceive
            | Self::CertifiedMail
            | Self::EFiling
            | Self::Filing => ActorClass::Lawyer,
            Self::Notarization | Self::Signature | Self::ClientReview | Self::Reask => {
                ActorClass::Respondent
            }
        }
    }
}

/// Canonical `(state-name prefix → StepKind)` table. [`step_kind_for`]
/// consults this; the drift-guard test
/// (`drift_guard_every_step_prefix_is_documented`) asserts every prefix
/// here appears in the status table of `docs/notation-authoring.md`, so
/// adding a prefix to the engine forces a matching doc update — the
/// doc's status can't silently rot.
///
/// `BEGIN` / `END` are deliberately excluded: they are trivial
/// runtime-driven markers, not steps that appear in the status table.
/// The `_signature` / `_signatures` *suffix* rule is a separate
/// fall-through in [`step_kind_for`] (it matches an open-ended family
/// of respondent-signing states rather than one literal prefix); its
/// documentation token is the literal `_signature`, also covered below.
pub const STEP_PREFIXES: &[(&str, StepKind)] = &[
    // System wait / transient states.
    ("sent_for_signature", StepKind::System),
    ("intake_persisted", StepKind::System),
    // Human and worker-driven steps.
    ("lawyer_review", StepKind::LawyerReview),
    ("client_review", StepKind::ClientReview),
    // A rejected `lawyer_review` routes here to re-collect the flagged
    // answers, then loops back to review — no side effect, no dead end.
    ("reask", StepKind::Reask),
    ("notarization", StepKind::Notarization),
    // Estate pipeline. The recorded sitting is transcribed
    // *offline* — AIDA on the already-paid Google Gemini Enterprise turns
    // the recording into a transcript at ~$0 marginal cost — and the
    // transcript is then *uploaded* through the reusable document-intake
    // step (`document_intake__transcript`): the worker files it into the
    // matter, so this kind has a real side effect (unlike the old
    // `transcribe__*` STT seam it replaces). The structured estate inputs
    // are mined from that transcript (`extract__*`), again by AIDA/Gemini —
    // no metered API — so extraction stays a System seam advanced by the
    // extraction-complete signal, like the signature webhook advances
    // `sent_for_signature__pending`.
    ("document_intake", StepKind::DocumentIntake),
    ("extract", StepKind::System),
    // Inbound contract review (fractional-GC, the first review-IN matter).
    // The deviation analysis runs in `web` (Vertex Gemini via the
    // `ContractReviewer` seam — the worker has no LLM access), so
    // `analysis__*` is a System wait state the orchestrator drives, advanced
    // by the `analysis_ready` signal `web` sends once the findings are
    // persisted — exactly like `extract__*` is advanced by `inputs_ready`.
    ("analysis", StepKind::System),
    // The estate drafts are rendered into one `review_documents` row per
    // instrument by `web` (the same way the retainer renders its document
    // body web-side), not by a worker `generate_pdf` PDF dispatch — so
    // `document_drafts__*` is a System wait state the orchestrator drives,
    // never a dispatched step. (Contrast `generate_pdf`, which renders a
    // single PDF to storage and demands a payload.)
    ("document_drafts", StepKind::System),
    // `firm_signature` is a literal prefix, matched here before the
    // `_signature` *suffix* fall-through below — so the firm-signs case
    // resolves to Lawyer rather than collapsing into the respondent
    // Signature family (the same guard `sent_for_signature` relies on).
    ("firm_signature", StepKind::FirmSignature),
    ("mailroom_send", StepKind::MailroomSend),
    ("mailroom_receive", StepKind::MailroomReceive),
    ("generate_pdf", StepKind::GeneratePdf),
    ("email_send", StepKind::EmailSend),
    ("certified_mail", StepKind::CertifiedMail),
    ("e_filing", StepKind::EFiling),
    ("filing", StepKind::Filing),
    ("onchain", StepKind::OnChainRecord),
    // Engineering intake (`templates/github/`), not a legal act: the
    // worker opens a GitHub issue from the rendered notation.
    ("github_issue", StepKind::GithubIssue),
    ("witnesses", StepKind::Signature),
    // Documentation token for the `_signature` / `_signatures` suffix
    // family handled by the fall-through below.
    ("_signature", StepKind::Signature),
];

/// Resolve a state name to its step kind. Returns `None` for
/// unrecognized prefixes; the caller decides whether to treat that
/// as an error or skip the state.
#[must_use]
pub fn step_kind_for(state: &StateName) -> Option<StepKind> {
    let prefix = state.prefix();
    // BEGIN/END are runtime-driven markers.
    if prefix == "BEGIN" || prefix == "END" {
        return Some(StepKind::System);
    }
    // Literal-prefix table. The synthetic `_signature` documentation
    // token is skipped here (it is the suffix family, matched below) so
    // a state literally prefixed `_signature` doesn't short-circuit.
    if let Some((_, kind)) = STEP_PREFIXES
        .iter()
        .find(|(p, _)| *p == prefix && *p != "_signature")
    {
        return Some(*kind);
    }
    // Open-ended respondent-signing family: any state whose prefix ends
    // in `_signature` / `_signatures` (e.g. `testator_signature`).
    // Matched after the literal table so wait states like
    // `sent_for_signature` don't collapse into it.
    if prefix.ends_with("_signature") || prefix.ends_with("_signatures") {
        return Some(StepKind::Signature);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{step_kind_for, ActorClass, StepKind};
    use crate::spec::StateName;

    #[test]
    fn begin_and_end_resolve_to_system_step() {
        assert_eq!(step_kind_for(&StateName::begin()), Some(StepKind::System));
        assert_eq!(step_kind_for(&StateName::end()), Some(StepKind::System));
    }

    #[test]
    fn lawyer_review_prefix_resolves_regardless_of_discriminator() {
        assert_eq!(
            step_kind_for(&StateName::from("lawyer_review")),
            Some(StepKind::LawyerReview),
        );
        assert_eq!(
            step_kind_for(&StateName::from("lawyer_review__for_trustee")),
            Some(StepKind::LawyerReview),
        );
    }

    #[test]
    fn signature_states_resolve_to_signature_step() {
        // Will, Trust, LLC custom signature states — previously
        // unresolved (no StepKind), now match the `_signature` /
        // `_signatures` suffix or the `witnesses` literal.
        assert_eq!(
            step_kind_for(&StateName::from("testator_signature")),
            Some(StepKind::Signature),
        );
        assert_eq!(
            step_kind_for(&StateName::from("trustee_signature")),
            Some(StepKind::Signature),
        );
        assert_eq!(
            step_kind_for(&StateName::from("member_signatures")),
            Some(StepKind::Signature),
        );
        assert_eq!(
            step_kind_for(&StateName::from("witnesses")),
            Some(StepKind::Signature),
        );
    }

    #[test]
    fn firm_signature_states_resolve_to_a_lawyer_driven_step() {
        // The closing letter is signed by the firm, not the client:
        // `firm_signature__closing_letter` must resolve to the
        // Lawyer-driven FirmSignature kind — NOT collapse into the
        // respondent `_signature` suffix family even though the prefix
        // ends in `_signature` (the literal table wins, as it does for
        // `sent_for_signature`).
        assert_eq!(
            step_kind_for(&StateName::from("firm_signature")),
            Some(StepKind::FirmSignature),
        );
        assert_eq!(
            step_kind_for(&StateName::from("firm_signature__closing_letter")),
            Some(StepKind::FirmSignature),
        );
        assert_eq!(StepKind::FirmSignature.actor(), ActorClass::Lawyer);
    }

    #[test]
    fn intake_persisted_states_are_system_transient() {
        // `intake_persisted__client` is the retainer's transient
        // state between the questionnaire reaching END and the
        // post-intake workflow advancing past it. No human drives
        // the transition out — the rendering side effect does.
        assert_eq!(
            step_kind_for(&StateName::from("intake_persisted__client")),
            Some(StepKind::System),
        );
    }

    #[test]
    fn sent_for_signature_pending_is_system_wait_state() {
        // `sent_for_signature__pending` is the retainer's
        // wait-for-webhook state; the signature provider drives the
        // transition, not the respondent. Must NOT collapse into the
        // Signature step kind even though the prefix ends in
        // `_signature`.
        assert_eq!(
            step_kind_for(&StateName::from("sent_for_signature__pending")),
            Some(StepKind::System),
        );
    }

    #[test]
    fn each_step_kind_maps_to_expected_actor() {
        assert_eq!(StepKind::System.actor(), ActorClass::System);
        assert_eq!(StepKind::LawyerReview.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::ClientReview.actor(), ActorClass::Respondent);
        assert_eq!(StepKind::Reask.actor(), ActorClass::Respondent);
        assert_eq!(StepKind::Notarization.actor(), ActorClass::Respondent);
        assert_eq!(StepKind::Signature.actor(), ActorClass::Respondent);
        assert_eq!(StepKind::FirmSignature.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::MailroomSend.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::MailroomReceive.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::GeneratePdf.actor(), ActorClass::System);
        assert_eq!(StepKind::DocumentIntake.actor(), ActorClass::System);
        assert_eq!(StepKind::EmailSend.actor(), ActorClass::System);
        assert_eq!(StepKind::CertifiedMail.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::EFiling.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::Filing.actor(), ActorClass::Lawyer);
        assert_eq!(StepKind::OnChainRecord.actor(), ActorClass::System);
    }

    #[test]
    fn filing_prefixes_resolve_to_their_step_kinds() {
        assert_eq!(
            step_kind_for(&StateName::from("certified_mail")),
            Some(StepKind::CertifiedMail),
        );
        assert_eq!(
            step_kind_for(&StateName::from("e_filing__nv_sos")),
            Some(StepKind::EFiling),
        );
        assert_eq!(
            step_kind_for(&StateName::from("filing__nv_sos")),
            Some(StepKind::Filing),
        );
    }

    #[test]
    fn onchain_record_prefix_resolves_to_a_system_driven_step() {
        // `onchain__record_attestation` is the Neon Law Node on-chain
        // record step — System-driven (no human in the loop), like
        // `generate_pdf`. The chain itself lives behind the Attestor
        // trait; the step kind stays generic so a second chain reuses it.
        assert_eq!(
            step_kind_for(&StateName::from("onchain__record_attestation")),
            Some(StepKind::OnChainRecord),
        );
        assert_eq!(StepKind::OnChainRecord.actor(), ActorClass::System);
    }

    #[test]
    fn estate_pipeline_prefixes_resolve_to_their_step_kinds() {
        // The estate flow adds three states the runtime must
        // route: `client_review` (the respondent-driven approval mirror
        // of `lawyer_review`), the reusable `document_intake__transcript`
        // step (the uploaded transcript is filed into the matter), and
        // the System-driven `extract__*` seam. None may fall through to
        // `None`, or the workflow-integrity test rejects the template.
        assert_eq!(
            step_kind_for(&StateName::from("client_review")),
            Some(StepKind::ClientReview),
        );
        assert_eq!(StepKind::ClientReview.actor(), ActorClass::Respondent);
        assert_eq!(
            step_kind_for(&StateName::from("document_intake__transcript")),
            Some(StepKind::DocumentIntake),
        );
        assert_eq!(
            step_kind_for(&StateName::from("extract__inputs")),
            Some(StepKind::System),
        );
    }

    #[test]
    fn analysis_prefix_is_a_system_seam_web_drives() {
        // Inbound contract review: `analysis__contract_deviations` is a
        // System wait state — `web` runs the LLM analysis and advances it
        // with `analysis_ready`, the same shape as `extract__*`. The worker
        // never dispatches it (no LLM in the worker).
        assert_eq!(
            step_kind_for(&StateName::from("analysis__contract_deviations")),
            Some(StepKind::System),
        );
        assert_eq!(StepKind::System.actor(), ActorClass::System);
    }

    #[test]
    fn email_send_prefix_resolves_to_email_send_step() {
        // `email_send__welcome` is the worker's seam for sending a
        // welcome email after first-login signup. The slug after the
        // prefix names the template; the step kind itself stays
        // generic so future flows can reuse it.
        assert_eq!(
            step_kind_for(&StateName::from("email_send__welcome")),
            Some(StepKind::EmailSend),
        );
    }

    #[test]
    fn generate_pdf_prefix_resolves_to_generate_pdf_step() {
        assert_eq!(
            step_kind_for(&StateName::from("generate_pdf__retainer_pdf")),
            Some(StepKind::GeneratePdf),
        );
    }

    #[test]
    fn reask_prefix_resolves_to_a_respondent_driven_wait_step() {
        // A `lawyer_review` that returns `changes_requested` parks the
        // notation at `reask__client`; the flagged answers are re-collected
        // there before it loops back to review. Resolves to the Reask kind
        // (no worker side effect) and must not fall through to `None`, or
        // the workflow-integrity rule rejects the retainer template.
        assert_eq!(
            step_kind_for(&StateName::from("reask__client")),
            Some(StepKind::Reask),
        );
        assert_eq!(StepKind::Reask.actor(), ActorClass::Respondent);
    }

    #[test]
    fn unknown_prefix_returns_none() {
        assert!(step_kind_for(&StateName::from("payment__after_signing")).is_none());
    }

    #[test]
    fn retired_document_open_prefix_no_longer_resolves() {
        // The step was renamed `document_open` → `generate_pdf` and the
        // old prefix removed outright (no alias). A state still authored
        // with the retired prefix must resolve to `None` so the workflow
        // never dispatches an unknown step.
        assert!(step_kind_for(&StateName::from("document_open")).is_none());
        assert!(step_kind_for(&StateName::from("document_open__retainer_pdf")).is_none());
    }

    #[test]
    fn every_table_prefix_resolves_through_step_kind_for() {
        // The literal-prefix table and `step_kind_for` must agree:
        // every prefix in STEP_PREFIXES resolves to the kind it
        // declares (the `_signature` token resolves via the suffix
        // family). Keeps the table from drifting from the resolver.
        for (prefix, kind) in super::STEP_PREFIXES {
            let state = if *prefix == "_signature" {
                StateName::from("testator_signature")
            } else {
                StateName::from(*prefix)
            };
            assert_eq!(
                step_kind_for(&state),
                Some(*kind),
                "prefix {prefix:?} should resolve to {kind:?}"
            );
        }
    }

    #[test]
    fn drift_guard_every_step_prefix_is_documented() {
        // The promise in docs/notation-authoring.md: the status table
        // cannot silently rot. If `step_kind_for` gains a prefix
        // (added to STEP_PREFIXES) that the doc's status table never
        // mentions, this fails — forcing a doc update in the same
        // change. Direction is code → doc only; the doc may mention
        // more (e.g. not-yet-built prefixes) than the engine knows.
        let doc_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/notation-authoring.md");
        let doc = std::fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", doc_path.display()));
        for (prefix, _) in super::STEP_PREFIXES {
            assert!(
                doc.contains(*prefix),
                "step prefix {prefix:?} is recognized by step_kind_for but not mentioned in \
                 docs/notation-authoring.md — document it in the status table so the doc can't \
                 silently rot"
            );
        }
    }
}
