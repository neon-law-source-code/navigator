//! JSON API surface.
//!
//! Read-only listings for the core domain tables. Every handler is a
//! thin SeaORM query + `Json(...)` response — no extra serializer
//! between the model and the wire because the entity `Model`s
//! derive `Serialize` directly.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequest, FromRequestParts, Multipart, Path, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::people_commands::{PeopleCommandError, UpdateContext};
use crate::SessionData;

/// State the `/app/api/*` router runs against. Read handlers extract just
/// `State<SurrealDb>` via the [`FromRef`] below; the People command
/// handlers (delete/update guard the bootstrap Owner, welcome dispatches
/// email) reach the extra seams through `State<ApiState>`.
#[derive(Clone)]
pub struct ApiState {
    /// The store. `persons` lives here, so the `/app/api/people*` surface and
    /// every participant reference read it.
    pub surreal: store::surreal::SurrealDb,
    pub email: Arc<dyn crate::email::EmailService>,
    pub bootstrap_owner_email: Option<String>,
    /// The operator's firm Entity name (`NAVIGATOR_BOOTSTRAP_COMPANY`),
    /// resolved once at router build. The Entity create command refuses to
    /// mint a second row under it, so a white-label operator's anchor is as
    /// unforkable through the API as through the lawyer form.
    pub bootstrap_company: String,
    /// The questionnaire timeline runtime, threaded through so the matter-open
    /// notation command (`POST /app/api/projects/{id}/notations`) can start a
    /// notation's questionnaire — the same runtime the lawyer notation door and
    /// the retainer walk use.
    pub questionnaire_runtime: Arc<dyn workflows::StateMachineRuntime>,
    /// Object storage, threaded through for the notation command (it persists
    /// the notation's frozen questionnaire snapshot).
    pub storage: Arc<dyn cloud::StorageService>,
    /// The post-questionnaire workflow runtime, for the notation-approve
    /// command (`POST /app/api/notations/{id}/approval`) which fires the `approved`
    /// transition. Distinct from `questionnaire_runtime`: they run different
    /// state machines (see `AppState`).
    pub workflow_runtime: Arc<dyn workflows::StateMachineRuntime>,
    /// The public assets bucket the government-form blank is pulled from during
    /// approve (the AcroForm-fill path).
    pub assets_storage: Arc<dyn cloud::StorageService>,
    /// The vendored government-form registry (AcroForm field maps + pins) the
    /// approve command reads when a template declares a `form:` binding.
    pub forms_registry: Arc<Vec<forms::FormMeta>>,
    /// The e-signature provider, for the send command
    /// (`POST /app/api/notations/{id}/signature`) which dispatches the reviewed
    /// PDF for signature.
    pub signature_provider: Arc<dyn crate::signature::SignatureProvider>,
    /// The contract-review deviation analyzer (the LLM reviewer, or the stub),
    /// for `POST /app/api/projects/{id}/contract-review`.
    pub contract_reviewer: Arc<dyn crate::contract_review::ContractReviewer>,
}

impl FromRef<ApiState> for store::surreal::SurrealDb {
    fn from_ref(s: &ApiState) -> Self {
        s.surreal.clone()
    }
}

/// The single registration table for the `/app/api/*` surface: one row per
/// operation as `(HTTP method, path, handler)`, in OpenAPI path-template
/// form (`{id}`, not axum's `:id`).
///
/// Both [`routes`] (which folds rows sharing a path into one
/// `MethodRouter`) and [`documented_api_operations`] (which reads the
/// `(method, path)` pairs) derive from *this* table — so an operation
/// cannot exist on the router without appearing in the inventory the
/// drift guard checks. That closes the route-drift shape entirely: a new
/// route added here can never be silently absent from
/// `documented_api_operations()`, so `web/tests/openapi_drift.rs` will
/// flag it against the OpenAPI document even if the author forgot the
/// doc. (The live-router probe in `web/tests/routes.rs` can only probe
/// paths it already knows; deriving the inventory from the table is what
/// catches an entirely new undocumented path.)
#[allow(clippy::too_many_lines)]
fn api_operation_table() -> Vec<(&'static str, &'static str, MethodRouter<ApiState>)> {
    use axum::routing::{delete, get, patch, post, put};
    vec![
        ("GET", "/app/api/people", get(list_people)),
        ("POST", "/app/api/people", post(create_person)),
        ("GET", "/app/api/people/{id}", get(get_person)),
        ("PATCH", "/app/api/people/{id}", patch(update_person)),
        ("DELETE", "/app/api/people/{id}", delete(delete_person)),
        ("POST", "/app/api/people/{id}/welcome", post(send_welcome)),
        ("GET", "/app/api/entities", get(list_entities)),
        ("POST", "/app/api/entities", post(create_entity)),
        ("POST", "/app/api/seed", post(reconcile_seed)),
        ("GET", "/app/api/entities/{id}", get(get_entity)),
        ("PATCH", "/app/api/entities/{id}", patch(update_entity)),
        ("DELETE", "/app/api/entities/{id}", delete(delete_entity)),
        ("GET", "/app/api/jurisdictions", get(list_jurisdictions)),
        ("GET", "/app/api/entity-types", get(list_entity_types)),
        // Deliberately *not* under `/app/api/projects/`: the policy rule for
        // that prefix admits any authenticated caller up to five segments, so a
        // reconciliation path nested there would be policy-reachable by a
        // client even though the handler refuses one. Its own noun keeps the
        // admin-only rule exact.
        (
            "GET",
            "/app/api/project-repositories",
            get(reconcile_project_repositories_door),
        ),
        // Own noun, same reason as `project-repositories`: the `projects`
        // GET rule admits any authenticated caller up to five segments, and
        // an admin-only reconcile nested there would be policy-reachable by
        // a client even though the handler refuses one.
        (
            "POST",
            "/app/api/project-surfaces/{id}",
            post(reconcile_project_surfaces_door),
        ),
        // Read clusters (#866): the matter-centric reads the portal pages load.
        ("GET", "/app/api/projects", get(list_projects_door)),
        ("GET", "/app/api/projects/{id}", get(get_project_door)),
        (
            "GET",
            "/app/api/projects/{id}/participants",
            get(list_participants_door),
        ),
        (
            "GET",
            "/app/api/projects/{id}/notations",
            get(list_notations_door),
        ),
        ("GET", "/app/api/notations/{id}", get(get_notation_door)),
        (
            "GET",
            "/app/api/notations/{id}/review-documents",
            get(list_review_documents_door),
        ),
        (
            "GET",
            "/app/api/projects/{id}/documents",
            get(list_documents_door),
        ),
        (
            "GET",
            "/app/api/projects/{id}/conversation",
            get(get_conversation_door),
        ),
        (
            "GET",
            "/app/api/expunge-requests",
            get(list_expunge_requests_door),
        ),
        ("GET", "/app/api/playbooks", get(list_playbooks_door)),
        ("GET", "/app/api/playbooks/{id}", get(get_playbook_door)),
        (
            "GET",
            "/app/api/contract-reviews/{id}",
            get(get_contract_review_door),
        ),
        ("POST", "/app/api/projects", post(open_project)),
        ("PATCH", "/app/api/projects/{id}", patch(update_project)),
        ("DELETE", "/app/api/projects/{id}", delete(delete_project)),
        ("POST", "/app/api/projects/{id}/close", post(close_matter)),
        (
            "POST",
            "/app/api/projects/{id}/conversation/messages",
            post(post_conversation_message_door),
        ),
        (
            "POST",
            "/app/api/projects/{id}/participants",
            post(add_participant),
        ),
        (
            "PATCH",
            "/app/api/projects/{id}/participants/{role_id}",
            patch(update_participant),
        ),
        (
            "DELETE",
            "/app/api/projects/{id}/participants/{role_id}",
            delete(remove_participant),
        ),
        (
            "PUT",
            "/app/api/projects/{id}/participants/{role_id}/dri",
            put(designate_participant_dri),
        ),
        (
            "DELETE",
            "/app/api/projects/{id}/participants/{role_id}/dri",
            delete(remove_participant_dri),
        ),
        (
            "POST",
            "/app/api/projects/{id}/notations",
            post(create_notation),
        ),
        (
            "POST",
            "/app/api/notations/{id}/answers",
            post(answer_notation_step),
        ),
        (
            "POST",
            "/app/api/notations/{id}/request-changes",
            post(request_notation_changes_door),
        ),
        (
            "POST",
            "/app/api/notations/{id}/reask",
            post(resubmit_reask_door),
        ),
        (
            "POST",
            "/app/api/notations/{id}/transcript",
            post(transcript_coverage_door),
        ),
        (
            "POST",
            "/app/api/notations/{id}/intake",
            post(send_notation_intake),
        ),
        (
            "POST",
            "/app/api/notations/{id}/approval",
            post(approve_notation),
        ),
        (
            "POST",
            "/app/api/notations/{id}/signature",
            post(send_notation_signature),
        ),
        (
            "POST",
            "/app/api/review-documents/{id}/comments",
            post(add_review_comment),
        ),
        (
            "POST",
            "/app/api/documents/{id}/deletion-requests",
            post(create_deletion_request),
        ),
        ("POST", "/app/api/notations/{id}/clauses", post(add_clause)),
        (
            "POST",
            "/app/api/projects/{id}/contract-review",
            post(upload_contract_review),
        ),
        (
            "POST",
            "/app/api/projects/{id}/documents",
            post(upload_document_door),
        ),
        (
            "PATCH",
            "/app/api/notations/{id}/clauses/{clause_id}",
            patch(edit_clause),
        ),
        (
            "DELETE",
            "/app/api/notations/{id}/clauses/{clause_id}",
            delete(delete_clause),
        ),
        (
            "POST",
            "/app/api/notations/{id}/clauses/{clause_id}/move",
            post(move_clause),
        ),
        ("POST", "/app/api/playbooks", post(create_playbook_door)),
        ("PUT", "/app/api/playbooks/{id}", put(update_playbook_door)),
        (
            "POST",
            "/app/api/contract-reviews/{id}/findings/{idx}",
            post(save_review_finding_door),
        ),
        (
            "POST",
            "/app/api/contract-reviews/{id}/summary",
            post(save_review_summary_door),
        ),
        (
            "POST",
            "/app/api/contract-reviews/{id}/approve",
            post(approve_review_door),
        ),
        (
            "POST",
            "/app/api/contract-reviews/{id}/reject",
            post(reject_review_door),
        ),
        (
            "POST",
            "/app/api/expunge-requests/{id}/authorize",
            post(authorize_expunge_door),
        ),
        (
            "POST",
            "/app/api/expunge-requests/{id}/deny",
            post(deny_expunge_door),
        ),
        (
            "POST",
            "/app/api/templates/validate",
            post(validate_template),
        ),
    ]
}

/// Mount every gated `/app/api/*` data route onto a router. Every route here
/// goes behind `require_policy` (embedded Rego policy) in [`crate::bootstrap`]; the
/// public documentation surfaces live in [`doc_routes`], which mounts
/// *outside* that gate.
///
/// Built by folding [`api_operation_table`]: rows sharing a path are
/// merged into one `MethodRouter` (axum panics on a duplicate `.route`
/// for the same path), preserving table order.
pub fn routes() -> Router<ApiState> {
    let mut by_path: Vec<(&'static str, MethodRouter<ApiState>)> = Vec::new();
    for (_method, path, method_router) in api_operation_table() {
        if let Some(slot) = by_path.iter_mut().find(|(p, _)| *p == path) {
            let merged = std::mem::replace(&mut slot.1, MethodRouter::new()).merge(method_router);
            slot.1 = merged;
        } else {
            by_path.push((path, method_router));
        }
    }
    by_path
        .into_iter()
        .fold(Router::new(), |router, (path, method_router)| {
            router.route(path, method_router)
        })
}

/// The API-documentation surfaces — the Swagger UI shell at the
/// `/app/api` root and the OpenAPI document at `/app/api/openapi.json`.
/// They describe the API but are not the API: they read no client data
/// and call no protected data endpoint themselves.
///
/// They sit under the same private `/app/api` prefix as the operations
/// they document, and [`crate::bootstrap`] gives them the same two layers:
/// the session boundary for reachability, then `require_policy` for the
/// tier. The policy admits Clerk and above and denies `client`, which is a
/// *narrower* audience than the data reads beside them — the document
/// describes every lawyer-only write, so it belongs to the people who
/// operate Navigator rather than to the clients they serve.
///
/// The tier gate lives in the Rego bundle rather than in a handler check,
/// and that direction matters. An earlier *public* exemption for these
/// paths could not live there: an allow rule is the only thing between a
/// path and default-deny, so when #204 shipped the exemption in the Rego
/// and an image-only deploy advanced the binary ahead of the bundle, the
/// policy default-denied the docs in production. A restrictive rule
/// inverts that failure — a stale bundle yields 403 and a redeploy fixes
/// it, rather than opening a surface to someone who should not see it.
///
/// Neither handler reads `Db`, so this router needs no state.
pub fn doc_routes() -> Router {
    Router::new()
        .route("/app/api", axum::routing::get(api_docs))
        .route("/app/api/openapi.json", axum::routing::get(openapi_json))
}

/// Every `/app/api/*` operation this router registers, as `(HTTP method,
/// path)` in OpenAPI path-template form (`{id}`, not axum's `:id`).
///
/// Derived from [`api_operation_table`] — the *same* table [`routes`]
/// builds the router from — so this inventory can never silently omit a
/// registered route. `web/tests/openapi_drift.rs` asserts this exact
/// `(method, path)` set equals the operations in
/// [`crate::openapi::document`], so both a new *method* on an
/// already-listed path (e.g. `PUT /app/api/people`) and an entirely new
/// *path* are caught against the document — the old path-only inventory
/// was blind to method drift, and a hand-maintained list could omit a
/// registered route outright.
///
/// Excludes the `/app/api` shell and `/app/api/openapi.json`: those are
/// documentation surfaces mounted *outside* the policy gate by
/// [`doc_routes`], not part of the API the document describes.
#[must_use]
pub fn documented_api_operations() -> Vec<(&'static str, &'static str)> {
    api_operation_table()
        .into_iter()
        .map(|(method, path, _handler)| (method, path))
        .collect()
}

/// Static Swagger UI shell, served at the `/app/api` root — the private
/// prefix's own front door, so a reader who lands on the API namespace
/// gets the explorer for the operations beneath it. Loads the vendored
/// `swagger-ui-dist` assets from `/public/swagger-ui/` and points the
/// renderer at the sibling `/app/api/openapi.json`. Mounted by
/// [`doc_routes`] *outside* the embedded Rego policy gate (see that fn
/// for why) — the documentation describes the API but is not the API, so
/// the policy guards the data endpoints it documents while the session
/// boundary guards the docs. The per-response
/// `Content-Security-Policy` header keeps script execution on the same
/// origin — the whole point of vendoring rather than CDN-loading the
/// dist is so this header can stay strict.
async fn api_docs() -> impl IntoResponse {
    const HTML: &str = include_str!("../../server/public/swagger-ui/index.html");
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; \
                 script-src 'self'; \
                 style-src 'self' 'unsafe-inline'; \
                 img-src 'self' data:; \
                 connect-src 'self'; \
                 frame-ancestors 'none'",
            ),
        ],
        axum::response::Html(HTML),
    )
}

