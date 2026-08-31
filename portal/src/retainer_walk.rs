//! Stepwise retainer flow: create a Notation, walk the
//! questionnaire one question per request, then hand off to the
//! post-intake workflow.
//!
//! Routes:
//!
//! - `GET /app/lawyer/retainers/new` — the small "start a walk" form
//!   (template code + client email).
//! - `POST /app/lawyer/retainers/new` — find-or-insert the Person,
//!   insert Project + role + Notation in one txn, redirect to
//!   `/app/lawyer/notations/:id/step`.
//! - `GET /app/lawyer/notations/:id/step` — render the current
//!   question (read from the journal + spec) or redirect when the
//!   questionnaire reaches END.
//! - `POST /app/lawyer/notations/:id/step` — write the respondent's
//!   answer (`answers` row + journal entry), signal the runtime,
//!   and either redirect for the next question or — on END —
//!   drive the post-intake workflow.

use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use serde::Deserialize;

use crate::admin::AdminState;
use crate::session::SessionData;
use store::notations::document_pdf_storage_key;
use workflows::{
    notation_session, MachineKind, NextStep, NotationSessionError, SignalContext,
    StateMachineRuntime, StateName,
};

/// The Person a lawyer-driven workflow transition is attributed to: the
/// authenticated session Person, else the seeded firm principal
/// (`default_firm_dri`) — a real Person, never the notation's client and
/// never a sentinel. `None` only when the DB has no firm principal seeded
/// (an unseeded dev DB); the caller then signals context-less and the
/// store layer flags the missing actor.
pub(crate) async fn resolve_lawyer_actor(
    surreal: &store::surreal::SurrealDb,
    session: Option<&SessionData>,
) -> Option<Uuid> {
    if let Some(id) = session.and_then(|s| s.person_id) {
        return Some(id);
    }
    // No session-linked Person. In production this is unreachable: every
    // `/app/lawyer/*` route is gated on an authenticated, DB-linked lawyer Person
    // (the `persons.role` authz tier), so a real lawyer action always carries
    // its own `person_id`. Only the local dev auth-bypass (a session with no
    // linked Person) reaches here. Fall back to the seeded firm principal —
    // the same `default_firm_dri` convention the matter-open/self-serve
    // paths use for the lawyer DRI — so the transition is attributed
    // firm-side, never to the notation's client. Warn so the substitution is
    // never silent.
    let firm_dri = store::persons::default_firm_dri(surreal)
        .await
        .ok()
        .flatten();
    tracing::warn!(
        fallback_actor = ?firm_dri,
        "no session-linked Person for a lawyer workflow drive; attributing to \
         the firm principal (local dev auth-bypass) — a real lawyer session \
         records its own Person",
    );
    firm_dri
}

/// Fire a workflow-machine signal, attributing it to `actor` when one is
/// known. Every lawyer-driven transition (`intake_submitted`, the
/// `*_rendered` edge, `approved`, `pdf_persisted`, `close_requested`,
/// `signed`) MUST carry the authenticated actor so the `notation_events`
/// journal records who acted — the notation's client must never be
/// attributed a lawyer action by fallback (issue #252). A `None` actor (an
/// unseeded dev DB with no resolvable firm principal) degrades to a
/// context-less signal, which `store::entity::notation_event` flags.
async fn signal_workflow(
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
    condition: &str,
    payload: Option<&str>,
    actor: Option<Uuid>,
) -> Result<StateName, workflows::WorkflowRuntimeError> {
    match actor {
        Some(acting_person_id) => {
            runtime
                .signal_with_context(
                    MachineKind::Workflow,
                    notation_id,
                    condition,
                    payload,
                    SignalContext { acting_person_id },
                )
                .await
        }
        None => {
            runtime
                .signal(MachineKind::Workflow, notation_id, condition, payload)
                .await
        }
    }
}

/// POST body for `/app/lawyer/retainers/new`. Two fields — everything
/// else the walker collects.
#[derive(Debug, Clone, Deserialize)]
pub struct StartWalkBody {
    pub client_email: String,
    pub retainer_template_code: String,
}

/// Refuse a start: redirect back to the Dioxus form (post/redirect/get) with the
/// reason as `?error=` and the submitted values echoed, so nothing is retyped.
///
/// The form used to re-render inline from this `POST`. It now renders through
/// Dioxus at `GET /app/lawyer/retainers/new`, which reads exactly these three query
/// parameters back (#956 Phase 4).
fn refuse_start(body: &StartWalkBody, error: &str) -> Response {
    let mut query = String::new();
    crate::admin::push_query(&mut query, "error", error);
    crate::admin::push_query(&mut query, "client_email", body.client_email.trim());
    crate::admin::push_query(
        &mut query,
        "retainer_template_code",
        body.retainer_template_code.trim(),
    );
    Redirect::to(&format!("/app/lawyer/retainers/new?{query}")).into_response()
}

