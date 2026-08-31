//! `/app/projects/{project_code}/intake/:notation_id` — the client self-serve
//! intake surface (the magic link).
//!
//! A client answers (or confirms) the client-facing questions on a
//! notation, one per step, pre-filled with anything lawyer already entered
//! on their behalf. It is the demand-side mirror of the admin walker
//! (`portal::retainer_walk`): same notation, both authorships interleaved.
//!
//! Auth is the same cookie-session + row-scope every other `/app/*`
//! page uses — no second token scheme. A non-participant gets `404`, never
//! `403`. The client may edit only while the notation is *still in
//! intake*: once it has gone out for signature the answers are frozen, so
//! the page shows the "your part is done" landing instead of a form.
//!
//! Two routes:
//!
//! - `GET …/intake/:notation_id` — the current client step, or the
//!   completion landing.
//! - `POST …/intake/:notation_id` — save one answer (`source = client`)
//!   and advance.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use store::question_registry::{Cardinality, QuestionType};
use workflows::notation_session::{self, ClientIntakeStep};
use String;

use crate::admin::AdminState;
use crate::session::SessionData;

/// A notation past intake — gone out for signature or finished — no
/// longer takes client edits; its assembled bytes are being signed.
fn is_past_intake(state: &str) -> bool {
    state.starts_with("sent_for_signature") || state == workflows::StateName::END
}

/// Find the first notation on a client-visible matter whose client-facing
/// questionnaire still needs an answer. The matter visibility check is kept
/// here beside the intake route's authoritative check; the matter page only
/// receives a link after the same client-side participation predicate passes.
pub(crate) async fn pending_client_intake_for_project(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    session: &SessionData,
    project_id: Uuid,
) -> Result<Option<Uuid>, String> {
    if session.role != store::persons::Role::Client || session.person_id.is_none() {
        return Ok(None);
    }
    let visible =
        store::projects::can_access_as_client_in_surreal(surreal, session.person_id, project_id)
            .await
            .map_err(|e| e.to_string())?;
    if !visible {
        return Ok(None);
    }

    let notations = store::notations::list_by_project(surreal, project_id)
        .await
        .map_err(|e| e.to_string())?;
    for notation in notations {
        if is_past_intake(&notation.state) {
            continue;
        }
        let step = match notation_session::client_intake_step(surreal, Some(storage), notation.id)
            .await
        {
            // A matter can carry a document-only notation. It has no
            // client-facing questionnaire to continue, but that should not
            // make the matter page itself unavailable.
            Err(
                workflows::notation_session::NotationSessionError::TemplateHasNoQuestionnaire(_)
                | workflows::notation_session::NotationSessionError::QuestionNotSeeded(_),
            ) => {
                continue;
            }
            Err(error) => return Err(error.to_string()),
            Ok(step) => step,
        };
        if matches!(step, ClientIntakeStep::NeedsAnswer { .. }) {
            return Ok(Some(notation.id));
        }
    }
    Ok(None)
}

/// Seeded jurisdiction names a question's select offers, per the
/// registry's `jurisdiction_type_filter` (today: `country` questions).
/// Empty for every other `answer_type`, so callers can pass it
/// unconditionally.
pub(crate) async fn jurisdiction_option_names(
    surreal: &store::surreal::SurrealDb,
    answer_type: &str,
) -> Vec<String> {
    let Some(filter) =
        QuestionType::from_token(answer_type).and_then(|t| t.jurisdiction_type_filter())
    else {
        return Vec::new();
    };
    match store::jurisdictions::list_by_type(surreal, filter).await {
        Ok(rows) => rows.into_iter().map(|r| r.name).collect(),
        Err(e) => {
            tracing::error!(error = %e, answer_type, "intake: loading jurisdiction options failed");
            Vec::new()
        }
    }
}

/// One selectable existing row for a record/reference question — the
/// `{id, name}` a DB-backed picker offers to the site intake flow.
/// CLI walker numbers into a pick-list. `id` is what a picker selection
/// posts back (over the free-typed `value`), so the stored answer carries
/// the row it selected, not just its display string.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Candidate {
    pub id: uuid::Uuid,
    pub name: String,
}