async fn openapi_json(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
    let authority = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok());
    let base = crate::openapi::base_url_for(authority);
    Json(crate::openapi::document_with_base(&base))
}

async fn list_people(
    State(surreal): State<store::surreal::SurrealDb>,
) -> Result<Json<Vec<store::persons::Person>>, ApiError> {
    Ok(Json(
        store::persons::list_directory(&surreal, "", "", &[]).await?,
    ))
}

async fn create_person(
    State(surreal): State<store::surreal::SurrealDb>,
    LawyerSession(session): LawyerSession,
    JsonOrForm(input): JsonOrForm<crate::people_commands::CreatePersonCommand>,
) -> Response {
    let mut command = input;
    // The role select is authoritative server-side, not just disabled in
    // the browser: a caller who can't change roles always creates a
    // `client`, so a lawyer can't POST `role=admin` past the form.
    // Only coerce a *recognized* role — an unrecognized value still falls
    // through to the command's validation and is rejected as a 400.
    if !may_change_roles(&session) && crate::people_commands::parse_role(&command.role).is_some() {
        command.role = store::persons::Role::Client.as_str().to_string();
    }
    if crate::people_commands::parse_role(&command.role)
        .is_some_and(|role| role.authority_rank() > session.role.authority_rank())
    {
        return mutation_error(PeopleCommandError::Blocked(
            "You cannot assign a system role above your own.",
        ));
    }
    match crate::people_commands::create_person(&surreal, &command).await {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(e) => mutation_error(e),
    }
}

async fn update_person(
    State(state): State<ApiState>,
    LawyerSession(session): LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<crate::people_commands::UpdatePersonCommand>,
) -> Response {
    let ctx = UpdateContext {
        bootstrap_owner_email: state.bootstrap_owner_email.as_deref(),
        actor_role: session.role,
        may_change_roles: may_change_roles(&session),
    };
    match crate::people_commands::update_person(&state.surreal, id, &input, &ctx).await {
        Ok(updated) => Json(updated).into_response(),
        Err(e) => mutation_error(e),
    }
}

async fn delete_person(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Response {
    match crate::people_commands::delete_person(
        &state.surreal,
        id,
        state.bootstrap_owner_email.as_deref(),
    )
    .await
    {
        Ok(deleted) => Json(deleted).into_response(),
        Err(e) => mutation_error(e),
    }
}

async fn send_welcome(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Response {
    let base_url = workflows::email::base_url_from_env();
    match crate::people_commands::send_welcome(&state.surreal, state.email.as_ref(), &base_url, id)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "sent" })),
        )
            .into_response(),
        // A send failure is a typed `502`, not a 5xx page: the caller is a
        // machine, and the Dioxus show view surfaces its own `?notice=` flag
        // from its own native POST to `/app/admin/people/{id}/welcome`.
        Err(PeopleCommandError::SendFailed) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": "send_failed" })),
        )
            .into_response(),
        Err(e) => mutation_error(e),
    }
}

/// A lawyer-tier caller, resolved from the session the auth layers
/// injected. As a [`FromRequestParts`] extractor it runs *before* the
/// body extractor ([`JsonOrForm`], a [`FromRequest`]), so an anonymous
/// (401) or `client` (403) caller is rejected before the body is parsed
/// — a malformed body can never mask an auth failure as a 400. API
/// writes are never anonymous and never `client`; this is the explicit
/// defense-in-depth check that holds even if the embedded Rego policy layer in front is
/// ever misconfigured to allow more through.
struct LawyerSession(SessionData);

impl<S> FromRequestParts<S> for LawyerSession
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<SessionData>() {
            None => Err(ApiError::Unauthenticated),
            Some(session) if !session.role.is_lawyer_tier() => Err(ApiError::Forbidden),
            Some(session) => Ok(Self(session.clone())),
        }
    }
}

/// Any authenticated session, of *any* tier — 401 only when there is no
/// session at all. Unlike [`LawyerSession`] it does not gate on `lawyer`,
/// because a few `/app/api` doors are client-writable (a client comments on their
/// own review document). The per-matter scope (client-lens or firm-lens) is
/// then enforced in the handler, so this extractor only proves "someone is
/// logged in".
struct AuthedSession(SessionData);

impl<S> FromRequestParts<S> for AuthedSession
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<SessionData>() {
            None => Err(ApiError::Unauthenticated),
            Some(session) => Ok(Self(session.clone())),
        }
    }
}

/// Whether the caller may set another person's role: Owner/Admin, and not
/// currently impersonating (an impersonating privileged caller acts with the
/// impersonated client's reach). Mirrors the lawyer form's role lock.
fn may_change_roles(session: &SessionData) -> bool {
    session.role.is_admin_tier() && session.impersonation.is_none()
}

/// Render a command failure as the typed JSON [`ApiError`] with its
/// proper status. Every caller of these endpoints is a machine.
fn mutation_error(error: PeopleCommandError) -> Response {
    ApiError::from(error).into_response()
}

/// Extractor that accepts a body as either JSON (`application/json`,
/// machine clients) or url-encoded form. Lets one command endpoint serve
/// both callers.
pub struct JsonOrForm<T>(pub T);

impl<S, T> FromRequest<S> for JsonOrForm<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"));
        if is_json {
            let Json(value) = Json::<T>::from_request(req, state).await.map_err(|_| {
                ApiError::Command(PeopleCommandError::Invalid("Malformed JSON body."))
            })?;
            Ok(Self(value))
        } else {
            let axum::extract::Form(value) = axum::extract::Form::<T>::from_request(req, state)
                .await
                .map_err(|_| {
                    ApiError::Command(PeopleCommandError::Invalid("Malformed form body."))
                })?;
            Ok(Self(value))
        }
    }
}

async fn get_person(
    State(surreal): State<store::surreal::SurrealDb>,
    Path(id): Path<Uuid>,
) -> Result<Json<store::persons::Person>, ApiError> {
    store::persons::find_by_id(&surreal, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

// --- Read clusters (#866) --------------------------------------------------
//
// The matter-centric reads the portal pages load. Each self-scopes to what the
// caller may see: `visible_projects` returns the caller's own matters (the
// directory lens for Owner/Admin, participation for lawyer/clerk, the client's
// own matter for a client); the by-id reads gate on `can_see_project` and
// collapse an out-of-scope resource to a non-disclosing 404. Playbooks and
// contract reviews are firm tools, so they take the lawyer tier.

/// `GET /app/api/project-repositories` — reconcile every Project row against
/// the repository it records.
///
/// Admin-tier only, and the tier is the point rather than a precaution. Every
/// other matter read on this surface is participation-scoped — Owner and Admin
/// included, because `store::access::visible_projects_as_lawyer` grants no
/// silent bypass and privileged reach is a place you navigate to. A
/// reconciliation report is that place: the question is about the whole
/// deployment, so it reads [`store::projects::all`] and gates on
/// `is_admin_tier` instead of pretending a per-caller lens is an inventory.
///
/// What it discloses is one code and one repository URL per matter, and no
/// matter content — which is what makes reading every row proportionate.
///
/// The report also carries `project_codes`: every live code, sorted, including
/// the ones with no finding. That is what the *repository* side reads — a
/// checkout asking whether its declared code names a live row cannot learn it
/// from the findings, because a row that is entirely fine produces none.
///
/// The deployment's forge pair is read where it is available and skipped where
/// it is not. Absent configuration is the local loop and the test suite, not an
/// error: every *failing* finding is computable without it, and the report
/// carries `compared_against_deployment_forge` so a reader is told which
/// comparison did not run rather than reading its absence as agreement.
async fn reconcile_project_repositories_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
) -> Result<Response, ApiError> {
    if !authed.0.role.is_admin_tier() {
        return Err(ApiError::Forbidden);
    }
    let projects = store::projects::all(&state.surreal)
        .await
        .map_err(|error| ApiError::Db(error.to_string()))?;
    let rows: Vec<store::project_reconcile::RowUnderReview<'_>> =
        projects.iter().map(Into::into).collect();
    let deployment = cloud::workspace::WorkspaceConfig::from_env().ok();
    let report = store::project_reconcile::reconcile(&rows, deployment.as_ref());
    Ok((StatusCode::OK, Json(report)).into_response())
}

/// `POST /app/api/project-surfaces/{id}` — create or adopt the three handles
/// a Project opens with: the documents-bucket prefix, the Drive ingest
/// folder, and the source repository.
///
/// Admin-tier only. Matter-open already runs the same reconcile best-effort
/// and does not roll the open back when Drive or the forge is down; this
/// door is the retry for that pass and for a legacy row that never got one.
/// Missing Drive or forge configuration skips that surface rather than
/// failing. A recorded `repository_url` is left alone.
async fn reconcile_project_surfaces_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !authed.0.role.is_admin_tier() {
        return Err(ApiError::Forbidden);
    }
    let surfaces = store::project_surfaces::reconcile_from_env(&state.surreal, id)
        .await
        .map_err(surface_error)?;
    Ok((StatusCode::OK, Json(surfaces)).into_response())
}

fn surface_error(error: store::project_surfaces::SurfaceError) -> ApiError {
    match error {
        store::project_surfaces::SurfaceError::NotFound => ApiError::NotFound,
        store::project_surfaces::SurfaceError::Command(error) => ApiError::Project(error),
        other => ApiError::Db(other.to_string()),
    }
}

/// `GET /app/api/projects` — the matters the caller may see, already scoped.
async fn list_projects_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
) -> Result<Response, ApiError> {
    let projects =
        store::access::visible_projects(&state.surreal, authed.0.person_id, authed.0.role)
            .await
            .map_err(ApiError::Db)?;
    Ok((StatusCode::OK, Json(projects)).into_response())
}

/// `GET /app/api/projects/{id}` — one matter, or 404 if the caller may not see it.
async fn get_project_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !can_see(&state, &authed, id).await {
        return Err(ApiError::NotFound);
    }
    let project = store::projects::find_by_id(&state.surreal, id)
        .await
        .ok()
        .flatten()
        .ok_or(ApiError::NotFound)?;
    Ok((StatusCode::OK, Json(project)).into_response())
}

/// `GET /app/api/projects/{id}/participants` — the matter's participation ledger.
async fn list_participants_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !can_see(&state, &authed, id).await {
        return Err(ApiError::NotFound);
    }
    let rows = store::projects::participations_for_project(&state.surreal, id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(rows)).into_response())
}

/// `GET /app/api/projects/{id}/notations` — the notations opened on the matter.
async fn list_notations_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !can_see(&state, &authed, id).await {
        return Err(ApiError::NotFound);
    }
    let notations = store::notations::list_by_project(&state.surreal, id).await?;
    Ok((StatusCode::OK, Json(notations)).into_response())
}

/// `GET /app/api/notations/{id}` — one notation, scoped by its matter.
async fn get_notation_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if !can_see(&state, &authed, notation.project_id).await {
        return Err(ApiError::NotFound);
    }
    Ok((StatusCode::OK, Json(notation)).into_response())
}

/// `GET /app/api/playbooks` — the firm's contract-review playbooks (lawyer tier).
async fn list_playbooks_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
) -> Result<Response, ApiError> {
    let playbooks = store::playbooks::all(&state.surreal)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(playbooks)).into_response())
}

/// `GET /app/api/playbooks/{id}` — one playbook (lawyer tier).
async fn get_playbook_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let playbook = store::playbooks::by_id(&state.surreal, id)
        .await
        .ok()
        .flatten()
        .ok_or(ApiError::NotFound)?;
    Ok((StatusCode::OK, Json(playbook)).into_response())
}

/// `GET /app/api/contract-reviews/{id}` — one inbound-contract review, scoped to
/// its matter (a lawyer not on the matter, or a non-firm caller, gets 404).
async fn get_contract_review_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let review = store::contract_reviews::by_id(&state.surreal, id)
        .await
        .ok()
        .flatten()
        .ok_or(ApiError::NotFound)?;
    let notation = store::notations::find_by_id(&state.surreal, review.notation_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    Ok((StatusCode::OK, Json(review)).into_response())
}

/// Whether `authed` may see matter `project_id`, in either lens.
async fn can_see(state: &ApiState, authed: &AuthedSession, project_id: Uuid) -> bool {
    store::access::can_see_project(
        &state.surreal,
        authed.0.person_id,
        authed.0.role,
        project_id,
    )
    .await
    .unwrap_or(false)
}

