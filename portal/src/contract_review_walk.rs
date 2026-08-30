//! Inbound contract review — the web-driven upload + analysis pipeline.
//!
//! A client (or lawyer acting for the client) uploads a third-party contract
//! into an existing Project; this module then:
//!
//!   1. opens a `memo__contract_review` notation and files the contract
//!      through the workflow — `contract_uploaded` lands on
//!      `document_intake__inbound_contract`, and the worker writes the
//!      content-addressed blob + `documents` row (the same
//!      [`store::documents::ingest_bytes`] seam the transcript intake uses);
//!   2. drives the analysis web-side — `intake_filed` lands on
//!      `analysis__contract_deviations`, where it loads the client Entity's
//!      [playbook](store::playbooks), runs the
//!      [`ContractReviewer`](crate::contract_review::ContractReviewer)
//!      against the contract text, opens a `contract_reviews` row and records
//!      the findings, then signals `analysis_ready` to land the matter at
//!      `lawyer_review` for an attorney.
//!
//! Analysis runs here, not in `workflows-service`, because the LLM seam
//! lives in `web` only — KIND and the tests use the deterministic
//! [`StubContractReviewer`](crate::contract_review::StubContractReviewer).
//! `web` drives the post-intake transitions once the provided artifact is
//! filed.
//!
//! Authorization: the upload route is row-scoped to the Project (a
//! non-participant gets `404`, never `403`); the admin review surface lives
//! under `/app/lawyer/*` and is gated by embedded Rego policy's `lawyer_tier` rule.

use std::sync::Arc;

