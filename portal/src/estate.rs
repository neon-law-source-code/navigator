//! Estate pipeline — transcript → answers → review drafts.
//!
//! After the recorded sitting's transcript is filed
//! (`document_intake__transcript`), the matter has to turn that
//! transcript into the attorney-reviewable drafts the Phase A surface
//! renders. This module drives that, web-side, the same way the retainer
//! renders its document web-side (`retainer_walk`):
//!
//!   transcript_ready → extract__inputs   (write `answers`, source `extracted`)
//!   inputs_ready     → document_drafts__estate (render instruments → review_documents)
//!   drafts_persisted → lawyer_review      (the attorney gate)
//!
//! Extraction is a seam: [`EstateExtractor`] maps a transcript onto the
//! estate question codes. [`StubEstateExtractor`] ships now (deterministic
//! `Label: value` scanning, ~$0); the AIDA/Gemini Enterprise extractor
//! swaps in behind the same trait later. Machine-proposed answers are
//! written with `source = extracted` so an attorney can see and correct
//! them before any draft leaves `draft` — the human-in-the-loop boundary.

use std::collections::BTreeMap;

use axum::extract::{Extension, Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use workflows::{MachineKind, StateMachineRuntime};

use crate::admin::AdminState;
use crate::session::SessionData;
use store::review_documents::{STATUS_DRAFT, STATUS_PENDING_REVIEW};

/// One estate instrument: the catalog code of its template stub and the
/// `review_documents.kind` its rendered draft is filed under. The two
/// directives are separate rows so the client can comment on each
/// independently; the review listing groups them under one heading.
struct Instrument {
    template_code: &'static str,
    kind: &'static str,
}

const ESTATE_INSTRUMENTS: &[Instrument] = &[
    Instrument {
        template_code: "instrument__will",
        kind: "will",
    },
    Instrument {
        template_code: "instrument__trust",
        kind: "trust",
    },
    Instrument {
        template_code: "instrument__directive_health",
        kind: "directive_health",
    },
    Instrument {
        template_code: "instrument__directive_financial",
        kind: "directive_financial",
    },
];

/// Maps a recorded sitting's transcript onto answers to the estate
/// question set. The seam the Gemini extractor swaps in behind.
pub trait EstateExtractor: Send + Sync {
    /// Return `(question_code, value)` pairs for whatever the transcript
    /// answers. Codes it can't find are simply absent (a coverage gap),
    /// never an error.
    fn extract(&self, transcript: &str) -> Vec<(String, String)>;
}

/// Deterministic, dependency-free extractor: scans the transcript for
/// `Label: value` segments (value runs to the next `.`, `;`, or newline),
/// one set of labels per estate question code. Good enough to drive the
/// full pipeline and the demo at ~$0; the real extractor is AIDA on the
/// already-paid Gemini Enterprise, behind the same trait.
pub struct StubEstateExtractor;

/// `(state_name, &[label aliases])`. The first label found wins. The state
/// name matches the typed glossary roles in the estate instrument bodies
/// and disambiguates the several roles that share one registry question.
const STUB_LABELS: &[(&str, &[&str])] = &[
    (
        "person__testator",
        &["testator", "full legal name", "my name is"],
    ),
    ("person__executor", &["executor"]),
    (
        "person__successor_trustee",
        &["successor trustee", "trustee"],
    ),
    ("person__guardian_for_minors", &["guardian"]),
    (
        "person__residuary_beneficiary",
        &["residuary beneficiary", "beneficiary"],
    ),
    (
        "person__healthcare_agent",
        &["health-care agent", "healthcare agent", "health care agent"],
    ),
    ("person__financial_agent", &["financial agent"]),
];

impl EstateExtractor for StubEstateExtractor {
    fn extract(&self, transcript: &str) -> Vec<(String, String)> {
        let lower = transcript.to_lowercase();
        let mut out = Vec::new();

        // Two-party-consent confirmation: any mention of consent.
        if lower.contains("consent") {
            out.push((
                "custom_yes_no__recording_consent".to_string(),
                "Yes".to_string(),
            ));
        }

        for (code, labels) in STUB_LABELS {
            if let Some(value) = labels
                .iter()
                .find_map(|label| value_after_label(&lower, transcript, label))
            {
                out.push(((*code).to_string(), value));
            }
        }
        out
    }
}

/// Find `label:` in `lower` (lowercased haystack), then slice the
/// corresponding span out of the original `transcript`, taking the text
/// after the colon up to the next sentence/segment break. Returns the
/// trimmed value if non-empty.
fn value_after_label(lower: &str, transcript: &str, label: &str) -> Option<String> {
    let needle = format!("{label}:");
    let at = lower.find(&needle)?;
    let start = at + needle.len();
    let tail = &transcript[start..];
    let end = tail.find(['.', ';', '\n']).unwrap_or(tail.len());
    let value = tail[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Which estate questions the sitting answered vs. left open. Surfaced so
/// lawyers know what to follow up on before releasing drafts to the client.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CoverageReport {
    /// Instrument fields the transcript answered (a non-empty value).
    pub answered: Vec<String>,
    /// Instrument fields the transcript left unanswered — the drafts
    /// render with a blank in their place until lawyers fill them.
    pub unanswered: Vec<String>,
}

/// Failure of the estate pipeline. The caller (the transcript handler)
/// logs this and still redirects: the transcript is already filed, so a
/// pipeline hiccup must not 500 the lawyer upload.
#[derive(Debug, thiserror::Error)]
pub enum EstatePipelineError {
    #[error("database: {0}")]
    Db(String),
    #[error("workflow runtime: {0}")]
    Runtime(#[from] workflows::WorkflowRuntimeError),
    #[error("template body: {0}")]
    TemplateBody(#[from] store::templates::TemplateBodyError),
    #[error("template: {0}")]
    Template(#[from] store::templates::TemplateError),
    #[error("notation {0} vanished mid-pipeline")]
    NotationMissing(Uuid),
    #[error("notation store: {0}")]
    Notation(#[from] store::notations::NotationError),
    #[error("review document: {0}")]
    ReviewDocument(#[from] store::review_documents::ReviewDocumentError),
}

impl From<String> for EstatePipelineError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// Persist each non-empty extracted field as an answer with
/// `source = extracted`, and return the placeholder map the drafts render
/// from. Both the bare `code` and its `.name` form are keyed, because a
/// template may reference either.
///
/// An extracted answer whose question code was never seeded is logged and
/// skipped rather than failing the pipeline: the sitting still produced the
/// other fields, and a missing catalog row is a seeding problem, not a
/// reason to lose the transcript.
async fn persist_extracted_answers(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    extracted: Vec<(String, String)>,
) -> Result<BTreeMap<String, String>, EstatePipelineError> {
    let mut answers: BTreeMap<String, String> = BTreeMap::new();
    for (code, value) in extracted {
        if value.trim().is_empty() {
            continue;
        }
        if !workflows::notation_session::record_extracted_answer(
            surreal,
            notation_id,
            &code,
            &value,
        )
        .await?
        {
            tracing::warn!(
                code = %code,
                "extracted answer for an unseeded question code — skipped"
            );
        }
        answers.insert(code.clone(), value.clone());
        answers.insert(format!("{code}.name"), value);
    }
    Ok(answers)
}

/// Drive the estate pipeline from a freshly-filed transcript through to
/// the attorney gate (`lawyer_review`), returning the coverage report.
pub async fn drive_estate_pipeline(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
    transcript: &str,
    extractor: &dyn EstateExtractor,
) -> Result<CoverageReport, EstatePipelineError> {
    // Guard that the notation still exists; batch coverage attributes each
    // extracted answer to the notation's bound respondent (resolved inside
    // `record_extracted_answer`).
    if store::notations::find_by_id(surreal, notation_id)
        .await?
        .is_none()
    {
        return Err(EstatePipelineError::NotationMissing(notation_id));
    }

    // transcript_ready → extract__inputs.
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "transcript_ready",
        None,
    )
    .await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;

    let answers =
        persist_extracted_answers(surreal, notation_id, extractor.extract(transcript)).await?;

    // inputs_ready → document_drafts__estate.
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "inputs_ready",
        None,
    )
    .await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;

    // Render each instrument from the answers into one review_documents
    // row at `draft` (hidden from the client until an attorney advances
    // it past draft). Track which fields the drafts needed and which the
    // sitting actually answered.
    let mut needed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for inst in ESTATE_INSTRUMENTS {
        let Some(template) = store::templates::resolve(surreal, None, inst.template_code).await?
        else {
            tracing::warn!(
                code = inst.template_code,
                "estate instrument template not seeded — skipping draft"
            );
            continue;
        };
        let body = store::templates::body(surreal, storage, &template).await?;
        for code in data_placeholders(&body) {
            needed.insert(code);
        }
        let rendered = views::markdown::render(&substitute(&body, &answers));
        // Idempotent on `(notation_id, kind)`: re-rendering a notation
        // (a corrected transcript, a re-answered question) must never
        // insert a sibling row alongside an earlier draft.
        store::review_documents::upsert_draft(
            surreal,
            &store::review_documents::NewReviewDocument {
                notation_id,
                kind: inst.kind,
                title: &template.title,
                body_html: &rendered,
            },
        )
        .await?;
    }

    // drafts_persisted → lawyer_review (the attorney gate).
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "drafts_persisted",
        None,
    )
    .await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;

    let mut report = CoverageReport::default();
    for code in needed {
        if answers.contains_key(&code) {
            report.answered.push(code);
        } else {
            report.unanswered.push(code);
        }
    }
    tracing::info!(
        %notation_id,
        answered = report.answered.len(),
        unanswered = ?report.unanswered,
        "estate pipeline: drafts rendered, coverage computed"
    );
    Ok(report)
}

/// Find the project's transcript-driven onboarding notation — the
/// transcript-driven estate matter. Data-driven, never a hard-coded template
/// code: a notation qualifies when its bound template's workflow has a
/// `transcript_uploaded` edge out of `BEGIN` (the signal the creation
/// flow, the transcript handler, and the matter page all key off).
pub async fn transcript_driven_notation(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
) -> Option<store::notations::Notation> {
    let notations = store::notations::list_by_project(surreal, project_id)
        .await
        .unwrap_or_default();
    for n in notations {
        let Some(t) = store::templates::find_by_id(surreal, n.template_id)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        let transcript_driven = workflows::catalog_spec_yaml(&t.code)
            .and_then(|yaml| workflows::workflow_spec_from_yaml(yaml).ok())
            .is_some_and(|spec| {
                spec.transitions_from(&workflows::StateName::begin())
                    .is_some_and(|tm| {
                        tm.lookup(crate::transcript_intake::TRANSCRIPT_UPLOADED)
                            .is_some()
                    })
            });
        if transcript_driven {
            return Some(n);
        }
    }
    None
}

/// `POST /app/projects/{project_code}/approve-plan` — the client approves the plan.
///
/// The mirror of the lawyer release: at `client_review`, the client (or a
/// lawyer/admin acting on the matter) fires `client_approved`, advancing
/// `client_review --client_approved--> sent_for_signature__pending` and
/// flipping every released draft from `pending_review` to `approved`.
///
/// The substantive gate embedded Rego policy can't see (no DB state) lives here and 404s
/// otherwise: the caller must see the matter, the matter must be at
/// `client_review`, and **every** draft must already be `pending_review`
/// (released by an attorney, and not already approved — approve only once).
/// Why a client's estate-plan approval failed. Shared by the client
/// `/app/projects/{project_code}/approve-plan` form and the
/// `/app/api/projects/{id}/approve-plan` door. Both callers collapse
/// `NotAuthorized` and `NothingToApprove` to a non-disclosing 404.
#[derive(Debug)]
pub enum ApproveEstatePlanError {
    /// The caller is not a client of the matter.
    NotAuthorized,
    /// No transcript-driven notation is parked at `client_review` with every
    /// released draft pending the client's review — nothing to approve.
    NothingToApprove,
    /// The `client_approved` workflow signal failed.
    Db(String),
}

impl std::fmt::Display for ApproveEstatePlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAuthorized => write!(f, "not a client of this matter"),
            Self::NothingToApprove => {
                write!(f, "no released estate plan awaiting the client's approval")
            }
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for ApproveEstatePlanError {}

/// The client approves their released estate plan: fire the `client_approved`
/// transition and flip every released draft from `pending_review` to `approved`.
/// The one command behind both the client approve-plan form and the REST door.
/// `person_id` is the acting client; the command enforces client-lens matter
/// access itself so both adapters share one gate.
pub async fn approve_estate_plan(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    person_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApproveEstatePlanError> {
    if !store::projects::can_access_as_client_in_surreal(surreal, Some(person_id), project_id)
        .await
        .unwrap_or(false)
    {
        return Err(ApproveEstatePlanError::NotAuthorized);
    }
    let notation_row = transcript_driven_notation(surreal, project_id)
        .await
        .ok_or(ApproveEstatePlanError::NothingToApprove)?;
    if notation_row.state != "client_review" {
        return Err(ApproveEstatePlanError::NothingToApprove);
    }
    // Approve only once every draft has been released to pending_review.
    let docs = store::review_documents::for_notation(surreal, notation_row.id)
        .await
        .unwrap_or_default();
    if docs.is_empty() || docs.iter().any(|d| d.status != STATUS_PENDING_REVIEW) {
        return Err(ApproveEstatePlanError::NothingToApprove);
    }
    let next = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_row.id,
        "client_approved",
        None,
    )
    .await
    .map_err(|e| ApproveEstatePlanError::Db(e.to_string()))?;
    if let Err(e) = sync_notation_state(surreal, notation_row.id, next.as_str()).await {
        tracing::warn!(error = %e, notation_id = %notation_row.id, "approve-plan: state sync failed");
    }
    // A status-flip failure after the signal is logged, not fatal: the plan is
    // approved and the flip is idempotent on retry.
    if let Err(e) = advance_drafts(
        surreal,
        notation_row.id,
        STATUS_PENDING_REVIEW,
        store::review_documents::STATUS_APPROVED,
    )
    .await
    {
        tracing::error!(error = %e, notation_id = %notation_row.id, "approve-plan: status flip failed");
    }
    Ok(())
}

pub async fn approve_plan_post(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return not_found();
    };

    let Some(person_id) = session.person_id else {
        return not_found();
    };
    match approve_estate_plan(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        person_id,
        project_id,
    )
    .await
    {
        Ok(()) => {
            Redirect::to(&crate::dioxus_app::project_show_path(&state.surreal, project_id).await)
                .into_response()
        }
        Err(ApproveEstatePlanError::NotAuthorized | ApproveEstatePlanError::NothingToApprove) => {
            not_found()
        }
        Err(e) => {
            tracing::error!(error = %e, %project_id, "approve-plan: failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// `POST /lawyer/notations/:id/release-drafts` — the attorney gate.
///
/// At `lawyer_review`, a lawyer disclosed to the matter approves the
/// generated drafts: this advances `lawyer_review --approved--> client_review`
/// and flips every `draft` instrument to `pending_review`, which is what
/// makes it visible to the client on the Phase A review surface. No
/// client-facing auto-generated legal document leaves `draft` without this
/// human step. Row-scoped: a non-participant (non-admin) gets `404`.
pub async fn release_drafts_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(notation_row) = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return not_found();
    };
    // Row-scope to the matter: admin bypasses, the lawyer must be disclosed.
    let Some(session) = session.as_deref() else {
        return not_found();
    };
    if !store::access::can_see_project_as_lawyer(
        &state.surreal,
        session.person_id,
        session.role,
        notation_row.project_id,
    )
    .await
    .unwrap_or(false)
    {
        return not_found();
    }

    match release_drafts(&state.surreal, state.workflow_runtime.as_ref(), notation_id).await {
        Ok(_) => {
            Redirect::to(&format!("/app/projects/{}", notation_row.project_id)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "release-drafts: approve signal failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Advance a notation from `lawyer_review` to `client_review` (the attorney
/// gate) and release every draft instrument to `pending_review`, which makes
/// it visible to the client on the review surface. Shared by the lawyer form
/// ([`release_drafts_post`]) and the REST door
/// (`POST /app/api/notations/{id}/release-drafts`). The workflow signal is the only
/// hard error; a state-sync or draft-flip hiccup is logged and not surfaced,
/// because the flips are idempotent and can be re-driven.
pub async fn release_drafts(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
) -> Result<workflows::StateName, workflows::WorkflowRuntimeError> {
    let next = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "approved",
        None,
    )
    .await?;
    if let Err(e) = sync_notation_state(surreal, notation_id, next.as_str()).await {
        tracing::warn!(error = %e, %notation_id, "release-drafts: state sync failed");
    }
    if let Err(e) = advance_drafts(surreal, notation_id, STATUS_DRAFT, STATUS_PENDING_REVIEW).await
    {
        tracing::error!(error = %e, %notation_id, "release-drafts: status flip failed");
    }
    Ok(next)
}

/// Flip every review document on the notation from `from` to `to`.
async fn advance_drafts(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    from: &str,
    to: &str,
) -> Result<(), store::review_documents::ReviewDocumentError> {
    let docs = store::review_documents::for_notation(surreal, notation_id).await?;
    for d in docs {
        if d.status == from {
            store::review_documents::set_status(surreal, d.id, to).await?;
        }
    }
    Ok(())
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}

/// Substitute `{{code}}` placeholders in a template body with answer
/// values, leaving an unanswered placeholder as a visible blank.
fn substitute(body: &str, answers: &BTreeMap<String, String>) -> String {
    let mut out = body.to_string();
    for code in data_placeholders(body) {
        let value = answers.get(&code).map_or("________", String::as_str);
        out = out.replace(&format!("{{{{{code}}}}}"), value);
    }
    out
}

/// Every `{{ … }}` data placeholder, skipping signature/date anchors.
fn data_placeholders(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        let token = after[..close].trim();
        if !token.is_empty() && !is_signature_anchor(token) && !out.iter().any(|c| c == token) {
            out.push(token.to_string());
        }
        rest = &after[close + 2..];
    }
    out
}

fn is_signature_anchor(token: &str) -> bool {
    let Some((signer, field)) = token.split_once('.') else {
        return false;
    };
    matches!(signer, "client" | "firm") && matches!(field, "signature" | "date" | "initials")
}

/// Mirror the runtime's resulting state onto the `notations` row.
async fn sync_notation_state(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    new_state: &str,
) -> Result<(), store::notations::NotationError> {
    store::notations::update_state(surreal, notation_id, new_state).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{data_placeholders, substitute, EstateExtractor, StubEstateExtractor};
    use std::collections::BTreeMap;

    #[test]
    fn stub_extracts_labelled_values_from_a_sentence_transcript() {
        let t = "Consent recorded. Executor: Aries. Successor trustee: Capricorn. \
                 Residuary beneficiary: Gemini.";
        let pairs = StubEstateExtractor.extract(t);
        let map: BTreeMap<_, _> = pairs.into_iter().collect();
        assert_eq!(
            map.get("custom_yes_no__recording_consent")
                .map(String::as_str),
            Some("Yes")
        );
        assert_eq!(
            map.get("person__executor").map(String::as_str),
            Some("Aries")
        );
        assert_eq!(
            map.get("person__successor_trustee").map(String::as_str),
            Some("Capricorn")
        );
        assert_eq!(
            map.get("person__residuary_beneficiary").map(String::as_str),
            Some("Gemini")
        );
        // Nothing said about a financial agent → absent (a coverage gap).
        assert!(!map.contains_key("person__financial_agent"));
    }

    #[test]
    fn substitute_fills_known_codes_and_blanks_the_rest() {
        let body = "Executor {{person__executor.name}} and agent {{person__financial_agent.name}}.";
        let mut answers = BTreeMap::new();
        answers.insert("person__executor.name".to_string(), "Aries".to_string());
        let out = substitute(body, &answers);
        assert!(out.contains("Executor Aries"));
        assert!(out.contains("________"));
        assert!(!out.contains("{{"));
    }

    #[test]
    fn data_placeholders_skips_signature_anchors() {
        let codes = data_placeholders("{{person__testator.name}} signs {{client.signature}} once.");
        assert_eq!(codes, vec!["person__testator.name".to_string()]);
    }
}