/// `GET /app/api/projects/{id}/documents` — the matter's filed documents. A
/// client sees only client-visible documents (internal work product is filtered
/// out, #782); the firm sees them all.
async fn list_documents_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !can_see(&state, &authed, id).await {
        return Err(ApiError::NotFound);
    }
    let mut docs = store::assets::for_project(&state.surreal, id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    if matches!(authed.0.role, store::persons::Role::Client) {
        docs.retain(|a| a.visibility == store::documents::visibility::CLIENT);
    }
    Ok((StatusCode::OK, Json(docs)).into_response())
}

/// `GET /app/api/projects/{id}/conversation` — the matter conversation. A client
/// sees the client-visible thread (internal notes filtered out); the firm sees
/// the full thread.
async fn get_conversation_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !can_see(&state, &authed, id).await {
        return Err(ApiError::NotFound);
    }
    let messages = if matches!(authed.0.role, store::persons::Role::Client) {
        store::communications::for_project_client_visible(&state.surreal, id).await
    } else {
        store::communications::for_project(&state.surreal, id).await
    }
    .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(messages)).into_response())
}

/// `GET /app/api/notations/{id}/review-documents` — the review drafts on a
/// notation (firm work product). Lawyer-tier and matter-scoped.
async fn list_review_documents_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    let docs = store::review_documents::for_notation(&state.surreal, id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(docs)).into_response())
}

/// `GET /app/api/expunge-requests` — the pending client document-deletion queue
/// (lawyer tier; the firm's review queue).
async fn list_expunge_requests_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
) -> Result<Response, ApiError> {
    let requests = store::expunge_requests::list_pending(&state.surreal)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(requests)).into_response())
}

/// `POST /app/api/entities` — the Entity create command. Lawyer-tier only, like
/// every other `/app/api/*` write: the [`LawyerSession`] extractor rejects an
/// anonymous caller with 401 and a `client`/`clerk` one with 403 before the
/// body is parsed. The write itself — the blank-name check, the firm-anchor
/// guard, and the insert — belongs to `store::entity_commands::create_entity`,
/// which the `/app/admin/entities` form and the inline project modal call too, so
/// this handler only authorizes and renders.
async fn create_entity(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    JsonOrForm(input): JsonOrForm<store::entity_commands::CreateEntityCommand>,
) -> Result<Response, ApiError> {
    let created =
        store::entity_commands::create_entity(&state.surreal, &state.bootstrap_company, &input)
            .await?;
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// The authenticated command behind `navigator site import`. The CLI sends the
/// seed document verbatim; parsing, natural-key lookup, and writes happen in
/// the typed store registry rather than in a client with database credentials.
#[derive(Debug, Deserialize)]
struct SeedRequest {
    model: String,
    yaml: String,
    #[serde(default)]
    overwrite: bool,
}

/// This route's own address, matched against a scoped session's
/// [`crate::session::SeedScope::endpoint`]. A session scoped to a different
/// endpoint string is refused before the body is even parsed further.
const SEED_ENDPOINT: &str = "/app/api/seed";

async fn reconcile_seed(
    State(state): State<ApiState>,
    LawyerSession(session): LawyerSession,
    Json(input): Json<SeedRequest>,
) -> Response {
    let outcome = async {
        let model = store::seed::SeedModel::parse(&input.model)?;
        if let Some(scope) = &session.scope {
            if scope.endpoint != SEED_ENDPOINT {
                return Err(anyhow::Error::new(
                    store::seed::ScopeViolation::EndpointNotScoped,
                ));
            }
            if !scope.models.contains(&model) {
                return Err(anyhow::Error::new(
                    store::seed::ScopeViolation::ModelNotScoped(model.term()),
                ));
            }
        }
        store::seed::reconcile_yaml(
            &state.surreal,
            model,
            &input.yaml,
            &state.bootstrap_company,
            input.overwrite,
            &store::seed::ReconcileActor {
                bootstrap_owner_email: state.bootstrap_owner_email.as_deref(),
                actor_role: session.role,
                may_change_roles: may_change_roles(&session),
                project_scope: session
                    .scope
                    .as_ref()
                    .map(|scope| scope.project_code.as_str()),
            },
        )
        .await
    }
    .await;
    match outcome {
        Ok(report) => (StatusCode::OK, Json(report)).into_response(),
        Err(error) => {
            // A scope refusal is distinguishable from an ordinary malformed
            // or invalid seed document — a 403 the audit trail can tell
            // apart from a 422, per ENG-344.
            if error
                .downcast_ref::<store::seed::ScopeViolation>()
                .is_some()
            {
                (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": "scope_violation",
                        "message": error.to_string(),
                    })),
                )
                    .into_response()
            } else {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "invalid_seed",
                        "message": error.to_string(),
                    })),
                )
                    .into_response()
            }
        }
    }
}

/// `PATCH /app/api/entities/{id}` — the Entity update command. Same lawyer-tier
/// gate as create. Every field is a full replacement; the firm anchor's
/// *name* is immutable while its type and jurisdiction stay editable, and a
/// rename into the anchor's name is refused. Those rules live in
/// `store::entity_commands::update_entity`, which the `/app/admin/entities/{id}`
/// edit form calls too.
async fn update_entity(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<store::entity_commands::UpdateEntityCommand>,
) -> Result<Json<store::entities::Entity>, ApiError> {
    let updated =
        store::entity_commands::update_entity(&state.surreal, id, &state.bootstrap_company, &input)
            .await?;
    Ok(Json(updated))
}

/// `DELETE /app/api/entities/{id}` — the Entity delete command. Same lawyer-tier
/// gate as create and update. The firm anchor is undeletable (409), and an
/// Entity other rows still reference is refused, naming the tables and
/// counts that still point at it. Those rules live in
/// `store::entity_commands::delete_entity`, which the lawyer delete button
/// calls too. Returns the removed row.
async fn delete_entity(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Json<store::entities::Entity>, ApiError> {
    let deleted =
        store::entity_commands::delete_entity(&state.surreal, id, &state.bootstrap_company).await?;
    Ok(Json(deleted))
}

/// Request body for `POST /app/api/projects` — open a matter. Every field is the
/// caller's *except* the attester, which the server resolves from the
/// authenticated session (never trusting the client to name who attested).
#[derive(Debug, Deserialize)]
struct OpenProjectRequest {
    name: String,
    /// The matter code. Required — no `serde(default)`: it names the matter's
    /// folder in the firm's shared drive as well as its repo, and the mapping is
    /// an equality check (#938), so a derived code would name no folder. An
    /// omitted `code` is a 422 at deserialize.
    code: String,
    client_id: Uuid,
    entity_id: Uuid,
    #[serde(default)]
    description: Option<String>,
    /// The opening attorney's conflict attestation. Must be `true`: a matter
    /// open with no attestation is refused.
    #[serde(default)]
    attestation: bool,
}

/// `POST /app/api/projects` — open a matter. Lawyer-tier only, which at this firm
/// means an attorney: the `LawyerSession` gate is therefore the "an attorney is
/// acting" check, and the session's person is the attester recorded on the
/// audit row and designated the accountable lawyer DRI. The command
/// (`store::projects::open_matter`) runs the conflict check, requires the
/// attestation on every open, and writes the audit trail; a blocking conflict
/// (adverse to a current client) is a hard 409 no attestation overrides. The
/// Drive ingest folder and source repository are then created or adopted
/// best-effort: a Drive or forge fault leaves the matter open, and
/// [`reconcile_project_surfaces_door`] retries.
async fn open_project(
    State(state): State<ApiState>,
    LawyerSession(session): LawyerSession,
    JsonOrForm(input): JsonOrForm<OpenProjectRequest>,
) -> Result<Response, ApiError> {
    // The attester must resolve to a person: a lawyer session always carries
    // one, and a matter cannot be opened (nor attested) by a session that
    // doesn't name who is acting.
    let acting = session.person_id.ok_or(ApiError::Forbidden)?;
    let command = store::projects::OpenMatterCommand {
        name: input.name,
        code: input.code,
        client_id: input.client_id,
        entity_id: input.entity_id,
        description: input.description,
        attestation: input.attestation,
        acting_person_id: acting,
    };
    let matter = store::projects::open_matter(&state.surreal, &command).await?;
    store::project_surfaces::reconcile_after_open(&state.surreal, matter.id).await;
    Ok((StatusCode::CREATED, Json(matter)).into_response())
}

/// `PATCH /app/api/projects/{id}` — update a matter's descriptive fields (name,
/// status, entity, scope narrative). Lawyer-tier only. This is the descriptive
/// update, deliberately not the matter-open path: it runs no conflict check
/// and provisions no repo. The rules live in
/// `store::projects::update_project`, which the `/app/projects/{project_code}` edit
/// form calls too.
async fn update_project(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<store::projects::UpdateProjectCommand>,
) -> Result<Json<store::projects::Project>, ApiError> {
    let updated = store::projects::update_project(&state.surreal, id, &input).await?;
    Ok(Json(updated))
}

/// `DELETE /app/api/projects/{id}` — delete a matter. Lawyer-tier only. A matter
/// opened the normal way carries dependents (DRI participations, notations),
/// whose foreign keys block the delete — surfaced as
/// 409 with the database's own detail naming the referencing table, a conflict
/// the caller resolves by detaching those first, not a server fault. The rules
/// live in `store::projects::delete_project`, which the `/app/projects` door
/// calls too. Returns the removed matter.
async fn delete_project(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Json<store::projects::Project>, ApiError> {
    let deleted = store::projects::delete_project_with_surreal(&state.surreal, id).await?;
    Ok(Json(deleted))
}

/// Request body for `POST /app/api/projects/{id}/participants` — add a person to a
/// matter's participation ledger. The matter is the path id; the body names the
/// person, and nothing else — the participation follows their `persons.role`.
#[derive(Deserialize)]
struct AddParticipantRequest {
    person_id: Uuid,
}

/// `POST /app/api/projects/{id}/participants` — add a person to a matter's
/// participation ledger. Lawyer-tier and matter-scoped: the acting lawyer must
/// already participate in this matter (admin/owner bypass, same as
/// [`close_matter`]) — a lawyer with no row on the matter gets the same
/// non-disclosing `404` an unrelated caller would. The command
/// (`store::participation::add_participant`) validates the matter and person
/// exist, derives the participation from the person's tier, and enforces one row
/// per person + matter; the same command the lawyer participation form funnels
/// through. This is also the door a participating lawyer uses to grant or
/// revoke a Clerk's portal visibility on their own matter: adding a Clerk here
/// is what makes `store::access::can_see_project` admit them, and removing the
/// row (see [`remove_participant`]) is the toggle back off.
async fn add_participant(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<AddParticipantRequest>,
) -> Result<Response, ApiError> {
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    let row = store::participation::add_participant(
        &state.surreal,
        &store::participation::AddParticipantCommand {
            project_id: id,
            person_id: input.person_id,
            dri: store::participation::DriRequest::Unchanged,
            // This door cannot move a marker, so there is no DRI actor to name.
            // Wiring `dri` here means wiring the session's person id with it.
            actor: store::participation::DriActor::System,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(row)).into_response())
}

/// Request body for `PATCH /app/api/projects/{id}/participants/{role_id}` — re-point
/// a participation row at a different person.
#[derive(Deserialize)]
struct UpdateParticipantRequest {
    person_id: Uuid,
}

/// `PATCH /app/api/projects/{id}/participants/{role_id}` — edit a matter
/// participation row. Lawyer-tier and matter-scoped, like [`add_participant`].
/// The command (`store::participation::update_participant`) validates the row
/// belongs to the matter and the person exists, re-derives the participation
/// from that person's tier, and checks no other row duplicates the person and
/// that the edit does not strand the matter's lawyer DRI.
async fn update_participant(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
    JsonOrForm(input): JsonOrForm<UpdateParticipantRequest>,
) -> Result<Response, ApiError> {
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    let row = store::participation::update_participant(
        &state.surreal,
        &store::participation::UpdateParticipantCommand {
            project_id: id,
            role_id,
            person_id: input.person_id,
            dri: store::participation::DriRequest::Unchanged,
            // As above: no marker moves through this door, so no actor is read.
            actor: store::participation::DriActor::System,
        },
    )
    .await?;
    Ok((StatusCode::OK, Json(row)).into_response())
}

/// `DELETE /app/api/projects/{id}/participants/{role_id}` — remove a matter
/// participation row. Lawyer-tier and matter-scoped, like [`add_participant`].
/// The command (`store::participation::remove_participant`) refuses to remove
/// the matter's *last* lawyer DRI (that would strand its accountable lawyer).
/// `204 No Content` on success.
///
/// Removing a row that carries a marker is a DRI change, so this door names its
/// actor: the command gates the removal on it and writes it to the audit trail.
/// A session with no `persons` row cannot be that actor and is refused.
async fn remove_participant(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let acting = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    store::participation::remove_participant(
        &state.surreal,
        id,
        role_id,
        store::participation::DriActor::Person(acting),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PUT /app/api/projects/{id}/participants/{role_id}/dri` — designate the
/// participant named by `role_id` as one of the matter's DRIs. The side
/// (lawyer or client) follows from that person's tier, exactly as the lawyer
/// workbench derives it, so the door takes no body. Lawyer-tier only, and the
/// shared command additionally gates the change on the caller already holding
/// the marker for that side — a matter's lawyer DRIs govern their own set — so
/// a lawyer who is not a current holder is refused `403`. `204` on success.
async fn designate_participant_dri(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let acting = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    let row = participation_on_matter(&state.surreal, id, role_id).await?;
    let person = store::persons::find_by_id(&state.surreal, row.person_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let side = if person.role.is_lawyer_tier() {
        store::projects::DriSide::Lawyer
    } else {
        store::projects::DriSide::Client
    };
    store::participation::update_participant(
        &state.surreal,
        &store::participation::UpdateParticipantCommand {
            project_id: id,
            role_id,
            person_id: row.person_id,
            dri: store::participation::DriRequest::Designate(side),
            actor: store::participation::DriActor::Person(acting),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /app/api/projects/{id}/participants/{role_id}/dri` — clear the DRI
/// marker from the participant named by `role_id`. Lawyer-tier only and
/// actor-gated like designation; the shared command refuses to clear the
/// matter's last lawyer DRI (a matter always keeps one), answering `422`.
/// `204 No Content` on success. Clearing a marker that is not set is a no-op
/// the command accepts.
async fn remove_participant_dri(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, role_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let acting = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    let row = participation_on_matter(&state.surreal, id, role_id).await?;
    store::participation::update_participant(
        &state.surreal,
        &store::participation::UpdateParticipantCommand {
            project_id: id,
            role_id,
            person_id: row.person_id,
            dri: store::participation::DriRequest::Clear,
            actor: store::participation::DriActor::Person(acting),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// One participation row confirmed to belong to `project_id`, or
/// [`ApiError::NotFound`]. The API mirror of the workbench's `participation_row`:
/// a `role_id` from another matter must not disclose that it exists, so a
/// mismatch — like a missing row or a lookup failure — collapses to 404.
async fn participation_on_matter(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    role_id: Uuid,
) -> Result<store::projects::PersonProjectRole, ApiError> {
    store::projects::participation_by_id(surreal, role_id)
        .await
        .ok()
        .flatten()
        .filter(|row| row.project_id == project_id)
        .ok_or(ApiError::NotFound)
}

/// `POST /app/api/projects/{id}/close` — open the matter's firm-signed
/// closing-letter notation, the REST mirror of the lawyer close control. Both
/// converge on `retainer_walk::open_closing_notation`. Lawyer-tier only and
/// matter-scoped: a lawyer who does not participate in the matter (admin
/// bypasses) gets a bare 404. `201 Created` carrying the new notation id; a
/// matter with no client to address the letter to is `409`. The status flip to
/// `closed` is the close workflow's job once the walk completes, not this door's.
async fn close_matter(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    match crate::retainer_walk::open_closing_notation(&state.surreal, &state.storage, id).await {
        Ok(notation_id) => Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({ "notation_id": notation_id })),
        )
            .into_response()),
        Err(crate::retainer_walk::CloseMatterError::MatterNotFound) => Err(ApiError::NotFound),
        Err(crate::retainer_walk::CloseMatterError::NoClient) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "no_client",
                "message": "This matter has no client to address the closing letter to."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, project_id = %id, "api close_matter: failed");
            Err(ApiError::Db(
                "the closing notation could not be opened".into(),
            ))
        }
    }
}

/// Request body for `POST /app/api/projects/{id}/conversation/messages` — the
/// message body and, for the lawyer lens only, whether it is an internal note.
#[derive(Deserialize)]
struct PostConversationMessageRequest {
    body: String,
    #[serde(default)]
    internal: bool,
}

/// `POST /app/api/projects/{id}/conversation/messages` — post one message to a
/// matter's conversation, the REST mirror of the portal message control. Both
/// converge on `conversation::post_conversation_message`. Client-writable: any
/// authenticated caller reaches it (embedded Rego), and the command enforces
/// either-lens matter access (a non-participant is a non-disclosing 404). The
/// tier decides the side (inbound for a client, outbound/internal for a lawyer);
/// a client's `internal` flag is ignored. `204` on success; an empty body is
/// `400`.
async fn post_conversation_message_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<PostConversationMessageRequest>,
) -> Result<Response, ApiError> {
    match crate::conversation::post_conversation_message(
        &state.surreal,
        authed.0.person_id,
        authed.0.role,
        id,
        &input.body,
        input.internal,
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(crate::conversation::PostMessageError::NotAuthorized) => Err(ApiError::NotFound),
        Err(crate::conversation::PostMessageError::EmptyBody) => Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "empty_body",
                "message": "The message body is empty."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, project_id = %id, "api conversation message: failed");
            Err(ApiError::Db("the message could not be posted".into()))
        }
    }
}

