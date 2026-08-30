//! `POST /app/projects/{project_code}/notations/new` — open a
//! notation for an **existing** Project from a template authored in that
//! Project's git repo.
//!
//! This is the project-scoped half of the one `notation create` front door
//! (issue #252, slice 2). It is a thin adapter: it resolves the project and
//! the client Person, then hands off to
//! [`workflows::create_notation_from_repo`], which runs the shared
//! `read-repo → validate → persist` engine ([`store::template_source`]) and
//! opens the notation pinned to the just-saved immutable version — the same
//! function the CLI's `notation create --project` drives over HTTP, so both
//! surfaces auto-save the template the same way with no separate `import`
//! step. Creation is lawyer-only *and* matter-scoped: the route lives under
//! `/app/lawyer` and additionally requires the acting lawyer to participate
//! in the target project (`access::can_see_project`; admin bypasses), so a
//! lawyer session cannot open a notation in a matter outside its scope.
//! `lawyer_review` remains the gate before any binding step (enforced
//! downstream by N116 + the workflow).

use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use serde::Deserialize;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;
use store::persons::Role;
use store::template_source::TemplateSourceError;
use workflows::{NotationSessionError, StartOutcome, StateMachineRuntime};

/// POST body — the template code to read from the repo and the client the
/// notation is bound to. Everything else the questionnaire collects.
#[derive(Debug, Clone, Deserialize)]
pub struct NewProjectNotationBody {
    pub template_code: String,
    pub client_email: String,
}

/// The matter-open notation command, shared by the lawyer browser form
/// ([`project_notation_new_post`]) and the REST door
/// (`POST /app/api/projects/{id}/notations`). It resolves the matter and the
/// client Person, enforces matter scope, then hands off to the shared
/// `read-repo → validate → persist` engine ([`workflows::create_notation_from_repo`]).
/// Both doors converge here so the matter-scope rule and the client-resolution
/// behavior can never drift between the browser and the API — the convergence
/// issue #355 applies to every write.
#[allow(clippy::too_many_arguments)]
pub async fn create_project_notation(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: &Arc<dyn cloud::StorageService>,
    acting_person_id: Option<Uuid>,
    acting_role: Role,
    project_id: Uuid,
    template_code: &str,
    client_email: &str,
) -> Result<StartOutcome, CreateProjectNotationError> {
    let code = template_code.trim();
    let email = client_email.trim();
    if code.is_empty() || email.is_empty() {
        return Err(CreateProjectNotationError::EmptyInput);
    }

    // The matter must already exist — this command opens a notation *within*
    // it, unlike the onboarding retainer walk that creates the project.
    let project = match store::projects::find_by_id(surreal, project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return Err(CreateProjectNotationError::ProjectNotFound),
        Err(e) => {
            return Err(CreateProjectNotationError::Db(e.to_string()));
        }
    };

    // Matter-scoped: creation is not merely lawyer-tier, it is bound to the
    // matter. The acting lawyer must participate in this project (admin
    // bypasses), the same rule the contract-review routes enforce. A miss maps
    // to "not found" so neither door discloses a project outside the caller's
    // scope.
    let in_scope = store::access::can_see_project_as_lawyer(
        surreal,
        acting_person_id,
        acting_role,
        project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(CreateProjectNotationError::ProjectNotFound);
    }

    let person_id = find_or_create_client(surreal, email, project_id).await?;

    // The repo layer is env-selected, exactly like the documents export path
    // (`project_export.rs`); a process with no repo root cannot read a repo.
    let repo = repos::RepoStore::from_env().map_err(|e| {
        tracing::error!(error = %e, "create_project_notation: git repo storage not configured");
        CreateProjectNotationError::RepoUnconfigured
    })?;

    workflows::create_notation_from_repo(
        surreal,
        runtime,
        storage,
        &repo,
        code,
        person_id,
        project_id,
        Some(project.entity_id),
    )
    .await
    .map_err(CreateProjectNotationError::Session)
}

/// Typed failure of [`create_project_notation`]. Each door renders it its own
/// way — the lawyer form to friendly text plus a redirect on success, the REST
/// door to a typed JSON `ApiError` — but the *decisions* live here, once.
#[derive(Debug)]
pub enum CreateProjectNotationError {
    /// `template_code` or `client_email` was blank.
    EmptyInput,
    /// No such matter, or the acting lawyer does not participate in it. Both
    /// collapse to "not found" so the door never discloses an out-of-scope
    /// project.
    ProjectNotFound,
    /// The client email already belongs to a different Person.
    ///
    /// Nothing constructs this now. [`find_or_create_client`] resolves an
    /// email to whichever person holds it — including one created a
    /// moment earlier by a concurrent request — so there is no longer a
    /// case where the address is "someone else's". The variant and its
    /// documented `409` stay because they are part of the published API
    /// contract (`openapi.rs`); retiring them is an API decision rather
    /// than a consequence of this port.
    ClientEmailTaken,
    /// The repo storage backend is not configured on this process.
    RepoUnconfigured,
    /// The shared notation engine refused — template missing, engagement
    /// ordering, invalid paper, or an internal failure.
    Session(NotationSessionError),
    /// A database read or write failed.
    Db(String),
    /// A read or write against the person directory failed.
    Person(store::persons::PersonError),
}