/// Remove a pending self-serve intake when the open will not complete.
/// There is no ambient transaction to roll back, so callers compensate
/// explicitly before they return an error or refusal after creating the
/// Project.
async fn discard_pending_intake_project(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
) -> Result<(), String> {
    let roles = store::projects::participations_for_project(surreal, project_id)
        .await
        .map_err(|error| error.to_string())?;
    for role in roles {
        store::projects::remove_participation(surreal, role.id)
            .await
            .map_err(|error| error.to_string())?;
    }
    // `link_retainer_rows` may have already opened the retainer Notation
    // before this abort — never shown to the client, never walked. The
    // project-delete guard below refuses to remove a project any real
    // notation still references, so this request's own aborted one must
    // go first, via the one narrow exception documented on
    // `store::notations::delete_uncommitted`.
    for notation in store::notations::list_by_project(surreal, project_id)
        .await
        .map_err(|error| error.to_string())?
    {
        store::notations::delete_uncommitted(surreal, notation.id)
            .await
            .map_err(|error| error.to_string())?;
    }
    store::projects::delete_project_with_surreal(surreal, project_id)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// POST `/app/lawyer/retainers/new` — create the four rows the
/// retainer lifecycle needs, then redirect to the walker.
#[allow(clippy::too_many_lines)]
pub async fn start_post(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Form(body): Form<StartWalkBody>,
) -> Response {
    let client_email = body.client_email.trim();
    let code = body.retainer_template_code.trim();

    if !client_email.contains('@') {
        return refuse_start(&body, "client email must contain an @");
    }
    if code.is_empty() {
        return refuse_start(&body, "choose an onboarding template");
    }

    // Resolved before the write opens: `templates` moved to
    // SurrealDB with ENG-121, so this read is on the other engine and a
    // miss should refuse the intake without having opened a transaction at
    // all.
    let template_row = match store::templates::resolve(&state.surreal, None, code).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return refuse_start(&body, "that onboarding template was not found");
        }
        Err(e) => {
            tracing::error!(error = %e, "start_post: template lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };

    let questionnaire_snapshot = match notation_session::questionnaire_snapshot_for_template(
        &state.surreal,
        Some(&state.storage),
        &template_row,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(e) => {
            tracing::error!(error = %e, "start_post: questionnaire snapshot failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };

    // `projects.entity_id` is NOT NULL, but a self-serve intake has no
    // lawyer to designate a pre-existing entity. Open the matter against
    // a fresh `Human` entity for this natural person.
    let entity_id = match create_human_entity(&state.surreal, client_email).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "start_post: human entity create failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };

    // A self-serve intake has no lawyer in the room, so the lawyer DRI falls
    // back to the seeded firm principal (`nick@neonlaw.com`) — a real person,
    // no sentinel. The client DRI is designated below, once
    // `link_retainer_rows` creates the self-serve client.
    let lawyer_dri_id = if let Some(id) = session.as_deref().and_then(|s| s.person_id) {
        id
    } else if let Ok(Some(id)) = store::persons::default_firm_dri(&state.surreal).await {
        id
    } else {
        tracing::error!("start_post: no lawyer DRI resolvable (unseeded db?)");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    };

    // The matter the walk opens is brand-new, so the project name is a
    // placeholder until the `project_name` question lands.
    let project = match store::projects::create(
        &state.surreal,
        &store::projects::NewProject {
            code: store::projects::code_from_name(
                &format!("(pending) {client_email}"),
                Uuid::now_v7(),
            ),
            name: format!("(pending) {client_email}"),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    {
        Ok(project) => project,
        Err(e) => {
            tracing::error!(error = %e, "start_post: project insert failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };
    let project_id = project.id;

    // Find-or-create the client, attach the `client` role, and create the
    // retainer Notation — the shared "hang a retainer on a matter" helper
    // the matter-open form (`crate::admin`) also calls. The walk collects
    // the client name later in the questionnaire and the client signs
    // *embedded* (the historical default), so name is `None` and delivery
    // is `embedded`.
    let rows = match link_retainer_rows(
        &state.surreal,
        template_row.id,
        project_id,
        client_email,
        None,
        store::notations::DELIVERY_EMBEDDED,
        Some(questionnaire_snapshot),
    )
    .await
    {
        Ok(rows) => rows,
        Err(resp) => {
            if let Err(error) = discard_pending_intake_project(&state.surreal, project_id).await {
                tracing::error!(error = %error, %project_id, "start_post: pending project cleanup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
            }
            return resp;
        }
    };
    let notation_id = rows.notation_id;

    // Conflict check on the self-serve client of record. A brand-new intake has
    // no relationships, but `link_retainer_rows` find-or-creates the client by
    // email, so an intake whose email matches a person already adverse to a
    // current client is caught here. A **blocking** conflict refuses the intake:
    // `discard_pending_intake_project` below undoes the whole intake (the
    // matter, the fresh entity, the client role, and the retainer notation —
    // every write here is its own immediate Surreal commit, so undoing it is
    // this handler's job, not a dropped transaction's). The form re-renders
    // with a **generic** message that
    // never discloses *why* — telling a self-serve visitor they are adverse to a
    // current client would breach that client's confidentiality. Soft
    // (non-blocking) findings proceed: unlike the lawyer / API / CLI doors there
    // is no attorney in the room to attest, so the walk's downstream
    // lawyer-review gate is where an attorney reviews the intake before the
    // retainer is finalized. (Whether self-serve intake is gated at all, and the
    // exact wording, are the matter-open self-serve policy points in #355.)
    match store::conflicts::check_new_matter(&state.surreal, rows.person_id, entity_id).await {
        Ok(report) if report.has_blocking() => {
            tracing::warn!(
                %project_id,
                "start_post: self-serve intake refused — adverse to a current client",
            );
            if let Err(error) = discard_pending_intake_project(&state.surreal, project_id).await {
                tracing::error!(error = %error, %project_id, "start_post: pending project cleanup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
            }
            return refuse_start(
                &body,
                "We're unable to start this intake online. \
                 Please contact our office to proceed.",
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %e, "start_post: conflict check failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    }

    // Move the client-DRI marker onto the self-serve client
    // `link_retainer_rows` just created. That call already wrote their
    // `client` participation row, so this flags the row in place — the
    // ledger and the accountability marker are now the same fact and cannot
    // drift the way the old column did.
    if let Err(e) = store::projects::designate_dri_in_surreal(
        &state.surreal,
        project_id,
        rows.person_id,
        store::projects::DriSide::Client,
    )
    .await
    {
        tracing::error!(error = %e, "start_post: client DRI designation failed");
        if let Err(error) = discard_pending_intake_project(&state.surreal, project_id).await {
            tracing::error!(error = %error, %project_id, "start_post: pending project cleanup failed");
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }

    // Designate the accountable lawyer resolved above — the opening lawyer,
    // or the firm principal on a self-serve walk with no lawyer in the room.
    // `can_see_project` 404s a lawyer who isn't on the matter, so without this
    // the matter would be invisible to the firm. Using the resolved value
    // rather than only the session person also closes the old gap where an
    // unauthenticated walk named a principal who had no membership row at all.
    if let Err(e) = store::projects::designate_dri_in_surreal(
        &state.surreal,
        project_id,
        lawyer_dri_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    {
        tracing::error!(error = %e, "start_post: lawyer DRI designation failed");
        if let Err(error) = discard_pending_intake_project(&state.surreal, project_id).await {
            tracing::error!(error = %error, %project_id, "start_post: pending project cleanup failed");
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }

    Redirect::to(&format!("/app/lawyer/notations/{notation_id}/step")).into_response()
}

/// The client Person + Notation a retainer-type matter hangs off of,
/// created by [`link_retainer_rows`].
pub(crate) struct RetainerRows {
    pub person_id: Uuid,
    pub notation_id: Uuid,
}

/// Create a fresh `Human` entity for a solo natural person, returning its
/// id. Used by the self-serve intake walk, which (unlike the admin
/// matter-open) has no lawyer to pick a pre-existing entity.
/// Find-or-creates the `Human` entity type and a jurisdiction so a
/// self-serve intake never fails on missing reference data (the
/// canonical seed normally supplies both; this is the fallback).
async fn create_human_entity(
    surreal: &store::surreal::SurrealDb,
    label: &str,
) -> anyhow::Result<Uuid> {
    // Every table this touches is Surreal-resident since ENG-120, so
    // nothing here runs inside the caller's write — which
    // it never did for the reference tables anyway.
    let type_id = store::entity_types::find_or_create(surreal, "Human")
        .await?
        .id;
    let jurisdiction_id = store::jurisdictions::find_or_create(
        surreal,
        &store::jurisdictions::NewJurisdiction::new("United States", "US", "country"),
    )
    .await?
    .id;
    Ok(store::entities::create(
        surreal,
        &store::entities::NewEntity {
            name: label.to_string(),
            entity_type_id: type_id,
            jurisdiction_id,
            phone: None,
            url: None,
            // A solo client's `Human` entity is never the firm anchor.
            firm_anchor_key: None,
        },
    )
    .await?
    .id)
}

/// Find-or-create the client Person by `client_email` (role `client`),
/// attach a `client` participation role to `project_id`, and create the
/// retainer Notation bound to `template_id` at `BEGIN` with the given
/// `delivery`.
///
/// The one code path for "hang a retainer on a matter," shared by the
/// standalone retainer walk ([`start_post`]) and the matter-open form
/// (`crate::admin::projects_create_lawyer_only`). The caller owns project
/// creation — the walk inserts a pending project, the matter-open form
/// already inserted the real one. A conflict short-circuits to the same
/// status responses the walk historically produced (returned as the
/// `Err` so the caller can `return` it and undo what this call already
/// committed — see `discard_pending_intake_project`).
#[allow(clippy::too_many_arguments)] // + the Surreal handle (#1093; ENG-19)
pub(crate) async fn link_retainer_rows(
    surreal: &store::surreal::SurrealDb,
    template_id: Uuid,
    project_id: Uuid,
    client_email: &str,
    client_name: Option<&str>,
    delivery: &str,
    questionnaire_snapshot: Option<serde_json::Value>,
) -> Result<RetainerRows, Response> {
    // A new client takes the name from the form when given (the
    // matter-open signer field), else the email as a stand-in — the walk
    // asks for the name later in the questionnaire. An existing client
    // keeps the name they already have.
    //
    // `find_or_create` rather than look-then-create: two lawyers opening a
    // matter for the same new client at once would otherwise leave the
    // slower one holding a unique-index violation, and there is no honest
    // way to report that. It is not "this email belongs to someone else"
    // — it is the very person being onboarded, created a moment ago.
    let name = client_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(client_email);
    let person_id = match store::persons::find_or_create(
        surreal,
        &store::persons::NewPerson::with_role(name, client_email, store::persons::Role::Client),
    )
    .await
    {
        Ok(person) => person.id,
        Err(e) => {
            tracing::error!(error = %e, "link_retainer_rows: person lookup failed");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response());
        }
    };

    if let Err(e) =
        store::projects::add_participation(surreal, project_id, person_id, "client").await
    {
        tracing::error!(error = %e, "link_retainer_rows: role insert failed");
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response());
    }

    let mut new_notation =
        store::notations::NewNotation::new(template_id, person_id, project_id, StateName::BEGIN)
            .with_delivery(delivery);
    if let Some(snapshot) = questionnaire_snapshot {
        new_notation = new_notation.with_questionnaire_snapshot(snapshot);
    }
    let notation_id = match store::notations::create(surreal, &new_notation).await {
        Ok(n) => n.id,
        Err(e) => {
            tracing::error!(error = %e, "link_retainer_rows: notation insert failed");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response());
        }
    };

    Ok(RetainerRows {
        person_id,
        notation_id,
    })
}

/// Template code for the firm-signed matter-close letter.
const CLOSING_TEMPLATE_CODE: &str = "offboarding__letter";

/// Why opening a matter's closing-letter notation failed. Shared by the lawyer
/// `/app/projects/{project_code}/close` form and the `/app/api/projects/{id}/close` door
/// so both adapters agree on the same refusals.
#[derive(Debug)]
pub enum CloseMatterError {
    /// No matter with that id.
    MatterNotFound,
    /// The `offboarding__letter` template is not seeded for this deployment.
    ClosingTemplateMissing,
    /// The matter has no client participant to address the letter to.
    NoClient,
    /// A read or write against the store failed.
    Db(String),
}

impl std::fmt::Display for CloseMatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MatterNotFound => write!(f, "matter not found"),
            Self::ClosingTemplateMissing => {
                write!(f, "the offboarding__letter template is not seeded")
            }
            Self::NoClient => write!(
                f,
                "this matter has no client to address the closing letter to"
            ),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for CloseMatterError {}

/// Open the firm-signed closing-letter notation on an existing matter, returning
/// the new notation's id. The one command behind both the lawyer close form and
/// the REST close door: it creates only the `offboarding__letter` Notation — bound
/// to the matter and addressed to the matter's client. The status flip to
/// `closed` is the close workflow's job when the walk completes (see
/// `workflows-service`), not this command's; this only starts the walk.
pub async fn open_closing_notation(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    project_id: Uuid,
) -> Result<Uuid, CloseMatterError> {
    let project_row = store::projects::find_by_id(surreal, project_id)
        .await
        .map_err(|e| CloseMatterError::Db(e.to_string()))?
        .ok_or(CloseMatterError::MatterNotFound)?;
    let template_row = store::templates::resolve(surreal, Some(project_id), CLOSING_TEMPLATE_CODE)
        .await
        .map_err(|e| CloseMatterError::Db(e.to_string()))?
        .ok_or(CloseMatterError::ClosingTemplateMissing)?;
    let questionnaire_snapshot = notation_session::questionnaire_snapshot_for_template(
        surreal,
        Some(storage),
        &template_row,
    )
    .await
    .map_err(|e| CloseMatterError::Db(e.to_string()))?;
    // The matter's client is the closing letter's respondent.
    let client_role = store::projects::participations_for_project(surreal, project_id)
        .await
        .map_err(|e| CloseMatterError::Db(e.to_string()))?
        .into_iter()
        .find(|role| role.participation == "client")
        .ok_or(CloseMatterError::NoClient)?;
    let notation = store::notations::create(
        surreal,
        &store::notations::NewNotation::new(
            template_row.id,
            client_role.person_id,
            project_id,
            StateName::BEGIN,
        )
        .with_entity(project_row.entity_id)
        .with_questionnaire_snapshot(questionnaire_snapshot),
    )
    .await
    .map_err(|e| CloseMatterError::Db(e.to_string()))?;
    Ok(notation.id)
}

/// POST `/app/projects/{project_code}/close` — open the closing-letter walk for an existing
/// matter, then redirect into the generic walker. A thin adapter over
/// [`open_closing_notation`], which the REST close door shares.
pub async fn close_matter_post(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
) -> Response {
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return (StatusCode::NOT_FOUND, "matter not found").into_response();
    };
    match open_closing_notation(&state.surreal, &state.storage, project_id).await {
        Ok(notation_id) => {
            Redirect::to(&format!("/app/lawyer/notations/{notation_id}/step")).into_response()
        }
        Err(CloseMatterError::MatterNotFound) => {
            (StatusCode::NOT_FOUND, "matter not found").into_response()
        }
        Err(CloseMatterError::NoClient) => (
            StatusCode::CONFLICT,
            "this matter has no client to address the closing letter to",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %project_id, "close_matter_post: failed to open closing notation");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// POST `/app/lawyer/notations/:id/send-intake` — hand the matter's
/// client their self-serve intake link.
///
/// The client signs into the portal and answers the client-facing
/// questions on this notation themselves (see [`crate::intake`]) — the
/// demand-side mirror of this admin walker. There is no second token
/// scheme: the link is gated by the same cookie-session + project ACL as
/// every other `/app/*` page, so this handler just ensures the client
/// carries a participation row for the matter (idempotent) and emails the
/// URL.
/// Typed failure of [`send_intake`].
#[derive(Debug, thiserror::Error)]
pub enum SendIntakeError {
    #[error("notation {0} not found")]
    NotationNotFound(Uuid),
    #[error("notation {0} has no client person")]
    NoClient(Uuid),
    #[error("database: {0}")]
    Db(String),
    #[error("person directory: {0}")]
    Person(#[from] store::persons::PersonError),
    #[error("notation: {0}")]
    Notation(#[from] store::notations::NotationError),
}

impl From<String> for SendIntakeError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// Hand the notation's client their self-serve intake magic link by email.
/// Shared by the lawyer form ([`send_intake_post`]) and the REST door
/// (`POST /app/api/notations/{id}/intake`). It ensures the client participates in
/// the matter (the link's backing), then dispatches the email. Email delivery
/// is best-effort: a send failure is logged, not surfaced — the link is
/// idempotent and can be re-sent — so a captured-but-undelivered email never
/// fails the command. Returns the recipient address.
pub(crate) async fn send_intake(
    surreal: &store::surreal::SurrealDb,
    email: &dyn crate::email::EmailService,
    notation_id: Uuid,
) -> Result<String, SendIntakeError> {
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await?
        .ok_or(SendIntakeError::NotationNotFound(notation_id))?;
    let client = store::persons::find_by_id(surreal, notation_row.person_id)
        .await?
        .ok_or(SendIntakeError::NoClient(notation_id))?;

    // Ensure the client can see the matter (they already should from
    // matter-open; this is the find-or-create that backs the magic link).
    let participation =
        store::projects::participation_for_person(surreal, client.id, notation_row.project_id)
            .await
            .map_err(|error| SendIntakeError::Db(error.to_string()))?;
    if participation.is_none() {
        store::projects::add_participation(surreal, notation_row.project_id, client.id, "client")
            .await
            .map_err(|error| SendIntakeError::Db(error.to_string()))?;
    }

    let base_url = workflows::email::base_url_from_env();
    let link = format!(
        "{base_url}/app/projects/{}/intake/{notation_id}",
        notation_row.project_id
    );
    let body = format!(
        "Your legal team has started your paperwork and needs you to confirm a few \
         details. Open your secure intake here and finish your part:\n\n{link}\n\n\
         Your answers save as you go, so you can stop and pick up where you left off. \
         Nothing is sent for signature until an attorney has reviewed it."
    );
    let html = workflows::email::render_email_html(&body, &base_url);
    let msg = crate::email::OutboundEmail::new(
        client.email.clone(),
        "Finish your Neon Law Navigator intake",
        body,
    )
    .with_html(html)
    .with_person(client.id.to_string());
    // `person_id`, never the address: an email identifies a client, and
    // telemetry leaves the firm's trust boundary (`telemetry/src/lib.rs`).
    // The id answers "which intake failed" just as well.
    if let Err(e) = email.send(msg).await {
        tracing::warn!(error = %e, %notation_id, person_id = %client.id, "send_intake: email send failed");
    } else {
        tracing::info!(%notation_id, person_id = %client.id, "send_intake: intake link sent");
    }

    Ok(client.email)
}

pub async fn send_intake_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
) -> Response {
    match send_intake(&state.surreal, state.email.as_ref(), notation_id).await {
        Ok(_) => Redirect::to(&format!("/app/lawyer/notations/{notation_id}/step")).into_response(),
        Err(SendIntakeError::NotationNotFound(_)) => {
            (StatusCode::NOT_FOUND, "notation not found").into_response()
        }
        Err(SendIntakeError::NoClient(_)) => {
            (StatusCode::CONFLICT, "notation has no client").into_response()
        }
        Err(SendIntakeError::Person(e)) => {
            tracing::error!(error = %e, "send_intake: person directory failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
        Err(SendIntakeError::Db(e)) => {
            tracing::error!(error = %e, %notation_id, "send_intake failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
        Err(SendIntakeError::Notation(e)) => {
            tracing::error!(error = %e, %notation_id, "send_intake failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// The latest stored answer for a `(notation, state, respondent)` — the walk's
/// pre-fill. Keyed on the full `state_name`, not just `question_id`: several
/// states share one registry question (the four retainer fields are all
/// `custom_text`), so keying on the question alone would bleed one field's
/// answer into another. Surfaces a batch-coverage proposal (`source =
/// extracted`) the same way, so the walk can offer it as a default to confirm.
async fn prior_answer_row(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    question_id: Uuid,
    state_name: &str,
    person_id: Uuid,
) -> Option<store::answers::Answer> {
    store::answers::latest_for_state(surreal, question_id, state_name, person_id, notation_id)
        .await
        .ok()
        .flatten()
}

/// Resolve the current walker step for `GET /app/lawyer/notations/:id/step`, in the
/// wasm-safe shape the Dioxus page renders (#956 Phase 4).
///
/// The Dioxus route's pre-layer calls this and either injects the result or
/// returns the `Response` this hands back instead of rendering:
///
/// - `?format=json` — the narrow machine surface the site intake flow
///   CLI walks. HTML scraping is brittle, so it stays a branch on this path.
/// - the questionnaire is complete — a redirect to `/app/lawyer`.
/// - the notation is gone — a `404`; a runtime failure — a `500`.
pub(crate) async fn resolve_walker_step(
    state: &AdminState,
    notation_id: Uuid,
    format: Option<&str>,
) -> Result<webapp::walker_step::WalkerStepData, Response> {
    // The runtime — not the journal — is the source of truth for
    // state; the worker writes `notation_events` rows via `ctx.run` as a
    // projection (see `docs/glossary.md` → `ctx.run`).
    // `notation_session::current_step` reads from the runtime and resolves the
    // question row in one call.
    let step = notation_session::current_step(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        Some(&state.storage),
        notation_id,
    )
    .await;

    if format == Some("json") {
        return Err(step_json(state, notation_id, step).await);
    }

    let question = match step {
        Ok(NextStep::NeedsAnswer { question }) => question,
        Ok(NextStep::QuestionnaireComplete) => {
            return Err(Redirect::to("/app/lawyer").into_response());
        }
        Err(NotationSessionError::NotationNotFound(_)) => {
            return Err((StatusCode::NOT_FOUND, "notation not found").into_response());
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "walker: current_step failed");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response());
        }
    };

    // Resolve the notation's bound template once. The walker is generic over any
    // notation, so both the prior-answer lookup (needs person_id) and the chrome
    // (title + progress total) must follow the actual template — not assume the
    // retainer.
    let notation_row = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten();
    let person_id = notation_row
        .as_ref()
        .map_or_else(Uuid::nil, |n| n.person_id);
    let template_row = match notation_row.as_ref() {
        Some(n) => store::templates::find_by_id(&state.surreal, n.template_id)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    // Pre-fill any prior answer for this (state, person) pair so navigating back
    // re-displays without mutating durable state.
    let prior_answer = prior_answer_row(
        &state.surreal,
        notation_id,
        question.id,
        &question.code,
        person_id,
    )
    .await
    .map(|a| store::answers::display_value(&a.value))
    .unwrap_or_default();

    // The chrome names the actual template (e.g. "Retainer Agreement",
    // "Closing Letter") rather than hard-coding the retainer.
    let flow_label = template_row
        .as_ref()
        .map_or("Retainer intake", |t| t.title.as_str())
        .to_string();
    let current_state = StateMachineRuntime::current_state(
        state.questionnaire_runtime.as_ref(),
        MachineKind::Questionnaire,
        notation_id,
    )
    .await
    .unwrap_or_else(StateName::begin);
    let (position, total) = walker_progress(state, notation_id, &current_state).await;
    tracing::info!(
        %notation_id,
        rendered_question = %question.code,
        current_state = %current_state.as_str(),
        progress_current = position,
        progress_total = total,
        "walker: rendering question",
    );

    let country_options =
        crate::intake::jurisdiction_option_names(&state.surreal, &question.answer_type).await;
    Ok(webapp::walker_step::WalkerStepData {
        notation_id: notation_id.to_string(),
        flow_label,
        question_code: question.code,
        question_prompt: question.prompt,
        answer_type: question.answer_type,
        prior_answer,
        country_options,
        position,
        total,
    })
}

/// Render the current questionnaire step as the JSON body the `navigator
/// site intake flow walks: either the next question (code,
/// prompt, `answer_type`, any `radio` choices, and — for a record/reference
/// question — the DB-backed `candidates` the CLI numbers into a pick-list)
/// or `complete: true` once the machine reaches END. A `people_list`
/// question's `choices` and `candidates` are both empty — the CLI assembles
/// its `p{row}_{part}` rows from `--person` flags / interactive prompts, not
/// from a fixed list.
async fn step_json(
    state: &AdminState,
    notation_id: Uuid,
    step: Result<NextStep, NotationSessionError>,
) -> Response {
    match step {
        Ok(NextStep::NeedsAnswer { question }) => {
            let choices: Vec<serde_json::Value> = question
                .choices
                .iter()
                .map(|choice| serde_json::json!({ "value": choice.value, "label": choice.label }))
                .collect();
            // The read-only step display tolerates a candidate-lookup
            // failure — show an empty list rather than failing the whole
            // step; the POST resolver is where a lookup error must be loud.
            let candidates = crate::intake::reference_candidates(
                &state.surreal,
                &question.answer_type,
                notation_id,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, %notation_id, "walker: reference_candidates failed");
                Vec::new()
            });
            // Surface any prior answer for this (state, respondent) — including
            // a batch transcript-coverage proposal (`source = extracted`) — so
            // the CLI walk shows it as a default to confirm or edit. The
            // `prior_source` lets the CLI mark a proposal ("from transcript")
            // apart from a previously-typed answer.
            let person_id = store::notations::find_by_id(&state.surreal, notation_id)
                .await
                .ok()
                .flatten()
                .map_or_else(Uuid::nil, |n| n.person_id);
            let prior = prior_answer_row(
                &state.surreal,
                notation_id,
                question.id,
                &question.code,
                person_id,
            )
            .await;
            let prior_answer = prior
                .as_ref()
                .map(|a| store::answers::display_value(&a.value));
            let prior_source = prior.as_ref().map(|a| a.source.clone());
            axum::Json(serde_json::json!({
                "notation_id": notation_id,
                "complete": false,
                "question": {
                    "code": question.code,
                    "prompt": question.prompt,
                    "answer_type": question.answer_type,
                    "choices": choices,
                    "candidates": candidates,
                    "prior_answer": prior_answer,
                    "prior_source": prior_source,
                },
            }))
            .into_response()
        }
        Ok(NextStep::QuestionnaireComplete) => axum::Json(serde_json::json!({
            "notation_id": notation_id,
            "complete": true,
            "question": serde_json::Value::Null,
        }))
        .into_response(),
        Err(NotationSessionError::NotationNotFound(_)) => {
            (StatusCode::NOT_FOUND, "notation not found").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "walker: current_step (json) failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// POST `/app/lawyer/notations/:id/step` — capture one answer and
/// advance the questionnaire.
///
/// The runtime is the sole writer of `notation_events`: in
/// production, the `workflows-service` worker handler journals each
/// transition inside `ctx.run("append-…", …)` (see
/// `workflows-service::notation_service::questionnaire_signal`); in
/// tests, the in-memory runtime records the transition in its own
/// `Vec<WorkflowEvent>`. The shared `notation_session` service
/// does *not* write the journal itself, so a production deploy
/// sees exactly one row per signal and replays don't
/// double-insert.
/// Resolve the question the runtime currently expects an answer for,
/// mapping every non-answerable state to its HTTP response.
async fn expected_question(
    state: &AdminState,
    notation_id: Uuid,
) -> Result<notation_session::QuestionDescriptor, Response> {
    match notation_session::current_step(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        Some(&state.storage),
        notation_id,
    )
    .await
    {
        Ok(NextStep::NeedsAnswer { question }) => {
            tracing::info!(%notation_id, code = %question.code, "step_post: current_step → NeedsAnswer");
            Ok(question)
        }
        Ok(NextStep::QuestionnaireComplete) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "questionnaire is already complete",
        )
            .into_response()),
        Err(NotationSessionError::NotationNotFound(_)) => {
            Err((StatusCode::NOT_FOUND, "notation not found").into_response())
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "walker: current_step failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response())
        }
    }
}

/// Resolve one scalar step answer to `(value, reference_id)`, or an early
/// response: a `500` when the candidate lookup fails (never a silent
/// no-advance on a transient error), or a redirect back to the same step
/// when a reference pick names no in-scope row. Aggregates (`people_list`)
/// are assembled by the caller, not here.
async fn resolved_scalar_answer(
    state: &AdminState,
    answer_type: &str,
    notation_id: Uuid,
    body: &std::collections::BTreeMap<String, String>,
) -> Result<(String, Option<Uuid>), Response> {
    match crate::intake::resolve_reference_answer(&state.surreal, answer_type, notation_id, body)
        .await
    {
        Ok(crate::intake::ReferenceResolution::Resolved { value, id }) => Ok((value, id)),
        Ok(crate::intake::ReferenceResolution::Rejected(_)) => {
            Err(Redirect::to(&format!("/app/lawyer/notations/{notation_id}/step")).into_response())
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "walker: resolve_reference_answer failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response())
        }
    }
}

pub async fn step_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(body): Form<std::collections::BTreeMap<String, String>>,
) -> Response {
    tracing::info!(%notation_id, field_count = body.len(), "step_post: enter");
    // The admin walker is lawyer entering the answer on the client's
    // behalf: the typist is the logged-in lawyer/admin person, the source
    // is `lawyer`. The respondent stays the notation's bound client.
    let author =
        notation_session::AnswerAuthor::lawyer(session.as_deref().and_then(|s| s.person_id));
    // The HTML form submits `value` (or the `people_list` widget's
    // `p{row}_{part}` inputs); ask the service which question the
    // runtime is currently expecting so we can pass the right code —
    // and assemble the right value shape — into `answer_step`.
    let question = match expected_question(&state, notation_id).await {
        Ok(question) => question,
        Err(resp) => return resp,
    };
    // A reference answer must name a seeded row (e.g. a `country`
    // question), and a picker selection posts the chosen row's `id`. The
    // select/pick enforces this in the browser and the CLI; a hand-crafted
    // POST that names no in-scope row just doesn't advance — the redirect
    // re-renders the same step. A record type (`person`/`entity`) may
    // free-type a new row, so it resolves to a value with no id.
    let (value, reference_id) =
        if store::question_registry::answer_type_is_aggregate(&question.answer_type) {
            (crate::people_list_answer::assemble(&body), None)
        } else {
            match resolved_scalar_answer(&state, &question.answer_type, notation_id, &body).await {
                Ok(pair) => pair,
                Err(resp) => return resp,
            }
        };

    let next = match notation_session::answer_step_with_reference(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        Some(&state.storage),
        notation_id,
        &question.code,
        value.as_str(),
        reference_id,
        author,
    )
    .await
    {
        Ok(n) => n,
        Err(NotationSessionError::AlreadyComplete) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "questionnaire is already complete",
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "walker: answer_step failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };

    match next {
        NextStep::NeedsAnswer { .. } => {
            // Round-trip back to GET so the user sees the next
            // question.
            Redirect::to(&format!("/app/lawyer/notations/{notation_id}/step")).into_response()
        }
        NextStep::QuestionnaireComplete => {
            // The lawyer completing the walk owns every workflow
            // transition it fires (intake, render, approve, close) — attribute
            // them to that session Person, not the notation's client (#252).
            let acting = resolve_lawyer_actor(&state.surreal, session.as_deref()).await;
            // The closing letter is firm-signed and ends the matter, so
            // it drives a different post-questionnaire workflow than the
            // client-signed retainer. Branch on the bound template.
            if notation_template_code(&state.surreal, notation_id)
                .await
                .as_deref()
                == Some("offboarding__letter")
            {
                return match drive_closing_workflow(&state, notation_id, acting).await {
                    // The matter is now closed and the letter firm-signed.
                    // Closing a matter records legal work; it raises no
                    // money. Accounting originates in Xero, where lawyers
                    // agree the price and raise the invoice themselves.
                    Ok(_end) => Redirect::to("/app/lawyer").into_response(),
                    Err(e) => {
                        tracing::error!(error = %e, %notation_id, "walker: closing drive failed");
                        (StatusCode::INTERNAL_SERVER_ERROR, "closing failed").into_response()
                    }
                };
            }
            // Hand off to the post-intake workflow: intake →
            // retainer_rendered → sent_for_signature. The
            // rendering context comes from the Answer rows the
            // walker just landed, so the workflow drive is
            // self-contained.
            match drive_post_questionnaire_workflow(&state, notation_id, acting).await {
                Ok(out) => {
                    // Lawyer who complete intake themselves land here parked at
                    // lawyer_review — offer the same "Request changes" panel the
                    // review page does, so a wrong answer can be flagged for
                    // re-collection without a detour.
                    tracing::info!(
                        %notation_id,
                        final_state = %out.final_state.as_str(),
                        "walker: intake complete",
                    );
                    back_to_review(notation_id)
                }
                Err(e) => {
                    tracing::error!(error = %e, %notation_id, "walker: workflow drive failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "workflow failed").into_response()
                }
            }
        }
    }
}

/// POST `/app/lawyer/notations/:id/transcript` — the batch **transcript input mode**
/// of the intake walk. Runs `live_inquiry` coverage over the notation's
/// template and the uploaded transcript (form field `transcript`), persists
/// each covered inquiry as a proposed answer (`source = extracted`) via
/// [`notation_session::record_extracted_answer`], and returns a JSON coverage
/// summary. It never advances the questionnaire: the covered answers surface as
/// the walk's prior-answer defaults for the lawyer to confirm or edit, and the
/// uncovered questions still prompt. "Live inquiry" and "upload a transcript"
/// both resolve here — real-time streaming stays out of scope (#152).
/// The outcome of running a transcript against a notation's bound questionnaire.
pub struct TranscriptCoverage {
    pub template_code: String,
    /// One `{code, proposed_answer}` per inquiry the transcript likely answered
    /// and which was recorded as a proposed default.
    pub covered: Vec<serde_json::Value>,
    /// Inquiry codes the transcript left uncovered (the walk will still ask).
    pub uncovered: Vec<String>,
}

/// Why running a transcript coverage pass failed.
#[derive(Debug)]
pub enum TranscriptCoverageError {
    NotationNotFound,
    TemplateNotFound,
    /// The bound template declares no questionnaire to cover.
    NoQuestionnaire,
    Db(String),
}

impl std::fmt::Display for TranscriptCoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotationNotFound => write!(f, "notation not found"),
            Self::TemplateNotFound => write!(f, "template not found"),
            Self::NoQuestionnaire => write!(f, "template has no questionnaire"),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for TranscriptCoverageError {}

/// Run `transcript` against the notation's bound questionnaire, recording every
/// likely-answered inquiry as a proposed default and returning what was covered
/// vs. still open. The one command behind both the lawyer/CLI transcript form and
/// the REST door. Coverage runs against the DB-bound template (including a
/// project-scoped override), never a file on the caller's disk.
pub async fn record_transcript_coverage(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    notation_id: Uuid,
    transcript: &str,
) -> Result<TranscriptCoverage, TranscriptCoverageError> {
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await
        .ok()
        .flatten()
        .ok_or(TranscriptCoverageError::NotationNotFound)?;
    let template = store::templates::find_by_id(surreal, notation_row.template_id)
        .await
        .ok()
        .flatten()
        .ok_or(TranscriptCoverageError::TemplateNotFound)?;
    let code = template.code.clone();
    let template_body = store::templates::body(surreal, storage, &template)
        .await
        .map_err(|e| TranscriptCoverageError::Db(e.to_string()))?;
    let loaded = live_inquiry::load_template_from_str(&template_body, &code)
        .map_err(|_| TranscriptCoverageError::NoQuestionnaire)?;
    let coverage = live_inquiry::cover_text(
        loaded,
        transcript,
        live_inquiry::TranscriptSource::TranscriptFile {
            path: "upload".to_string(),
        },
    )
    .map_err(|e| TranscriptCoverageError::Db(e.to_string()))?;

    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for finding in &coverage.findings {
        match &finding.proposed_answer {
            Some(value) if finding.status == live_inquiry::CoverageStatus::LikelyAnswered => {
                match notation_session::record_extracted_answer(
                    surreal,
                    notation_id,
                    &finding.inquiry_code,
                    value,
                )
                .await
                {
                    Ok(true) => covered.push(serde_json::json!({
                        "code": finding.inquiry_code,
                        "proposed_answer": value,
                    })),
                    // Unseeded question code — skipped, not covered.
                    Ok(false) => uncovered.push(finding.inquiry_code.clone()),
                    Err(e) => return Err(TranscriptCoverageError::Db(e)),
                }
            }
            _ => uncovered.push(finding.inquiry_code.clone()),
        }
    }
    Ok(TranscriptCoverage {
        template_code: coverage.template_code,
        covered,
        uncovered,
    })
}

pub async fn transcript_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    Form(body): Form<BTreeMap<String, String>>,
) -> Response {
    let Some(transcript) = body
        .get("transcript")
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    else {
        return (StatusCode::BAD_REQUEST, "transcript is required").into_response();
    };
    match record_transcript_coverage(&state.surreal, &state.storage, notation_id, transcript).await
    {
        Ok(c) => axum::Json(serde_json::json!({
            "template_code": c.template_code,
            "covered": c.covered,
            "uncovered": c.uncovered,
        }))
        .into_response(),
        Err(TranscriptCoverageError::NotationNotFound) => {
            (StatusCode::NOT_FOUND, "notation not found").into_response()
        }
        Err(TranscriptCoverageError::TemplateNotFound) => {
            (StatusCode::NOT_FOUND, "template not found").into_response()
        }
        Err(TranscriptCoverageError::NoQuestionnaire) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "template has no questionnaire",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "transcript coverage failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowDriveError {
    #[error("workflow runtime: {0}")]
    Runtime(#[from] workflows::WorkflowRuntimeError),
    #[error("person directory: {0}")]
    Person(#[from] store::persons::PersonError),
    #[error("build the placeholder context: {0}")]
    Context(#[from] ContextError),
    #[error("signature provider: {0}")]
    Signature(#[from] crate::signature::SignatureError),
    #[error("database: {0}")]
    Db(String),
    #[error("template: {0}")]
    Template(#[from] store::templates::TemplateError),
    #[error("template `{0}` vanished mid-flight")]
    TemplateMissing(Uuid),
    #[error("notation: {0}")]
    Notation(#[from] store::notations::NotationError),
    #[error("signature store: {0}")]
    SignatureStore(#[from] store::signatures::SignatureError),
    #[error("serialize document payload: {0}")]
    Payload(serde_json::Error),
    #[error("closing workflow spec: {0}")]
    Spec(String),
    #[error("storage: {0}")]
    Storage(#[from] cloud::StorageError),
    #[error("template body: {0}")]
    TemplateBody(#[from] store::templates::TemplateBodyError),
    /// The worker has not yet rendered + persisted the notation's PDF, so
    /// there is nothing to send. Distinguished from a hard failure: the
    /// send route maps this to `409` + a "retry" reason, never a 500.
    #[error("document not ready: the retainer PDF for notation {0} has not been rendered yet")]
    DocumentNotReady(Uuid),
    /// The template carries the custom-clause slot and the notation has no
    /// clauses, so its fee terms were never written. Refused before the
    /// `pdf_persisted` transition, so the walk stays at `lawyer_review`
    /// where a lawyer can add the clauses and approve again.
    #[error(
        "the fee terms have not been written: notation {0} has no clauses, \
         and its engagement agreement leaves the fee to them"
    )]
    ClausesRequired(Uuid),
    /// The template's `form:` binding or its field map failed to
    /// resolve — a vendoring or mapping defect, never silently skipped.
    #[error("government form `{form_code}`: {reason}")]
    Form { form_code: String, reason: String },
}

impl From<String> for WorkflowDriveError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

/// Where the post-questionnaire workflow left the notation.
///
/// Only the state survives the drive: the caller redirects to the review screen,
/// which re-reads the signature request and re-assembles the document from the
/// committed rows. Carrying them out of here would mean rendering twice and
/// risking a screen that disagrees with the database.
struct WorkflowOutput {
    final_state: StateName,
}

/// Run the post-questionnaire signing workflow against an
/// already-walked Notation. Template-agnostic — the retainer, the
/// Nevada trust, and any future signed template walk the same path:
///
///   intake_submitted  → intake_persisted__<respondent>
///   <doc>_rendered     → lawyer_review
///   approved           → generate_pdf__<doc>_pdf
///   pdf_persisted      → sent_for_signature__pending
///
/// The workflow spec is resolved from the notation's bound template
/// **code** (not a cached retainer spec), and the only per-template
/// condition — the `*_rendered` edge out of `intake_persisted__*` — is
/// read straight from that spec, so adding a signed template needs no
/// code change here. The rendering context is built from the Answer
/// rows the walker just persisted; the PDF lands in
/// `cloud::StorageService` keyed by [`document_pdf_storage_key`].
/// Start the workflow machine for `notation_id` and advance it from
/// `BEGIN` to the `lawyer_review` gate: fire `intake_submitted`, then the
/// template's single `*_rendered` edge out of `intake_persisted__*`,
/// syncing `notation.state` at each step. Returns the resulting
/// `lawyer_review` state.
///
/// It never sends — the human approve step renders + parks
/// ([`approve_send_post`] → [`render_and_park`]) and the deliberate send
/// ([`send_post`] → [`dispatch_signature`]) is the only thing that emits an
/// envelope. The `*_rendered` condition is read from the spec, not
/// hard-coded, so a new signed template needs no change here. Shared by the
/// questionnaire walker's post-intake drive and the matter-open form, so
/// both reach the gate by one code path.
pub async fn advance_to_lawyer_review(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
    acting: Option<Uuid>,
) -> Result<StateName, WorkflowDriveError> {
    // The send path is keyed off the template code, so the spec follows
    // the actual bound template.
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let template_row = store::templates::find_by_id(surreal, notation_row.template_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let yaml = workflows::catalog_spec_yaml(&template_row.code)
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let spec = workflows::workflow_spec_from_yaml(yaml)
        .map_err(|e| WorkflowDriveError::Spec(e.to_string()))?;

    StateMachineRuntime::start(runtime, MachineKind::Workflow, notation_id, &spec).await?;
    let intake_state =
        signal_workflow(runtime, notation_id, "intake_submitted", None, acting).await?;
    sync_notation_state(surreal, notation_id, intake_state.as_str()).await?;

    // The condition that advances the persisted intake into lawyer review
    // names the rendered document (`retainer_rendered`, `trust_rendered`,
    // …). It is the single edge out of the `intake_persisted__*` state, so
    // read it from the spec rather than hard-coding the retainer's name.
    let rendered_condition = spec
        .transitions_from(&intake_state)
        .and_then(|t| t.conditions().next())
        .map(ToString::to_string)
        .ok_or_else(|| {
            WorkflowDriveError::Spec(format!(
                "no rendered transition out of `{}`",
                intake_state.as_str()
            ))
        })?;
    let s = signal_workflow(runtime, notation_id, &rendered_condition, None, acting).await?;
    sync_notation_state(surreal, notation_id, s.as_str()).await?;
    Ok(s)
}

async fn drive_post_questionnaire_workflow(
    state: &AdminState,
    notation_id: Uuid,
    acting: Option<Uuid>,
) -> Result<WorkflowOutput, WorkflowDriveError> {
    // The client's final questionnaire answer parks the notation at the
    // `lawyer_review` gate and returns cleanly — no PDF is rendered on this
    // request. Rendering is a dedicated `generate_pdf` step that fires only
    // after a lawyer approves (`approve_send_post` → `render_and_park`
    // fires `approved`, the worker renders + persists on entering
    // `generate_pdf__*`), and the binding send is a third deliberate command
    // (`send_post` → `dispatch_signature`). Decoupling the render from the
    // completion request is what keeps a render failure at the lawyer step,
    // never on the client's last answer, and makes `lawyer_review` a true
    // human gate (N116). The matter-open form reaches this same gate through
    // `advance_to_lawyer_review` directly.
    let final_state = advance_to_lawyer_review(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        notation_id,
        acting,
    )
    .await?;
    // Assemble the document here even though the caller redirects to a screen
    // that assembles it again: a substitution failure has to surface as an error
    // on the request that completed intake, not as a silently-empty preview on
    // the review screen. The markup itself is discarded.
    render_assembled_document(state, notation_id).await?;
    Ok(WorkflowOutput { final_state })
}

/// Render the reviewed document on the worker and PARK — the durable
/// first half of the send. From `lawyer_review`, fire `approved`
/// (threading the Typst `DocumentPayload` the worker renders + persists on
/// entering `generate_pdf__*`), sync `notation.state` to the
/// `generate_pdf__*` step, and return. It does **not** fire
/// `pdf_persisted`, read the PDF back, or send: the worker durably owns
/// render+persist, and the workflow waits at the document step for an
/// explicit [`dispatch_signature`].
///
/// Splitting render-and-park from the send is what makes the pipeline
/// durable against real Restate Cloud, where the worker's render+persist
/// is a separate invocation from the `web` request that fired `approved`
/// — synchronously reading the PDF back in the same request (the old
/// `assemble_and_send`) raced that invocation and 500'd. The bytes are
/// assembled from the *current* answers + clauses, so what the attorney
/// reviewed is what renders.
///
/// Self-contained so both the auto-path
/// ([`drive_post_questionnaire_workflow`]) and the attorney's explicit
/// [`approve_send_post`] reach it identically.
/// The dependencies the approve command core (`render_and_park` +
/// `acroform_payload`) borrows from whichever web state drives it — the lawyer
/// `AdminState` or the REST `ApiState`. Decoupling the core from either
/// concrete state lets both doors render + park a notation identically.
pub(crate) struct RenderDeps<'a> {
    /// The other store — the bound client's identity is a `persons` row.
    pub surreal: &'a store::surreal::SurrealDb,
    pub runtime: &'a dyn StateMachineRuntime,
    pub storage: &'a Arc<dyn cloud::StorageService>,
    /// The public assets bucket the government-form blank is pulled from.
    pub assets_storage: &'a Arc<dyn cloud::StorageService>,
    /// The vendored government-form registry (AcroForm field maps + pins).
    pub forms_registry: &'a [forms::FormMeta],
}

pub(crate) async fn render_and_park(
    deps: &RenderDeps<'_>,
    notation_id: Uuid,
    acting: Option<Uuid>,
) -> Result<StateName, WorkflowDriveError> {
    let runtime = deps.runtime;
    let notation_row = store::notations::find_by_id(deps.surreal, notation_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let template_row = store::templates::find_by_id(deps.surreal, notation_row.template_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;

    // Re-assemble the body — template + custom clauses — so the PDF
    // reflects the latest reviewed content.
    let raw_template_body =
        store::templates::body(deps.surreal, deps.storage, &template_row).await?;
    let clauses = store::notation_clauses::for_notation(deps.surreal, notation_id)
        .await
        .map_err(|error| WorkflowDriveError::Db(error.to_string()))?;
    let template_body = store::notation_clauses::splice(&raw_template_body, &clauses);

    // Two rendering paths, declared by the template: a `form:` binding
    // fills the vendored government packet's AcroForm from the answers
    // (the artifact the SoS receives is the state's own form); without
    // one, the body renders through Typst as before. The AcroForm fill
    // reads the raw choice *keys* (a form radio selects by key); the Typst
    // body reads the human *labels*.
    let document_payload = if let Some(form_code) = template_row.form_code.as_deref() {
        let form_ctx = render_form_context_from_answers(deps.surreal, notation_id).await?;
        acroform_payload(deps, notation_id, form_code, &form_ctx).await?
    } else {
        let ctx = render_context_from_answers(deps.surreal, notation_id).await?;
        // Convert the Markdown body to Typst, then expand signature
        // placeholders into anchored Typst blocks. Only the Typst source
        // matters here; the placed fields are rebuilt at send time from the
        // same deterministic expansion over the same conversion.
        let (typst_source, _signature_fields) = crate::signature_render::expand_signatures(
            &typst_body_from_template(&template_body, &ctx),
        );
        serde_json::to_string(&workflows::DocumentPayload::Typst {
            storage_key: document_pdf_storage_key(notation_id),
            typst_source,
        })
        .map_err(WorkflowDriveError::Payload)?
    };

    // Fire `approved`: the worker renders + persists the PDF on entering
    // `generate_pdf__retainer_pdf`. We do NOT advance past it — the send
    // is a separate, deliberate command that first confirms the PDF
    // landed.
    let s = signal_workflow(
        runtime,
        notation_id,
        "approved",
        Some(&document_payload),
        acting,
    )
    .await?;
    sync_notation_state(deps.surreal, notation_id, s.as_str()).await?;
    Ok(s)
}

/// Build the `DocumentPayload::Acroform` JSON for a template with a
/// `form:` binding: resolve the field map against the answers, pull the
/// blank from the public assets bucket, verify it against the repo's
/// `.sha256` pin, and stage the verified bytes in documents storage for
/// the worker. Every failure is loud — a missing bucket object, a pin
/// mismatch, or a mis-mapped form must park the matter, never fill a
/// blank and never fall back to other bytes.
async fn acroform_payload(
    deps: &RenderDeps<'_>,
    notation_id: Uuid,
    form_code: &str,
    ctx: &BTreeMap<String, String>,
) -> Result<String, WorkflowDriveError> {
    let form_err = |reason: String| WorkflowDriveError::Form {
        form_code: form_code.to_string(),
        reason,
    };
    let form = deps
        .forms_registry
        .iter()
        .find(|f| f.code == form_code)
        .ok_or_else(|| form_err("not in the vendored forms registry".into()))?;
    // A `.fields.toml`-mapped form resolves through its map; a
    // re-authored form's `/T` names are the data paths themselves and
    // resolve through its `.fields` manifest.
    let fields = forms::fill_values(form_code, ctx)
        .map_err(|e| form_err(e.to_string()))?
        .ok_or_else(|| form_err("no field map or manifest vendored for this form".into()))?;

    // Always-pull: the assets bucket is the only source of the blank.
    let blank = deps
        .assets_storage
        .get(form.object_path)
        .await
        .map_err(|e| match e {
            cloud::StorageError::NotFound(_) => form_err(format!(
                "blank not in the assets bucket at `{}` — vendor it with `navigator forms sync`",
                form.object_path
            )),
            other => WorkflowDriveError::Storage(other),
        })?;
    form.verify(&blank.bytes)
        .map_err(|e| form_err(e.to_string()))?;

    // Stage the just-verified bytes where the worker reads
    // `blank_form_key` (the private documents lane), overwriting
    // unconditionally so a re-vendored blank + new pin propagates.
    let blank_form_key = form.object_path.to_string();
    deps.storage
        .put(&blank_form_key, &blank.bytes, "application/pdf")
        .await?;
    serde_json::to_string(&workflows::DocumentPayload::Acroform {
        storage_key: document_pdf_storage_key(notation_id),
        blank_form_key,
        fields,
    })
    .map_err(WorkflowDriveError::Payload)
}

/// Whether the worker has rendered + persisted the notation's document
/// PDF — a cheap existence probe on [`document_pdf_storage_key`] (a
/// metadata-only HEAD on GCS). [`dispatch_signature`] gates on this before
/// advancing the workflow and sending the envelope, and `notation status`
/// surfaces it as `document_ready`, so a misconfigured worker that never
/// wrote the PDF is visible rather than an opaque 500 at send time.
pub(crate) async fn document_pdf_ready(
    storage: &dyn cloud::StorageService,
    notation_id: Uuid,
) -> Result<bool, cloud::StorageError> {
    storage.exists(&document_pdf_storage_key(notation_id)).await
}

/// Dispatch the rendered document for signature — the deliberate,
/// authenticated "send" half of the pipeline. Confirms the worker's PDF
/// is present, fires `pdf_persisted` (→ `sent_for_signature__pending`),
/// reads the persisted PDF back, builds the manifest, and sends exactly
/// one envelope, persisting the `signature_request_id`.
///
/// Idempotent (find-or-create keyed on `notation_id`): a notation that
/// already carries a `signature_request_id` reuses it and neither
/// re-fires the transition nor re-sends. When the PDF isn't present yet
/// (the worker hasn't rendered, or its storage is misconfigured), returns
/// [`WorkflowDriveError::DocumentNotReady`] so the caller can answer
/// "not yet — retry" (a `409`) instead of looping or 500-ing. The
/// provider ALSO sends an X-DocuSign-Idempotency-Key so a concurrent
/// double-send dedupes at DocuSign, not just on the id check here.
/// The dependencies the send command core (`dispatch_signature`) borrows from
/// whichever web state drives it — the lawyer `AdminState` or the REST
/// `ApiState`. Distinct from [`RenderDeps`]: send needs the signature provider
/// but not the government-form registry / assets bucket.
pub(crate) struct SendDeps<'a> {
    /// The other store — the notation's client is a `persons` row.
    pub surreal: &'a store::surreal::SurrealDb,
    pub runtime: &'a dyn StateMachineRuntime,
    pub storage: &'a Arc<dyn cloud::StorageService>,
    pub signature_provider: &'a dyn crate::signature::SignatureProvider,
}

pub(crate) async fn dispatch_signature(
    deps: &SendDeps<'_>,
    notation_id: Uuid,
    acting: Option<Uuid>,
) -> Result<(StateName, crate::signature::SignatureRequestId), WorkflowDriveError> {
    let runtime = deps.runtime;
    let notation_row = store::notations::find_by_id(deps.surreal, notation_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;

    // Idempotency: this notation already has an envelope out. Reuse the
    // recorded id, fire nothing, send nothing — the post-state is
    // whatever the notation already records.
    if let Some(existing) =
        store::signatures::request_id_for_notation(deps.surreal, notation_id).await?
    {
        return Ok((
            StateName::from(notation_row.state.as_str()),
            crate::signature::SignatureRequestId(existing),
        ));
    }

    // Readiness gate: the worker durably renders + persists the PDF on
    // entering `generate_pdf__*`. Confirm it landed before advancing —
    // against real Restate Cloud that render is a separate invocation, so
    // "approved fired" does not imply "PDF written."
    if !document_pdf_ready(deps.storage.as_ref(), notation_id).await? {
        return Err(WorkflowDriveError::DocumentNotReady(notation_id));
    }

    // Resolve the template + its clauses before advancing. The manifest's
    // recipients and placed signature fields are built from them below, and
    // the fee-clause gate reads them here: the walk parks at
    // `sent_for_signature__pending` once `pdf_persisted` fires, so a refusal
    // has to land before that transition rather than unwind it.
    let template_row = store::templates::find_by_id(deps.surreal, notation_row.template_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let raw_template_body =
        store::templates::body(deps.surreal, deps.storage, &template_row).await?;
    let clauses = store::notation_clauses::for_notation(deps.surreal, notation_id)
        .await
        .map_err(|error| WorkflowDriveError::Db(error.to_string()))?;

    // A template that carries the custom-clause slot expects its terms to
    // arrive as clauses — for the engagement agreement that is the fee
    // basis, which NRPC 1.5(b) and Cal. B&P § 6148 require the client be
    // told in writing. `splice` substitutes the marker with whatever exists,
    // including nothing, so an empty slot renders cleanly and would dispatch
    // an agreement with no fee terms in it. Refuse instead.
    if raw_template_body.contains(store::notation_clauses::CUSTOM_CLAUSES_MARKER)
        && clauses.is_empty()
    {
        return Err(WorkflowDriveError::ClausesRequired(notation_id));
    }

    let s = signal_workflow(runtime, notation_id, "pdf_persisted", None, acting).await?;
    sync_notation_state(deps.surreal, notation_id, s.as_str()).await?;

    // Now at sent_for_signature__pending; fire the signature seam. The
    // client signs first (routing 1), the firm countersigns (routing 2) so
    // the engagement forms on the firm's signature. The captive client's
    // identity comes from the questionnaire answers when present (the
    // retainer asks `person__client`, exposing `person__client.name`) and
    // otherwise from the notation's bound Person row — never hardcoded in
    // the provider.
    let template_body = store::notation_clauses::splice(&raw_template_body, &clauses);
    let ctx = render_context_from_answers(deps.surreal, notation_id).await?;
    let (_typst_source, signature_fields) =
        crate::signature_render::expand_signatures(&typst_body_from_template(&template_body, &ctx));

    // Read the PDF the worker persisted back from storage so the bytes
    // sent are exactly the bytes stored (one renderer, no second
    // in-process copy to drift).
    let pdf_bytes = deps
        .storage
        .get(&document_pdf_storage_key(notation_id))
        .await?
        .bytes;
    let client = store::persons::find_by_id(deps.surreal, notation_row.person_id).await?;
    // `emailed` delivery → non-captive client (DocuSign emails the signing
    // link); anything else (`embedded`, the default) keeps the captive
    // embedded-signing recipient. Read off the notation so the single send
    // path serves both without a second route.
    let captive = notation_row.delivery != store::notations::DELIVERY_EMAILED;
    let manifest = build_signature_manifest(
        notation_id,
        &signature_fields,
        &ctx,
        client.as_ref(),
        captive,
    );
    let id = deps
        .signature_provider
        .send_for_signature(notation_id, &pdf_bytes, &manifest)
        .await?;
    // Persist the request id so the inbound completion webhook
    // (`crate::esignature_webhook`) can resolve its callback back to this
    // notation.
    persist_signature_request_id(deps.surreal, notation_id, &id.0).await?;

    Ok((s, id))
}

/// POST `/app/lawyer/notations/:id/approve-send` — the attorney
/// approves a notation parked at `lawyer_review` (it carried custom
/// content). This now renders + parks only: it fires `approved` so the
/// worker durably renders + persists the reviewed bytes and the workflow
/// waits at `generate_pdf__retainer_pdf`. The binding send is a separate,
/// deliberate command ([`send_post`] / `navigator retainer send`) that
/// first confirms the PDF landed — so a real Restate Cloud worker's
/// render never races the send.
pub async fn approve_send_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    // Idempotent approve: if the worker has already rendered + persisted
    // this notation's PDF — a prior approve, or the auto-approve a clean
    // machine-only intake takes when it walks straight through to
    // signature (`drive_post_questionnaire_workflow`) — approving again is
    // a no-op success. The bytes the attorney would approve already exist,
    // and re-firing `approved` from a state with no such edge (e.g.
    // `sent_for_signature__pending`) would otherwise 500 with NoTransition.
    // The matter-open retainer path parks at `lawyer_review` with no PDF
    // yet, so this guard never short-circuits a genuine first approve.
    if document_pdf_ready(state.storage.as_ref(), notation_id)
        .await
        .unwrap_or(false)
    {
        let notation_row = store::notations::find_by_id(&state.surreal, notation_id)
            .await
            .ok()
            .flatten();
        let workflow_state = notation_row
            .as_ref()
            .map_or_else(String::new, |n| n.state.clone());
        let signature_request_id =
            store::signatures::request_id_for_notation(&state.surreal, notation_id)
                .await
                .ok()
                .flatten();
        tracing::info!(
            %notation_id, %workflow_state, outcome = "already-ready",
            "approve_send: document already rendered — no-op approve",
        );
        let _ = signature_request_id;
        return back_to_review(notation_id);
    }

    // The attorney approving owns the `approved` transition — attribute it
    // to their session Person, not the notation's client (#252).
    let acting = resolve_lawyer_actor(&state.surreal, session.as_deref()).await;
    let deps = RenderDeps {
        surreal: &state.surreal,
        runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        assets_storage: &state.assets_storage,
        forms_registry: &state.forms_registry,
    };
    match render_and_park(&deps, notation_id, acting).await {
        // No signature_request_id yet — the result view shows the "Send
        // for signature" action for the parked document.
        Ok(final_state) => {
            tracing::info!(
                %notation_id, final_state = %final_state.as_str(), outcome = "parked",
                "approve_send: rendered and parked",
            );
            back_to_review(notation_id)
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "approve_send: render_and_park failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "approve failed").into_response()
        }
    }
}

/// POST `/app/lawyer/notations/:id/send` — dispatch the rendered
/// document for signature. The deliberate, authenticated "send" half of
/// the pipeline, reached from the browser's "Send for signature" button
/// and the `navigator retainer send` CLI command.
///
/// Confirms the worker's PDF is present, then sends exactly one envelope
/// (see [`dispatch_signature`]). When the PDF isn't ready yet — the worker
/// hasn't rendered, or its storage is misconfigured — it returns `409`
/// with a JSON `{error, reason}` body so the operator gets an actionable
/// "not yet, retry" instead of an opaque 500 or a silent retry loop.
pub async fn send_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    // The lawyer sending owns the `pdf_persisted` transition — attribute
    // it to their session Person, not the notation's client (#252).
    let acting = resolve_lawyer_actor(&state.surreal, session.as_deref()).await;
    let deps = SendDeps {
        surreal: &state.surreal,
        runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        signature_provider: state.signature_provider.as_ref(),
    };
    match dispatch_signature(&deps, notation_id, acting).await {
        Ok((final_state, signature_request_id)) => {
            tracing::info!(
                %notation_id,
                final_state = %final_state.as_str(),
                signature_request_id = %signature_request_id.0,
                "send: envelope dispatched",
            );
            back_to_review(notation_id)
        }
        Err(WorkflowDriveError::DocumentNotReady(_)) => {
            tracing::info!(%notation_id, "send: document not ready yet");
            (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "error": "document_not_ready",
                    "reason": "the retainer PDF has not been rendered yet — \
                               the worker is still rendering, or its storage is \
                               misconfigured. Re-run send in a moment; check \
                               `notation status` for document_ready.",
                })),
            )
                .into_response()
        }
        Err(WorkflowDriveError::ClausesRequired(_)) => {
            tracing::info!(%notation_id, "send: refused — the fee clause is missing");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "clauses_required",
                    "reason": "the fee terms have not been written — this engagement \
                               agreement leaves the fee to its custom clauses. Add at \
                               least one clause on the review screen, then send again.",
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "send: dispatch_signature failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "send failed").into_response()
        }
    }
}

/// Resolve the notation review screen for `GET /app/lawyer/notations/:id/review`, in
/// the wasm-safe shape the Dioxus page renders (#956 Phase 4).
///
/// The Dioxus route's pre-layer calls this and either injects the result or
/// returns the `Response` this hands back instead of rendering:
///
/// - `?format=json` — the machine-readable view `navigator notation status`
///   reads (workflow state, signature request id, and `document_ready`, the gate
///   the `send` command honors). HTML scraping is brittle, so the CLI keeps a
///   narrow branch on this same path rather than a parallel API tree.
/// - the notation is gone — a `404`.
pub(crate) async fn resolve_intake_review(
    state: &AdminState,
    notation_id: Uuid,
    format: Option<&str>,
) -> Result<webapp::intake_review::IntakeReviewData, Response> {
    let Some(notation_row) = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return Err((StatusCode::NOT_FOUND, "notation not found").into_response());
    };
    let signature_request_id =
        store::signatures::request_id_for_notation(&state.surreal, notation_id)
            .await
            .ok()
            .flatten();

    if format == Some("json") {
        // Per-matter pipeline state: a `StorageService` existence probe on the
        // document PDF key. A storage error here is non-fatal to the status
        // read — report `document_ready:false` and let the operator retry
        // rather than 500 the status call.
        let document_ready = document_pdf_ready(state.storage.as_ref(), notation_id)
            .await
            .unwrap_or(false);
        return Err(axum::Json(serde_json::json!({
            "notation_id": notation_id,
            "state": notation_row.state,
            "signature_request_id": signature_request_id,
            "delivery": notation_row.delivery,
            "document_ready": document_ready,
        }))
        .into_response());
    }

    let rendered_html = render_assembled_document(state, notation_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, %notation_id, "review: render failed");
            String::new()
        });
    // Offer the questionnaire's questions as flaggable in the "Request changes"
    // panel while the notation awaits review.
    let reask_questions = questionnaire_questions(state, notation_id)
        .await
        .into_iter()
        .map(|(code, label)| webapp::intake_review::ReaskQuestion { code, label })
        .collect();

    Ok(webapp::intake_review::IntakeReviewData {
        notation_id: notation_id.to_string(),
        workflow_state: notation_row.state,
        signature_request_id,
        rendered_html,
        reask_questions,
        approve_send_label: "Approve and send for signature".to_string(),
    })
}

/// Where every terminal action on a notation lands: the review screen for that
/// notation.
///
/// The screen renders through Dioxus on a `GET` (#956 Phase 4), so the actions
/// that used to render it inline — completing intake, approving, sending —
/// redirect here instead (post/redirect/get). A refresh after dispatching an
/// envelope therefore re-reads the screen rather than re-posting the send.
pub(crate) fn back_to_review(notation_id: Uuid) -> Response {
    Redirect::to(&format!("/app/lawyer/notations/{notation_id}/review")).into_response()
}

/// The notation's questionnaire questions as `(state_code, label)` — the
/// set the "Request changes" panel offers lawyer to flag. Labels come from
/// the seeded question rows, falling back to the code. Best-effort: a read
/// failure yields an empty list, so the review page still renders (just
/// without the request-changes panel).
async fn questionnaire_questions(state: &AdminState, notation_id: Uuid) -> Vec<(String, String)> {
    let codes = notation_session::questionnaire_chain_for_notation(
        &state.surreal,
        Some(&state.storage),
        notation_id,
    )
    .await
    .unwrap_or_default();
    let mut out = Vec::with_capacity(codes.len());
    for code in codes {
        let canonical = code.split_once("__").map_or(code.as_str(), |(p, _)| p);
        let prompt = store::questions::find_by_code(&state.surreal, canonical)
            .await
            .ok()
            .flatten()
            .map_or_else(|| code.clone(), |q| q.prompt);
        // Render the same clean label the client saw — resolve the prompt's
        // `{{for_label}}` / `{{label}}` role placeholders against the state.
        let label = notation_session::localize_prompt_for_state(&prompt, &code);
        out.push((code, label));
    }
    out
}

/// POST `/app/lawyer/notations/:id/request-changes` — the "send back for
/// changes" half of `lawyer_review`. Records the flagged question codes +
/// reviewer note on the attributed journal ([`store::reask`]) and routes
/// the workflow `changes_requested -> reask__client`, so a rejected review
/// re-collects the wrong answers instead of dead-ending. `rejected -> END`
/// stays the separate, deliberate "decline the matter" path.
/// Why sending a notation back for changes failed. Shared by the lawyer
/// `/app/lawyer/notations/{id}/request-changes` form and the
/// `/app/api/notations/{id}/request-changes` door.
#[derive(Debug)]
pub enum RequestChangesError {
    /// No notation with that id.
    NotationNotFound,
    /// The notation is not at the `lawyer_review` gate, so there is nothing to
    /// send back.
    NotInReview,
    /// No answer was flagged for re-collection.
    NothingFlagged,
    /// A store write or the workflow signal failed.
    Db(String),
}

impl std::fmt::Display for RequestChangesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotationNotFound => write!(f, "notation not found"),
            Self::NotInReview => write!(f, "notation is not awaiting review; nothing to send back"),
            Self::NothingFlagged => {
                write!(f, "select at least one answer to send back for changes")
            }
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for RequestChangesError {}

/// Send a notation back to its client for changes: record the flagged answers
/// (and an optional note), fire the `changes_requested` transition attributed to
/// `actor`, and sync the stored state. The one command behind both the lawyer
/// request-changes form and the REST door. `actor` is the lawyer owning the
/// transition; where none resolves (an unseeded dev DB) the record falls back to
/// the respondent so re-collection still works, but the signal stays unattributed
/// rather than crediting the client a lawyer action (#252).
pub async fn request_notation_changes(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
    actor: Option<Uuid>,
    flagged: &[String],
    note: Option<&str>,
) -> Result<(), RequestChangesError> {
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|e| RequestChangesError::Db(e.to_string()))?
        .ok_or(RequestChangesError::NotationNotFound)?;
    if notation_row.state != "lawyer_review" {
        return Err(RequestChangesError::NotInReview);
    }
    if flagged.is_empty() {
        return Err(RequestChangesError::NothingFlagged);
    }
    let record_actor = actor.unwrap_or(notation_row.person_id);
    store::reask::record_change_request(surreal, notation_id, record_actor, flagged, note)
        .await
        .map_err(|e| RequestChangesError::Db(e.to_string()))?;
    let next = signal_workflow(runtime, notation_id, "changes_requested", None, actor)
        .await
        .map_err(|e| RequestChangesError::Db(e.to_string()))?;
    let _ = sync_notation_state(surreal, notation_id, next.as_str()).await;
    Ok(())
}

pub async fn request_changes_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(fields): Form<std::collections::BTreeMap<String, String>>,
) -> Response {
    // Checkbox fields arrive as `q:<code>=on`; the note is a free-text field.
    let flagged: Vec<String> = fields
        .keys()
        .filter_map(|k| k.strip_prefix("q:").map(str::to_string))
        .collect();
    let note = fields
        .get("note")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    // The lawyer requesting changes owns the transition — attribute it to their
    // session Person, never the notation's client (#252).
    let acting = resolve_lawyer_actor(&state.surreal, session.as_deref()).await;
    match request_notation_changes(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        notation_id,
        acting,
        &flagged,
        note,
    )
    .await
    {
        Ok(()) => {
            Redirect::to(&format!("/app/lawyer/notations/{notation_id}/reask")).into_response()
        }
        Err(RequestChangesError::NotationNotFound) => {
            (StatusCode::NOT_FOUND, "notation not found").into_response()
        }
        Err(RequestChangesError::NotInReview) => (
            StatusCode::CONFLICT,
            "notation is not awaiting review; nothing to send back",
        )
            .into_response(),
        Err(RequestChangesError::NothingFlagged) => (
            StatusCode::BAD_REQUEST,
            "select at least one answer to send back for changes",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "request_changes: failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Resolve the lawyer-on-behalf re-ask surface for `GET
/// /app/lawyer/notations/:id/reask`, in the wasm-safe shape the Dioxus page renders
/// (#956 Phase 4).
///
/// The Dioxus route's pre-layer calls this and either injects the result or
/// returns the `Response` this hands back instead of rendering: a `404` for a
/// notation that is gone, or a redirect to the review screen when nothing is
/// parked for re-collection.
pub(crate) async fn resolve_reask(
    state: &AdminState,
    notation_id: Uuid,
) -> Result<webapp::reask::ReaskData, Response> {
    let Some(notation_row) = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()
    else {
        return Err((StatusCode::NOT_FOUND, "notation not found").into_response());
    };
    if notation_row.state != store::reask::REASK_STATE {
        // Nothing parked for re-collection — send lawyer to the review page.
        return Err(back_to_review(notation_id));
    }
    let request = store::reask::latest_change_request(&state.surreal, notation_id)
        .await
        .ok()
        .flatten();
    let flagged_codes = request
        .as_ref()
        .map(|r| r.flagged_questions.clone())
        .unwrap_or_default();
    // Resolve a human label per flagged code from the questionnaire set.
    let labels: std::collections::HashMap<String, String> =
        questionnaire_questions(state, notation_id)
            .await
            .into_iter()
            .collect();
    let flagged = flagged_codes
        .iter()
        .map(|code| webapp::reask::FlaggedQuestion {
            label: labels.get(code).cloned().unwrap_or_else(|| code.clone()),
            code: code.clone(),
        })
        .collect();

    Ok(webapp::reask::ReaskData {
        notation_id: notation_id.to_string(),
        flagged,
        note: request.and_then(|r| r.note),
    })
}

/// POST `/app/lawyer/notations/:id/reask` — save the re-collected answers (lawyer
/// on the client's behalf) and resubmit for review. Each `a:<code>` field
/// carries one re-answer; the write is gated to the flagged set by the
/// shared engine. After saving, fires `intake_resubmitted -> lawyer_review`
/// so the matter returns to the attorney rather than dead-ending.
/// Why resubmitting a re-collected notation failed. Shared by the lawyer
/// `/app/lawyer/notations/{id}/reask` form and the `/app/api/notations/{id}/reask`
/// door.
#[derive(Debug)]
pub enum ResubmitReaskError {
    /// No notation with that id.
    NotationNotFound,
    /// The notation is not parked at the re-collection state.
    NotAwaitingReask,
    /// A flagged question has no non-empty re-collected value; names the code.
    MissingAnswer(String),
    /// A re-collected answer could not be persisted (e.g. a stale flag whose
    /// question is not seeded). The resubmit is refused whole and the matter
    /// stays parked, rather than landing a partial correction.
    AnswerWriteFailed(String),
    /// A store read or the workflow signal failed.
    Db(String),
}

impl std::fmt::Display for ResubmitReaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotationNotFound => write!(f, "notation not found"),
            Self::NotAwaitingReask => write!(f, "notation is not awaiting re-collection"),
            Self::MissingAnswer(code) => write!(
                f,
                "re-collect the flagged answer `{code}` before resubmitting for review"
            ),
            Self::AnswerWriteFailed(e) => {
                write!(f, "a re-collected answer could not be saved: {e}")
            }
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for ResubmitReaskError {}

/// Resubmit a re-collected notation for review: validate that every flagged
/// question has a non-empty re-collected value in `answers` (keyed by bare
/// question code), write each answer attributed to `actor`, fire the
/// `intake_resubmitted` transition, and sync the stored state. The one command
/// behind both the lawyer reask form and the REST door. Validates the whole set
/// before writing, so a blank flagged answer refuses the resubmit rather than
/// leaving it partial.
pub async fn resubmit_reask(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    notation_id: Uuid,
    actor: Option<Uuid>,
    answers: &std::collections::BTreeMap<String, String>,
) -> Result<(), ResubmitReaskError> {
    let notation_row = store::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|e| ResubmitReaskError::Db(e.to_string()))?
        .ok_or(ResubmitReaskError::NotationNotFound)?;
    if notation_row.state != store::reask::REASK_STATE {
        return Err(ResubmitReaskError::NotAwaitingReask);
    }
    let flagged = store::reask::flagged_questions(surreal, notation_id)
        .await
        .map_err(|e| ResubmitReaskError::Db(e.to_string()))?;
    let mut to_write: Vec<(&str, &str)> = Vec::with_capacity(flagged.len());
    for code in &flagged {
        match answers
            .get(code)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            Some(value) => to_write.push((code.as_str(), value)),
            None => return Err(ResubmitReaskError::MissingAnswer(code.clone())),
        }
    }
    let author = notation_session::AnswerAuthor::lawyer(actor);
    // Each re-collected answer is written independently (Surreal has no
    // cross-write transaction here); a mid-write failure leaves already-written
    // answers in place rather than rolling the set back.
    for (code, value) in &to_write {
        notation_session::record_reask_answer(surreal, notation_id, code, value, None, author)
            .await
            .map_err(|e| ResubmitReaskError::AnswerWriteFailed(e.to_string()))?;
    }
    let next = signal_workflow(runtime, notation_id, "intake_resubmitted", None, actor)
        .await
        .map_err(|e| ResubmitReaskError::Db(e.to_string()))?;
    let _ = sync_notation_state(surreal, notation_id, next.as_str()).await;
    Ok(())
}

pub async fn reask_post(
    State(state): State<AdminState>,
    AxumPath(notation_id): AxumPath<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(fields): Form<std::collections::BTreeMap<String, String>>,
) -> Response {
    // Re-collected answers arrive as `a:<code>=<value>`; strip the prefix so the
    // command sees a map keyed by bare question code.
    let answers: std::collections::BTreeMap<String, String> = fields
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("a:")
                .map(|code| (code.to_string(), v.clone()))
        })
        .collect();
    let acting = resolve_lawyer_actor(&state.surreal, session.as_deref()).await;
    match resubmit_reask(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        notation_id,
        acting,
        &answers,
    )
    .await
    {
        Ok(()) => {
            Redirect::to(&format!("/app/lawyer/notations/{notation_id}/review")).into_response()
        }
        Err(ResubmitReaskError::NotationNotFound) => {
            (StatusCode::NOT_FOUND, "notation not found").into_response()
        }
        Err(ResubmitReaskError::NotAwaitingReask) => (
            StatusCode::CONFLICT,
            "notation is not awaiting re-collection",
        )
            .into_response(),
        Err(ResubmitReaskError::MissingAnswer(_)) => (
            StatusCode::BAD_REQUEST,
            "re-collect every flagged answer before resubmitting for review",
        )
            .into_response(),
        Err(ResubmitReaskError::AnswerWriteFailed(_)) => (
            StatusCode::BAD_REQUEST,
            "could not save a re-collected answer",
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, %notation_id, "reask: resubmit failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

/// Re-render a notation's assembled document (template body + custom
/// clauses + answers) to the HTML preview, for the result page.
async fn render_assembled_document(
    state: &AdminState,
    notation_id: Uuid,
) -> Result<String, WorkflowDriveError> {
    let notation_row = store::notations::find_by_id(&state.surreal, notation_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let template_row = store::templates::find_by_id(&state.surreal, notation_row.template_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let raw_template_body =
        store::templates::body(&state.surreal, &state.storage, &template_row).await?;
    let clauses = store::notation_clauses::for_notation(&state.surreal, notation_id)
        .await
        .map_err(|error| WorkflowDriveError::Db(error.to_string()))?;
    let template_body = store::notation_clauses::splice(&raw_template_body, &clauses);
    let ctx = render_context_from_answers(&state.surreal, notation_id).await?;
    Ok(views::notation::render_filled_in(&template_body, &ctx))
}

/// Storage-key convention for the offboarding letter PDF of a notation.
///
/// **The `closing-letter` segment is deliberately frozen at the old
/// spelling — do not rename it to `offboarding-letter`.** Objects are
/// already stored under this prefix, and an object key is not a symbol:
/// renaming it here does not move the bytes, it just stops resolving them,
/// orphaning every offboarding letter already filed. The vocabulary
/// elsewhere says offboarding (see `docs/glossary.md`); this one string is
/// a storage address, and addresses only change with a migration that
/// copies the objects first.
#[must_use]
pub fn closing_letter_storage_key(notation_id: Uuid) -> String {
    format!("notations/{notation_id}/closing-letter.pdf")
}

/// Fetch the template `code` bound to a notation, if resolvable. Used
/// to pick the post-questionnaire workflow drive.
async fn notation_template_code(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
) -> Option<String> {
    let n = store::notations::find_by_id(surreal, notation_id)
        .await
        .ok()
        .flatten()?;
    let t = store::templates::find_by_id(surreal, n.template_id)
        .await
        .ok()
        .flatten()?;
    Some(t.code)
}

/// Drive the offboarding-letter workflow for an already-walked closing
/// notation:
///
///   close_requested → lawyer_review
///   approved        → generate_pdf__closing_letter  (render + persist)
///   pdf_persisted   → firm_signature__closing_letter
///   signed          → END
///
/// **The two `__closing_letter` step names are deliberately frozen at the
/// old spelling — do not rename them to `__offboarding_letter`.** A Restate
/// step name is part of a durable journal: an invocation already in flight
/// replays against the names recorded when it started, so renaming one does
/// not rename history, it strands the invocation mid-workflow. The template
/// `code` and the spec file moved to `offboarding__letter`; these two
/// journal keys did not, and they are the reason the spec's `workflow:`
/// block still reads `closing_letter` under an `offboarding__letter` file.
/// Nothing branches on the suffix — `workflows::closing::closes_matter`
/// keys off the `firm_signature__*` *prefix* — so the stale word costs
/// nothing but a comment like this one.
///
/// The mirror of [`drive_post_questionnaire_workflow`], but the closing
/// letter is signed by the *firm*, not the client — there is no
/// e-signature send. The status flip `open` → `closed` is the runtime's
/// `close_matter` side effect on the firm-signature transition. Returns
/// the terminal state (END).
async fn drive_closing_workflow(
    state: &AdminState,
    notation_id: Uuid,
    acting: Option<Uuid>,
) -> Result<StateName, WorkflowDriveError> {
    let yaml = workflows::catalog_spec_yaml("offboarding__letter")
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let spec = workflows::workflow_spec_from_yaml(yaml)
        .map_err(|e| WorkflowDriveError::Spec(e.to_string()))?;
    let runtime = state.workflow_runtime.as_ref();

    StateMachineRuntime::start(runtime, MachineKind::Workflow, notation_id, &spec).await?;
    let s = signal_workflow(runtime, notation_id, "close_requested", None, acting).await?;
    sync_notation_state(&state.surreal, notation_id, s.as_str()).await?;

    // Render the closing letter from the answers the walker just landed.
    let notation_row = store::notations::find_by_id(&state.surreal, notation_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let template_row = store::templates::find_by_id(&state.surreal, notation_row.template_id)
        .await?
        .ok_or(WorkflowDriveError::TemplateMissing(notation_id))?;
    let template_body =
        store::templates::body(&state.surreal, &state.storage, &template_row).await?;
    let ctx = render_context_from_answers(&state.surreal, notation_id).await?;

    // Lawyer review short-circuits to `approved` in the dev loop (a real
    // lawyer-review handler swaps in for prod). The `approved` signal
    // threads the closing letter's Typst source + storage key; the
    // worker renders and persists the PDF on entering
    // `generate_pdf__closing_letter`.
    let document_payload = serde_json::to_string(&workflows::DocumentPayload::Typst {
        storage_key: closing_letter_storage_key(notation_id),
        typst_source: typst_body_from_template(&template_body, &ctx),
    })
    .map_err(WorkflowDriveError::Payload)?;
    let s = signal_workflow(
        runtime,
        notation_id,
        "approved",
        Some(&document_payload),
        acting,
    )
    .await?;
    sync_notation_state(&state.surreal, notation_id, s.as_str()).await?;

    let s = signal_workflow(runtime, notation_id, "pdf_persisted", None, acting).await?;
    sync_notation_state(&state.surreal, notation_id, s.as_str()).await?;

    // The firm signs the closing letter; this transition lands on END
    // and closes the matter (the runtime's `close_matter` side effect).
    let s = signal_workflow(runtime, notation_id, "signed", None, acting).await?;
    sync_notation_state(&state.surreal, notation_id, s.as_str()).await?;

    Ok(s)
}

/// Evaluate data placeholders in the markdown body with the same grammar
/// preview uses, but return plain text suitable for feeding into the Typst
/// compiler. Signature placeholders stay in place for
/// `signature_render::expand_signatures`, so final PDFs process data first
/// and signature anchors second.
fn substitute_template_body(body: &str, ctx: &BTreeMap<String, String>) -> String {
    views::notation::fill(body, ctx)
}

/// Assemble a notation template body into compilable Typst: substitute the
/// answer `ctx`, then convert the Markdown body to Typst markup via
/// [`pdf::to_typst`]. Notation bodies are authored in Markdown, so the
/// conversion is what escapes the prose sigils Typst reads as syntax — most
/// importantly a bare `@` (an email like `support@neonlaw.com` is otherwise
/// parsed as a Typst label reference and fails to compile), as well as
/// `#`/`$`/`*`. Signature placeholders (`{{client.signature}}`) survive the
/// conversion verbatim so [`crate::signature_render::expand_signatures`] can
/// expand them into anchored blocks afterward.
fn typst_body_from_template(body: &str, ctx: &BTreeMap<String, String>) -> String {
    pdf::to_typst(&substitute_template_body(body, ctx))
}

/// The captive `clientUserId` for the client recipient of `notation_id`.
/// It must be replayed verbatim when requesting the embedded recipient
/// view (see [`crate::esign_view`]), so both sides derive it from the
/// notation id rather than storing it.
#[must_use]
pub fn client_user_id(notation_id: Uuid) -> String {
    format!("client-{notation_id}")
}

/// Assemble the signature manifest from the placed fields. Only roles
/// that actually anchor a field become recipients, in routing order:
/// the client (routing 1) signs from their questionnaire answers; the
/// firm (routing 2) countersigns from the `DOCUSIGN_SIGNER_*` config
/// (defaulting to the firm support inbox, mirroring
/// `DocuSignSignatureProvider::from_env`). Empty fields → empty manifest
/// (the provider's single-signer fallback).
///
/// The captive client's name/email come from the questionnaire answers
/// when the template captured them (the retainer asks `person__client`,
/// whose dotted `.name`/`.email` fields land in the render context) and
/// otherwise from the notation's bound Person row in `client` (the trust
/// questionnaire never asks for an email, and the retainer captures the
/// email out-of-band, so `.email` falls through to the Person row). That
/// same Person is what [`crate::esign_view`] resolves the
/// embedded recipient against, so envelope creation and the recipient
/// view agree.
///
/// The client is **captive** (a `client_user_id` derived from the
/// notation): they sign embedded inside Neon Law Navigator, so DocuSign does not
/// email them. The firm is left non-captive — it countersigns from the
/// support inbox via the usual emailed link.
fn build_signature_manifest(
    notation_id: Uuid,
    fields: &[crate::signature::SignatureField],
    ctx: &BTreeMap<String, String>,
    client: Option<&store::persons::Person>,
    captive: bool,
) -> crate::signature::SignatureManifest {
    use crate::signature::{SignatureManifest, SignatureRecipient};
    if fields.is_empty() {
        return SignatureManifest::default();
    }
    let role_present = |role: &str| fields.iter().any(|f| f.recipient_role == role);
    let env = |k: &str, default: &str| {
        std::env::var(k)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    // Prefer the answered value; fall back to the bound Person when the
    // questionnaire didn't capture it (empty answers fall back too).
    let answered = |key: &str| ctx.get(key).filter(|s| !s.is_empty()).cloned();

    let mut recipients = Vec::new();
    if role_present("client") {
        recipients.push(SignatureRecipient {
            role: "client".into(),
            email: answered("person__client.email")
                .or_else(|| client.map(|c| c.email.clone()))
                .unwrap_or_default(),
            name: answered("person__client.name")
                .or_else(|| client.map(|c| c.name.clone()))
                .unwrap_or_default(),
            routing_order: 1,
            // Captive (`embedded` delivery): a `client_user_id` makes the
            // client an embedded recipient DocuSign does NOT email — they
            // sign inside Neon Law Navigator (`crate::esign_view`). Non-captive
            // (`emailed` delivery): `None`, so DocuSign emails them a
            // signing link they open from their own inbox.
            client_user_id: captive.then(|| client_user_id(notation_id)),
        });
    }
    if role_present("firm") {
        recipients.push(SignatureRecipient {
            role: "firm".into(),
            email: env("DOCUSIGN_SIGNER_EMAIL", "support@neonlaw.com"),
            name: env("DOCUSIGN_SIGNER_NAME", "Neon Law"),
            routing_order: 2,
            client_user_id: None,
        });
    }
    SignatureManifest {
        recipients,
        fields: fields.to_vec(),
    }
}

/// Build the `{{state}} → answer` context map for `notation_id`.
///
/// Notation-scoped, not person-scoped: only the answers this Notation
/// collected, so a person's other matters never bleed in (the old
/// person-only filter is what let two matters' answers collide). Each
/// answer is keyed on its full `<type>__<role>` state
/// (`entity__company`, `entity__subsidiary`) so two records of one type
/// stay distinct, and the bare canonical code is also exposed → its latest
/// answer for a direct `{{code}}` placeholder. Template-agnostic: it
/// surfaces whatever states the bound questionnaire collected, so a body
/// interpolates only its own placeholders and any extra keys are inert.
/// The placeholder context for rendering a **document or preview** — a
/// single/multiple-choice answer surfaces its human *label*, so a typst
/// body (the retainer, the closing letter) and the lawyer HTML preview read
/// "Married", not the stored key "married".
async fn render_context_from_answers(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
) -> Result<BTreeMap<String, String>, ContextError> {
    build_answer_context(surreal, notation_id, ChoiceRendering::Labels).await
}

/// The placeholder context for an **AcroForm fill** — choice answers keep
/// their raw *key*. A re-authored government form selects a radio group's
/// member by the choice key (its checkbox on-state), so the fill must see
/// `five_year`, not the label; only the human-facing render substitutes the
/// label. Everything else (merge fields, the `person__client.email` seeded
/// from the Person row) is identical.
async fn render_form_context_from_answers(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
) -> Result<BTreeMap<String, String>, ContextError> {
    build_answer_context(surreal, notation_id, ChoiceRendering::Keys).await
}

/// Whether a choice answer renders as its human label (documents, preview)
/// or its stored key (government-form radio fill).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChoiceRendering {
    Labels,
    Keys,
}

/// A context build that failed in one engine or the other: the bound
/// template, the answers, their questions, and the bound
/// client are all SurrealDB.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("database: {0}")]
    Db(String),
    #[error(transparent)]
    Notation(#[from] store::notations::NotationError),
    #[error(transparent)]
    Template(#[from] store::templates::TemplateError),
    #[error(transparent)]
    Person(#[from] store::persons::PersonError),
    #[error(transparent)]
    Answer(#[from] store::answers::AnswerError),
    #[error(transparent)]
    Question(#[from] store::questions::QuestionError),
}

impl From<String> for ContextError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

async fn build_answer_context(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    choice_rendering: ChoiceRendering,
) -> Result<BTreeMap<String, String>, ContextError> {
    let answers = store::answers::for_notation(surreal, notation_id).await?;

    // The notation's bound template + Person. For a document/preview render
    // the template's bundled spec supplies the choice metadata
    // (`value → label`) so a single-choice answer renders its label; the
    // Person row carries the client identity the questionnaire never
    // captures (it asks `person__client` for the name, never the email).
    let notation_row = store::notations::find_by_id(surreal, notation_id).await?;
    let choices = match (choice_rendering, notation_row.as_ref()) {
        (ChoiceRendering::Labels, Some(n)) => store::templates::find_by_id(surreal, n.template_id)
            .await?
            .and_then(|t| workflows::catalog_spec_yaml(&t.code))
            .and_then(|yaml| workflows::merged_choices_from_yaml(yaml).ok())
            .unwrap_or_default(),
        // Key rendering (AcroForm fill) and the no-notation case both leave
        // choices unmapped, so answers keep their raw stored value.
        _ => BTreeMap::new(),
    };

    // Resolve the question codes for the answered questions in one query.
    let question_ids: Vec<Uuid> = answers.iter().map(|a| a.question_id).collect();
    let code_by_id: BTreeMap<Uuid, String> = store::questions::find_by_ids(surreal, &question_ids)
        .await?
        .into_iter()
        .map(|q| (q.id, q.code))
        .collect();
    let mut ctx = context_from_answers(&answers, &code_by_id, &choices);

    // Seed the bound client's identity from the Person row where the
    // questionnaire didn't capture it — the retainer/naturalization intake
    // asks `person__client` for the name only, so `{{person__client.email}}`
    // (and the N-400's email field) fills from `persons.email`. Answers win:
    // only fill a gap, never overwrite a captured value.
    if let Some(person) = match notation_row {
        Some(n) => store::persons::find_by_id(surreal, n.person_id).await?,
        None => None,
    } {
        ctx.entry("person__client.email".to_string())
            .or_insert(person.email);
        ctx.entry("person__client.name".to_string())
            .or_insert(person.name);
    }
    default_governing_law(&mut ctx, choice_rendering);
    Ok(ctx)
}

/// Default the fillable governing-law clause to Nevada when the intake never
/// captured it — e.g. an in-flight product-retainer notation whose frozen
/// questionnaire graph predates the fillable governing-law clause (#363), so
/// it walked straight from `project__engagement` to `END` and never stored a
/// `custom_single_choice__governing_law` answer. The retainer body references
/// `{{custom_single_choice__governing_law}}`, so a missing answer would
/// otherwise render the raw placeholder in a binding document. Answers always
/// win — only a gap is filled — matching the questionnaire's documented
/// "Nevada by default", in the label/key form the rendering mode expects.
fn default_governing_law(ctx: &mut BTreeMap<String, String>, rendering: ChoiceRendering) {
    let nevada = match rendering {
        ChoiceRendering::Labels => "Nevada",
        ChoiceRendering::Keys => "nevada",
    };
    ctx.entry("custom_single_choice__governing_law".to_string())
        .or_insert_with(|| nevada.to_string());
}

/// Key notation-scoped answer rows into the placeholder context. Pure (no
/// DB) so the keying is unit-tested directly. Rows arrive in ascending-id
/// (answer) order, so the last answer for any key wins — append-only
/// latest-answer-wins, both for a re-answered state and for the bare code.
fn context_from_answers(
    answers: &[store::answers::Answer],
    code_by_id: &BTreeMap<Uuid, String>,
    choices: &BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let mut ctx = BTreeMap::new();
    for a in answers {
        let code = code_by_id.get(&a.question_id);
        let raw = store::answers::display_value(&a.value);
        // A single/multiple-choice answer stores the choice *key*; render
        // its human *label* wherever a `{{state}}` / `{{code}}` placeholder
        // shows it (and wherever the AcroForm fill reads it). Keyed on the
        // answer's full state, so the right question's options resolve;
        // falls back to the raw value for a free-text answer.
        let display = a
            .state_name
            .as_deref()
            .or(code.map(String::as_str))
            .and_then(|state| workflows::choice_label(choices, state, &raw))
            .unwrap_or_else(|| raw.clone());
        // Bare canonical code → latest answer (for a `{{code}}` placeholder).
        if let Some(code) = code {
            ctx.insert(code.clone(), display.clone());
        }
        // Full `<type>__<role>` state → this answer (for `{{type__role}}`),
        // falling back to the bare code when the answer carries no role.
        // Storing the state on the row is what stops two records of one
        // type collapsing — no insertion-order alignment needed.
        if let Some(key) = a.state_name.clone().or_else(|| code.cloned()) {
            ctx.insert(key.clone(), display);
            insert_dotted_answer_fields(&mut ctx, &key, &a.value);
        }
    }
    ctx
}

fn insert_dotted_answer_fields(
    ctx: &mut BTreeMap<String, String>,
    key: &str,
    value: &serde_json::Value,
) {
    let Some(fields) = value.as_object() else {
        return;
    };
    for (field, value) in fields {
        if field == "value" {
            continue;
        }
        ctx.insert(format!("{key}.{field}"), json_scalar(value));
    }
}

fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Record the e-signature provider's request id in `signatures` so a
/// later completion webhook can resolve its callback by `(provider,
/// provider_id)`. See [`crate::esignature_webhook`].
async fn persist_signature_request_id(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    request_id: &str,
) -> Result<(), store::signatures::SignatureError> {
    store::signatures::record_request(
        surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        request_id,
    )
    .await?;
    Ok(())
}

async fn sync_notation_state(
    surreal: &store::surreal::SurrealDb,
    notation_id: Uuid,
    new_state: &str,
) -> Result<(), String> {
    store::notations::update_state(surreal, notation_id, new_state)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The linear question order the walker presents: follow the `_`
/// transition from BEGIN until END. This chain is the source of the
/// rendered "step N of M", so it must cover every question state the
/// spec declares — the corpus test
/// `every_shipped_questionnaire_renders_an_honest_step_count` holds
/// the two shapes equal for every shipped template.
fn questionnaire_chain(spec: &workflows::QuestionnaireSpec) -> Vec<StateName> {
    let mut order: Vec<StateName> = Vec::new();
    let mut here = StateName::begin();
    while let Some(next) = spec
        .transitions_from(&here)
        .and_then(|t| t.lookup("_"))
        .cloned()
    {
        if next == StateName::end() || order.contains(&next) {
            break;
        }
        order.push(next.clone());
        here = next;
    }
    order
}

/// `(current, total)` for the progress indicator.
///
/// `total` is the length of the [`questionnaire_chain`] — equal to the
/// count of every declared state except `BEGIN` and `END` by
/// construction: `QuestionnaireSpec` validation rejects any spec whose
/// `_` chain doesn't cover every state and terminate at END. `current` is
/// `1 + index of the next question after `current_state` on that
/// chain. If `current_state` is `BEGIN`, we're on question 1.
fn progress_for(spec: &workflows::QuestionnaireSpec, current_state: &StateName) -> (usize, usize) {
    progress_from_chain(&questionnaire_chain(spec), current_state)
}

/// Progress `(current, total)` for the walker's chrome, sourced from the
/// notation's frozen questionnaire snapshot — the scoped questionnaire the
/// client actually answers when the pinned template carried its own blob, not
/// the compile-time bundled spec. Falls back to the shipped retainer spec held
/// in `AppState` if the snapshot can't be read, so the retainer flow is
/// unchanged.
async fn walker_progress(
    state: &AdminState,
    notation_id: Uuid,
    current_state: &StateName,
) -> (usize, usize) {
    match notation_session::questionnaire_chain_for_notation(
        &state.surreal,
        Some(&state.storage),
        notation_id,
    )
    .await
    {
        Ok(codes) => {
            let order: Vec<StateName> = codes
                .into_iter()
                .map(|c| StateName::from(c.as_str()))
                .collect();
            progress_from_chain(&order, current_state)
        }
        Err(_) => progress_for(&state.retainer_intake_questionnaire, current_state),
    }
}

/// Progress `(current, total)` for a `current_state` within an ordered
/// question chain (BEGIN → … → END, minus the terminals). Split out from
/// [`progress_for`] so the walker can feed it the chain sourced from the
/// notation's frozen snapshot — the scoped questionnaire the client actually
/// answers — rather than re-deriving it from the template code.
fn progress_from_chain(order: &[StateName], current_state: &StateName) -> (usize, usize) {
    let total = order.len();
    let current = if current_state == &StateName::begin() {
        1
    } else {
        order
            .iter()
            .position(|s| s == current_state)
            .map_or(total, |i| i + 2)
            .min(total)
    };
    (current, total)
}

#[cfg(test)]
mod tests {
    use super::{context_from_answers, progress_for, substitute_template_body};
    use std::collections::BTreeMap;
    use uuid::Uuid;
    use workflows::{retainer_intake_questionnaire, StateName};

    /// Build an answer row carrying `state_name` and a primitive value, as
    /// the walker write sites now do.
    fn answer_row(question_id: Uuid, state_name: &str, value: &str) -> store::answers::Answer {
        record_answer_row(question_id, state_name, store::answers::primitive(value))
    }

    fn record_answer_row(
        question_id: Uuid,
        state_name: &str,
        value: serde_json::Value,
    ) -> store::answers::Answer {
        store::answers::Answer {
            id: Uuid::now_v7(),
            question_id,
            person_id: Uuid::nil(),
            notation_id: Some(Uuid::nil()),
            state_name: Some(state_name.to_string()),
            value,
            source: store::answers::SOURCE_LAWYER.to_string(),
            authored_by_person_id: None,
            inserted_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn progress_for_begin_is_step_1() {
        // Eight questions: entity, principal office, client identity, firm
        // DRI, engagement name, engagement start date, engagement scope,
        // governing law (N120 grounded the four bare placeholders the
        // retainer body used to leave undeclared).
        let spec = retainer_intake_questionnaire();
        assert_eq!(progress_for(&spec, &StateName::begin()), (1, 8));
    }

    #[test]
    fn progress_for_client_state_is_step_2() {
        // After answering the entity question, the next question is the
        // entity's principal office — the walker should display "step 2 of
        // 8."
        let spec = retainer_intake_questionnaire();
        assert_eq!(progress_for(&spec, &StateName::from("entity")), (2, 8));
    }

    #[test]
    fn progress_for_last_answered_question_caps_at_total() {
        let spec = retainer_intake_questionnaire();
        assert_eq!(
            progress_for(
                &spec,
                &StateName::from("custom_single_choice__governing_law")
            ),
            (8, 8)
        );
    }

    #[test]
    fn context_keys_role_state_and_bare_code() {
        // One `entity__company` answer keys both its full state and the
        // bare `entity` code for a `{{entity}}` placeholder.
        let entity_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(entity_q, "entity".to_string())]);
        let answers = [answer_row(entity_q, "entity__company", "Libra LLC")];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("entity__company").map(String::as_str),
            Some("Libra LLC")
        );
        assert_eq!(ctx.get("entity").map(String::as_str), Some("Libra LLC"));
    }

    #[test]
    fn context_renders_single_choice_answers_as_labels_not_keys() {
        // A single-choice answer stores the choice *key* (`five_year`); the
        // render context must surface its *label* so the document reads
        // "Five years as a permanent resident", not "five_year". Keyed on
        // the answer's state, the choice metadata resolves via the
        // custom-question key (`eligibility_basis`).
        let basis_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(basis_q, "custom_single_choice".to_string())]);
        let answers = [answer_row(
            basis_q,
            "custom_single_choice__eligibility_basis",
            "five_year",
        )];
        let choices = BTreeMap::from([(
            "eligibility_basis".to_string(),
            BTreeMap::from([(
                "five_year".to_string(),
                "Five years as a permanent resident".to_string(),
            )]),
        )]);

        let ctx = context_from_answers(&answers, &code_by_id, &choices);

        assert_eq!(
            ctx.get("custom_single_choice__eligibility_basis")
                .map(String::as_str),
            Some("Five years as a permanent resident"),
        );
        // A free-text answer with no choice metadata falls back to the raw
        // value — unaffected by the label lookup.
        let text_q = Uuid::now_v7();
        let text_answers = [answer_row(text_q, "custom_text__time_outside_us", "45")];
        let text_ctx = context_from_answers(
            &text_answers,
            &BTreeMap::from([(text_q, "custom_text".to_string())]),
            &choices,
        );
        assert_eq!(
            text_ctx
                .get("custom_text__time_outside_us")
                .map(String::as_str),
            Some("45"),
        );
    }

    #[test]
    fn context_keeps_single_choice_keys_for_form_fill_context() {
        // AcroForm fill paths pass no choice-label metadata into the pure
        // context builder, so a radio group receives the stored on-state key
        // instead of the human label used by Typst previews/documents.
        let basis_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(basis_q, "custom_single_choice".to_string())]);
        let answers = [answer_row(
            basis_q,
            "custom_single_choice__eligibility_basis",
            "five_year",
        )];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("custom_single_choice__eligibility_basis")
                .map(String::as_str),
            Some("five_year"),
        );
        assert_eq!(
            ctx.get("custom_single_choice").map(String::as_str),
            Some("five_year"),
        );
    }

    #[test]
    fn governing_law_defaults_to_nevada_only_when_unanswered() {
        use super::{default_governing_law, ChoiceRendering};

        // A frozen intake that predates the fillable clause never stored a
        // governing_law answer. Document rendering (Labels) defaults to the
        // Nevada *label* so the binding retainer never shows the raw
        // `{{custom_single_choice__governing_law}}` placeholder.
        let mut labels = BTreeMap::new();
        default_governing_law(&mut labels, ChoiceRendering::Labels);
        assert_eq!(
            labels
                .get("custom_single_choice__governing_law")
                .map(String::as_str),
            Some("Nevada"),
        );

        // Form-fill (Keys) defaults to the stored *value*.
        let mut keys = BTreeMap::new();
        default_governing_law(&mut keys, ChoiceRendering::Keys);
        assert_eq!(
            keys.get("custom_single_choice__governing_law")
                .map(String::as_str),
            Some("nevada"),
        );

        // Answers win: a captured choice is never overwritten by the default.
        let mut answered = BTreeMap::from([(
            "custom_single_choice__governing_law".to_string(),
            "California".to_string(),
        )]);
        default_governing_law(&mut answered, ChoiceRendering::Labels);
        assert_eq!(
            answered
                .get("custom_single_choice__governing_law")
                .map(String::as_str),
            Some("California"),
        );
    }

    #[test]
    fn context_expands_singular_record_answer_fields() {
        let person_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(person_q, "person".to_string())]);
        let answers = [record_answer_row(
            person_q,
            "person__client",
            serde_json::json!({
                "id": Uuid::now_v7(),
                "name": "Libra",
                "email": "libra@example.com",
            }),
        )];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("person__client.name").map(String::as_str),
            Some("Libra")
        );
        assert_eq!(
            ctx.get("person__client.email").map(String::as_str),
            Some("libra@example.com")
        );
    }

    #[test]
    fn final_document_payload_uses_shared_data_evaluator_before_signature_anchors() {
        let body = "Client: {{person__client.name}}\n\
Members:\n{{#for m in people__members}}- {{m.name}} from {{m.city}}\n{{/for}}\n\
Sign: {{client.signature}}";
        let ctx = BTreeMap::from([
            ("person__client.name".to_string(), "Libra Prime".to_string()),
            (
                "people__members".to_string(),
                r#"[{"name":"Aries","city":"Las Vegas"},{"name":"Virgo","city":"Reno"}]"#
                    .to_string(),
            ),
        ]);

        let data_filled = substitute_template_body(body, &ctx);
        assert!(data_filled.contains("Client: Libra Prime"));
        assert!(data_filled.contains("- Aries from Las Vegas"));
        assert!(data_filled.contains("- Virgo from Reno"));
        assert!(
            data_filled.contains("{{client.signature}}"),
            "signature placeholders are expanded after data fill"
        );

        let (typst_source, fields) = crate::signature_render::expand_signatures(&data_filled);
        assert!(!typst_source.contains("{{person__client.name}}"));
        assert!(!typst_source.contains("{{#for"));
        assert!(!typst_source.contains("{{client.signature}}"));
        assert!(typst_source.contains("nlsig-client-signature-1"));
        assert_eq!(fields.len(), 1);
    }

    /// A bound Person carrying a distinct name/email, so a fixture can tell
    /// the answered value apart from the Person-row fallback.
    fn person_named(name: &str, email: &str) -> store::persons::Person {
        store::persons::Person {
            id: Uuid::now_v7(),
            name: name.to_string(),
            given_name: None,
            family_name: None,
            middle_name: None,
            email: email.to_string(),
            oidc_subject: None,
            role: store::persons::Role::Client,
            title: None,
            phone: None,
            xero_contact_id: None,
            profile_image_url: None,
            inserted_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn person_client_structured_name_flattens_to_dotted_answer_keys() {
        // `seed_lawyer_answer_value` exposes the client's structured
        // legal-name parts on the `person__client` object; the flatten
        // must surface them as `person__client.family/given/middle` so
        // the re-authored N-400 fills Part 2 Line 1 (#311). An unset part
        // is JSON `null` → empty string, which the fill path skips,
        // leaving that name box for a lawyer.
        let mut ctx = BTreeMap::new();
        let value = serde_json::json!({
            "value": "María Santos Gómez",
            "name": "María Santos Gómez",
            "family": "Santos Gómez",
            "given": "María",
            "middle": serde_json::Value::Null,
            "email": "maria@example.com",
        });
        super::insert_dotted_answer_fields(&mut ctx, "person__client", &value);

        assert_eq!(
            ctx.get("person__client.family").map(String::as_str),
            Some("Santos Gómez")
        );
        assert_eq!(
            ctx.get("person__client.given").map(String::as_str),
            Some("María")
        );
        assert_eq!(
            ctx.get("person__client.middle").map(String::as_str),
            Some("")
        );
        assert_eq!(
            ctx.get("person__client.email").map(String::as_str),
            Some("maria@example.com")
        );
        // The `value` sentinel is the display string, never a dotted field.
        assert!(!ctx.contains_key("person__client.value"));
    }

    fn client_signature_field() -> crate::signature::SignatureField {
        crate::signature::SignatureField {
            recipient_role: "client".into(),
            kind: crate::signature::SignatureFieldKind::Signature,
            anchor: "{{client.signature}}".into(),
        }
    }

    #[test]
    fn signature_manifest_prefers_answered_client_name_over_person_row() {
        // The migration renamed the client-identity state to
        // `person__client`, whose `.name`/`.email` fields land in the render
        // context. The manifest must read those — not the removed
        // `custom_text__client_*` keys — so the questionnaire-confirmed
        // legal name reaches the DocuSign recipient. In production the bound
        // Person is created with `name = <email>` until the questionnaire
        // fills it, so a fallback silently signs the client under their
        // email address. The fixture name differs from the answer to prove
        // the answered value wins.
        let ctx = BTreeMap::from([
            ("person__client.name".to_string(), "Libra Prime".to_string()),
            (
                "person__client.email".to_string(),
                "libra@example.com".to_string(),
            ),
        ]);
        let client = person_named("libra@example.com", "libra@example.com");
        let fields = [client_signature_field()];

        let manifest =
            super::build_signature_manifest(Uuid::now_v7(), &fields, &ctx, Some(&client), true);

        let recipient = manifest
            .recipients
            .iter()
            .find(|r| r.role == "client")
            .expect("client recipient present");
        assert_eq!(recipient.name, "Libra Prime");
        assert_eq!(recipient.email, "libra@example.com");
    }

    #[test]
    fn signature_manifest_falls_back_to_person_row_when_unanswered() {
        // The trust questionnaire never captures an email and other flows
        // capture it out-of-band, so with no `person__client.*` in context
        // the recipient resolves from the bound Person row.
        let ctx = BTreeMap::new();
        let client = person_named("Libra Prime", "libra@example.com");
        let fields = [client_signature_field()];

        let manifest =
            super::build_signature_manifest(Uuid::now_v7(), &fields, &ctx, Some(&client), true);

        let recipient = manifest
            .recipients
            .iter()
            .find(|r| r.role == "client")
            .expect("client recipient present");
        assert_eq!(recipient.name, "Libra Prime");
        assert_eq!(recipient.email, "libra@example.com");
    }

    #[test]
    fn context_does_not_collapse_two_records_of_one_type() {
        // The data-loss bug: `entity__company` and `entity__subsidiary`
        // both point at the bare `entity` question. Keying on the stored
        // state keeps them distinct — the old prefix-collapse let the
        // second overwrite the first.
        let entity_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(entity_q, "entity".to_string())]);
        let answers = [
            answer_row(entity_q, "entity__company", "Libra LLC"),
            answer_row(entity_q, "entity__subsidiary", "Libra Sub LLC"),
        ];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("entity__company").map(String::as_str),
            Some("Libra LLC"),
            "the first record must survive the second"
        );
        assert_eq!(
            ctx.get("entity__subsidiary").map(String::as_str),
            Some("Libra Sub LLC")
        );
    }

    #[test]
    fn context_two_typed_states_keep_their_own_answers() {
        // Two `custom_text__*` states share the canonical `custom_text`
        // question; each placeholder renders the answer given for *that*
        // state.
        let custom_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(custom_q, "custom_text".to_string())]);
        let answers = [
            answer_row(
                custom_q,
                "custom_text__mission_statement",
                "Expand legal access",
            ),
            answer_row(
                custom_q,
                "custom_text__revenue_strategy",
                "Flat-fee retainers",
            ),
        ];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("custom_text__mission_statement")
                .map(String::as_str),
            Some("Expand legal access")
        );
        assert_eq!(
            ctx.get("custom_text__revenue_strategy").map(String::as_str),
            Some("Flat-fee retainers")
        );
    }

    #[test]
    fn context_reanswered_state_uses_latest_answer() {
        // Append-only: two answers for one state resolve to the freshest,
        // since rows arrive in ascending-id order and the last write wins.
        let custom_q = Uuid::now_v7();
        let code_by_id = BTreeMap::from([(custom_q, "custom_text".to_string())]);
        let answers = [
            answer_row(custom_q, "custom_text__mission_statement", "First draft"),
            answer_row(custom_q, "custom_text__mission_statement", "Final answer"),
        ];

        let ctx = context_from_answers(&answers, &code_by_id, &BTreeMap::new());

        assert_eq!(
            ctx.get("custom_text__mission_statement")
                .map(String::as_str),
            Some("Final answer")
        );
    }
}