/// Request body for `POST /app/api/projects/{id}/notations` — open a notation on
/// an existing matter from a template (authored in the matter's repo, or the
/// firm catalog fallback), bound to a client email.
#[derive(Deserialize)]
struct CreateNotationRequest {
    template_code: String,
    client_email: String,
}

/// Success body for `POST /app/api/projects/{id}/notations` — the opened
/// notation's id, so the caller can drive its questionnaire next.
#[derive(Serialize)]
struct CreateNotationResponse {
    notation_id: Uuid,
}

/// `POST /app/api/projects/{id}/notations` — open a notation on a matter. Lawyer-tier
/// only (embedded Rego policy) *and* matter-scoped: the shared command
/// (`crate::project_notation::create_project_notation`) re-checks that the
/// acting lawyer participates in the project (admin bypasses), collapsing an
/// out-of-scope matter to 404 so the door never discloses it. This is the same
/// command the lawyer browser form drives, so both surfaces open notations
/// identically. `201 Created` with the new notation id on success.
async fn create_notation(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<CreateNotationRequest>,
) -> Result<Response, ApiError> {
    let outcome = crate::project_notation::create_project_notation(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        &state.storage,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
        &input.template_code,
        &input.client_email,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateNotationResponse {
            notation_id: outcome.notation_id,
        }),
    )
        .into_response())
}

/// Request body for `POST /app/api/notations/{id}/answers` — answer the
/// questionnaire's current step. `question_code` must match the step the
/// questionnaire is currently asking (it advances one step at a time);
/// `reference_id` names the picked row for a record/reference question and is
/// omitted for a free-typed answer.
#[derive(Deserialize)]
struct AnswerStepRequest {
    question_code: String,
    value: String,
    #[serde(default)]
    reference_id: Option<Uuid>,
}

/// One question the caller must collect next.
#[derive(Serialize)]
struct NotationQuestion {
    id: Uuid,
    code: String,
    prompt: String,
    answer_type: String,
    choices: Vec<workflows::QuestionChoice>,
}

/// Success body for `POST /app/api/notations/{id}/answers` — where the
/// questionnaire is after recording the answer: either the next question to
/// collect, or that it is complete.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum NotationStepResponse {
    /// Collect this question and POST it back to the same endpoint.
    NeedsAnswer { question: NotationQuestion },
    /// The questionnaire reached END; there is nothing more to answer.
    Complete,
}

impl From<workflows::NextStep> for NotationStepResponse {
    fn from(next: workflows::NextStep) -> Self {
        match next {
            workflows::NextStep::NeedsAnswer { question } => Self::NeedsAnswer {
                question: NotationQuestion {
                    id: question.id,
                    code: question.code,
                    prompt: question.prompt,
                    answer_type: question.answer_type,
                    choices: question.choices,
                },
            },
            workflows::NextStep::QuestionnaireComplete => Self::Complete,
        }
    }
}

/// `POST /app/api/notations/{id}/answers` — record an answer to the notation's
/// current questionnaire step, attributed to the acting lawyer (source =
/// lawyer; the notation's bound Person stays the respondent). Lawyer-tier only
/// (embedded Rego policy) *and* matter-scoped: the acting lawyer must participate in the
/// notation's matter (admin bypasses), collapsing an out-of-scope notation to
/// 404 so the door never discloses it. The client-facing self-serve intake
/// (the magic-link walk) is a separate surface, not this REST command
/// boundary. Returns the next step (a question to collect, or complete).
async fn answer_notation_step(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<AnswerStepRequest>,
) -> Result<Response, ApiError> {
    // Resolve the notation and enforce matter scope on its project before
    // touching the questionnaire. A miss (no notation, or out of scope)
    // collapses to 404 so the door never discloses a notation the caller
    // cannot see.
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }

    let next = workflows::answer_step_with_reference(
        &state.surreal,
        state.questionnaire_runtime.as_ref(),
        Some(&state.storage),
        id,
        input.question_code.trim(),
        &input.value,
        input.reference_id,
        workflows::AnswerAuthor::lawyer(lawyer.0.person_id),
    )
    .await?;
    Ok((StatusCode::OK, Json(NotationStepResponse::from(next))).into_response())
}

/// Load a notation and confirm the lawyer caller may see its matter, or 404.
/// The notation-door mirror of [`participation_on_matter`]: an out-of-scope or
/// unknown notation collapses to 404 so a door never discloses one the caller
/// cannot reach (admin bypasses the scope check).
async fn notation_in_lawyer_scope(
    state: &ApiState,
    lawyer: &LawyerSession,
    notation_id: Uuid,
) -> Result<store::notations::Notation, ApiError> {
    let notation = store::notations::find_by_id(&state.surreal, notation_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    Ok(notation)
}

/// Request body for `POST /app/api/notations/{id}/request-changes` — the flagged
/// question codes to send back and an optional note.
#[derive(Deserialize)]
struct RequestNotationChangesRequest {
    #[serde(default)]
    flagged: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /app/api/notations/{id}/request-changes` — send a notation at
/// `lawyer_review` back to its client for changes, the REST mirror of the lawyer
/// request-changes control. Both converge on
/// `retainer_walk::request_notation_changes`. Lawyer-tier only and matter-scoped
/// (out-of-scope → 404, admin bypasses). `204` on success; not at review is
/// `409`, an empty flagged set is `400`.
async fn request_notation_changes_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<RequestNotationChangesRequest>,
) -> Result<Response, ApiError> {
    let acting = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    notation_in_lawyer_scope(&state, &lawyer, id).await?;
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match crate::retainer_walk::request_notation_changes(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        id,
        Some(acting),
        &input.flagged,
        note,
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(crate::retainer_walk::RequestChangesError::NotationNotFound) => Err(ApiError::NotFound),
        Err(crate::retainer_walk::RequestChangesError::NotInReview) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "not_in_review",
                "message": "This notation is not awaiting review; there is nothing to send back."
            })),
        )
            .into_response()),
        Err(crate::retainer_walk::RequestChangesError::NothingFlagged) => Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "nothing_flagged",
                "message": "Flag at least one answer to send back for changes."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, notation_id = %id, "api request_changes: failed");
            Err(ApiError::Db(
                "the change request could not be recorded".into(),
            ))
        }
    }
}

/// Request body for `POST /app/api/notations/{id}/reask` — the re-collected
/// answers, keyed by bare question code.
#[derive(Deserialize)]
struct ReaskResubmitRequest {
    #[serde(default)]
    answers: std::collections::BTreeMap<String, String>,
}

/// `POST /app/api/notations/{id}/reask` — resubmit a re-collected notation for
/// review, the REST mirror of the lawyer reask control. Both converge on
/// `retainer_walk::resubmit_reask`. Lawyer-tier only and matter-scoped
/// (out-of-scope → 404, admin bypasses). `204` on success; not awaiting
/// re-collection is `409`, a flagged answer left blank is `400`.
async fn resubmit_reask_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<ReaskResubmitRequest>,
) -> Result<Response, ApiError> {
    let acting = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    notation_in_lawyer_scope(&state, &lawyer, id).await?;
    match crate::retainer_walk::resubmit_reask(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        id,
        Some(acting),
        &input.answers,
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(crate::retainer_walk::ResubmitReaskError::NotationNotFound) => Err(ApiError::NotFound),
        Err(crate::retainer_walk::ResubmitReaskError::NotAwaitingReask) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "not_awaiting_reask",
                "message": "This notation is not awaiting re-collection."
            })),
        )
            .into_response()),
        Err(crate::retainer_walk::ResubmitReaskError::MissingAnswer(code)) => Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "missing_answer",
                "message":
                    format!("Re-collect the flagged answer `{code}` before resubmitting for review.")
            })),
        )
            .into_response()),
        Err(crate::retainer_walk::ResubmitReaskError::AnswerWriteFailed(_)) => Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "answer_write_failed",
                "message": "A re-collected answer could not be saved; the resubmit was refused."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, notation_id = %id, "api reask: failed");
            Err(ApiError::Db("the re-collection could not be resubmitted".into()))
        }
    }
}

/// Request body for `POST /app/api/notations/{id}/transcript` — a batch
/// transcript to run against the notation's questionnaire.
#[derive(Deserialize)]
struct TranscriptCoverageRequest {
    #[serde(default)]
    transcript: String,
}

/// `POST /app/api/notations/{id}/transcript` — run a transcript against the
/// notation's bound questionnaire, recording every likely-answered inquiry as a
/// proposed default; the REST mirror of the lawyer/CLI transcript control,
/// converging on `retainer_walk::record_transcript_coverage`. Lawyer-tier only
/// and matter-scoped (out-of-scope → 404). `200` with `{template_code, covered,
/// uncovered}`; an empty transcript is `400`; a template with no questionnaire
/// is `422`.
async fn transcript_coverage_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<TranscriptCoverageRequest>,
) -> Result<Response, ApiError> {
    notation_in_lawyer_scope(&state, &lawyer, id).await?;
    let transcript = input.transcript.trim();
    if transcript.is_empty() {
        return Ok(bad_request(
            "transcript_required",
            "A transcript is required.",
        ));
    }
    match crate::retainer_walk::record_transcript_coverage(
        &state.surreal,
        &state.storage,
        id,
        transcript,
    )
    .await
    {
        Ok(c) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "template_code": c.template_code,
                "covered": c.covered,
                "uncovered": c.uncovered,
            })),
        )
            .into_response()),
        Err(
            crate::retainer_walk::TranscriptCoverageError::NotationNotFound
            | crate::retainer_walk::TranscriptCoverageError::TemplateNotFound,
        ) => Err(ApiError::NotFound),
        Err(crate::retainer_walk::TranscriptCoverageError::NoQuestionnaire) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "no_questionnaire",
                "message": "This notation's template has no questionnaire to cover."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, notation_id = %id, "api transcript coverage: failed");
            Err(ApiError::Db("the transcript coverage pass failed".into()))
        }
    }
}

/// Success body for `POST /app/api/notations/{id}/intake` — the notation and the
/// client address the intake link was dispatched to.
#[derive(Serialize)]
struct SendIntakeResponse {
    notation_id: Uuid,
    recipient: String,
}