impl CreateProjectNotationError {
    /// Render for the lawyer browser form — friendly text and the same statuses
    /// the handler returned before the command was extracted.
    fn into_lawyer_response(self) -> Response {
        match self {
            Self::EmptyInput => (
                StatusCode::BAD_REQUEST,
                "template_code and client_email are required",
            )
                .into_response(),
            Self::ProjectNotFound => (StatusCode::NOT_FOUND, "project not found").into_response(),
            Self::Person(e) => {
                tracing::error!(error = %e, "project_notation_new: person directory failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
            }
            Self::ClientEmailTaken => (
                StatusCode::CONFLICT,
                "that client email already belongs to another person",
            )
                .into_response(),
            Self::RepoUnconfigured => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "git repo storage is not configured",
            )
                .into_response(),
            // The code was authored in neither this Project's repo nor the
            // bundled firm catalog. (A repo miss alone falls back to the
            // catalog inside `create_notation_from_repo`; this fires only when
            // the catalog has no such code either.)
            Self::Session(NotationSessionError::TemplateNotFound(code)) => (
                StatusCode::NOT_FOUND,
                format!("no template `{code}` in this project's repo or the firm catalog"),
            )
                .into_response(),
            // The matter has no notation yet and this template is not an
            // engagement (a retainer or an onboarding).
            Self::Session(NotationSessionError::EngagementMustBeFirst { code, kind }) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "the first notation on this matter must be the engagement that opens it — \
                     a retainer or an onboarding — `{code}` is kind `{kind}`. Open the \
                     engagement first, then add this."
                ),
            )
                .into_response(),
            Self::Session(NotationSessionError::TemplateSource(TemplateSourceError::Invalid {
                code,
                violations,
            })) => {
                tracing::warn!(%code, count = violations.len(), "project_notation_new: template refused");
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "template `{code}` has {} blocking rule violation(s); fix it in the repo and retry",
                        violations.len()
                    ),
                )
                    .into_response()
            }
            Self::Session(e) => {
                tracing::error!(error = %e, "project_notation_new: create failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
            }
            Self::Db(e) => {
                tracing::error!(error = %e, "project_notation_new: database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
            }
        }
    }
}

pub async fn project_notation_new_post(
    State(state): State<AdminState>,
    AxumPath(project_code): AxumPath<String>,
    session: Option<Extension<SessionData>>,
    Form(body): Form<NewProjectNotationBody>,
) -> Response {
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return (StatusCode::NOT_FOUND, "matter not found").into_response();
    };

    // The route lives under `/app/lawyer`, so the tier is already enforced; the
    // command re-checks matter scope from the session. No session → no scope.
    let (acting_person_id, acting_role) = match session.as_deref() {
        Some(s) => (s.person_id, s.role),
        None => (None, Role::Client),
    };

    match create_project_notation(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        &state.storage,
        acting_person_id,
        acting_role,
        project_id,
        &body.template_code,
        &body.client_email,
    )
    .await
    {
        Ok(outcome) => Redirect::to(&format!(
            "/app/lawyer/notations/{}/step",
            outcome.notation_id
        ))
        .into_response(),
        Err(e) => e.into_lawyer_response(),
    }
}

/// Resolve the client Person by email, creating a `client`-role Person on
/// first sight, and ensure they participate in the project (so the matter's
/// client can see their own notation). Mirrors `link_retainer_rows`.
async fn find_or_create_client(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    project_id: Uuid,
) -> Result<Uuid, CreateProjectNotationError> {
    // `find_or_create` rather than look-then-create: two lawyers naming the
    // same new client at once would otherwise leave the slower one holding
    // a unique-index violation there is no honest way to report — it is
    // not "this email belongs to someone else", it is the very client
    // being added, a moment earlier.
    let person_id = match store::persons::find_or_create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, store::persons::Role::Client),
    )
    .await
    {
        Ok(person) => person.id,
        Err(e) => {
            tracing::error!(error = %e, "project_notation_new: person lookup failed");
            return Err(CreateProjectNotationError::Person(e));
        }
    };

    // Attach client participation if not already present (idempotent).
    if store::projects::participation_for_person(surreal, person_id, project_id)
        .await
        .map_err(|error| CreateProjectNotationError::Db(error.to_string()))?
        .is_none()
    {
        store::projects::add_participation(surreal, project_id, person_id, "client")
            .await
            .map_err(|error| CreateProjectNotationError::Db(error.to_string()))?;
    }

    Ok(person_id)
}