/// The existing rows a record/reference **singular** question may pick
/// from. Global seed reference data (`country`/`jurisdiction`) lists every
/// seeded row; `person`/`entity` are **project-scoped** to the notation's
/// matter (the people on the matter, the matter's entity) — never every
/// row in the DB (see the `authorization-model` skill). Empty for free-text
/// primitives, aggregates, and any type with no candidate source.
///
/// Errors on a DB failure rather than returning an empty list: a swallowed
/// error would make a partial/empty candidate set look *authoritative*, and
/// the POST resolver would then reject a valid pick as off-list. Callers
/// choose the failure mode — the GET step tolerates it (empty display), the
/// POST resolver propagates it (a loud `500`, never a silent no-advance).
pub(crate) async fn reference_candidates(
    surreal: &store::surreal::SurrealDb,
    answer_type: &str,
    notation_id: uuid::Uuid,
) -> Result<Vec<Candidate>, CandidateError> {
    let Some(qt) = QuestionType::from_token(answer_type) else {
        return Ok(Vec::new());
    };
    if qt.cardinality() != Cardinality::Singular {
        return Ok(Vec::new());
    }
    match qt {
        QuestionType::Country | QuestionType::Jurisdiction => {
            Ok(jurisdiction_candidates(surreal, qt.jurisdiction_type_filter()).await?)
        }
        QuestionType::Person => project_person_candidates(surreal, notation_id).await,
        QuestionType::Entity => Ok(project_entity_candidates(surreal, notation_id).await?),
        // entity_type / product / project keep the free-text path
        // for now — no picker yet.
        _ => Ok(Vec::new()),
    }
}

/// Seeded jurisdictions, optionally narrowed to one `jurisdiction_type`
/// (`country`). Global reference data — no project scope. The table
/// lives in SurrealDB since ENG-20.
async fn jurisdiction_candidates(
    surreal: &store::surreal::SurrealDb,
    type_filter: Option<&str>,
) -> Result<Vec<Candidate>, store::jurisdictions::JurisdictionError> {
    let rows = match type_filter {
        Some(filter) => store::jurisdictions::list_by_type(surreal, filter).await?,
        None => store::jurisdictions::list_all(surreal).await?,
    };
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            id: r.id,
            name: r.name,
        })
        .collect())
}

/// The project a notation belongs to, for project-scoped candidate lists.
/// `Ok(None)` means the notation or its project row is genuinely absent;
/// a DB failure propagates so a transient error isn't read as "no scope".
async fn project_for_notation(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
) -> Result<Option<store::projects::Project>, CandidateError> {
    let Some(notation) = store::notations::find_by_id(surreal, notation_id).await? else {
        return Ok(None);
    };
    Ok(store::projects::find_by_id(surreal, notation.project_id).await?)
}