/// `POST /app/api/notations/{id}/intake` — email the notation's client their
/// self-serve intake magic link. Lawyer-tier only (embedded Rego policy) *and* matter-scoped:
/// the acting lawyer must participate in the notation's matter (admin bypasses),
/// collapsing an out-of-scope or unknown notation to 404. This is the same
/// command the lawyer form drives. Email delivery is best-effort (a send failure
/// is logged, not surfaced — the link is idempotent), so `200 OK` reports that
/// the link was dispatched to the returned recipient.
async fn send_notation_intake(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    // Resolve the notation and enforce matter scope on its project before
    // dispatching anything. A miss collapses to 404 so the door never
    // discloses a notation the caller cannot see.
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }

    let recipient =
        crate::retainer_walk::send_intake(&state.surreal, state.email.as_ref(), id).await?;
    Ok((
        StatusCode::OK,
        Json(SendIntakeResponse {
            notation_id: id,
            recipient,
        }),
    )
        .into_response())
}

/// Success body for a notation lifecycle transition — the notation and its
/// workflow state after the action.
#[derive(Serialize)]
struct NotationLifecycleResponse {
    notation_id: Uuid,
    state: String,
}

/// `POST /app/api/notations/{id}/approval` — the attorney approves a notation
/// parked at `lawyer_review`: re-assembles the reviewed document and fires the
/// `approved` transition so the worker renders + persists its PDF, parking at
/// `generate_pdf__*` (the deliberate signature dispatch is a separate door).
/// Lawyer-tier only (embedded Rego policy) *and* matter-scoped (admin bypasses); an out-of-scope
/// or unknown notation is 404. Idempotent: if the PDF is already rendered
/// (a prior approve, or a clean machine-only intake that walked straight
/// through), approving again is a no-op success reporting the current state.
/// This drives the same command core (`render_and_park`) as the lawyer form.
async fn approve_notation(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    // Resolve the notation and enforce matter scope on its project first. A
    // miss collapses to 404 so the door never discloses a notation the caller
    // cannot see.
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }

    // Idempotent approve: if the worker already rendered + persisted this
    // notation's PDF, re-firing `approved` from a state with no such edge
    // would error — so treat a repeat approve as a no-op success. The lawyer
    // form applies the same guard.
    if crate::retainer_walk::document_pdf_ready(state.storage.as_ref(), id)
        .await
        .unwrap_or(false)
    {
        return Ok((
            StatusCode::OK,
            Json(NotationLifecycleResponse {
                notation_id: id,
                state: notation.state,
            }),
        )
            .into_response());
    }

    let deps = crate::retainer_walk::RenderDeps {
        surreal: &state.surreal,
        runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        assets_storage: &state.assets_storage,
        forms_registry: &state.forms_registry,
    };
    let final_state = crate::retainer_walk::render_and_park(&deps, id, lawyer.0.person_id).await?;
    Ok((
        StatusCode::OK,
        Json(NotationLifecycleResponse {
            notation_id: id,
            state: final_state.as_str().to_string(),
        }),
    )
        .into_response())
}

/// Success body for `POST /app/api/notations/{id}/signature` — the notation, its
/// workflow state after dispatch, and the provider's signature request id (so
/// the caller can correlate the inbound completion webhook).
#[derive(Serialize)]
struct NotationSignatureResponse {
    notation_id: Uuid,
    state: String,
    signature_request_id: String,
}

/// `POST /app/api/notations/{id}/signature` — dispatch the notation's rendered
/// document for signature: fires `pdf_persisted` and sends exactly one
/// envelope through the signature provider. Lawyer-tier only (embedded Rego policy) *and*
/// matter-scoped (admin bypasses); an out-of-scope or unknown notation is 404.
/// Idempotent: a notation that already has an envelope out reuses its request
/// id and sends nothing. When the worker has not rendered the PDF yet, returns
/// `409 document_not_ready` (retry) rather than dispatching a missing document.
/// This drives the same command core (`dispatch_signature`) as the lawyer form.
async fn send_notation_signature(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    // Resolve the notation and enforce matter scope on its project first. A
    // miss collapses to 404 so the door never discloses a notation the caller
    // cannot see.
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }

    let deps = crate::retainer_walk::SendDeps {
        surreal: &state.surreal,
        runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        signature_provider: state.signature_provider.as_ref(),
    };
    let (final_state, signature_request_id) =
        crate::retainer_walk::dispatch_signature(&deps, id, lawyer.0.person_id).await?;
    Ok((
        StatusCode::OK,
        Json(NotationSignatureResponse {
            notation_id: id,
            state: final_state.as_str().to_string(),
            signature_request_id: signature_request_id.0,
        }),
    )
        .into_response())
}

/// Request body for `POST /app/api/review-documents/{id}/comments` — one anchored
/// comment on a review document. The anchor is a ProseMirror position range
/// plus the text it covered, captured client-side from the read-only document.
#[derive(Deserialize)]
struct AddReviewCommentRequest {
    anchor_start: i32,
    anchor_end: i32,
    quoted_text: String,
    body: String,
}

/// Success body for `POST /app/api/review-documents/{id}/comments` — the created
/// comment and its conversation-log spine row.
#[derive(Serialize)]
struct AddReviewCommentResponse {
    comment_id: Uuid,
    communication_id: Uuid,
}

/// `POST /app/api/review-documents/{id}/comments` — add one anchored comment to a
/// review document, folding it into the matter's conversation log. This is the
/// first **client-writable** `/app/api` door: any authenticated caller reaches it
/// (embedded Rego policy), but the shared command (`crate::review::create_review_comment`)
/// enforces **client-lens** matter scope — the caller must participate in the
/// document's matter through the client side, the same gate the read-only
/// review surface uses — so a firm-side-only lawyer or a non-participant gets a
/// bare 404. `direction` is derived from the caller's role. `201` on success.
async fn add_review_comment(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<AddReviewCommentRequest>,
) -> Result<Response, ApiError> {
    let created = crate::review::create_review_comment(
        &state.surreal,
        authed.0.role,
        authed.0.person_id,
        id,
        None,
        input.anchor_start,
        input.anchor_end,
        &input.quoted_text,
        &input.body,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(AddReviewCommentResponse {
            comment_id: created.comment_id,
            communication_id: created.communication_id,
        }),
    )
        .into_response())
}

/// Success body for `POST /app/api/documents/{id}/deletion-requests` — the pending
/// expunge request, and whether it already existed (idempotent re-ask).
#[derive(Serialize)]
struct DeletionRequestResponse {
    request_id: Uuid,
    already_pending: bool,
}

/// `POST /app/api/documents/{id}/deletion-requests` — a matter participant asks for
/// a document to be deleted (a lawyer/admin must later authorize the actual
/// expunge; this only records the request). Client-writable like the review
/// surface: any authenticated caller reaches it (embedded Rego policy), but the shared command
/// (`crate::expunge_request_route::request_document_deletion`) enforces
/// **client-lens** matter scope — a firm-side-only lawyer or a non-participant
/// gets a bare 404. Idempotent: a second ask while one is pending returns the
/// existing request (`200`); a fresh request is `201`.
async fn create_deletion_request(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let outcome = crate::expunge_request_route::request_document_deletion(
        &state.surreal,
        authed.0.person_id,
        id,
        None,
    )
    .await?;
    let status = if outcome.already_pending {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(DeletionRequestResponse {
            request_id: outcome.request_id,
            already_pending: outcome.already_pending,
        }),
    )
        .into_response())
}

/// Request body for `POST /app/api/notations/{id}/clauses` — one custom clause to
/// append to a notation's assembled document.
#[derive(Deserialize)]
struct AddClauseRequest {
    body: String,
}

/// Success body for `POST /app/api/notations/{id}/clauses` — the appended clause.
#[derive(Serialize)]
struct AddClauseResponse {
    clause_id: Uuid,
}

/// `POST /app/api/notations/{id}/clauses` — append a custom clause to a notation's
/// document (spliced into the assembled body at render time). Lawyer-tier only
/// (embedded Rego policy) *and* matter-scoped: the acting lawyer must participate in the
/// notation's matter (admin bypasses), collapsing an out-of-scope or unknown
/// notation to 404. Appends through `store::notation_clauses::append` — the
/// same store command the lawyer clause form drives. `201` with the clause id.
async fn add_clause(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<AddClauseRequest>,
) -> Result<Response, ApiError> {
    let body = input.body.trim();
    if body.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "message": "A clause body is required."
            })),
        )
            .into_response());
    }

    // Resolve the notation and enforce matter scope before appending.
    let notation = store::notations::find_by_id(&state.surreal, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }

    let clause_id = store::notation_clauses::append(&state.surreal, id, body, lawyer.0.person_id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(AddClauseResponse { clause_id })).into_response())
}

/// Resolve a clause under a notation and enforce lawyer matter scope. Returns
/// 404 (never disclosing) when the notation is missing or out of the acting
/// lawyer's scope, or the clause does not belong to that notation.
async fn clause_in_scope(
    state: &ApiState,
    lawyer: &SessionData,
    notation_id: Uuid,
    clause_id: Uuid,
) -> Result<(), ApiError> {
    let notation = store::notations::find_by_id(&state.surreal, notation_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.person_id,
        lawyer.role,
        notation.project_id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    let clause = store::notation_clauses::find_by_id(&state.surreal, clause_id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if clause.notation_id != notation_id {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// `PATCH /app/api/notations/{id}/clauses/{clause_id}` — replace a clause's body.
/// Lawyer-tier only (embedded Rego policy) *and* matter-scoped; a blank body is 400, and a clause
/// on another notation (or an out-of-scope notation) is 404. Drives the same
/// store command as the lawyer clause form.
async fn edit_clause(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, clause_id)): Path<(Uuid, Uuid)>,
    JsonOrForm(input): JsonOrForm<AddClauseRequest>,
) -> Result<Response, ApiError> {
    let body = input.body.trim();
    if body.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "message": "A clause body is required."
            })),
        )
            .into_response());
    }
    clause_in_scope(&state, &lawyer.0, id, clause_id).await?;
    store::notation_clauses::update_body(&state.surreal, clause_id, body)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok((StatusCode::OK, Json(AddClauseResponse { clause_id })).into_response())
}