use axum::extract::{Extension, Multipart, Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use tower_cookies::Cookies;
use uuid::Uuid;
use workflows::{IntakeArtifact, IntakePayload, MachineKind, StateMachineRuntime};

use store::playbooks::{self, Playbook};

use crate::admin::AdminState;
use crate::contract_review::ReviewError;
use crate::session::SessionData;

/// The condition that, fired from `BEGIN`, lands the contract-review
/// workflow on `document_intake__inbound_contract`. Kept in one place so the
/// upload handler and its test agree.
pub(crate) const CONTRACT_UPLOADED: &str = "contract_uploaded";
/// The condition out of `document_intake__inbound_contract` — the worker has
/// filed the blob; web now drives the analysis.
pub(crate) const INTAKE_FILED: &str = "intake_filed";
/// The condition out of `analysis__contract_deviations` — web has produced
/// the findings; the matter lands at `lawyer_review`.
pub(crate) const ANALYSIS_READY: &str = "analysis_ready";
/// Template code for the inbound-contract-review matter.
pub(crate) const CONTRACT_REVIEW_TEMPLATE_CODE: &str = "memo__contract_review";
/// `documents.kind` (and the `document_intake__<slug>` slug) for the filed
/// inbound contract.
pub(crate) const INBOUND_CONTRACT_KIND: &str = "inbound_contract";

/// Failure of the contract-review pipeline. The upload handler maps each to
/// an HTTP response.
#[derive(Debug, thiserror::Error)]
pub enum ContractReviewError {
    #[error("database: {0}")]
    Db(String),
    #[error("asset: {0}")]
    Asset(#[from] store::assets::AssetError),
    #[error("template: {0}")]
    Template(#[from] store::templates::TemplateError),
    #[error("workflow runtime: {0}")]
    Runtime(#[from] workflows::WorkflowRuntimeError),
    #[error("contract-review template not seeded")]
    TemplateMissing,
    #[error("workflow spec: {0}")]
    Spec(String),
    #[error("questionnaire snapshot: {0}")]
    Snapshot(String),
    #[error("serialize intake payload: {0}")]
    Payload(serde_json::Error),
    #[error("no active playbook on file for this Entity")]
    NoPlaybook,
    #[error("playbook positions malformed: {0}")]
    Positions(serde_json::Error),
    #[error("reviewer: {0}")]
    Reviewer(#[from] ReviewError),
    #[error("notation {0} vanished mid-pipeline")]
    NotationMissing(Uuid),
}

impl From<String> for ContractReviewError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// `POST /app/projects/{project_code}/contract-review` — upload an inbound contract
/// for playbook review.
///
/// Opens a `memo__contract_review` notation on the project, files the
/// uploaded contract, runs the deviation analysis against the client
/// Entity's playbook, and lands the matter at `lawyer_review`. Row-scoped: a
/// caller who can't see the project gets `404`.
pub async fn upload(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
    cookies: Cookies,
    session: Option<Extension<SessionData>>,
    mut multipart: Multipart,
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
    if !store::access::can_see_project(&state.surreal, Some(person_id), session.role, project_id)
        .await
        .unwrap_or(false)
    {
        return not_found();
    }

    let redirect_back = crate::dioxus_app::project_show_path(&state.surreal, project_id).await;

    // Verify the `_csrf` first field before reading the contract bytes —
    // the multipart equivalent of the middleware's form-field check.
    // Bearer callers carry no cookie and are exempt.
    if let Err(status) =
        crate::csrf::require_multipart_csrf(&cookies, &session, &mut multipart).await
    {
        return status.into_response();
    }

    // The contract bytes + text. We capture the text now (for analysis)
    // rather than reading the blob back, so the analysis never races the
    // worker's durable file write (mirrors the transcript path).
    let Some(form) = parse_form(multipart).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(artifact) = form.into_artifact() else {
        return Redirect::to(&redirect_back).into_response();
    };
    let contract_text = artifact.contract_text();
    let filename = artifact.default_filename();

    let deps = ReviewDeps {
        surreal: &state.surreal,
        workflow_runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        contract_reviewer: state.contract_reviewer.as_ref(),
    };
    match drive_contract_review(
        &deps,
        project_id,
        person_id,
        &filename,
        &contract_text,
        artifact.into_workflow_artifact(),
    )
    .await
    {
        Ok(review_id) => {
            Redirect::to(&format!("/app/lawyer/contract-reviews/{review_id}")).into_response()
        }
        Err(ContractReviewError::NoPlaybook) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "This Company has no contract-review playbook on file yet. An attorney must \
             create one under Admin → Playbooks before a contract can be reviewed.",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %project_id, "contract-review upload failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Open the notation, file the contract, run the analysis, and land at
/// `lawyer_review`. Returns the new `contract_reviews` row id.
///
/// Public so the integration tests can drive the pipeline without crafting
/// a multipart HTTP request.
///
/// # Errors
///
/// [`ContractReviewError`] when the Entity has no playbook, the template
/// isn't seeded, the reviewer fails, or a database/runtime call errors.
/// The dependencies the contract-review pipeline ([`drive_contract_review`])
/// borrows from whichever web state drives it — the lawyer/portal `AdminState`
/// or the REST `ApiState`. Decoupling the core from either concrete state lets
/// both doors run the deviation analysis identically.
pub struct ReviewDeps<'a> {
    pub surreal: &'a store::surreal::SurrealDb,
    pub workflow_runtime: &'a dyn StateMachineRuntime,
    pub storage: &'a Arc<dyn cloud::StorageService>,
    pub contract_reviewer: &'a dyn crate::contract_review::ContractReviewer,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn drive_contract_review(
    deps: &ReviewDeps<'_>,
    project_id: Uuid,
    person_id: Uuid,
    filename: &str,
    contract_text: &str,
    artifact: IntakeArtifact,
) -> Result<Uuid, ContractReviewError> {
    let runtime = deps.workflow_runtime;

    // The Entity (the Company) the contract is reviewed for, and its
    // playbook. Resolve the playbook first so we fail fast — before opening
    // a notation — when there is nothing to measure the contract against.
    let project_row = store::projects::find_by_id(deps.surreal, project_id)
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?
        .ok_or(ContractReviewError::NotationMissing(project_id))?;
    let entity_id = project_row.entity_id;
    let playbook_row = active_playbook(deps.surreal, entity_id)
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?
        .ok_or(ContractReviewError::NoPlaybook)?;
    let positions =
        playbooks::positions_of(&playbook_row).map_err(ContractReviewError::Positions)?;

    let (template_row, questionnaire_snapshot) =
        contract_review_template_and_snapshot(deps.surreal, deps.storage, project_id).await?;

    // Open the notation at BEGIN, bound to the client Entity.
    let new_notation = store::notations::NewNotation::new(
        template_row.id,
        person_id,
        project_id,
        workflows::StateName::BEGIN,
    )
    .with_entity(entity_id)
    .with_delivery(store::notations::DELIVERY_EMBEDDED)
    .with_questionnaire_snapshot(questionnaire_snapshot);
    let notation_id = store::notations::create(deps.surreal, &new_notation)
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?
        .id;

    // Start the workflow and file the contract: `contract_uploaded` lands on
    // `document_intake__inbound_contract`, whose document-intake side effect
    // writes the blob + `documents` row.
    let yaml = workflows::catalog_spec_yaml(&template_row.code)
        .ok_or(ContractReviewError::TemplateMissing)?;
    let spec = workflows::workflow_spec_from_yaml(yaml)
        .map_err(|e| ContractReviewError::Spec(e.to_string()))?;
    StateMachineRuntime::start(runtime, MachineKind::Workflow, notation_id, &spec).await?;

    let payload = IntakePayload {
        kind: INBOUND_CONTRACT_KIND.to_string(),
        filename: filename.to_string(),
        artifact,
    };
    let value = serde_json::to_string(&payload).map_err(ContractReviewError::Payload)?;
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        CONTRACT_UPLOADED,
        Some(&value),
    )
    .await?;
    sync_notation_state(deps.surreal, notation_id, s.as_str())
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?;

    // intake_filed → analysis__contract_deviations.
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        INTAKE_FILED,
        None,
    )
    .await?;
    sync_notation_state(deps.surreal, notation_id, s.as_str())
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?;

    // Run the deviation analysis web-side. The contract text was captured at
    // upload; the document id is a best-effort link (the worker's file write
    // is durable but may lag the analysis in prod).
    let report = deps
        .contract_reviewer
        .review(&playbook_row.name, &positions, contract_text)
        .await?;
    let asset_id = latest_document(deps.surreal, project_id, INBOUND_CONTRACT_KIND).await?;
    let review_id = store::contract_reviews::create(
        deps.surreal,
        &store::contract_reviews::NewContractReview {
            notation_id,
            playbook_id: playbook_row.id,
            asset_id,
        },
    )
    .await
    .map_err(|error| ContractReviewError::Db(error.to_string()))?;
    store::contract_reviews::record_analysis(
        deps.surreal,
        review_id,
        &report.risk_summary,
        &report.findings,
    )
    .await
    .map_err(|error| ContractReviewError::Db(error.to_string()))?;

    // analysis_ready → lawyer_review (the attorney gate).
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        ANALYSIS_READY,
        None,
    )
    .await?;
    sync_notation_state(deps.surreal, notation_id, s.as_str())
        .await
        .map_err(|error| ContractReviewError::Db(error.to_string()))?;

    tracing::info!(
        %notation_id, %review_id, findings = report.findings.len(),
        "contract review: analysis complete, parked at lawyer_review"
    );
    Ok(review_id)
}

async fn contract_review_template_and_snapshot(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    project_id: Uuid,
) -> Result<(store::templates::Template, serde_json::Value), ContractReviewError> {
    let template_row =
        store::templates::resolve(surreal, Some(project_id), CONTRACT_REVIEW_TEMPLATE_CODE)
            .await?
            .ok_or(ContractReviewError::TemplateMissing)?;
    let questionnaire_snapshot = workflows::notation_session::questionnaire_snapshot_for_template(
        surreal,
        Some(storage),
        &template_row,
    )
    .await
    .map_err(|e| ContractReviewError::Snapshot(e.to_string()))?;
    Ok((template_row, questionnaire_snapshot))
}

/// The most recent active playbook for an Entity, if any.
async fn active_playbook(
    surreal: &store::surreal::SurrealDb,
    entity_id: Uuid,
) -> Result<Option<Playbook>, store::playbooks::PlaybookError> {
    let mut rows = playbooks::for_entity(surreal, entity_id).await?;
    rows.retain(|p| p.active);
    // `for_entity` returns most-recent-first; keep that order.
    Ok(rows.into_iter().next())
}

/// The most recent document `assets` row of `kind` on the project — the
/// just-filed inbound contract. Best-effort: `None` when the worker's file
/// write hasn't landed yet (prod), in which case the review opens unlinked.
async fn latest_document(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    kind: &str,
) -> Result<Option<Uuid>, store::assets::AssetError> {
    Ok(store::assets::latest_of_kind(surreal, project_id, kind)
        .await?
        .map(|d| d.id))
}

/// Mirror the runtime's resulting state onto the `notations` row (the
/// runtime is the source of truth; the row is a convenience read).
async fn sync_notation_state(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    new_state: &str,
) -> Result<(), store::notations::NotationError> {
    store::notations::update_state(surreal, notation_id, new_state).await?;
    Ok(())
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}

// --- multipart capture ----------------------------------------------------
//
// The phone-friendly capture shape from the transcript intake, narrowed to
// what a contract upload needs: a file or a pasted text body.

/// One provided capture mode.
pub(crate) enum ParsedArtifact {
    Text(String),
    File {
        bytes: Vec<u8>,
        content_type: String,
        filename: String,
    },
}

impl ParsedArtifact {
    pub(crate) fn default_filename(&self) -> String {
        match self {
            Self::Text(_) => "inbound-contract.txt".to_string(),
            Self::File { filename, .. } => filename.clone(),
        }
    }

    /// The contract text the reviewer reads: a paste directly, or a file
    /// decoded as UTF-8. A non-UTF-8 file (a binary PDF) yields an empty
    /// string for now — PDF text extraction is a flagged follow-up; the
    /// blob is still filed and the attorney reviews against the playbook.
    pub(crate) fn contract_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::File { bytes, .. } => String::from_utf8(bytes.clone()).unwrap_or_default(),
        }
    }

    pub(crate) fn into_workflow_artifact(self) -> IntakeArtifact {
        match self {
            Self::Text(text) => IntakeArtifact::Text { text },
            Self::File {
                bytes,
                content_type,
                ..
            } => IntakeArtifact::File {
                bytes_base64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    bytes,
                ),
                content_type,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct ContractForm {
    text: Option<String>,
    file: Option<(Vec<u8>, String, String)>,
}

impl ContractForm {
    /// Prefer a file, then a text paste; blank fields are ignored.
    pub(crate) fn into_artifact(self) -> Option<ParsedArtifact> {
        if let Some((bytes, content_type, filename)) = self.file {
            if !bytes.is_empty() {
                return Some(ParsedArtifact::File {
                    bytes,
                    content_type,
                    filename,
                });
            }
        }
        if let Some(text) = self
            .text
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        {
            return Some(ParsedArtifact::Text(text));
        }
        None
    }
}

pub(crate) async fn parse_form(mut multipart: Multipart) -> Option<ContractForm> {
    let mut form = ContractForm::default();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return None,
        };
        match field.name().map(String::from).as_deref() {
            Some("contract_text") => form.text = field.text().await.ok(),
            Some("file") => {
                let filename = field
                    .file_name()
                    .map_or_else(|| "inbound-contract".to_string(), String::from);
                let content_type = field
                    .content_type()
                    .map_or_else(|| "application/octet-stream".to_string(), String::from);
                if let Ok(bytes) = field.bytes().await {
                    form.file = Some((bytes.to_vec(), content_type, filename));
                }
            }
            _ => {}
        }
    }
    Some(form)
}

#[cfg(test)]
mod tests {
    use super::{ContractForm, ParsedArtifact};

    #[test]
    fn file_capture_wins_over_text_and_decodes_utf8() {
        let form = ContractForm {
            text: Some("ignored paste".into()),
            file: Some((
                b"MASTER SERVICES AGREEMENT".to_vec(),
                "text/plain".into(),
                "msa.txt".into(),
            )),
        };
        let artifact = form.into_artifact().expect("a file is present");
        assert_eq!(artifact.default_filename(), "msa.txt");
        assert_eq!(artifact.contract_text(), "MASTER SERVICES AGREEMENT");
    }

    #[test]
    fn text_paste_is_used_when_no_file() {
        let form = ContractForm {
            text: Some("  pasted contract body  ".into()),
            file: None,
        };
        let artifact = form.into_artifact().expect("a paste is present");
        assert!(matches!(artifact, ParsedArtifact::Text(_)));
        assert_eq!(artifact.contract_text(), "pasted contract body");
    }

    #[test]
    fn binary_file_decodes_to_empty_text_but_still_files() {
        let form = ContractForm {
            text: None,
            file: Some((
                vec![0xff, 0xfe, 0x00],
                "application/pdf".into(),
                "c.pdf".into(),
            )),
        };
        let artifact = form.into_artifact().expect("a file is present");
        assert_eq!(artifact.contract_text(), "");
        assert_eq!(artifact.default_filename(), "c.pdf");
    }

    #[test]
    fn empty_form_yields_nothing() {
        assert!(ContractForm::default().into_artifact().is_none());
    }
}
