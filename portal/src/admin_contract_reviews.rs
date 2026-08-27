//! Attorney review surface for an inbound contract review —
//! `/lawyer/contract-reviews/:id`.
//!
//! After the web-side analysis ([`crate::contract_review_walk`]) opens a
//! `contract_reviews` row of machine-proposed findings, the matter parks at
//! `lawyer_review`. Here a licensed attorney (`lawyer` tier — `lawyer` includes
//! attorneys) acts on the review:
//!
//! - **edits and decides each finding** — `attorney_note`, `suggested_redline`,
//!   `severity`, and an explicit *accept* or *reject*. There is **no
//!   bulk-accept**: every save is a per-finding decision, and nothing is
//!   accepted until the attorney acts. Each decision is written to
//!   `notation_events` (the immutable audit trail) so the memo is provably
//!   attorney-reviewed;
//! - **edits the risk summary**;
//! - **approves** — only once *every* finding has been acted on. Approval
//!   assembles the review memo from the exact signed-off findings + risk
//!   summary + the load-bearing disclaimers, renders it to a PDF filed into
//!   the Project, and drives the workflow `approved` →
//!   `generate_pdf__review_memo` → `memo_rendered` → `END`;
//! - **rejects** — `lawyer_review --rejected--> END`, no memo.
//!
//! Authorization: the route lives under `/lawyer/*`, so embedded Rego's
//! `lawyer_tier` rule gates it; the handlers add a per-matter row scope (a
//! client role, or a lawyer not disclosed to the project, gets `404`).