/// `DELETE /app/api/notations/{id}/clauses/{clause_id}` — remove a clause.
/// Lawyer-tier only (embedded Rego policy) *and* matter-scoped. `204 No Content` on success.
async fn delete_clause(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, clause_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    clause_in_scope(&state, &lawyer.0, id, clause_id).await?;
    store::notation_clauses::delete(&state.surreal, clause_id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Request body for `POST /app/api/notations/{id}/clauses/{clause_id}/move`.
#[derive(Deserialize)]
struct MoveClauseRequest {
    /// `up` moves the clause one step earlier in render order; anything else
    /// (e.g. `down`) moves it later. A move at the ends is a no-op.
    direction: String,
}

/// `POST /app/api/notations/{id}/clauses/{clause_id}/move` — reorder a clause by
/// swapping it with its neighbour. Lawyer-tier only (embedded Rego policy) *and* matter-scoped.
/// `204 No Content` on success (a move at the ends is an idempotent no-op).
async fn move_clause(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((id, clause_id)): Path<(Uuid, Uuid)>,
    JsonOrForm(input): JsonOrForm<MoveClauseRequest>,
) -> Result<Response, ApiError> {
    clause_in_scope(&state, &lawyer.0, id, clause_id).await?;
    let up = input.direction == "up";
    store::notation_clauses::move_clause(&state.surreal, clause_id, up)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Success body for `POST /app/api/projects/{id}/contract-review` — the created
/// `contract_reviews` row (its deviation analysis is attached and the matter
/// is parked at `lawyer_review`).
#[derive(Serialize)]
struct ContractReviewResponse {
    review_id: Uuid,
}

/// `POST /app/api/projects/{id}/contract-review` — upload an inbound third-party
/// contract for playbook review (`multipart/form-data`: a `file` part, or a
/// `text` part with the pasted contract). Opens a `memo__contract_review`
/// notation, files the contract, runs the deviation analysis against the
/// client Entity's playbook, and lands the matter at `lawyer_review`.
///
/// Client-writable like the review surface (a matter's client can submit their
/// own contract, or the firm can): any authenticated caller reaches it (embedded Rego policy),
/// but the door then enforces matter scope through *either* lens (lawyer or
/// client), so a non-participant gets a bare 404. If the client Entity has no
/// playbook yet, returns `422`. Drives the same command
/// (`crate::contract_review_walk::drive_contract_review`) as the lawyer/portal
/// upload form. `201` with the new review id.
#[allow(clippy::too_many_lines)]
async fn upload_contract_review(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    // A contract must be attributable to a person.
    let Some(person_id) = authed.0.person_id else {
        return Err(ApiError::Forbidden);
    };
    // Matter scope through either lens — the firm reviews its client's
    // contract, and the client may submit one. A non-participant sees 404.
    let lawyer_ok = store::access::can_see_project_as_lawyer(
        &state.surreal,
        Some(person_id),
        authed.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    let client_ok = store::access::can_see_project_as_client(&state.surreal, Some(person_id), id)
        .await
        .unwrap_or(false);
    if !lawyer_ok && !client_ok {
        return Err(ApiError::NotFound);
    }

    let bad_request = |message: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_request", "message": message })),
        )
            .into_response()
    };
    let Some(form) = crate::contract_review_walk::parse_form(multipart).await else {
        return Ok(bad_request("malformed multipart body"));
    };
    let Some(artifact) = form.into_artifact() else {
        return Ok(bad_request(
            "a `file` part or a non-empty `text` part is required",
        ));
    };
    let contract_text = artifact.contract_text();
    let filename = artifact.default_filename();

    let deps = crate::contract_review_walk::ReviewDeps {
        surreal: &state.surreal,
        workflow_runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        contract_reviewer: state.contract_reviewer.as_ref(),
    };
    let review_id = crate::contract_review_walk::drive_contract_review(
        &deps,
        id,
        person_id,
        &filename,
        &contract_text,
        artifact.into_workflow_artifact(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ContractReviewResponse { review_id }),
    )
        .into_response())
}

async fn list_entities(
    State(surreal): State<store::surreal::SurrealDb>,
) -> Result<Json<Vec<store::entities::Entity>>, ApiError> {
    let rows = store::entities::all(&surreal).await?;
    Ok(Json(rows))
}

async fn get_entity(
    State(surreal): State<store::surreal::SurrealDb>,
    Path(id): Path<Uuid>,
) -> Result<Json<store::entities::Entity>, ApiError> {
    store::entities::find_by_id(&surreal, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn list_jurisdictions(
    State(surreal): State<store::surreal::SurrealDb>,
) -> Result<Json<Vec<store::jurisdictions::Jurisdiction>>, ApiError> {
    let rows = store::jurisdictions::list_all(&surreal).await?;
    Ok(Json(rows))
}

async fn list_entity_types(
    State(surreal): State<store::surreal::SurrealDb>,
) -> Result<Json<Vec<store::entity_types::EntityType>>, ApiError> {
    let rows = store::entity_types::list(&surreal, &[]).await?;
    Ok(Json(rows))
}

/// Request body for `POST /app/api/templates/validate`. The caller hands
/// over markdown they're drafting; we lint it and return violations
/// without touching the database.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    /// Raw markdown body, including any YAML frontmatter.
    pub contents: String,
    /// Optional pretend filename so rules that key off the path
    /// (`N103` snake_case) and the response have something meaningful
    /// to report. Defaults to `template.md` — a snake_case placeholder
    /// so the default doesn't pollute the response with a filename
    /// complaint the caller never intended.
    #[serde(default)]
    pub path: Option<String>,
    /// When true, lint with the Markdown-only rule set (drops the
    /// N-family, adds `S102` line packing) — use this for plain prose.
    /// Defaults to false: the full Neon Law Navigator notation rule set
    /// runs.
    #[serde(default)]
    pub markdown_only: bool,
}

#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    pub path: String,
    pub clean: bool,
    pub violations: Vec<ValidationViolation>,
}

#[derive(Debug, Serialize)]
pub struct ValidationViolation {
    pub code: &'static str,
    pub line: usize,
    pub message: String,
}

/// Request body for `POST /app/api/projects/{id}/documents` — a single document
/// to file into the matter. The browser uploads via multipart; the REST door
/// takes the bytes base64-encoded so a programmatic caller needs no form.
#[derive(Deserialize)]
struct UploadDocumentRequest {
    filename: String,
    /// Base64-encoded file bytes.
    content_base64: String,
    content_type: Option<String>,
    /// Required asset-lane classification. Omitted or blank is `400
    /// kind_required`; a value outside the lane is `400 invalid_kind`.
    kind: Option<String>,
    /// `"client"` makes the document client-visible; anything else (the default)
    /// files it as internal work product.
    visibility: Option<String>,
    description: Option<String>,
}

/// `POST /app/api/projects/{id}/documents` — file a document into a matter, the
/// REST mirror of the lawyer upload control. Both converge on
/// `matter_documents::record_document`. Lawyer-tier only and matter-scoped
/// (out-of-scope → 404). `201` with the new document id; a blank filename,
/// a missing or blank `kind`, undecodable base64, or a `kind` the asset lane
/// does not accept is `400`.
///
/// That last one is why this door carries [`ApiError::Ingest`] rather than collapsing
/// every ingest failure into a 500: the lawyer form constrains `kind` to a `<select>`
/// and so cannot produce a bad value, but this door is reachable without the form.
async fn upload_document_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<UploadDocumentRequest>,
) -> Result<Response, ApiError> {
    let in_scope = store::access::can_see_project_as_lawyer(
        &state.surreal,
        lawyer.0.person_id,
        lawyer.0.role,
        id,
    )
    .await
    .unwrap_or(false);
    if !in_scope {
        return Err(ApiError::NotFound);
    }
    let filename = input.filename.trim();
    if filename.is_empty() {
        return Ok(bad_request("filename_required", "A filename is required."));
    }
    let kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(kind) = kind else {
        return Ok(bad_request(
            "kind_required",
            &format!(
                "A document kind is required. Accepted values are: {}.",
                accepted_asset_kinds().join(", ")
            ),
        ));
    };
    let bytes = {
        use base64::Engine as _;
        match base64::engine::general_purpose::STANDARD.decode(input.content_base64.as_bytes()) {
            Ok(b) => b,
            Err(_) => {
                return Ok(bad_request(
                    "invalid_base64",
                    "content_base64 is not valid base64.",
                ))
            }
        }
    };
    let visibility = if input.visibility.as_deref() == Some(store::documents::visibility::CLIENT) {
        store::documents::visibility::CLIENT
    } else {
        store::documents::visibility::INTERNAL
    };
    // File under the acting lawyer for git attribution, mirroring the form.
    let (author_name, author_email) = match lawyer.0.person_id {
        Some(pid) => match store::persons::find_by_id(&state.surreal, pid)
            .await
            .ok()
            .flatten()
        {
            Some(p) => (p.name, p.email),
            None => ("Navigator API".to_string(), "api@neonlaw.com".to_string()),
        },
        None => ("Navigator API".to_string(), "api@neonlaw.com".to_string()),
    };
    let args = store::documents::IngestArgs {
        project_id: id,
        source: store::documents::source::UPLOAD,
        filename,
        kind,
        content_type: input
            .content_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("application/octet-stream"),
        description: input
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        secondary_storage_key: None,
        visibility,
    };
    let ingested = crate::matter_documents::record_document(
        &state.surreal,
        &state.storage,
        repos::Author {
            name: &author_name,
            email: &author_email,
        },
        &args,
        &bytes,
    )
    .await
    .map_err(ApiError::Ingest)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "document_id": ingested.asset_id })),
    )
        .into_response())
}

/// Map a contract-review action outcome onto an API response: `204` on success,
/// `404` for a missing/out-of-scope review or finding, `409` off-gate, `422`
/// when approval is blocked by unacted findings, `500` on a store/runtime fault.
// `ApiError` is the pervasive Err type across this whole surface; the async
// door handlers carry it without the lint, and this sync helper mirrors them.
#[allow(clippy::result_large_err)]
fn review_action_response(
    result: Result<(), crate::admin_contract_reviews::ReviewActionError>,
    review_id: Uuid,
) -> Result<Response, ApiError> {
    use crate::admin_contract_reviews::ReviewActionError as E;
    match result {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(E::NotFoundOrScoped | E::FindingNotFound) => Err(ApiError::NotFound),
        Err(E::NotOpen) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "not_open",
                "message": "This contract review is not at the gate this action requires."
            })),
        )
            .into_response()),
        Err(E::FindingsUnacted) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "findings_unacted",
                "message": "Every finding must be accepted or rejected before the memo can be approved."
            })),
        )
            .into_response()),
        Err(e @ E::Db(_)) => {
            tracing::error!(error = %e, %review_id, "api contract-review action failed");
            Err(ApiError::Db("the contract-review action failed".into()))
        }
    }
}

/// Request body for `POST /app/api/contract-reviews/{id}/findings/{idx}` — the
/// attorney's decision and edits on one finding.
#[derive(Deserialize)]
struct SaveReviewFindingRequest {
    /// Accept (`true`) or reject (`false`) the finding for delivery.
    #[serde(default)]
    accept: bool,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    suggested_redline: String,
    #[serde(default)]
    attorney_note: String,
}

/// `POST /app/api/contract-reviews/{id}/findings/{idx}` — save the attorney's
/// edits and accept/reject decision on one finding, the REST mirror of the
/// review surface. Both converge on `admin_contract_reviews::save_review_finding`.
/// Lawyer-tier only and matter-scoped (out-of-scope → 404). `204` on success; a
/// closed review is `409`; an out-of-range finding index is `404`.
async fn save_review_finding_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path((review_id, idx)): Path<(Uuid, usize)>,
    JsonOrForm(input): JsonOrForm<SaveReviewFindingRequest>,
) -> Result<Response, ApiError> {
    let result = crate::admin_contract_reviews::save_review_finding(
        &state.surreal,
        review_id,
        idx,
        lawyer.0.person_id,
        lawyer.0.role,
        input.accept,
        &input.severity,
        &input.suggested_redline,
        &input.attorney_note,
    )
    .await;
    review_action_response(result, review_id)
}

/// Request body for `POST /app/api/contract-reviews/{id}/summary`.
#[derive(Deserialize)]
struct SaveReviewSummaryRequest {
    #[serde(default)]
    risk_summary: String,
}

/// `POST /app/api/contract-reviews/{id}/summary` — edit a review's risk summary,
/// the REST mirror of the review surface. Converges on
/// `admin_contract_reviews::save_review_summary`. Lawyer-tier only and
/// matter-scoped. `204` on success; out of scope is `404`.
async fn save_review_summary_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(review_id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<SaveReviewSummaryRequest>,
) -> Result<Response, ApiError> {
    let result = crate::admin_contract_reviews::save_review_summary(
        &state.surreal,
        review_id,
        lawyer.0.person_id,
        lawyer.0.role,
        &input.risk_summary,
    )
    .await;
    review_action_response(result, review_id)
}

/// `POST /app/api/contract-reviews/{id}/approve` — assemble and deliver the
/// review memo and approve, the REST mirror of the review surface. Converges on
/// `admin_contract_reviews::approve_review`. Lawyer-tier only and matter-scoped.
/// `204` on success; not at the review gate is `409`; approving before every
/// finding is acted on is `422`.
async fn approve_review_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(review_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let result = crate::admin_contract_reviews::approve_review(
        &state.surreal,
        &state.storage,
        state.workflow_runtime.as_ref(),
        review_id,
        lawyer.0.person_id,
        lawyer.0.role,
    )
    .await;
    review_action_response(result, review_id)
}

/// `POST /app/api/contract-reviews/{id}/reject` — reject a review without a memo,
/// the REST mirror of the review surface. Converges on
/// `admin_contract_reviews::reject_review`. Lawyer-tier only and matter-scoped.
/// `204` on success; not at the review gate is `409`.
async fn reject_review_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(review_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let result = crate::admin_contract_reviews::reject_review(
        &state.surreal,
        state.workflow_runtime.as_ref(),
        review_id,
        lawyer.0.person_id,
        lawyer.0.role,
    )
    .await;
    review_action_response(result, review_id)
}

/// Request body for `POST /app/api/playbooks` — a client Entity's negotiating
/// positions. Unlike the lawyer form (a pipe-delimited textarea), the REST door
/// takes the positions as structured objects.
#[derive(Deserialize)]
struct CreatePlaybookRequest {
    entity_id: Uuid,
    name: String,
    #[serde(default)]
    positions: Vec<store::playbooks::Position>,
}

/// `POST /app/api/playbooks` — create a contract-review playbook for a Company,
/// the REST mirror of the lawyer playbook form. Both converge on
/// `store::playbooks::create`. Lawyer-tier only. `201` with the new id; a blank
/// name or empty position set is `400`; a duplicate name on that Company is
/// `409`.
async fn create_playbook_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    JsonOrForm(input): JsonOrForm<CreatePlaybookRequest>,
) -> Result<Response, ApiError> {
    if input.name.trim().is_empty() {
        return Ok(bad_request(
            "playbook_name_required",
            "A playbook name is required.",
        ));
    }
    if input.positions.is_empty() {
        return Ok(bad_request(
            "positions_required",
            "Enter at least one negotiating position.",
        ));
    }
    match store::playbooks::create(
        &state.surreal,
        &store::playbooks::NewPlaybook {
            entity_id: input.entity_id,
            name: input.name.trim(),
            positions: &input.positions,
        },
    )
    .await
    {
        Ok(id) => Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({ "playbook_id": id })),
        )
            .into_response()),
        Err(store::playbooks::PlaybookError::DuplicateName(_)) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "duplicate_name",
                "message": "That Company already has a playbook with that name."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, "api create playbook: failed");
            Err(ApiError::Db("the playbook could not be created".into()))
        }
    }
}

/// Request body for `PUT /app/api/playbooks/{id}` — the replacement position set.
#[derive(Deserialize)]
struct UpdatePlaybookRequest {
    #[serde(default)]
    positions: Vec<store::playbooks::Position>,
}

/// `PUT /app/api/playbooks/{id}` — replace a playbook's position set, the REST
/// mirror of the lawyer edit form. Both converge on
/// `store::playbooks::update_positions`. Lawyer-tier only. `204` on success; an
/// unknown playbook is `404`; an empty position set is `400`.
async fn update_playbook_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Path(id): Path<Uuid>,
    JsonOrForm(input): JsonOrForm<UpdatePlaybookRequest>,
) -> Result<Response, ApiError> {
    if store::playbooks::by_id(&state.surreal, id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(ApiError::NotFound);
    }
    if input.positions.is_empty() {
        return Ok(bad_request(
            "positions_required",
            "Enter at least one negotiating position.",
        ));
    }
    match store::playbooks::update_positions(&state.surreal, id, &input.positions).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => {
            tracing::error!(error = %e, %id, "api update playbook: failed");
            Err(ApiError::Db(
                "the playbook positions could not be saved".into(),
            ))
        }
    }
}

/// A `400 Bad Request` JSON body in the shared `{error, message}` shape.
fn bad_request(error: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

/// `POST /app/api/expunge-requests/{id}/authorize` — an admin authorizes a
/// pending client expunge request, running the governed expunge. Admin-tier only
/// (owner/admin): embedded Rego admits admin, and the extractor re-checks the
/// tier. `204` on success; a request already resolved is `409`; an unknown
/// request or its vanished document is `404`. Converges on
/// `expunge_request_route::authorize_expunge_request`.
async fn authorize_expunge_door(
    State(state): State<ApiState>,
    authed: AuthedSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    if !authed.0.role.is_admin_tier() {
        return Err(ApiError::Forbidden);
    }
    let authorizer = authed.0.person_id.ok_or(ApiError::Forbidden)?;
    match crate::expunge_request_route::authorize_expunge_request(
        &state.surreal,
        &state.storage,
        id,
        authorizer,
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(crate::expunge_request_route::ExpungeRequestActionError::NotFound) => {
            Err(ApiError::NotFound)
        }
        Err(crate::expunge_request_route::ExpungeRequestActionError::AlreadyResolved) => Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "already_resolved",
                "message": "This expunge request has already been resolved."
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!(error = %e, request_id = %id, "api authorize expunge: failed");
            Err(ApiError::Db(
                "the expunge request could not be authorized".into(),
            ))
        }
    }
}