/// People on the notation's matter — everyone with a `person_project_roles`
/// row on the project, plus the client/lawyer DRIs (who may be named on the
/// project directly without a participation row). Project-scoped, so the
/// picker never offers a person from an unrelated matter. A failed role
/// lookup errors rather than truncating the list to just the DRIs.
/// A candidate lookup that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CandidateError {
    #[error("database: {0}")]
    Db(String),
    #[error(transparent)]
    Person(#[from] store::persons::PersonError),
    #[error(transparent)]
    Jurisdiction(#[from] store::jurisdictions::JurisdictionError),
    #[error(transparent)]
    Project(#[from] store::projects::ProjectStoreError),
    #[error(transparent)]
    Entity(#[from] store::entities::EntityError),
    #[error(transparent)]
    Notation(#[from] store::notations::NotationError),
}

impl From<String> for CandidateError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

async fn project_person_candidates(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
) -> Result<Vec<Candidate>, CandidateError> {
    let Some(project) = project_for_notation(surreal, notation_id).await? else {
        return Ok(Vec::new());
    };
    // Every person on the matter, DRIs included — they are membership rows
    // now, so there is nothing to union in separately.
    let ids: Vec<uuid::Uuid> = store::projects::participations_for_project(surreal, project.id)
        .await?
        .into_iter()
        .map(|role| role.person_id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(store::persons::find_by_ids(surreal, &ids)
        .await?
        .into_iter()
        .map(|p| Candidate {
            id: p.id,
            name: p.name,
        })
        .collect())
}

/// The matter's entity — the one legal Entity the notation's project is
/// bound to. Project-scoped: a single candidate (or none if it vanished).
async fn project_entity_candidates(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
) -> Result<Vec<Candidate>, CandidateError> {
    let Some(project) = project_for_notation(surreal, notation_id).await? else {
        return Ok(Vec::new());
    };
    Ok(store::entities::find_by_id(surreal, project.entity_id)
        .await?
        .map(|e| Candidate {
            id: e.id,
            name: e.name,
        })
        .into_iter()
        .collect())
}

/// Whether a submitted display name must match a seeded row from the
/// closed candidate list rather than being kept as free text. Only
/// `country` today: the browser `<select>` and the CLI pick-list both offer
/// the full seeded set, so an off-list value is a mistake to reject. Every
/// other reference (`jurisdiction`/`project`/…) and every record type keeps
/// the free-text path — a picker selection still resolves its id through the
/// posted `id` field regardless.
fn reference_requires_pick(answer_type: &str) -> bool {
    matches!(
        QuestionType::from_token(answer_type),
        Some(QuestionType::Country)
    )
}

/// How a submitted scalar answer to a record/reference question resolves.
pub(crate) enum ReferenceResolution {
    /// Store `value` (the row's display name), embedding `id` when the
    /// answer selected — or resolved to — an existing row.
    Resolved {
        value: String,
        id: Option<uuid::Uuid>,
    },
    /// A reference pick that names no in-scope row; the walk must not
    /// advance. Carries the message a re-rendered form shows.
    Rejected(&'static str),
}

/// Resolve a scalar POST body for one record/reference question. A picker
/// selection posts `id` (the CLI, and a future id-posting select); the
/// browser's `country` `<select>` still posts the display name as `value`,
/// which resolves to the same seeded row's id. A record type (`person`/
/// `entity`) with no `id` keeps the free-text create path. Both write sites
/// (lawyer [`crate::retainer_walk::step_post`] and client [`intake_save`])
/// share this so the `id`-in-envelope contract holds either side.
pub(crate) async fn resolve_reference_answer(
    surreal: &store::surreal::SurrealDb,
    answer_type: &str,
    notation_id: uuid::Uuid,
    body: &std::collections::BTreeMap<String, String>,
) -> Result<ReferenceResolution, CandidateError> {
    // Propagate a candidate-lookup failure as an error (the caller maps it to
    // a `500`) rather than validating a pick against a silently-empty list.
    let candidates = reference_candidates(surreal, answer_type, notation_id).await?;
    // A picker selection: `id` names the chosen row. Validate it against the
    // in-scope candidates so a hand-crafted POST can't smuggle an
    // out-of-scope id (an unrelated matter's person, a made-up uuid).
    if let Some(id_str) = body.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let Ok(id) = uuid::Uuid::parse_str(id_str) else {
            return Ok(ReferenceResolution::Rejected(
                "Choose an option from the list.",
            ));
        };
        return Ok(match candidates.iter().find(|c| c.id == id) {
            Some(c) => ReferenceResolution::Resolved {
                value: c.name.clone(),
                id: Some(c.id),
            },
            None => ReferenceResolution::Rejected("Choose an option from the list."),
        });
    }
    // No `id`: the display-name path. `value` names the row.
    let value = body.get("value").cloned().unwrap_or_default();
    if reference_requires_pick(answer_type) {
        // A `country` must match a seeded row; resolve its id from the name
        // and reject an off-list value, exactly as the `<select>` enforced —
        // now also storing the id.
        return Ok(match candidates.iter().find(|c| c.name == value) {
            Some(c) => ReferenceResolution::Resolved {
                value: c.name.clone(),
                id: Some(c.id),
            },
            None => ReferenceResolution::Rejected("Choose an option from the list."),
        });
    }
    // A record type may create a new row: free text, no resolved id.
    Ok(ReferenceResolution::Resolved { value, id: None })
}

/// Resolve `(project_id, notation_id)` to a notation the caller may see,
/// or a `404` response. Enforces, in order: the notation exists, it
/// belongs to *this* project, and the caller may see the project.
async fn visible_notation(
    surreal: &store::surreal::SurrealDb,
    session: &SessionData,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<store::notations::Notation, Response> {
    let notation = store::notations::find_by_id(surreal, notation_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(not_found)?;
    if notation.project_id != project_id {
        return Err(not_found());
    }
    let visible = store::projects::can_access_as_client_in_surreal(
        surreal,
        session.person_id,
        project_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, %project_id, %notation_id, "intake: client visibility lookup failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
    })?;
    if !visible {
        return Err(not_found());
    }
    Ok(notation)
}

/// The bound template's title, for the page chrome. Falls back to a
/// generic label if the template vanished.
async fn flow_label(surreal: &store::surreal::SurrealDb, template_id: Uuid) -> String {
    store::templates::find_by_id(surreal, template_id)
        .await
        .ok()
        .flatten()
        .map_or_else(|| "intake".to_string(), |t| t.title)
}

/// Resolve the client's current intake state for `GET
/// /app/projects/{project_code}/intake/:notation_id`, in the wasm-safe shape the Dioxus
/// page renders (#956 Phase 4).
///
/// The Dioxus route's pre-layer calls this and injects the result, because
/// resolving a step means calling `workflows::notation_session` and `webapp`
/// does not depend on `workflows`. `Err` is the response to return instead of
/// rendering — a `404` for an unknown or unauthorised notation (never a `403`,
/// so the page never confirms a matter exists), or a `500` on a read failure.
pub(crate) async fn resolve_intake_state(
    state: &AdminState,
    session: Option<&SessionData>,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<webapp::client_intake::IntakeState, Response> {
    let Some(session) = session else {
        return Err(not_found());
    };
    let notation = visible_notation(&state.surreal, session, project_id, notation_id).await?;
    let Some(project) = store::projects::find_by_id(&state.surreal, project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, %project_id, "intake: project lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        })?
    else {
        return Err(not_found());
    };
    let flow_label = flow_label(&state.surreal, notation.template_id).await;

    let step =
        notation_session::client_intake_step(&state.surreal, Some(&state.storage), notation_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, %notation_id, "intake: client_intake_step failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
            })?;

    // Frozen once the document has gone out for signature: the completion
    // landing, not an editable form.
    if is_past_intake(&notation.state) {
        return Ok(webapp::client_intake::IntakeState::Complete {
            project_code: project.code,
            flow_label,
            total: total_of(&step),
        });
    }

    Ok(match step {
        ClientIntakeStep::NeedsAnswer {
            question,
            prior_value,
            position,
            total,
        } => {
            let country_options =
                jurisdiction_option_names(&state.surreal, &question.answer_type).await;
            webapp::client_intake::IntakeState::NeedsAnswer(Box::new(
                webapp::client_intake::IntakeStepData {
                    project_id: project_id.to_string(),
                    project_code: project.code,
                    notation_id: notation_id.to_string(),
                    flow_label,
                    question_code: question.code,
                    question_prompt: question.prompt,
                    answer_type: question.answer_type,
                    prior_value: prior_value.unwrap_or_default(),
                    country_options,
                    position,
                    total,
                },
            ))
        }
        ClientIntakeStep::Complete { total } => webapp::client_intake::IntakeState::Complete {
            project_code: project.code,
            flow_label,
            total,
        },
    })
}

/// `POST /app/projects/{project_code}/intake/{notation_id}` — save one
/// client-sourced answer and advance. The body is the whole form: one
/// `value` field for scalar questions, or the `people_list` widget's
/// `p{row}_{part}` inputs assembled into a JSON answer.
pub async fn intake_save(
    State(state): State<AdminState>,
    Path((project_code, notation_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
    axum::Form(body): axum::Form<std::collections::BTreeMap<String, String>>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return not_found();
    };
    let notation = match visible_notation(&state.surreal, &session, project_id, notation_id).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    // An answer must be attributable to a person; an anonymous session
    // can't author one.
    let Some(person_id) = session.person_id else {
        return not_found();
    };
    let back = format!("/app/projects/{project_code}/intake/{notation_id}");
    // Frozen: bounce back to GET, which renders the completion landing.
    if is_past_intake(&notation.state) {
        return Redirect::to(&back).into_response();
    }

    // Re-derive which question the client is on so a stale or hand-crafted
    // POST can't write an answer out of order.
    let step = match notation_session::client_intake_step(
        &state.surreal,
        Some(&state.storage),
        notation_id,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "intake: client_intake_step failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };
    let ClientIntakeStep::NeedsAnswer { question, .. } = step else {
        // Already done — nothing to save.
        return Redirect::to(&back).into_response();
    };
    let (value, reference_id) = if store::question_registry::answer_type_is_aggregate(
        &question.answer_type,
    ) {
        (crate::people_list_answer::assemble(&body), None)
    } else {
        let resolved = match resolve_reference_answer(
            &state.surreal,
            &question.answer_type,
            notation_id,
            &body,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, %notation_id, "intake: resolve_reference_answer failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
            }
        };
        match resolved {
            ReferenceResolution::Resolved { value, id } => (value, id),
            // The page renders through Dioxus on the `GET`, so the rejection
            // redirects back to it (post/redirect/get) with the reason as the
            // `?error=` flash it reads. The answer is not recorded, so the same
            // question comes back with what the client already had.
            ReferenceResolution::Rejected(error) => {
                return Redirect::to(&format!(
                    "{back}?error={}",
                    crate::admin::encode_query_value(error)
                ))
                .into_response();
            }
        }
    };

    if let Err(e) = notation_session::record_client_answer_with_reference(
        &state.surreal,
        Some(&state.storage),
        notation_id,
        question.code.as_str(),
        value.as_str(),
        reference_id,
        person_id,
    )
    .await
    {
        tracing::error!(error = %e, %notation_id, "intake: record_client_answer failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }
    Redirect::to(&back).into_response()
}

/// Pull the `total` out of either step variant.
fn total_of(step: &ClientIntakeStep) -> usize {
    match step {
        ClientIntakeStep::NeedsAnswer { total, .. } | ClientIntakeStep::Complete { total } => {
            *total
        }
    }
}

/// The `404` the Dioxus intake route's pre-layer returns for an unknown or
/// unauthorised notation — the same page the handler returned, and never a
/// `403`, which would confirm the matter exists.
pub(crate) fn client_intake_not_found() -> Response {
    not_found()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}