use axum::extract::{Extension, Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use store::contract_reviews::{self, ContractReview, Finding, MACHINE_CONTRACT_REVIEW};
use store::notation_events::{append_event, TransitionRecord};
use store::persons::Role;
use store::playbooks::{self, Playbook, SEVERITY_HIGH, SEVERITY_LOW, SEVERITY_MEDIUM};
use workflows::{DocumentPayload, MachineKind, StateMachineRuntime};

use crate::admin::AdminState;
use crate::session::SessionData;

const FINDING_ACCEPTED: &str = "finding_accepted";
const FINDING_REJECTED: &str = "finding_rejected";

/// Storage-key convention for a review memo PDF.
#[must_use]
pub fn memo_storage_key(notation_id: Uuid) -> String {
    format!("notations/{notation_id}/review-memo.pdf")
}

/// `documents.kind` the rendered memo is filed under in the Project.
///
/// [`rules::kind::Kind::Memo`] — "an analytical work product (a review memo or
/// opinion)" — which is what this is. The value has to be a real `Kind` in the
/// asset lane, because [`store::documents::ingest_bytes`] refuses anything
/// else. What keeps the memo off the client's document list is its
/// `visibility`, not its kind.
const MEMO_KIND: &str = "memo";

// --- the loaded review + its matter ---------------------------------------

struct Loaded {
    review: ContractReview,
    notation: store::notations::Notation,
    playbook: Playbook,
}

/// Why a contract-review action failed. Shared by the lawyer review surface and
/// the `/app/api/contract-reviews/{id}/*` doors so both adapters agree.
#[derive(Debug)]
pub enum ReviewActionError {
    /// No such review, or the caller may not see its matter (non-disclosing).
    NotFoundOrScoped,
    /// The review or its notation is not at the gate this action needs.
    NotOpen,
    /// The finding index names no finding on this review.
    FindingNotFound,
    /// Approval blocked: not every finding has an accept/reject decision.
    FindingsUnacted,
    /// A store write or the workflow signal failed.
    Db(String),
}

impl std::fmt::Display for ReviewActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFoundOrScoped => write!(f, "contract review not found"),
            Self::NotOpen => write!(f, "contract review is not at the required gate"),
            Self::FindingNotFound => write!(f, "no such finding"),
            Self::FindingsUnacted => write!(
                f,
                "every finding must be accepted or rejected before the memo can be approved"
            ),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for ReviewActionError {}

/// Load the review, its notation, and its playbook, enforcing the per-matter row
/// scope from the caller's `(person_id, role)`. The command-level form of the
/// old `load_scoped`: a missing review, a client caller, or a lawyer not on the
/// matter all collapse to [`ReviewActionError::NotFoundOrScoped`].
async fn load_review_scoped(
    surreal: &store::surreal::SurrealDb,
    review_id: Uuid,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Loaded, ReviewActionError> {
    let review = contract_reviews::by_id(surreal, review_id)
        .await
        .ok()
        .flatten()
        .ok_or(ReviewActionError::NotFoundOrScoped)?;
    let notation = store::notations::find_by_id(surreal, review.notation_id)
        .await
        .ok()
        .flatten()
        .ok_or(ReviewActionError::NotFoundOrScoped)?;
    // A client never reaches a lawyer surface; a lawyer must be assigned to the
    // matter (admin bypasses in the lawyer-lens helper).
    if matches!(role, Role::Client) {
        return Err(ReviewActionError::NotFoundOrScoped);
    }
    if !store::access::can_see_project_as_lawyer(surreal, person_id, role, notation.project_id)
        .await
        .unwrap_or(false)
    {
        return Err(ReviewActionError::NotFoundOrScoped);
    }
    let playbook = playbooks::by_id(surreal, review.playbook_id)
        .await
        .ok()
        .flatten()
        .ok_or(ReviewActionError::NotFoundOrScoped)?;
    Ok(Loaded {
        review,
        notation,
        playbook,
    })
}

#[derive(Deserialize)]
pub struct FindingEdit {
    /// `accept` or `reject` — the submit button the attorney clicked.
    decision: String,
    severity: String,
    suggested_redline: String,
    attorney_note: String,
}

/// Record the attorney's edits and accept/reject decision on one finding. The
/// one command behind both the lawyer review form and the REST door.
#[allow(clippy::too_many_arguments)]
pub async fn save_review_finding(
    surreal: &store::surreal::SurrealDb,
    review_id: Uuid,
    idx: usize,
    person_id: Option<Uuid>,
    role: Role,
    accept: bool,
    severity: &str,
    suggested_redline: &str,
    attorney_note: &str,
) -> Result<(), ReviewActionError> {
    let loaded = load_review_scoped(surreal, review_id, person_id, role).await?;
    // Only an open (analyzed) review takes edits.
    if !matches!(
        loaded.review.status.as_str(),
        contract_reviews::STATUS_ANALYZED
    ) {
        return Err(ReviewActionError::NotOpen);
    }
    let mut findings = contract_reviews::findings_of(&loaded.review).unwrap_or_default();
    let Some(finding) = findings.get_mut(idx) else {
        return Err(ReviewActionError::FindingNotFound);
    };
    finding.accepted = accept;
    finding.attorney_note = non_empty(attorney_note);
    finding.suggested_redline = non_empty(suggested_redline);
    if is_severity(severity) {
        finding.severity = severity.to_lowercase();
    }
    let clause_ref = finding.clause_ref.clone();
    contract_reviews::update_findings(surreal, review_id, &findings)
        .await
        .map_err(|e| ReviewActionError::Db(e.to_string()))?;
    // Immutable per-finding attribution — who decided what, when.
    record_finding_decision(
        surreal,
        loaded.notation.id,
        idx,
        &clause_ref,
        accept,
        person_id,
    )
    .await;
    Ok(())
}

/// `POST /lawyer/contract-reviews/:id/findings/:idx` — save the edits
/// to one finding and record the accept / reject decision.
pub async fn save_finding(
    State(state): State<AdminState>,
    Path((review_id, idx)): Path<(Uuid, usize)>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<FindingEdit>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    match save_review_finding(
        &state.surreal,
        review_id,
        idx,
        session.person_id,
        session.role,
        input.decision == "accept",
        &input.severity,
        &input.suggested_redline,
        &input.attorney_note,
    )
    .await
    {
        // A closed review is a no-op back to the surface, not an error.
        Ok(()) | Err(ReviewActionError::NotOpen) => redirect_to(review_id),
        Err(ReviewActionError::NotFoundOrScoped | ReviewActionError::FindingNotFound) => {
            not_found()
        }
        Err(e) => {
            tracing::error!(error = %e, %review_id, idx, "save finding failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct SummaryEdit {
    risk_summary: String,
}

/// Edit a review's risk summary. The one command behind both the lawyer form and
/// the REST door.
pub async fn save_review_summary(
    surreal: &store::surreal::SurrealDb,
    review_id: Uuid,
    person_id: Option<Uuid>,
    role: Role,
    risk_summary: &str,
) -> Result<(), ReviewActionError> {
    load_review_scoped(surreal, review_id, person_id, role).await?;
    contract_reviews::update_risk_summary(surreal, review_id, risk_summary.trim())
        .await
        .map_err(|e| ReviewActionError::Db(e.to_string()))?;
    Ok(())
}

/// `POST /lawyer/contract-reviews/:id/summary` — edit the risk summary.
pub async fn save_summary(
    State(state): State<AdminState>,
    Path(review_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<SummaryEdit>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    match save_review_summary(
        &state.surreal,
        review_id,
        session.person_id,
        session.role,
        &input.risk_summary,
    )
    .await
    {
        Ok(()) => redirect_to(review_id),
        Err(ReviewActionError::NotFoundOrScoped) => not_found(),
        Err(e) => {
            tracing::error!(error = %e, %review_id, "save risk summary failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Assemble + deliver the review memo and approve. The one command behind both
/// the lawyer approve control and the REST door. Refuses until every finding has
/// an accept/reject decision.
pub async fn approve_review(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    runtime: &dyn StateMachineRuntime,
    review_id: Uuid,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<(), ReviewActionError> {
    let loaded = load_review_scoped(surreal, review_id, person_id, role).await?;
    if loaded.notation.state != "lawyer_review" {
        return Err(ReviewActionError::NotOpen);
    }
    let findings = contract_reviews::findings_of(&loaded.review).unwrap_or_default();
    // Force per-finding action: every finding must have a recorded decision.
    let acted = contract_reviews::acted_finding_indices(surreal, loaded.notation.id)
        .await
        .unwrap_or_default();
    if (0..findings.len()).any(|i| !acted.contains(&i)) {
        return Err(ReviewActionError::FindingsUnacted);
    }
    deliver_memo(surreal, storage, runtime, &loaded, &findings)
        .await
        .map_err(|e| ReviewActionError::Db(e.to_string()))?;
    Ok(())
}

/// `POST /lawyer/contract-reviews/:id/approve` — assemble + deliver the
/// memo and approve.
pub async fn approve(
    State(state): State<AdminState>,
    Path(review_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    match approve_review(
        &state.surreal,
        &state.storage,
        state.workflow_runtime.as_ref(),
        review_id,
        session.person_id,
        session.role,
    )
    .await
    {
        Ok(()) | Err(ReviewActionError::NotOpen) => redirect_to(review_id),
        Err(ReviewActionError::NotFoundOrScoped) => not_found(),
        Err(ReviewActionError::FindingsUnacted) => Redirect::to(&format!(
            "/lawyer/contract-reviews/{review_id}?error={}",
            crate::admin::encode_query_value(
                "Every finding must be accepted or rejected before the memo can be approved."
            )
        ))
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %review_id, "approve / memo delivery failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Reject a review without a memo. The one command behind both the lawyer reject
/// control and the REST door.
pub async fn reject_review(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    review_id: Uuid,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<(), ReviewActionError> {
    let loaded = load_review_scoped(surreal, review_id, person_id, role).await?;
    if loaded.notation.state != "lawyer_review" {
        return Err(ReviewActionError::NotOpen);
    }
    let next = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        loaded.notation.id,
        "rejected",
        None,
    )
    .await
    .map_err(|e| ReviewActionError::Db(e.to_string()))?;
    let _ = sync_notation_state(surreal, loaded.notation.id, next.as_str()).await;
    let _ =
        contract_reviews::set_status(surreal, review_id, contract_reviews::STATUS_REJECTED).await;
    Ok(())
}

/// `POST /lawyer/contract-reviews/:id/reject` — reject without a memo.
pub async fn reject(
    State(state): State<AdminState>,
    Path(review_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    match reject_review(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        review_id,
        session.person_id,
        session.role,
    )
    .await
    {
        Ok(()) | Err(ReviewActionError::NotOpen) => redirect_to(review_id),
        Err(ReviewActionError::NotFoundOrScoped) => not_found(),
        Err(e) => {
            tracing::error!(error = %e, %review_id, "reject signal failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

// --- memo delivery ---------------------------------------------------------

/// Assemble the memo from the exact signed-off findings, render + file it into
/// the Project, and drive the workflow to `END`.
async fn deliver_memo(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    runtime: &dyn StateMachineRuntime,
    loaded: &Loaded,
    findings: &[Finding],
) -> anyhow::Result<()> {
    let notation_id = loaded.notation.id;
    let risk_summary = loaded.review.risk_summary.clone().unwrap_or_default();
    let accepted: Vec<&Finding> = findings.iter().filter(|f| f.accepted).collect();
    let typst_source = assemble_memo_typst(&MemoInput {
        playbook_name: &loaded.playbook.name,
        risk_summary: &risk_summary,
        accepted_findings: &accepted,
    });

    // Render web-side and file the PDF into the Project (a `documents` row +
    // git commit, the per-Project system of record). The worker also persists
    // it to the storage key on the `generate_pdf__review_memo` step below —
    // the two writes are idempotent (same Typst → same bytes).
    let bytes = pdf::render(&typst_source)?;
    let args = store::documents::IngestArgs {
        project_id: loaded.notation.project_id,
        source: store::documents::source::UPLOAD,
        filename: "review-memo.pdf",
        kind: MEMO_KIND,
        content_type: "application/pdf",
        description: Some("Inbound contract review memo"),
        secondary_storage_key: None,
        // Attorney work product — never listed in the client's documents (#782).
        visibility: store::documents::visibility::INTERNAL,
    };
    store::documents::ingest_bytes(surreal, storage, &args, &bytes).await?;

    // approved → generate_pdf__review_memo (worker renders + persists),
    // then memo_rendered → END.
    let payload = serde_json::to_string(&DocumentPayload::Typst {
        storage_key: memo_storage_key(notation_id),
        typst_source,
    })?;
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "approved",
        Some(&payload),
    )
    .await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;
    contract_reviews::set_status(surreal, loaded.review.id, contract_reviews::STATUS_APPROVED)
        .await?;
    let s = StateMachineRuntime::signal(
        runtime,
        MachineKind::Workflow,
        notation_id,
        "memo_rendered",
        None,
    )
    .await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;
    Ok(())
}

/// What the memo is assembled from.
pub struct MemoInput<'a> {
    pub playbook_name: &'a str,
    pub risk_summary: &'a str,
    pub accepted_findings: &'a [&'a Finding],
}

/// Assemble the review-memo Typst source from the signed-off findings + risk
/// summary + the load-bearing disclaimers (named playbook; not a full audit;
/// attorney accountable; zero-retention AI). Every dynamic value is inserted
/// as a Typst string literal (`#"…"`) so arbitrary attorney/finding text can
/// never break the markup.
#[must_use]
pub fn assemble_memo_typst(input: &MemoInput<'_>) -> String {
    let mut out = String::new();
    out.push_str("#set page(paper: \"us-letter\", margin: 1in)\n");
    out.push_str("#set text(size: 11pt)\n");
    out.push_str("#set par(justify: true)\n\n");
    out.push_str(
        "#align(center)[#text(size: 16pt, weight: \"bold\")[Inbound Contract Review Memo]]\n\n",
    );
    out.push_str("*Measured against playbook:* ");
    out.push_str(&typ_str(input.playbook_name));
    out.push_str("\n\n== Risk summary\n");
    out.push_str(&typ_str(input.risk_summary));
    out.push_str("\n\n== Findings\n");
    if input.accepted_findings.is_empty() {
        out.push_str(&typ_str(
            "No deviations were flagged for delivery against this playbook.",
        ));
        out.push('\n');
    } else {
        for f in input.accepted_findings {
            out.push_str("\n=== ");
            out.push_str(&typ_str(&f.clause_ref));
            out.push_str(" — ");
            out.push_str(&typ_str(&severity_label(&f.severity)));
            out.push_str("\n\n");
            out.push_str(&typ_str(&f.deviation));
            out.push_str("\n\n");
            if let Some(redline) = f.suggested_redline.as_deref().filter(|s| !s.is_empty()) {
                out.push_str("*Suggested redline:* ");
                out.push_str(&typ_str(redline));
                out.push_str("\n\n");
            }
            if let Some(note) = f.attorney_note.as_deref().filter(|s| !s.is_empty()) {
                out.push_str("*Attorney note:* ");
                out.push_str(&typ_str(note));
                out.push_str("\n\n");
            }
        }
    }
    out.push_str("\n== Scope and disclaimers\n");
    out.push_str("This memo measures the contract against the ");
    out.push_str(&typ_str(input.playbook_name));
    out.push_str(
        " playbook only — it is not a full audit. A clause this memo does not flag is not \
         thereby approved; silence means the clause was outside the playbook's scope. A \
         licensed Neon Law attorney has reviewed and is accountable for every finding above. \
         To produce the review, the contract text was processed through a zero-retention AI \
         service that is not trained on Company data; the contract and this memo are \
         confidential.\n",
    );
    out
}

/// Insert `s` as a Typst string-literal expression (`#"…"`), escaping the two
/// characters that are significant inside a Typst string. In content (markup)
/// context this displays the string's characters verbatim, with no markup
/// interpretation — so arbitrary text is safe.
fn typ_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("#\"{escaped}\"")
}

fn severity_label(severity: &str) -> String {
    match severity {
        SEVERITY_HIGH => "High severity".to_string(),
        SEVERITY_MEDIUM => "Medium severity".to_string(),
        SEVERITY_LOW => "Low severity".to_string(),
        other => other.to_string(),
    }
}

// --- attribution -----------------------------------------------------------

/// Append one immutable per-finding decision to `notation_events`.
async fn record_finding_decision(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    idx: usize,
    clause_ref: &str,
    accepted: bool,
    acting_person_id: Option<Uuid>,
) {
    let payload = serde_json::json!({
        "index": idx,
        "clause_ref": clause_ref,
        "accepted": accepted,
        "acting_person_id": acting_person_id,
    })
    .to_string();
    let condition = if accepted {
        FINDING_ACCEPTED
    } else {
        FINDING_REJECTED
    };
    let recorded_at = chrono::Utc::now().to_rfc3339();
    let event = TransitionRecord {
        notation_id,
        acting_person_id,
        machine_kind: MACHINE_CONTRACT_REVIEW,
        from_state: "lawyer_review",
        to_state: "lawyer_review",
        condition,
        payload_json: Some(payload),
        recorded_at: &recorded_at,
    };
    if let Err(e) = append_event(surreal, event).await {
        tracing::error!(error = %e, %notation_id, idx, "record finding decision failed");
    }
}

// --- small helpers ---------------------------------------------------------

fn redirect_to(review_id: Uuid) -> Response {
    Redirect::to(&format!("/lawyer/contract-reviews/{review_id}")).into_response()
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn is_severity(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        SEVERITY_LOW | SEVERITY_MEDIUM | SEVERITY_HIGH
    )
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}

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
    use super::{assemble_memo_typst, typ_str, MemoInput};
    use store::contract_reviews::Finding;
    use store::playbooks::SEVERITY_HIGH;

    fn finding(clause: &str, deviation: &str) -> Finding {
        Finding {
            clause_ref: clause.into(),
            deviation: deviation.into(),
            severity: SEVERITY_HIGH.into(),
            suggested_redline: Some("Add a mutual cap.".into()),
            attorney_note: Some("Push this.".into()),
            accepted: true,
        }
    }

    #[test]
    fn typ_str_escapes_quotes_and_backslashes() {
        assert_eq!(typ_str("a\"b\\c"), "#\"a\\\"b\\\\c\"");
    }

    #[test]
    fn memo_includes_playbook_summary_and_findings_and_disclaimers() {
        let f = finding("§7.2 Liability", "Liability is uncapped.");
        let refs = [&f];
        let typ = assemble_memo_typst(&MemoInput {
            playbook_name: "Vendor MSA",
            risk_summary: "One high-severity deviation.",
            accepted_findings: &refs,
        });
        assert!(typ.contains("Vendor MSA"));
        assert!(typ.contains("One high-severity deviation."));
        assert!(typ.contains("§7.2 Liability"));
        assert!(typ.contains("Suggested redline:"));
        assert!(typ.contains("not a full audit"));
        assert!(typ.contains("zero-retention AI"));
    }

    #[test]
    fn memo_with_markup_chars_renders_to_a_real_pdf() {
        // Arbitrary attorney text full of Typst metacharacters must not break
        // the compile — proving the `#"…"` insertion is safe.
        let f = Finding {
            clause_ref: "#1 *Indemnity* [draft]".into(),
            deviation: "Caps at $0; see § 9.1 _and_ <Exhibit A> @ref `code`.".into(),
            severity: SEVERITY_HIGH.into(),
            suggested_redline: Some("Replace with = mutual cap #here".into()),
            attorney_note: None,
            accepted: true,
        };
        let refs = [&f];
        let typ = assemble_memo_typst(&MemoInput {
            playbook_name: "Edge \"quoted\" \\ playbook",
            risk_summary: "Summary with # and * and $.",
            accepted_findings: &refs,
        });
        let pdf = pdf::render(&typ).expect("memo Typst compiles to a PDF");
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn memo_with_no_accepted_findings_states_so() {
        let typ = assemble_memo_typst(&MemoInput {
            playbook_name: "P",
            risk_summary: "Nothing material.",
            accepted_findings: &[],
        });
        assert!(typ.contains("No deviations were flagged"));
        assert!(pdf::render(&typ).is_ok());
    }
}