/// `POST /app/api/expunge-requests/{id}/deny` — a lawyer or admin denies a
/// pending client expunge request without deleting anything. Lawyer-tier only.
/// `204` on success; an unknown or already-resolved request is `404`. Converges
/// on `expunge_request_route::deny_expunge_request`.
async fn deny_expunge_door(
    State(state): State<ApiState>,
    lawyer: LawyerSession,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let resolver = lawyer.0.person_id.ok_or(ApiError::Forbidden)?;
    match crate::expunge_request_route::deny_expunge_request(&state.surreal, id, resolver).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(crate::expunge_request_route::ExpungeRequestActionError::NotFound) => {
            Err(ApiError::NotFound)
        }
        Err(e) => {
            tracing::error!(error = %e, request_id = %id, "api deny expunge: failed");
            Err(ApiError::Db(
                "the expunge request could not be denied".into(),
            ))
        }
    }
}

/// Lint markdown without persisting it. Mirrors the `cli validate`
/// rule-set selection so a notation that passes the CLI passes here
/// and vice versa. Requires an authenticated session — same posture
/// as the rest of `/app/api/*`.
async fn validate_template(Json(req): Json<ValidateRequest>) -> Json<ValidateResponse> {
    let path = req.path.unwrap_or_else(|| "template.md".to_string());
    let rule_set = if req.markdown_only {
        rules::navigator_markdown_only_rules()
    } else {
        rules::navigator_default_rules()
    };
    let file = rules::SourceFile {
        path: std::path::PathBuf::from(&path),
        contents: req.contents,
    };
    let mut violations = Vec::new();
    for rule in &rule_set {
        for v in rule.lint(&file) {
            violations.push(ValidationViolation {
                code: v.code,
                line: v.line,
                message: v.message,
            });
        }
    }
    // `clean` means no *blocking* (Error-severity) violations. Yellow
    // advisories like N112 ("step allowed but not built yet") are still
    // returned in `violations` so the caller sees them, but they don't
    // flip `clean` to false — mirroring `navigator validate`.
    let clean = !violations
        .iter()
        .any(|v| rules::severity_for_code(v.code) == rules::Severity::Error);
    Json(ValidateResponse {
        path,
        clean,
        violations,
    })
}

#[derive(Debug)]
pub enum ApiError {
    Unauthenticated,
    Forbidden,
    NotFound,
    Command(crate::people_commands::PeopleCommandError),
    /// A read or write against the person directory failed.
    Person(store::persons::PersonError),
    /// A read against the jurisdiction reference table failed.
    Jurisdictions(store::jurisdictions::JurisdictionError),
    EntityTypes(store::entity_types::EntityTypeError),
    Entity(store::entity_commands::EntityCommandError),
    Project(store::projects::ProjectCommandError),
    OpenMatter(store::projects::OpenMatterError),
    AddParticipant(store::participation::AddParticipantError),
    UpdateParticipant(store::participation::UpdateParticipantError),
    RemoveParticipant(store::participation::RemoveParticipantError),
    CreateNotation(crate::project_notation::CreateProjectNotationError),
    AnswerNotation(workflows::NotationSessionError),
    SendIntake(crate::retainer_walk::SendIntakeError),
    /// A retainer workflow drive (approve or send-for-signature) failed.
    NotationWorkflow(crate::retainer_walk::WorkflowDriveError),
    ReviewComment(crate::review::ReviewCommentError),
    RequestDeletion(crate::expunge_request_route::RequestDeletionError),
    ContractReview(crate::contract_review_walk::ContractReviewError),
    Db(String),
    /// A read or write against the Surreal-backed `notations` store failed.
    Notation(store::notations::NotationError),
    /// Filing a document into a matter failed. Carried as the store's own
    /// error rather than a string so the door can tell a caller's bad `kind`
    /// (a 400) from a database or storage fault (a 500) by matching the
    /// variant — never by matching the message text.
    Ingest(store::documents::IngestError),
}

impl From<store::persons::PersonError> for ApiError {
    fn from(e: store::persons::PersonError) -> Self {
        Self::Person(e)
    }
}

impl From<store::entities::EntityError> for ApiError {
    fn from(e: store::entities::EntityError) -> Self {
        // Routed through the command error so one match arm renders every
        // entity failure — the read doors and the write doors agree.
        Self::Entity(store::entity_commands::EntityCommandError::from(e))
    }
}

impl From<store::jurisdictions::JurisdictionError> for ApiError {
    fn from(e: store::jurisdictions::JurisdictionError) -> Self {
        Self::Jurisdictions(e)
    }
}

impl From<store::entity_types::EntityTypeError> for ApiError {
    fn from(e: store::entity_types::EntityTypeError) -> Self {
        Self::EntityTypes(e)
    }
}

impl From<store::participation::AddParticipantError> for ApiError {
    fn from(e: store::participation::AddParticipantError) -> Self {
        Self::AddParticipant(e)
    }
}

impl From<store::participation::UpdateParticipantError> for ApiError {
    fn from(e: store::participation::UpdateParticipantError) -> Self {
        Self::UpdateParticipant(e)
    }
}

impl From<store::participation::RemoveParticipantError> for ApiError {
    fn from(e: store::participation::RemoveParticipantError) -> Self {
        Self::RemoveParticipant(e)
    }
}

impl From<crate::project_notation::CreateProjectNotationError> for ApiError {
    fn from(e: crate::project_notation::CreateProjectNotationError) -> Self {
        Self::CreateNotation(e)
    }
}

impl From<workflows::NotationSessionError> for ApiError {
    fn from(e: workflows::NotationSessionError) -> Self {
        Self::AnswerNotation(e)
    }
}

impl From<crate::retainer_walk::SendIntakeError> for ApiError {
    fn from(e: crate::retainer_walk::SendIntakeError) -> Self {
        Self::SendIntake(e)
    }
}

impl From<crate::retainer_walk::WorkflowDriveError> for ApiError {
    fn from(e: crate::retainer_walk::WorkflowDriveError) -> Self {
        Self::NotationWorkflow(e)
    }
}

impl From<crate::review::ReviewCommentError> for ApiError {
    fn from(e: crate::review::ReviewCommentError) -> Self {
        Self::ReviewComment(e)
    }
}

impl From<crate::expunge_request_route::RequestDeletionError> for ApiError {
    fn from(e: crate::expunge_request_route::RequestDeletionError) -> Self {
        Self::RequestDeletion(e)
    }
}

impl From<crate::contract_review_walk::ContractReviewError> for ApiError {
    fn from(e: crate::contract_review_walk::ContractReviewError) -> Self {
        Self::ContractReview(e)
    }
}

/// Map the retainer workflow-drive error (approve / send) to a typed JSON
/// response. The two caller-facing variants — the PDF isn't rendered yet
/// (retryable) and a government-form fill/config failure — get their own
/// status + code; the rest are internal faults, logged and returned as 500.
fn workflow_drive_error(e: crate::retainer_walk::WorkflowDriveError) -> Response {
    use crate::retainer_walk::WorkflowDriveError as E;
    match e {
        // The worker has not rendered + persisted the PDF yet — not a fault,
        // a "retry in a moment" the caller resolves by re-polling and retrying.
        E::DocumentNotReady(_) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "document_not_ready",
                "message": "The document PDF has not been rendered yet — the worker is still \
                            rendering, or its storage is misconfigured. Retry in a moment."
            })),
        )
            .into_response(),
        // The engagement agreement leaves its fee terms to the custom-clause
        // slot and no clause was written. Actionable by the lawyer: add the
        // fee clause, then send again. The walk is still at `lawyer_review`.
        E::ClausesRequired(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "clauses_required",
                "message": "The fee terms have not been written. This engagement agreement \
                            leaves the fee to its custom clauses, so add at least one clause \
                            before sending it for signature."
            })),
        )
            .into_response(),
        // A government-form (AcroForm) fill or config failure. The reason is
        // actionable (a missing blank, a pin mismatch, a mis-mapped field).
        E::Form { form_code, reason } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "form_error",
                "message": format!("Government form `{form_code}` could not be prepared: {reason}")
            })),
        )
            .into_response(),
        other => {
            tracing::error!(error = %other, "api: notation workflow drive failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal" })),
            )
                .into_response()
        }
    }
}

/// Map the notation questionnaire engine's error to a typed JSON response. The
/// caller-facing variants (wrong step, already complete, a question this intake
/// does not accept here) get their own status + code; the rest are internal
/// faults, logged and returned as a bare 500.
fn answer_notation_error(e: workflows::NotationSessionError) -> Response {
    use workflows::NotationSessionError as E;
    match e {
        // No such notation. (Out-of-scope is handled in the handler before the
        // engine is reached, and also collapses to a bare 404.)
        E::NotationNotFound(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not_found" })),
        )
            .into_response(),
        // The answer named a different step than the one the questionnaire is
        // currently asking — the caller is out of sync. 409 with both codes so
        // it can re-fetch and retry the expected step.
        E::QuestionMismatch { expected, got } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "question_mismatch",
                "message": format!("The questionnaire is asking `{expected}`, not `{got}`."),
                "expected": expected,
                "got": got
            })),
        )
            .into_response(),
        // Nothing left to answer.
        E::AlreadyComplete => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "already_complete",
                "message": "This notation's questionnaire is already complete."
            })),
        )
            .into_response(),
        // The question exists but this door does not accept it: not client-facing,
        // or not flagged by the lawyer review for re-collection.
        E::QuestionNotClientFacing(code) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "question_not_answerable",
                "message": format!("`{code}` is not a client-facing question on this notation's intake.")
            })),
        )
            .into_response(),
        E::QuestionNotFlagged(code) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "question_not_answerable",
                "message": format!("`{code}` was not flagged for re-collection by the lawyer review.")
            })),
        )
            .into_response(),
        // Everything else is an internal fault (seed gap, runtime, db, spec,
        // snapshot, or a creation-time error that cannot arise on answer).
        other => {
            tracing::error!(error = %other, "api: answer notation step failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal" })),
            )
                .into_response()
        }
    }
}

impl From<store::projects::ProjectCommandError> for ApiError {
    fn from(e: store::projects::ProjectCommandError) -> Self {
        Self::Project(e)
    }
}

impl From<store::projects::OpenMatterError> for ApiError {
    fn from(e: store::projects::OpenMatterError) -> Self {
        Self::OpenMatter(e)
    }
}

impl From<store::entity_commands::EntityCommandError> for ApiError {
    fn from(e: store::entity_commands::EntityCommandError) -> Self {
        Self::Entity(e)
    }
}

impl From<String> for ApiError {
    fn from(e: String) -> Self {
        Self::Db(e)
    }
}

impl From<store::notations::NotationError> for ApiError {
    fn from(e: store::notations::NotationError) -> Self {
        Self::Notation(e)
    }
}

impl From<store::documents::IngestError> for ApiError {
    fn from(e: store::documents::IngestError) -> Self {
        Self::Ingest(e)
    }
}

impl From<crate::people_commands::PeopleCommandError> for ApiError {
    fn from(e: crate::people_commands::PeopleCommandError) -> Self {
        Self::Command(e)
    }
}

impl IntoResponse for ApiError {
    // One arm per command-error variant; the match grows with each resource on
    // the command boundary. Splitting a flat dispatch into sub-functions would
    // scatter the wire-shape mapping, so allow the length (as `openapi` does).
    #[allow(clippy::too_many_lines)]
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "unauthenticated" })),
            )
                .into_response(),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "forbidden" })),
            )
                .into_response(),
            Self::NotFound
            | Self::Command(crate::people_commands::PeopleCommandError::NotFound)
            | Self::Entity(store::entity_commands::EntityCommandError::NotFound)
            | Self::Project(store::projects::ProjectCommandError::NotFound)
            | Self::AddParticipant(
                store::participation::AddParticipantError::ProjectNotFound
                | store::participation::AddParticipantError::PersonNotFound,
            )
            | Self::UpdateParticipant(
                store::participation::UpdateParticipantError::NotFound
                | store::participation::UpdateParticipantError::PersonNotFound,
            )
            | Self::RemoveParticipant(store::participation::RemoveParticipantError::NotFound)
            // No such matter, or the acting lawyer is not in its scope: both
            // collapse to a bare not_found so the door never discloses a
            // project outside the caller's matters.
            | Self::CreateNotation(
                crate::project_notation::CreateProjectNotationError::ProjectNotFound,
            )
            | Self::SendIntake(crate::retainer_walk::SendIntakeError::NotationNotFound(_))
            // No such review document, still a draft, out of the caller's
            // client-lens scope, or the caller can't author (anon/Clerk): all
            // collapse to a bare not_found so the door never discloses it.
            | Self::ReviewComment(crate::review::ReviewCommentError::NotFound)
            | Self::RequestDeletion(
                crate::expunge_request_route::RequestDeletionError::NotFound,
            ) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "not_found" })),
            )
                .into_response(),
            // The matter still has dependents (participations, notations): a
            // conflict the caller resolves by detaching those first, carrying
            // the database's own detail so they see which records.
            Self::Project(store::projects::ProjectCommandError::Referenced(detail)) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": format!("This matter is still referenced by other records ({detail}).")
                })),
            )
                .into_response(),
            Self::Project(store::projects::ProjectCommandError::Db(e)) => {
                tracing::error!(error = %e, "api: project command failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- matter open ---
            // A missing attestation is its own error code so a client can
            // distinguish "you must attest" from ordinary validation.
            Self::OpenMatter(store::projects::OpenMatterError::AttestationRequired) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "attestation_required",
                    "message": "Opening a matter requires the attorney's conflict attestation."
                })),
            )
                .into_response(),
            // A blocking conflict is a hard stop — adverse to a current client,
            // no attestation overrides it. Its own code + the findings, so the
            // caller sees why.
            Self::OpenMatter(store::projects::OpenMatterError::BlockingConflict(findings)) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict_blocked",
                    "message": "This matter is adverse to a current client. Resolve the conflict or record a waiver before opening.",
                    "findings": findings
                })),
            )
                .into_response(),
            Self::OpenMatter(store::projects::OpenMatterError::CodeConflict) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": "That project code is already in use."
                })),
            )
                .into_response(),
            Self::OpenMatter(store::projects::OpenMatterError::ClientNotAllowed) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": "The client of record must be a client, not a firm attorney."
                })),
            )
                .into_response(),
            Self::OpenMatter(store::projects::OpenMatterError::AttesterNotAllowed) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": "The attesting attorney must be a firm lawyer."
                })),
            )
                .into_response(),
            Self::OpenMatter(store::projects::OpenMatterError::NotFound(what)) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": format!("No such {what}.")
                })),
            )
                .into_response(),
            // The matter is open by the time an attestation write fails —
            // only its audit entry is missing — so it renders as the
            // server-side fault it is rather than as a refused open.
            Self::OpenMatter(store::projects::OpenMatterError::Attestation(e)) => {
                tracing::error!(error = %e, "api: matter opened but its attestation was not recorded");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::OpenMatter(store::projects::OpenMatterError::Db(e)) => {
                tracing::error!(error = %e, "api: open matter failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // A refused accountability change. Two of these are about *who is
            // asking* rather than about the request, so they answer `403` — a
            // caller who retries the same body with a different field will
            // never get past them.
            Self::AddParticipant(store::participation::AddParticipantError::Dri(
                e @ (store::participation::DriError::NotPermitted
                | store::participation::DriError::ActorUnknown),
            ))
            | Self::UpdateParticipant(store::participation::UpdateParticipantError::Dri(
                e @ (store::participation::DriError::NotPermitted
                | store::participation::DriError::ActorUnknown),
            ))
            | Self::RemoveParticipant(store::participation::RemoveParticipantError::Dri(
                e @ (store::participation::DriError::NotPermitted
                | store::participation::DriError::ActorUnknown),
            )) => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "forbidden",
                    "message": e.to_string()
                })),
            )
                .into_response(),
            // The rest are about the request: the wrong tier for that side, or
            // emptying the lawyer set. Unreachable on the add and update doors
            // while their request shapes carry no DRI field — mapped rather
            // than folded into the `500` arm so adding that field cannot
            // silently report a caller-correctable refusal as a server fault.
            // The command's own sentence is the message.
            Self::AddParticipant(store::participation::AddParticipantError::Dri(e))
            | Self::UpdateParticipant(store::participation::UpdateParticipantError::Dri(e))
            | Self::RemoveParticipant(store::participation::RemoveParticipantError::Dri(e)) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "unprocessable",
                    "message": e.to_string()
                })),
            )
                .into_response(),
            Self::AddParticipant(store::participation::AddParticipantError::Duplicate)
            | Self::UpdateParticipant(store::participation::UpdateParticipantError::Duplicate) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": "That person is already assigned to this matter."
                })),
            )
                .into_response(),
            Self::AddParticipant(store::participation::AddParticipantError::Db(e)) => {
                tracing::error!(error = %e, "api: add participant failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // The lawyer DRI can't be stranded off the firm side — a state
            // conflict the caller resolves by reassigning the DRI first, so 409.
            Self::UpdateParticipant(store::participation::UpdateParticipantError::DriLockout) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "dri_lockout",
                    "message": "That row is the matter's lawyer DRI; reassign the DRI before moving it off the firm side."
                })),
            )
                .into_response(),
            Self::RemoveParticipant(store::participation::RemoveParticipantError::DriLockout) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "dri_lockout",
                    "message": "That row is the matter's last lawyer DRI; designate another before removing it."
                })),
            )
                .into_response(),
            Self::UpdateParticipant(store::participation::UpdateParticipantError::Db(e)) => {
                tracing::error!(error = %e, "api: update participant failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::RemoveParticipant(store::participation::RemoveParticipantError::Db(e)) => {
                tracing::error!(error = %e, "api: remove participant failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- open a notation on a matter ---
            Self::CreateNotation(
                crate::project_notation::CreateProjectNotationError::EmptyInput,
            ) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": "template_code and client_email are required."
                })),
            )
                .into_response(),
            Self::CreateNotation(
                crate::project_notation::CreateProjectNotationError::ClientEmailTaken,
            ) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": "That client email already belongs to another person."
                })),
            )
                .into_response(),
            // The code was authored in neither this matter's repo nor the
            // bundled firm catalog.
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Session(
                workflows::NotationSessionError::TemplateNotFound(code),
            )) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not_found",
                    "message": format!("No template `{code}` in this matter's repo or the firm catalog.")
                })),
            )
                .into_response(),
            // The matter's first notation must be the engagement that opens it.
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Session(
                workflows::NotationSessionError::EngagementMustBeFirst { code, kind },
            )) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "engagement_first",
                    "message": format!(
                        "The first notation on this matter must be the engagement that opens it \
                         (a retainer or an onboarding); `{code}` is kind `{kind}`."
                    )
                })),
            )
                .into_response(),
            // The template is in the repo but fails a blocking authoring rule.
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Session(
                workflows::NotationSessionError::TemplateSource(
                    store::template_source::TemplateSourceError::Invalid { code, violations },
                ),
            )) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "template_invalid",
                    "message": format!(
                        "Template `{code}` has {} blocking rule violation(s); fix it in the repo and retry.",
                        violations.len()
                    )
                })),
            )
                .into_response(),
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Session(
                e,
            )) => {
                tracing::error!(error = %e, "api: create notation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::CreateNotation(
                crate::project_notation::CreateProjectNotationError::RepoUnconfigured,
            ) => {
                tracing::error!("api: create notation — git repo storage not configured");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Db(e)) => {
                tracing::error!(error = %e, "api: create notation database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- answer a notation's questionnaire step ---
            // The engine error enum has many variants; `answer_notation_error`
            // maps the caller-facing ones to typed JSON and the rest to 500.
            Self::AnswerNotation(e) => answer_notation_error(e),
            // --- send a notation's client their intake link ---
            // A notation with no bound client cannot be sent an intake link — a
            // state conflict the caller resolves by binding a client first.
            Self::SendIntake(crate::retainer_walk::SendIntakeError::NoClient(_)) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": "This notation has no client to send an intake link to."
                })),
            )
                .into_response(),
            Self::SendIntake(crate::retainer_walk::SendIntakeError::Db(e)) => {
                tracing::error!(error = %e, "api: send notation intake failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::SendIntake(crate::retainer_walk::SendIntakeError::Notation(e)) => {
                tracing::error!(error = %e, "api: send notation intake failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- approve / send a notation (fire the workflow transition) ---
            Self::NotationWorkflow(e) => workflow_drive_error(e),
            // --- upload a contract for playbook review ---
            // The client Entity has no playbook to measure the contract against
            // — a caller-actionable precondition, so 422 with the store's own
            // guidance rather than an opaque 500.
            Self::ContractReview(
                crate::contract_review_walk::ContractReviewError::NoPlaybook,
            ) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "no_playbook",
                    "message": "This company has no contract-review playbook on file yet. An attorney must create one before a contract can be reviewed."
                })),
            )
                .into_response(),
            Self::ContractReview(e) => {
                tracing::error!(error = %e, "api: contract-review upload failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- request a document's deletion ---
            // Authenticated, but the session carries no Person to attribute the
            // request to — forbidden rather than not-found, since the caller
            // *is* known to the door, they just can't author.
            Self::RequestDeletion(
                crate::expunge_request_route::RequestDeletionError::NoRequester,
            ) => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "forbidden",
                    "message": "The session has no linked person to attribute the request to."
                })),
            )
                .into_response(),
            Self::RequestDeletion(crate::expunge_request_route::RequestDeletionError::Db(e)) => {
                tracing::error!(error = %e, "api: request document deletion failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::RequestDeletion(crate::expunge_request_route::RequestDeletionError::Asset(e)) => {
                tracing::error!(error = %e, "api: request document deletion failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::RequestDeletion(crate::expunge_request_route::RequestDeletionError::Request(
                e,
            )) => {
                tracing::error!(error = %e, "api: request document deletion failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // --- add a comment to a review document ---
            Self::ReviewComment(crate::review::ReviewCommentError::Db(e)) => {
                tracing::error!(error = %e, "api: add review comment failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // Caller-correctable validation, whichever resource raised it:
            // one wire shape means a client parses `invalid_request` the same
            // way across the command surface.
            Self::Command(crate::people_commands::PeopleCommandError::Invalid(message))
            | Self::Entity(store::entity_commands::EntityCommandError::Invalid(message))
            | Self::Project(store::projects::ProjectCommandError::Invalid(message))
            | Self::OpenMatter(store::projects::OpenMatterError::Invalid(message))
            | Self::ReviewComment(crate::review::ReviewCommentError::Invalid(message)) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": message
                })),
            )
                .into_response(),
            Self::Command(crate::people_commands::PeopleCommandError::EmailConflict) => {
                tracing::warn!("api: create person email conflict");
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "conflict",
                        "message": "That email is already in use."
                    })),
                )
                    .into_response()
            }
            Self::Command(crate::people_commands::PeopleCommandError::Blocked(message)) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "blocked",
                    "message": message
                })),
            )
                .into_response(),
            Self::Command(crate::people_commands::PeopleCommandError::SendFailed) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": "send_failed" })),
            )
                .into_response(),
            // Both entity conflicts are the caller's to correct: a unique
            // violation and a second row under the firm anchor's name.
            Self::Entity(
                ref e @ (store::entity_commands::EntityCommandError::Conflict
                | store::entity_commands::EntityCommandError::FirmAnchorExists
                | store::entity_commands::EntityCommandError::FirmAnchorImmutable
                | store::entity_commands::EntityCommandError::FirmAnchorProtected
                | store::entity_commands::EntityCommandError::Referenced(_)),
            ) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "conflict",
                    "message": e.user_message()
                })),
            )
                .into_response(),
            Self::Entity(store::entity_commands::EntityCommandError::Entities(e)) => {
                tracing::error!(error = %e, "api: entity command failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Entity(store::entity_commands::EntityCommandError::Jurisdictions(e)) => {
                tracing::error!(error = %e, "api: entity jurisdiction lookup failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Entity(store::entity_commands::EntityCommandError::EntityTypes(e)) => {
                tracing::error!(error = %e, "api: entity type lookup failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::CreateNotation(crate::project_notation::CreateProjectNotationError::Person(e))
            | Self::SendIntake(crate::retainer_walk::SendIntakeError::Person(e)) => {
                tracing::error!(error = %e, "api: person directory error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Command(crate::people_commands::PeopleCommandError::Db(e)) => {
                tracing::error!(error = %e, "api: people command failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Person(e) => {
                tracing::error!(error = %e, "api: person directory error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Jurisdictions(e) => {
                tracing::error!(error = %e, "api: jurisdictions error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::EntityTypes(e) => {
                tracing::error!(error = %e, "api: entity types error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Db(e) => {
                tracing::error!(error = %e, "api: db error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            Self::Notation(e) => {
                tracing::error!(error = %e, "api: notation error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
            // The one caller-reachable ingest failure: the `kind` sent is not
            // an asset-lane classification. Every other `IngestError` variant
            // is a database, storage, or write-invariant fault, so only this
            // one is matched and the rest fall through to 500 below — a
            // catch-all 400 here would report a storage outage as the
            // caller's mistake.
            Self::Ingest(store::documents::IngestError::InvalidKind(kind)) => bad_request(
                "invalid_kind",
                &format!(
                    "`{kind}` is not a document kind. Accepted values are: {}.",
                    accepted_asset_kinds().join(", ")
                ),
            ),
            Self::Ingest(e) => {
                tracing::error!(error = %e, "api: document ingest error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
        }
    }
}

/// Every `kind` [`store::documents::ingest_bytes`] accepts, in the order
/// [`rules::kind::Kind::ALL`] declares them.
///
/// Derived from the same predicate the ingest boundary enforces
/// (`valid_for(Lane::Asset)`) rather than written out here, so the list a
/// rejected caller is shown cannot drift from the list actually accepted.
fn accepted_asset_kinds() -> Vec<&'static str> {
    rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
        .map(|k| k.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{api_operation_table, documented_api_operations, routes};
    use std::collections::BTreeSet;

    #[test]
    fn documented_operations_are_derived_from_the_route_table() {
        // The inventory the drift guard checks is exactly the table's
        // (method, path) projection — it cannot silently omit a
        // registered route, which is what closes the "new undocumented
        // path stays unprobed" gap: any row added to the table appears
        // here, and `openapi_drift.rs` then compares it to the document.
        let table: Vec<(&str, &str)> = api_operation_table()
            .into_iter()
            .map(|(method, path, _handler)| (method, path))
            .collect();
        assert_eq!(documented_api_operations(), table);
    }

    #[test]
    fn documented_operations_have_no_duplicates() {
        let ops = documented_api_operations();
        let unique: BTreeSet<_> = ops.iter().copied().collect();
        assert_eq!(
            unique.len(),
            ops.len(),
            "duplicate (method, path) in the API operation table: {ops:?}"
        );
    }

    #[test]
    fn routes_builds_without_panicking_folding_multi_method_paths() {
        // Exercises the path-folding/merge branch (two methods share
        // `/app/api/people` and three share `/app/api/people/{id}`); axum panics
        // if the same path is registered twice, so a green build here
        // proves the fold merges rather than re-registers.
        let _router = routes();
    }
}
