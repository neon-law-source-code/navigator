//! Admin routes — gated by [`auth::require_auth`].
//!
//! Each sub-page (dashboard, people, …) is a `Router` attached to
//! the same auth layer. New admin surfaces add another `.route(...)`
//! and inherit auth automatically.

use std::sync::Arc;
use uuid::Uuid;

use axum::extract::{DefaultBodyLimit, Extension, FromRef, Path, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};

use crate::session::{Impersonation, SessionData};
// Every Entity write — create, update, delete — belongs to
// `store::entity_commands`, including the firm-anchor rules and the advisory
// locks that make check-then-write atomic. This module only authorizes and
// renders, so the error type is all it needs from there.
use store::entity_commands::EntityCommandError;

/// The canonical seed's firm Entity, protected in every deployment. White-label
/// operators name their own firm Entity with `NAVIGATOR_BOOTSTRAP_COMPANY`,
/// which protects that row *in addition* to this one — `store::seed` re-creates
/// this row by exact name on every boot, so deleting or renaming it never
/// sticks and the surface must not offer the option.
pub const DEFAULT_BOOTSTRAP_COMPANY: &str = store::seed::FIRM_ENTITY_NAME;

/// Resolve the operator's firm entity once while the router is built. Blank
/// values fall back to the shipped legal entity, which is protected regardless.
#[must_use]
pub fn bootstrap_company_from_env() -> String {
    bootstrap_company_from_lookup(|key| std::env::var(key).ok())
}

fn bootstrap_company_from_lookup<F>(get: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    get("NAVIGATOR_BOOTSTRAP_COMPANY")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BOOTSTRAP_COMPANY.to_string())
}

/// `303 See Other` back to the list index after a row delete.
fn delete_response(redirect_to: &'static str) -> Response {
    Redirect::to(redirect_to).into_response()
}

/// Response for a delete that **failed** (most often a foreign-key block —
/// the row still has dependent records, or the lawyer-DRI lockout): a redirect
/// back to the listing carrying `message` as the `?error=` flash the form
/// handlers in this file already use.
///
/// The flash is what makes the refusal legible. Without it the row is still
/// there after the reload and nothing says why, which reads as a no-op rather
/// than as a refusal (navigator#995).
fn delete_refused_response(message: &str, redirect_to: &'static str) -> Response {
    Redirect::to(&format!(
        "{redirect_to}?error={}",
        encode_query_value(message)
    ))
    .into_response()
}

const PROJECT_PARTICIPATION_DELETE_ERROR: &str = "Couldn't remove that person from this matter.";
/// Refusal shown when removing a participation row would strand the matter's
/// accountable lawyer. The lawyer DRI reaches a matter through this row, so
/// dropping it while the column still names them would leave a matter whose
/// own lawyer cannot open it. Reassigning the DRI is the deliberate path.
const PROJECT_PARTICIPATION_DRI_LOCKOUT_ERROR: &str =
    "This person is the matter's lawyer DRI. Assign a different lawyer DRI before removing them.";
/// The one matter surface. Both former prefixes collapse here.
const APP_PROJECTS_PATH: &str = "/app/projects";

use serde::Deserialize;

use crate::auth::{require_auth, AuthConfig};
use crate::signature::SignatureProvider;

/// Per-router state for the admin sub-tree. Wraps the store handle plus
/// the durable-workflow + signature-provider seams that the
/// retainer-intake flow needs.
#[derive(Clone)]
pub struct AdminState {
    /// The store. `persons` lives here, so every people surface on the
    /// admin console reads and writes it.
    pub surreal: store::surreal::SurrealDb,
    pub workflow_runtime: Arc<dyn workflows::StateMachineRuntime>,
    pub signature_provider: Arc<dyn SignatureProvider>,
    /// Parsed questionnaire spec from the bundled retainer
    /// template. Drives the per-step walker at
    /// `/lawyer/notations/{id}/step`.
    pub retainer_intake_questionnaire: workflows::QuestionnaireSpec,
    /// Same `Arc` as `workflow_runtime` — kept as a separate field so
    /// the questionnaire walker reads from a name that matches the
    /// timeline it drives.
    pub questionnaire_runtime: Arc<dyn workflows::StateMachineRuntime>,
    /// Object storage seam — the retainer workflow's
    /// `generate_pdf__retainer_pdf` step writes the rendered PDF here.
    pub storage: Arc<dyn cloud::StorageService>,
    /// Public-assets storage — blank government forms are pulled from
    /// this lane at fill and download time and verified against their
    /// repo `.sha256` pins. Same `Arc` as `AppState.assets_storage`.
    pub assets_storage: Arc<dyn cloud::StorageService>,
    /// Vendored-forms registry the fill + download paths consult
    /// (`forms::registry()` in production; a test harness swaps in
    /// entries pinned to synthetic staged blanks). Same `Arc` as
    /// `AppState.forms_registry`.
    pub forms_registry: Arc<Vec<forms::FormMeta>>,
    /// Outbound email backend — same `Arc` as `AppState.email`,
    /// passed through so the admin "Send welcome" handler reaches
    /// the audited [`crate::email::LoggingEmail`] decorator.
    pub email: Arc<dyn crate::email::EmailService>,
    /// Accounting seam — read-only from `web`'s side. Invoices originate in
    /// Xero, where lawyers raise them directly; nothing here computes or
    /// raises money. Same `Arc` as `AppState.billing_provider`.
    pub billing_provider: Arc<dyn billing::BillingProvider>,
    /// Inbound-contract deviation reviewer (same `Arc` as
    /// `AppState.contract_reviewer`). The `analysis__contract_deviations`
    /// step runs this web-side to flag a contract against the client
    /// Entity's playbook. See [`crate::contract_review_walk`].
    pub contract_reviewer: Arc<dyn crate::contract_review::ContractReviewer>,
    /// Email of the protected Owner operator (see
    /// [`crate::oauth::AuthState::bootstrap_owner_email`]). The role
    /// editor uses this to lock the `role` field on that one row
    /// so the Owner cannot accidentally demote themselves from the UI.
    /// `None` disables the lock — every row's role becomes freely
    /// editable.
    pub bootstrap_owner_email: Option<String>,
    /// Legal name of the firm anchor Entity. The matching row is never
    /// deletable by application users, including admins.
    pub bootstrap_company: String,
    /// Session signer used when admin impersonation swaps the browser
    /// cookie into a client lens and when the banner exits back to admin.
    pub sessions: crate::SessionStore,
    /// Whether session cookies should carry `Secure`.
    pub secure_cookies: bool,
}

impl FromRef<AdminState> for store::surreal::SurrealDb {
    fn from_ref(s: &AdminState) -> Self {
        s.surreal.clone()
    }
}

impl FromRef<AdminState> for Arc<dyn crate::email::EmailService> {
    fn from_ref(s: &AdminState) -> Self {
        s.email.clone()
    }
}

impl FromRef<AdminState> for Arc<dyn cloud::StorageService> {
    fn from_ref(s: &AdminState) -> Self {
        s.storage.clone()
    }
}

/// Build the admin sub-router. The caller merges it into the main
/// router. Auth is applied here via `route_layer` so a missing or
/// invalid token fails before the handler runs. The `sessions`
/// store backs the CSRF middleware that gates every form-encoded
/// state-changing request.
#[allow(clippy::too_many_lines)]
pub fn routes(
    state: AdminState,
    auth: AuthConfig,
    sessions: crate::SessionStore,
    policy: crate::policy::PolicyClient,
) -> Router {
    // `/app/*` renders each caller's own lens. `/lawyer/*` is the firm lens.
    // A person may be eligible for both; the URL prefix decides which
    // project-visibility predicate applies.
    let mut r = Router::new();
    // Admin-only people administration: the one browser surface that creates or
    // edits a Person. Detail/edit use the singular `/admin/person`.
    // embedded Rego policy needs no `/admin` rule — the admin-bypass allows admin and
    // default-deny blocks lawyer; each handler re-checks the admin role.
    r = r
        // `/admin/people` (the list) and `/admin/people/new` (the create form)
        // now render through Dioxus (#641 Phase 3, `dioxus_app::admin_people_router`
        // and `csrf_page_router`); the create form posts to the native
        // `POST /admin/people` handler below, which axum merges with the Dioxus
        // list GET on the same path.
        .route("/admin/people", post(admin_people_create))
        // `/admin/person/{id}` (+ its `/edit` alias) — the show/edit render now
        // serves through Dioxus (#641 Phase 3,
        // `dioxus_app::admin_person_show_router`). Its native-form actions post
        // here: update (`POST /admin/person/{id}`), welcome-email send, delete,
        // and impersonate. axum merges the Dioxus GET and these POSTs on each path.
        .route("/admin/person/{id}", post(admin_person_update))
        .route("/admin/person/{id}/welcome", post(admin_person_welcome))
        .route("/admin/person/{id}/delete", post(admin_person_delete))
        .route("/admin/person/{id}/impersonate", post(people_impersonate));
    r = register_firm_routes(r, "/lawyer");
    // Firm brand fonts — the licensed GORP Serif desktop family, served as one
    // ZIP from the *private* documents bucket, so a direct object URL can never
    // bypass this gate. The bytes are uploaded out-of-band by
    // `navigator ops assets fonts upload-desktop`.
    //
    // Not under `/lawyer`: a brand asset is not lawyer work, and every firm
    // tier — Clerk included — may fetch it. Sitting under `/lawyer` made Clerk
    // an exact-path exception to "Clerk never enters /lawyer". Under
    // `/app/team` the page that offers the card and the object it links share
    // one prefix, so embedded Rego's existing `/app/team` rules admit exactly
    // the four firm tiers here and deny a client, with no rule of its own.
    // Registered here rather than inside `register_firm_routes` because that
    // helper's `{prefix}` is `/lawyer`.
    r = r.route(
        "/app/team/fonts/gorp-serif.zip",
        get(crate::brand_fonts::download_get),
    );
    r = register_project_routes(r);
    r = r.route(
        "/app/notations/{id}/documents/{doc_id}",
        get(crate::documents::download),
    );
    // Blank government forms — any authenticated person (embedded Rego policy's
    // `/app/forms` rule); the bytes are pulled from the public
    // assets bucket and verified against the repo's `.sha256` pins,
    // the same pull-and-verify path the workflows fill through.
    r = r.route("/app/forms/{file}", get(crate::gov_forms::download_get));
    let bearer_sessions = sessions.clone();
    r.with_state(state)
        .layer(middleware::from_fn_with_state(
            (sessions.clone(), crate::csrf::CsrfMode::Form),
            crate::csrf::require_csrf,
        ))
        // Policy check runs after bearer-token + session decode so
        // rego rules can read `input.session.role`. A deny short-
        // circuits with 403 — the CSRF layer below it never runs.
        .route_layer(middleware::from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_auth))
        // Outermost: resolve a `navigator` CLI bearer credential (the
        // same `SessionData` blob the cookie carries) into a
        // `SessionData` + `AuthClaims` extension, so the CLI drives every
        // `/app` handler over the same path the browser does. Sits
        // outside `require_auth` so the JWT layer short-circuits on the
        // injected `AuthClaims` instead of rejecting a session blob.
        .route_layer(middleware::from_fn_with_state(
            bearer_sessions,
            crate::auth::inject_bearer_session,
        ))
}

/// Owner/Admin-only gate shared by the `/admin/*` handlers. Returns a `403`
/// response when the caller is anonymous or outside that tier, or `None` when
/// the request may proceed. embedded Rego policy already denies lawyer on `/admin/*`; this
/// is the defense-in-depth handler check the access model calls for.
fn admin_gate(session: Option<&SessionData>) -> Option<Response> {
    match session {
        None => Some(
            (
                StatusCode::FORBIDDEN,
                webapp::error_pages::forbidden(webapp::error_pages::Viewer::Anonymous),
            )
                .into_response(),
        ),
        Some(s) if !s.role.is_admin_tier() => Some(
            (
                StatusCode::FORBIDDEN,
                webapp::error_pages::forbidden(webapp::error_pages::Viewer::SignedIn),
            )
                .into_response(),
        ),
        Some(_) => None,
    }
}

/// Register the firm-wide CRUD routes under `{prefix}/...`. Today
/// this is called once with `/lawyer`; the helper survives as
/// a single point of edit for the firm CRUD surface.
#[allow(clippy::too_many_lines)]
fn register_firm_routes(r: Router<AdminState>, prefix: &str) -> Router<AdminState> {
    // `GET /lawyer` (the workbench) renders through Dioxus (#956 Phase 4,
    // `dioxus_app::lawyer_dashboard_router`), so this chain now starts at the
    // firm brand fonts.
    r
        // `GET /lawyer/retainers/new` (the form) renders through Dioxus (#956
        // Phase 4, `dioxus_app::csrf_page_router`); the create posts here, and
        // axum merges the two same-path method routes.
        .route(
            &format!("{prefix}/retainers/new"),
            post(crate::retainer_walk::start_post),
        )
        // The step page renders through Dioxus (#956 Phase 4,
        // `dioxus_app::walker_step_router`, which also answers the
        // `?format=json` CLI surface on this path); the answer posts here, and
        // axum merges the two same-path method routes.
        .route(
            &format!("{prefix}/notations/{{id}}/step"),
            post(crate::retainer_walk::step_post),
        )
        // Batch transcript coverage — the walk's transcript input mode. Runs
        // `live_inquiry` over the notation's template and persists each covered
        // inquiry as a proposed `source = extracted` answer the walk confirms.
        .route(
            &format!("{prefix}/notations/{{id}}/transcript"),
            post(crate::retainer_walk::transcript_post),
        )
        // Open a notation for an existing matter from a template authored in
        // the Project's git repo — the project-scoped `notation create` front
        // door (auto-saves the template version, then opens the notation).
        // Hand the matter's client their self-serve intake link.
        .route(
            &format!("{prefix}/notations/{{id}}/send-intake"),
            post(crate::retainer_walk::send_intake_post),
        )
        // Attorney approves a notation parked at lawyer_review (it carries
        // custom content): fires `approved` so the worker renders + persists
        // the reviewed bytes, then parks at `generate_pdf__retainer_pdf`.
        .route(
            &format!("{prefix}/notations/{{id}}/approve-send"),
            post(crate::retainer_walk::approve_send_post),
        )
        // The deliberate send half: confirms the worker's PDF landed, then
        // dispatches exactly one envelope. 409 + JSON reason when not ready.
        .route(
            &format!("{prefix}/notations/{{id}}/send"),
            post(crate::retainer_walk::send_post),
        )
        // The review/approve screen for a notation parked at lawyer_review —
        // where the matter-open form lands lawyer after opening a matter with a
        // retainer — renders through Dioxus (#956 Phase 4,
        // `dioxus_app::intake_review_router`, which also answers the
        // `?format=json` CLI status surface on this path).
        // Send a reviewed notation back for changes: record the flagged
        // answers + note and route lawyer_review → reask__client, then
        // re-collect the flagged answers and resubmit for review. A rejected
        // review re-collects the wrong answers instead of dead-ending.
        .route(
            &format!("{prefix}/notations/{{id}}/request-changes"),
            post(crate::retainer_walk::request_changes_post),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/reask"),
            post(crate::retainer_walk::reask_post),
        )
        // Northstar: the attorney releases the generated estate drafts to
        // the client — advances lawyer_review → client_review and flips each
        // draft to pending_review (visible on the Phase A review surface).
        .route(
            &format!("{prefix}/notations/{{id}}/release-drafts"),
            post(crate::estate::release_drafts_post),
        )
        // Per-notation custom clauses spliced into the assembled document.
        // The editor itself renders through Dioxus (#956 Phase 4,
        // `dioxus_app::clause_editor_router`, which also answers the
        // `?format=json` CLI surface on this path); the add posts here, and
        // axum merges the two same-path method routes.
        .route(
            &format!("{prefix}/notations/{{id}}/clauses"),
            post(crate::clauses::clause_add),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/clauses/{{cid}}/edit"),
            post(crate::clauses::clause_edit),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/clauses/{{cid}}/delete"),
            post(crate::clauses::clause_delete),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/clauses/{{cid}}/move"),
            post(crate::clauses::clause_move),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/sign"),
            get(crate::esign_view::sign_get),
        )
        .route(
            &format!("{prefix}/notations/{{id}}/documents/{{doc_id}}"),
            get(crate::documents::download),
        )
        // Admin-only governed expunge of a filed document — drives the
        // history-rewrite + storage-delete + audit primitive, then redirects
        // back to the render. The handler 404s any non-admin session. The
        // confirmation + result render through Dioxus (#956 Phase 4,
        // `dioxus_app::LAWYER_DOCUMENT_EXPUNGE_PATH`); axum merges the methods.
        .route(
            &format!("{prefix}/documents/{{doc_id}}/expunge"),
            post(crate::expunge_route::run),
        )
        // Client document-deletion requests: a lawyer/admin queue, with
        // admin-only authorize (runs the expunge) + lawyer/admin deny. The read
        // queue at `{prefix}/expunge-requests` now renders through Dioxus (#641
        // Phase 3, `dioxus_app::expunge_queue_router`); its rows post to these
        // mutation handlers via native forms, so the POST routes stay.
        .route(
            &format!("{prefix}/expunge-requests/{{id}}/authorize"),
            post(crate::expunge_request_route::admin_authorize),
        )
        .route(
            &format!("{prefix}/expunge-requests/{{id}}/deny"),
            post(crate::expunge_request_route::admin_deny),
        )
        // Every Person command — create, update, delete, welcome-send — lives on
        // the REST boundary at `/app/api/people*` (lawyer tier, `LawyerSession`),
        // and the browser form for one is the admin console's (`/admin/people*`,
        // Owner/Admin). The firm prefix keeps only this directory export: a
        // lawyer-tier read with no admin sibling.
        .route(&format!("{prefix}/people.csv"), get(people_csv))
        .route("/app/impersonation/stop", post(stop_impersonation))
        // The entities list (GET) now renders through Dioxus (#641 Phase 3,
        // `dioxus_app::entity_list_router`); `POST` (create) stays here, and axum
        // merges the two same-path method routes.
        .route(&format!("{prefix}/entities"), post(entities_create))
        .route(&format!("{prefix}/entities.csv"), get(entities_csv))
        // `/lawyer/entities/new` (the create form) now renders through Dioxus
        // (#641 Phase 3, `dioxus_app::csrf_page_router`); it posts to the
        // `POST /lawyer/entities` create handler below, which is unchanged.
        // The edit form (`{prefix}/entities/{id}/edit`) now renders through
        // Dioxus (#641 Phase 3, `dioxus_app::csrf_page_router`); it posts to this
        // update handler. The handler is POST-only: on success it persists the
        // edit and redirects to `/lawyer/entities`. A validation or conflict
        // outcome re-renders the edit form with the submitted values and an
        // inline error (409 on a name conflict, 200 on a blank name) — the same
        // shape the create door holds; an unknown id is a 404 page and a
        // server-side fault a 500 page.
        .route(&format!("{prefix}/entities/{{id}}"), post(entities_update))
        .route(
            &format!("{prefix}/entities/{{id}}/delete"),
            post(entities_delete),
        )
        // Inbound-contract-review playbooks: a Company's negotiating
        // positions, the yardstick the deviation analysis measures a
        // third-party contract against. The three `GET` renders now go through
        // Dioxus (#956 Phase 4, `dioxus_app::LAWYER_PLAYBOOKS_PATH` and its two
        // form paths); only the writes stay here, and axum merges each onto the
        // path its Dioxus `GET` already holds.
        .route(
            &format!("{prefix}/playbooks"),
            post(crate::admin_playbooks::create),
        )
        .route(
            &format!("{prefix}/playbooks/{{id}}"),
            post(crate::admin_playbooks::update),
        )
        // Attorney review screen for an inbound contract review: act on
        // each finding, edit the risk summary, then approve (assemble +
        // deliver the memo) or reject. Row-scoped to the matter in the
        // handlers. The screen itself renders through Dioxus (#956 Phase 4,
        // `dioxus_app::LAWYER_CONTRACT_REVIEW_PATH`); these mutations sit on
        // deeper paths, so axum routes them independently.
        .route(
            &format!("{prefix}/contract-reviews/{{id}}/findings/{{idx}}"),
            post(crate::admin_contract_reviews::save_finding),
        )
        .route(
            &format!("{prefix}/contract-reviews/{{id}}/summary"),
            post(crate::admin_contract_reviews::save_summary),
        )
        .route(
            &format!("{prefix}/contract-reviews/{{id}}/approve"),
            post(crate::admin_contract_reviews::approve),
        )
        .route(
            &format!("{prefix}/contract-reviews/{{id}}/reject"),
            post(crate::admin_contract_reviews::reject),
        )
        // Read-only listings — these tables are seeded by the
        // workspace (`cli import`, `store/seeds/`) rather than
        // authored from the web UI.
        // `/lawyer/entity-types` now renders through Dioxus (#641 Phase 3,
        // `dioxus_app::entity_types_router`), so the route is retired.
        // Cron schedules are the single manual-operation surface for every
        // deployed CronJob. The reference page renders through Dioxus (#956
        // Phase 4, `dioxus_app::csrf_page_router`); only the manual-run `POST`
        // stays here, and it redirects back to the page with a `?notice=`.
        .route(
            &format!("{prefix}/schedules/{{job}}/run"),
            post(crate::cron_schedules::run),
        )
    // Every read-only lawyer admin surface now renders through Dioxus (#641
    // Phase 3, `dioxus_app::admin_listing_router`): the single-entity
    // `render_listing` pages, the join-backed `mailrooms` and `letters` lists,
    // the paginated `email-log`, and the `letters/{id}` detail page (a single
    // record keyed by its path param). Their routes are all retired.
}

/// Register the matter surface under [`APP_PROJECTS_PATH`].
///
/// One registration, one path per resource. The prefix used to name the
/// viewer — `/app/projects` mounted the firm handlers and `/app/projects`
/// the client ones, with four paths dispatching to the same handler from both.
/// Which lens a caller gets is now decided inside each handler from their tier
/// and their `person_project_roles` row (`store::access::can_see_project`),
/// which is the only place that decision was ever safe to make: a URL prefix is
/// chosen by the requester.
///
/// Row scoping is uniform — every tier, Owner and Admin included, needs a
/// participation row on the matter. The lawyer-only writes below additionally
/// require the lawyer tier in their own handlers, which is what replaces the
/// outer `/lawyer/*` policy rule those paths used to sit behind.
fn register_project_routes(r: Router<AdminState>) -> Router<AdminState> {
    let prefix = APP_PROJECTS_PATH;
    // `{prefix}` (the list), `{prefix}/{code}` (the matter workbench), the forms,
    // and the read pages all render through Dioxus (`dioxus_app`); the `POST`s
    // stay here and axum merges the same-path methods.
    r.route(prefix, post(projects_create_lawyer_only))
        .route(&format!("{prefix}.csv"), get(projects_csv))
        // Inline record creation from the Add-project form: create a new
        // entity or client without leaving the page. Static path segments,
        // so they win over the `{{id}}` capture below.
        .route(
            &format!("{prefix}/new/entity"),
            post(projects_new_entity_inline),
        )
        .route(
            &format!("{prefix}/new/client"),
            post(projects_new_client_inline),
        )
        .route(
            &format!("{prefix}/{{project_code}}"),
            post(projects_update_lawyer_only),
        )
        .route(
            &format!("{prefix}/{{project_code}}/people"),
            post(project_participation_create),
        )
        .route(
            &format!("{prefix}/{{project_code}}/people/{{role_id}}/edit"),
            post(project_participation_update),
        )
        .route(
            &format!("{prefix}/{{project_code}}/people/{{role_id}}/delete"),
            post(project_participation_delete),
        )
        .route(
            &format!("{prefix}/{{project_code}}/people/{{role_id}}/dri"),
            post(matter_dri_designate),
        )
        .route(
            &format!("{prefix}/{{project_code}}/people/{{role_id}}/dri/remove"),
            post(matter_dri_remove),
        )
        .route(
            &format!("{prefix}/{{project_code}}/delete"),
            post(projects_delete_lawyer_only),
        )
        .route(
            &format!("{prefix}/{{project_code}}/documents/upload"),
            // Axum's own default body limit (~2 MB) sits in front of this
            // handler's own `MAX_BATCH_BYTES` check — without raising it
            // here, a large scanned-PDF batch never reaches the handler at
            // all, it 413s at the framework layer.
            post(crate::project_documents::upload).layer(DefaultBodyLimit::max(
                crate::project_documents::MAX_BATCH_BYTES,
            )),
        )
        // Northstar: file a sitting's transcript into an estate matter
        // (text / file / link) — threads the reusable document-intake
        // step through the workflow's `transcript_uploaded` signal.
        .route(
            &format!("{prefix}/{{project_code}}/notations/{{nid}}/transcript"),
            post(crate::transcript_intake::upload),
        )
        // Open a notation for an existing matter from a template authored in
        // the Project's git repo — the project-scoped `notation create` front
        // door (auto-saves the template version, then opens the notation).
        // Matter-scoped, so it belongs here rather than on the firm prefix.
        .route(
            &format!("{prefix}/{{project_code}}/notations/new"),
            post(crate::project_notation::project_notation_new_post),
        )
        // Close the matter — opens the closing-letter walk.
        .route(
            &format!("{prefix}/{{project_code}}/close"),
            post(crate::retainer_walk::close_matter_post),
        )
        // Inbound contract review: a third-party contract uploaded for
        // playbook review, by the client or by the firm acting for them.
        // One of the four paths that already dispatched here from both
        // prefixes, so the merge is deletion.
        .route(
            &format!("{prefix}/{{project_code}}/contract-review"),
            post(crate::contract_review_walk::upload),
        )
        // `GET {prefix}/{id}/documents/{doc_id}` (the provenance page) renders
        // through Dioxus (`dioxus_app::project_document_router`); the
        // signed-URL download stays here. The client lens still never resolves
        // an `internal` asset — the tier selects that now, not the prefix.
        .route(
            &format!("{prefix}/{{project_code}}/documents/{{doc_id}}/download"),
            get(crate::project_documents::download),
        )
        // "Download all matter documents" — a ZIP of the matter's current
        // files (repo HEAD). The firm gets every asset; a client gets the
        // `visibility`-filtered set.
        .route(
            &format!("{prefix}/{{project_code}}/documents.zip"),
            get(crate::project_export::download_all),
        )
        // Client-initiated "Delete this document" — records a pending
        // request a lawyer/admin later authorizes. Request-only.
        .route(
            &format!("{prefix}/{{project_code}}/documents/{{doc_id}}/request-deletion"),
            post(crate::expunge_request_route::client_request),
        )
        // Northstar: the client approves their estate plan, firing
        // `client_approved` (client_review -> sent_for_signature__pending)
        // and flipping every released draft to `approved`.
        .route(
            &format!("{prefix}/{{project_code}}/approve-plan"),
            post(crate::estate::approve_plan_post),
        )
        // Comment-only client review surface (Northstar Phase A). The page
        // renders through Dioxus (`dioxus_app::review_router`); the comment
        // `GET`/`POST` (the custom element's data API) stay here.
        .route(
            &format!("{prefix}/{{project_code}}/review/{{doc_id}}/comments"),
            get(crate::review::list_comments).post(crate::review::create_comment),
        )
        // The matter's single privileged conversation log — document
        // comments, email (both directions), and portal messages interleaved
        // in time. The thread renders through Dioxus
        // (`dioxus_app::conversation_router`); the message `POST` stays here
        // and redirects back to the page (PRG). Only the firm lens may post
        // an internal note, which the handler decides from the tier.
        .route(
            &format!("{prefix}/{{project_code}}/conversation/messages"),
            post(crate::conversation::post_message),
        )
        // Client self-serve intake (the magic link): the demand-side
        // mirror of the admin walker. Row-scoped to the matter; the
        // client answers the client-facing questions, source `client`.
        // The `GET` renders through Dioxus
        // (`dioxus_app::client_intake_router`); the save posts here.
        .route(
            &format!("{prefix}/{{project_code}}/intake/{{notation_id}}"),
            post(crate::intake::intake_save),
        )
}

/// Returns `true` when the caller can act on the lawyer workbench.
/// Missing sessions fail closed; tests that need lawyer behavior should
/// pass an explicit lawyer/admin session just like production does.
pub(crate) fn is_lawyer_tier(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_lawyer_tier())
}

fn can_change_roles(session: Option<&SessionData>) -> bool {
    session.is_none_or(|s| s.role.is_admin_tier() && s.impersonation.is_none())
}

fn can_assign_role(session: Option<&SessionData>, requested: store::persons::Role) -> bool {
    session.is_none_or(|s| {
        s.impersonation.is_none()
            && s.role.is_admin_tier()
            && requested.authority_rank() <= s.role.authority_rank()
    })
}

/// Project participation controls the next caller's project scope, so it is
/// privileged membership administration rather than an ordinary lawyer write.
fn can_manage_project_participation(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_admin_tier() && s.impersonation.is_none())
}

/// Returns `true` for Owner and Admin. These tiers see internal DB-error
/// detail on a failed matter write; other lawyers get a generic message.
fn is_admin(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_admin_tier())
}

/// Pick the failure message for a matter write. Admins see the diagnostic
/// `detail` (so they can act without opening the logs); everyone else gets
/// `generic`. The detail is always logged server-side regardless, so it is
/// never lost — only withheld from non-admins in the UI.
fn admin_gated_message(is_admin: bool, detail: &str, generic: &str) -> String {
    if is_admin {
        detail.to_string()
    } else {
        generic.to_string()
    }
}

fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
}

/// `POST /admin/person/{id}/delete` — the native-form person delete behind the
/// Dioxus admin people list (#641 Phase 3). The row used an `hx-delete` to
/// the REST `/app/api/people/{id}`; this wraps the same delete command and redirects
/// (303) back to the list, or back with an `?error=` flash when the command
/// blocks the delete (the bootstrap Owner, or a non-client record).
async fn admin_person_delete(
    State(s): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Path(id): Path<Uuid>,
) -> Response {
    if let Some(forbidden) = admin_gate(session.as_deref()) {
        return forbidden;
    }
    match crate::people_commands::delete_person(&s.surreal, id, s.bootstrap_owner_email.as_deref())
        .await
    {
        Ok(_) => Redirect::to("/admin/people").into_response(),
        Err(e) => Redirect::to(&format!(
            "/admin/people?error={}",
            encode_query_value(&e.user_message())
        ))
        .into_response(),
    }
}

/// `GET /admin/people/new` — the admin console create form (Cancel returns
/// to `/admin/people`). Admin-gated.
/// `POST /admin/people` — the native-form create behind the Dioxus admin
/// add-person page (#641 Phase 3). Admin-gated; wraps the person create command
/// and redirects (303) to the list on success, or back to the form with an
/// `?error=` flash on failure. The form posted to the REST `/app/api/people`
/// over HTMX; this is the plain-form equivalent (the create form's GET now
/// renders through Dioxus).
async fn admin_people_create(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
    Form(command): Form<crate::people_commands::CreatePersonCommand>,
) -> Response {
    if let Some(forbidden) = admin_gate(session.as_deref()) {
        return forbidden;
    }
    if crate::people_commands::parse_role(&command.role)
        .is_some_and(|role| !can_assign_role(session.as_deref(), role))
    {
        return Redirect::to(
            "/admin/people/new?error=You%20cannot%20assign%20a%20system%20role%20above%20your%20own.",
        )
        .into_response();
    }
    match crate::people_commands::create_person(&surreal, &command).await {
        Ok(_) => Redirect::to("/admin/people").into_response(),
        Err(e) => Redirect::to(&format!(
            "/admin/people/new?error={}",
            encode_query_value(&e.user_message())
        ))
        .into_response(),
    }
}

/// Percent-encode a query-parameter value: the query-structural characters, and
/// everything outside printable ASCII. A refusal message travels back to its
/// form in a `Location` header, and a header value admits neither a control
/// character nor a non-ASCII byte — so an un-encoded newline (a conflict
/// block's joined findings) or em dash would turn the refusal into a `500`.
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'?' => out.push_str("%3F"),
            b'&' => out.push_str("%26"),
            b'#' => out.push_str("%23"),
            b'%' => out.push_str("%25"),
            b'+' => out.push_str("%2B"),
            b' ' => out.push_str("%20"),
            // Printable ASCII that RFC 3986 excludes from a query. Browsers
            // tolerate these, but a strict URI parser rejects the whole
            // `Location` — so a playbook's `|`-delimited positions text riding
            // a refusal would turn that refusal into a broken redirect.
            b'"' | b'<' | b'>' | b'[' | b'\\' | b']' | b'^' | b'`' | b'{' | b'|' | b'}' => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0f)] as char);
            }
            // Everything outside printable ASCII: control characters (a
            // conflict block's newline-joined findings) and every byte of a
            // non-ASCII character (an em dash in a refusal message). Both are
            // illegal in a `Location` header, so passing them through would
            // turn a refusal into a `500`.
            _ if byte <= 0x20 || byte >= 0x7f => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0f)] as char);
            }
            _ => out.push(byte as char),
        }
    }
    out
}

/// `POST /admin/person/{id}` — the native-form person update behind the Dioxus
/// admin show/edit page. Wraps the person update command; redirects back to the
/// show view on success (or with an `?error=` flash on a rejected write). The
/// form `PATCH`ed the REST `/app/api/people/{id}` over HTMX; this is the same
/// command with a redirect result so a plain form (no JavaScript) works.
async fn admin_person_update(
    State(s): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Path(id): Path<Uuid>,
    Form(command): Form<crate::people_commands::UpdatePersonCommand>,
) -> Response {
    if let Some(forbidden) = admin_gate(session.as_deref()) {
        return forbidden;
    }
    let ctx = crate::people_commands::UpdateContext {
        bootstrap_owner_email: s.bootstrap_owner_email.as_deref(),
        actor_role: session
            .as_deref()
            .map_or(store::persons::Role::Client, |s| s.role),
        may_change_roles: can_change_roles(session.as_deref()),
    };
    match crate::people_commands::update_person(&s.surreal, id, &command, &ctx).await {
        Ok(_) => Redirect::to(&format!("/admin/person/{id}")).into_response(),
        Err(e) => Redirect::to(&format!(
            "/admin/person/{id}?error={}",
            encode_query_value(&e.user_message())
        ))
        .into_response(),
    }
}

/// `POST /admin/person/{id}/welcome` — the native-form welcome-email send behind
/// the Dioxus admin show/edit page. Both outcomes redirect back to the show view
/// with a `?notice=` flag that floats a toned flash toast; a send failure is
/// surfaced there, not as a 5xx. The button posted to the REST
/// `/app/api/people/{id}/welcome` over HTMX.
async fn admin_person_welcome(
    State(s): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Path(id): Path<Uuid>,
) -> Response {
    if let Some(forbidden) = admin_gate(session.as_deref()) {
        return forbidden;
    }
    let base_url = workflows::email::base_url_from_env();
    let notice =
        match crate::people_commands::send_welcome(&s.surreal, s.email.as_ref(), &base_url, id)
            .await
        {
            Ok(_) => "welcome_sent",
            Err(_) => "welcome_failed",
        };
    Redirect::to(&format!("/admin/person/{id}?notice={notice}")).into_response()
}

async fn people_impersonate(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    cookies: tower_cookies::Cookies,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(Extension(session)) = session else {
        return (
            StatusCode::FORBIDDEN,
            webapp::error_pages::forbidden(webapp::error_pages::Viewer::Anonymous),
        )
            .into_response();
    };
    if !session.role.is_admin_tier() || session.impersonation.is_some() {
        return (
            StatusCode::FORBIDDEN,
            webapp::error_pages::forbidden(webapp::error_pages::Viewer::SignedIn),
        )
            .into_response();
    }
    let target = match store::persons::find_by_id(&state.surreal, id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, person_id = %id, "admin: load person for impersonation failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response();
        }
    };
    if target.role != store::persons::Role::Client {
        tracing::warn!(
            person_id = %id,
            role = target.role.as_str(),
            "admin: blocked impersonation of non-client person",
        );
        return (
            StatusCode::CONFLICT,
            "Only client users can be impersonated.",
        )
            .into_response();
    }

    let mut effective = SessionData::fresh(
        target
            .oidc_subject
            .clone()
            .unwrap_or_else(|| format!("person:{}", target.id)),
        store::persons::Role::Client,
    );
    effective.email = Some(target.email.clone());
    effective.person_id = Some(target.id);
    effective.source = session.source;
    effective.impersonation = Some(Impersonation {
        actor_sub: session.sub,
        actor_email: session.email,
        actor_person_id: session.person_id,
        target_name: target.name,
        target_email: target.email,
    });
    cookies.add(crate::oauth::session_cookie(
        state.sessions.encode(&effective),
        state.secure_cookies,
    ));
    // The actor now holds the impersonated client's session, so land them on
    // the client matter view they are standing in.
    Redirect::to("/app/projects").into_response()
}

async fn stop_impersonation(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    cookies: tower_cookies::Cookies,
) -> Response {
    let Some(Extension(session)) = session else {
        return (
            StatusCode::FORBIDDEN,
            webapp::error_pages::forbidden(webapp::error_pages::Viewer::Anonymous),
        )
            .into_response();
    };
    let Some(impersonation) = session.impersonation else {
        // Nothing to stop: bounce the firm person back to their own home.
        return Redirect::to("/app/team").into_response();
    };
    let (person_id, email, role) = match impersonation.actor_person_id {
        Some(id) => match store::persons::find_by_id(&state.surreal, id).await {
            Ok(Some(actor)) => (Some(actor.id), Some(actor.email), actor.role),
            Ok(None) => (
                None,
                impersonation.actor_email,
                store::persons::Role::Client,
            ),
            Err(e) => {
                tracing::error!(error = %e, person_id = %id, "admin: load impersonation actor failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    webapp::error_pages::server_error(),
                )
                    .into_response();
            }
        },
        None => (
            None,
            impersonation.actor_email,
            store::persons::Role::Client,
        ),
    };
    let mut restored = SessionData::fresh(impersonation.actor_sub, role);
    restored.email = email;
    restored.person_id = person_id;
    restored.source = session.source;
    cookies.add(crate::oauth::session_cookie(
        state.sessions.encode(&restored),
        state.secure_cookies,
    ));
    Redirect::to("/admin").into_response()
}

// ---- Entities ----

#[derive(Deserialize)]
struct EntityInput {
    name: String,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
}

async fn entities_create(State(s): State<AdminState>, Form(input): Form<EntityInput>) -> Response {
    let command = store::entity_commands::CreateEntityCommand {
        name: input.name,
        entity_type_id: input.entity_type_id,
        jurisdiction_id: input.jurisdiction_id,
    };
    match store::entity_commands::create_entity(&s.surreal, &s.bootstrap_company, &command).await {
        Ok(_) => Redirect::to("/lawyer/entities").into_response(),
        Err(e) => {
            if let EntityCommandError::Entities(ref err) = e {
                tracing::warn!(error = %err, "admin: create entity failed");
            }
            // Post/redirect/get: every refusal — a blank name, a conflict, a
            // server-side fault — sends the lawyer back to the form with
            // the message in the query, so a reload never resubmits the create.
            // The form this replaced was re-rendered inline, which let the
            // status code split (409 on a conflict, 200 otherwise); a redirect
            // carries one status. `/app/api/entities` keeps the per-outcome codes
            // for programmatic callers.
            // Redirect to the route that serves the form, so the two cannot
            // drift apart.
            back_to_entity_form(
                crate::dioxus_app::LAWYER_ENTITY_NEW_PATH,
                &e.user_message(),
                None,
            )
        }
    }
}

/// Redirect back to an entity form with `message` as the `?error=` flash, plus
/// the submitted values when `values` is set (the edit door, which must show the
/// rejected edit rather than silently reloading the stored row).
fn back_to_entity_form(
    path: &str,
    message: &str,
    values: Option<&store::entity_commands::UpdateEntityCommand>,
) -> Response {
    let mut query = String::new();
    push_query(&mut query, "error", message);
    if let Some(values) = values {
        push_query(&mut query, "name", &values.name);
        push_query(
            &mut query,
            "entity_type_id",
            &values.entity_type_id.to_string(),
        );
        push_query(
            &mut query,
            "jurisdiction_id",
            &values.jurisdiction_id.to_string(),
        );
    }
    if query.is_empty() {
        Redirect::to(path).into_response()
    } else {
        Redirect::to(&format!("{path}?{query}")).into_response()
    }
}

async fn entities_update(
    State(s): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(input): Form<EntityInput>,
) -> Response {
    let command = store::entity_commands::UpdateEntityCommand {
        name: input.name,
        entity_type_id: input.entity_type_id,
        jurisdiction_id: input.jurisdiction_id,
    };
    match store::entity_commands::update_entity(&s.surreal, id, &s.bootstrap_company, &command)
        .await
    {
        Ok(_) => Redirect::to("/lawyer/entities").into_response(),
        Err(EntityCommandError::NotFound) => {
            (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
        }
        Err(EntityCommandError::Entities(e)) => {
            tracing::error!(error = %e, entity_id = %id, "admin: update entity failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
        Err(e) => {
            // Post/redirect/get, as the create door does: a validation or
            // conflict outcome is the caller's to correct, so redirect back to
            // the edit form carrying the message and the rejected values rather
            // than re-rendering inline. The form this replaced could split
            // the status code (409 on a conflict, 200 on a blank name); a
            // redirect carries one. `/app/api/entities` keeps the per-outcome codes.
            back_to_entity_form(
                &format!("/lawyer/entities/{id}/edit"),
                &e.user_message(),
                Some(&command),
            )
        }
    }
}

async fn entities_delete(State(s): State<AdminState>, Path(id): Path<Uuid>) -> Response {
    match store::entity_commands::delete_entity(&s.surreal, id, &s.bootstrap_company).await {
        // `NotFound` renders as success on purpose: the row is already gone,
        // which is what the caller wanted, so a double-clicked delete button
        // or a second tab must not report an error. The API renders that same
        // outcome as a 404 — only this browser door treats it as done.
        Ok(_) | Err(EntityCommandError::NotFound) => delete_response("/lawyer/entities"),
        Err(EntityCommandError::FirmAnchorProtected) => (
            StatusCode::CONFLICT,
            store::entity_commands::FIRM_ANCHOR_PROTECTED_MESSAGE,
        )
            .into_response(),
        Err(EntityCommandError::Entities(e)) => {
            tracing::error!(error = %e, entity_id = %id, "entities_delete: failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!(entity_id = %id, "entities_delete: refused");
            delete_refused_response(&e.user_message(), "/lawyer/entities")
        }
    }
}

// ---- Projects ----

/// The matter-open form's path — the page every refusal on this cluster
/// redirects back to (post/redirect/get), carrying its message and the
/// submitted values in the query so nothing is retyped.
const PROJECT_NEW_PATH: &str = "/app/projects/new";

/// Append `key=value` to a redirect query being built. A blank value is skipped
/// so the URL carries only what was actually submitted.
pub(crate) fn push_query(query: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    if !query.is_empty() {
        query.push('&');
    }
    query.push_str(key);
    query.push('=');
    query.push_str(&encode_query_value(value));
}

/// Redirect back to the matter-open form with `query` (already encoded).
fn back_to_project_form(query: &str) -> Response {
    if query.is_empty() {
        Redirect::to(PROJECT_NEW_PATH).into_response()
    } else {
        Redirect::to(&format!("{PROJECT_NEW_PATH}?{query}")).into_response()
    }
}

#[derive(Deserialize)]
struct EntityInlineInput {
    #[serde(default)]
    entity_name: String,
    #[serde(default)]
    entity_type_id: String,
    #[serde(default)]
    jurisdiction_id: String,
}

/// `POST /app/projects/new/entity` — the inline "New entity" form on the
/// matter-open page. Creates the entity through the same `create_entity` command
/// as the standalone form, then redirects back to the form with `?entity=<id>`
/// so the Entity picker re-renders with it selected. A validation or conflict
/// error redirects back with `?entity_error=` and the submitted values echoed,
/// which re-opens the disclosure over what was typed. Nothing about the matter
/// form itself is touched either way.
async fn projects_new_entity_inline(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<EntityInlineInput>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found_response();
    }
    let name = input.entity_name.trim();
    let entity_type_id = Uuid::parse_str(input.entity_type_id.trim()).ok();
    let jurisdiction_id = Uuid::parse_str(input.jurisdiction_id.trim()).ok();

    let refuse = |message: &str| {
        let mut query = String::new();
        push_query(&mut query, "entity_error", message);
        push_query(&mut query, "entity_name", name);
        push_query(&mut query, "entity_type_id", input.entity_type_id.trim());
        push_query(&mut query, "jurisdiction_id", input.jurisdiction_id.trim());
        back_to_project_form(&query)
    };

    if name.is_empty() {
        return refuse("Name is required.");
    }
    let (Some(type_id), Some(jur_id)) = (entity_type_id, jurisdiction_id) else {
        return refuse("Pick an entity type and a jurisdiction.");
    };

    let command = store::entity_commands::CreateEntityCommand {
        name: name.to_string(),
        entity_type_id: type_id,
        jurisdiction_id: jur_id,
    };
    match store::entity_commands::create_entity(&state.surreal, &state.bootstrap_company, &command)
        .await
    {
        Ok(created) => {
            let mut query = String::new();
            push_query(&mut query, "entity", &created.id.to_string());
            back_to_project_form(&query)
        }
        Err(e) => {
            if let EntityCommandError::Entities(ref err) = e {
                tracing::warn!(error = %err, "admin: inline entity create failed");
            }
            refuse(&e.user_message())
        }
    }
}

#[derive(Deserialize)]
struct ClientInlineInput {
    #[serde(default)]
    client_name: String,
    #[serde(default)]
    client_email: String,
}

/// `POST /app/projects/new/client` — the inline "New client" form on the
/// matter-open page. Reuses [`crate::people_commands::create_person`] (the
/// People command boundary) with the role pinned to `client`, so the matter's
/// client-side DRI is a real client of record. On success it redirects back with
/// `?client=<id>` so the DRI picker re-renders with the new client selected; on
/// failure with `?client_error=` and the submitted values echoed.
async fn projects_new_client_inline(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<ClientInlineInput>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found_response();
    }
    let command = crate::people_commands::CreatePersonCommand {
        name: input.client_name.clone(),
        email: input.client_email.clone(),
        role: store::persons::Role::Client.as_str().to_string(),
        given_name: None,
        family_name: None,
        middle_name: None,
    };
    match crate::people_commands::create_person(&state.surreal, &command).await {
        Ok(created) => {
            let mut query = String::new();
            push_query(&mut query, "client", &created.id.to_string());
            back_to_project_form(&query)
        }
        Err(e) => {
            let mut query = String::new();
            push_query(&mut query, "client_error", &e.user_message());
            push_query(&mut query, "client_name", input.client_name.trim());
            push_query(&mut query, "client_email", input.client_email.trim());
            back_to_project_form(&query)
        }
    }
}

/// Deserialize an optional form field whose `<select>` posts an **empty
/// string** when it sits on the blank option — the shape every unselected
/// picker on the create form arrives in.
///
/// `Option<Uuid>` alone does not coerce `""` to `None`: `serde_urlencoded`
/// hands `""` to the `Uuid` parser, which fails, and Axum's `Form`
/// extractor then rejects the whole body with a bare 422 — before the
/// handler's own validation, which is the thing that would have told lawyer
/// what to pick, can run at all. Blank (or whitespace) means "not chosen"
/// and becomes `None`; a non-blank value must still parse, so a malformed
/// id is still an error rather than a silent `None`.
fn empty_string_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) => value.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

#[derive(Deserialize)]
struct ProjectInput {
    /// The matter code. `serde(default)` stays — an HTML form always posts the
    /// field, and a blank one must reach `open_matter` so lawyers see its
    /// "Code is required" message rather than a deserialize failure. The field
    /// is marked `required` in the form template.
    #[serde(default)]
    code: String,
    name: String,
    /// The entity this matter is opened against — `projects.entity_id` is
    /// NOT NULL, so a matter without one is a bug. Required on the form;
    /// an unselected picker posts `""`, which coerces to `None` here so
    /// the handler's own "Pick an entity" message is what lawyers see.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    entity_id: Option<Uuid>,
    /// The matter's scope narrative ("this project's story"). Persisted to
    /// `projects.description`.
    #[serde(default)]
    description: String,
    /// The required client-side DRI: which existing `Role::Client` person
    /// this matter is opened for. The client must pre-exist (the picker
    /// lists existing clients); validated below.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    client_dri_person_id: Option<Uuid>,
    #[serde(default)]
    scope_of_services: String,
    /// The lawyer-only Slack channel for this matter. Only read by the
    /// descriptive-update handler; the open-matter form has no field for it.
    #[serde(default)]
    internal_slack_channel_url: String,
    /// The Slack channel shared with the client, if any. Only read by the
    /// descriptive-update handler; the open-matter form has no field for it.
    #[serde(default)]
    external_slack_channel_url: String,
    /// The Project's source repository, as a whole URL on any forge — where
    /// its notation templates and client portal are sourced from. Only read by
    /// the descriptive-update handler; the open-matter form has no field for
    /// it, so a matter opens without one and records it later.
    #[serde(default)]
    repository_url: String,
    /// The firm-only Notion page for this matter. Only read by the
    /// descriptive-update handler; the open-matter form has no field for it.
    #[serde(default)]
    private_notion_page_url: String,
    /// The Notion page shared with the client, if any. Only read by the
    /// descriptive-update handler; the open-matter form has no field for it.
    #[serde(default)]
    shared_notion_page_url: String,
    /// Set (to `"1"`) when the opening attorney ticks the required conflict
    /// attestation checkbox. The shared `open_matter` command refuses the open
    /// without it (`AttestationRequired`) — every open is attested, never
    /// defaulted. At this firm `lawyer` is an attorney, so a lawyer/admin session
    /// opening a matter is an attorney attesting they have checked for and
    /// cleared conflicts. A *blocking* conflict (adverse to a current client)
    /// is a hard stop the attestation never overrides.
    #[serde(default)]
    attestation: Option<String>,
}

fn nonblank(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Resolve the firm-side person who is the matter's lawyer DRI: the opening
/// lawyer when their session is linked to a Person, else the firm's
/// default principal (resolved by role) so a matter still opens with a
/// real, NOT-NULL DRI under the dev auth-bypass. `None` only when neither
/// exists — an unseeded DB with an unlinked session, which the caller
/// rejects.
async fn resolve_lawyer_dri(
    surreal: &store::surreal::SurrealDb,
    session: Option<&SessionData>,
) -> Option<Uuid> {
    if let Some(id) = session.and_then(|s| s.person_id) {
        return Some(id);
    }
    store::persons::default_firm_dri(surreal)
        .await
        .ok()
        .flatten()
}

/// Echo the submitted matter-open fields back into a redirect query, under
/// `error`, so a refused open re-renders with nothing retyped. The conflict
/// attestation is deliberately never echoed: the opening attorney re-attests on
/// the corrected submission, so a refused open cannot leave it silently ticked.
fn project_form_query(input: &ProjectInput, error: &str) -> String {
    let mut query = String::new();
    push_query(&mut query, "error", error);
    push_query(&mut query, "name", input.name.trim());
    push_query(&mut query, "code", input.code.trim());
    if let Some(id) = input.entity_id {
        push_query(&mut query, "entity_id", &id.to_string());
    }
    push_query(&mut query, "description", input.description.trim());
    if let Some(id) = input.client_dri_person_id {
        push_query(&mut query, "client_dri_person_id", &id.to_string());
    }
    push_query(
        &mut query,
        "scope_of_services",
        input.scope_of_services.trim(),
    );
    query
}

/// Refuse a matter open: redirect back to the form with the message and the
/// submitted values (post/redirect/get). No matter is created — the caller
/// returns this before any insert, or after the open transaction rolled back.
fn refuse_open_matter(input: &ProjectInput, message: &str) -> Response {
    back_to_project_form(&project_form_query(input, message))
}

/// Validate the create form's selected client DRI: it must be present, exist,
/// and carry `Role::Client`. Returns the client's `store::persons::Person` (so the caller
/// has its email/name for the retainer) or the refusal redirect. The client-side
/// DRI is a real client of record, never a firm attorney — both the engineering
/// and legal councils flagged the firm-as-its-own-client default as a
/// conflict/loyalty problem.
async fn selected_client_dri(
    surreal: &store::surreal::SurrealDb,
    input: &ProjectInput,
) -> Result<store::persons::Person, Response> {
    let Some(id) = input.client_dri_person_id else {
        return Err(refuse_open_matter(
            input,
            "Pick the client this matter is for.",
        ));
    };
    let row = match store::persons::find_by_id(surreal, id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(refuse_open_matter(
                input,
                "That client was not found — pick an existing client (create them first if \
                 needed).",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "projects_create: client DRI lookup failed");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response());
        }
    };
    if row.role != store::persons::Role::Client {
        return Err(refuse_open_matter(
            input,
            "The client DRI must be an existing client person.",
        ));
    }
    Ok(row)
}

/// Map a matter-open command failure to the refusal redirect. Field-correctable
/// problems (a bad reference, a missing attestation, a code clash, a blocking
/// conflict) carry their own message; a mid-open database error carries the
/// admin-gated one. The command wrote nothing on any of these — it validates and
/// provisions in one transaction that rolls back on failure.
fn open_matter_form_error(
    session: Option<&SessionData>,
    input: &ProjectInput,
    err: store::projects::OpenMatterError,
) -> Response {
    use store::projects::OpenMatterError as E;
    let msg = match err {
        E::AttestationRequired => "Attest that you have checked for and cleared conflicts before \
             opening this matter."
            .to_string(),
        E::BlockingConflict(findings) => format!(
            "Conflict check blocked this matter — it is adverse to a current client. Resolve the \
             conflict or record a waiver before opening.\n\n{}",
            findings.join("\n"),
        ),
        E::CodeConflict => {
            "That project code is already in use. Choose a different code.".to_string()
        }
        E::Invalid(m) => m.to_string(),
        // The matter *did* open — only the conflict attestation failed to
        // record — so this must not read as a refusal. It says what is
        // missing, because the attestation is the record the firm's
        // conflict discipline rests on.
        E::Attestation(e) => {
            tracing::error!(error = %e, "open matter: attestation was not recorded");
            "The matter opened, but its conflict attestation was not recorded. Tell an \
             administrator before proceeding."
                .to_string()
        }
        E::ClientNotAllowed => {
            "The client of record must be an existing client person.".to_string()
        }
        E::AttesterNotAllowed => {
            "Your session isn't a firm attorney — a matter's attester must be a firm lawyer."
                .to_string()
        }
        E::NotFound(what) => {
            format!("That {what} was not found — open the matter against existing records.")
        }
        // The `String` description is shown to admins only; other lawyers get a
        // generic line. The caller has already logged the real error, so it is
        // queryable in `gcloud` regardless of what the UI shows.
        E::Db(e) => admin_gated_message(
            is_admin(session),
            &format!("Couldn't open this matter — {e}."),
            "Couldn't open this matter. The error has been logged; an admin can review the \
             details.",
        ),
    };
    refuse_open_matter(input, &msg)
}

/// POST `/app/projects` — open a matter, and **only** the matter: the
/// Project row, its conflict-review audit, and the lawyer-DRI + client
/// participations, in one transaction.
///
/// Opening a matter and opening its retainer are two steps. A Project is
/// created first (this handler); the retainer is then created on it like
/// any other Notation — `navigator notation create <retainer_code>
/// --project <code>`, or the lawyer retainer walk. The engagement-first rule
/// still holds and is enforced where notations are opened
/// (`workflows::notation_session`): a matter's *first* notation must be the
/// engagement that opens it. It is not this handler's job to pre-satisfy it
/// — a fresh matter legitimately has no engagement letter yet.
async fn projects_create_lawyer_only(
    State(state): State<AdminState>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<ProjectInput>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found_response();
    }
    if input.name.trim().is_empty() {
        return refuse_open_matter(&input, "Name is required.");
    }

    // Presentation validation: resolve the human-facing picks to the ids the
    // shared command needs, each with a friendly, field-specific form error.
    // The command re-validates every reference — these lookups are for the
    // message, not the trust boundary.
    let client = match selected_client_dri(&state.surreal, &input).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let Some(entity_id) = input.entity_id else {
        return refuse_open_matter(
            &input,
            "Pick an entity to open the matter against (create the entity first if needed).",
        );
    };

    // The attesting attorney and lawyer-side DRI: the opening lawyer when their
    // session is linked to a Person, else the firm principal under the dev
    // auth-bypass (never a sentinel). NOT NULL — a matter never opens without a
    // real responsible attorney.
    let Some(attester) = resolve_lawyer_dri(&state.surreal, session.as_deref()).await else {
        return refuse_open_matter(
            &input,
            "Your session isn't linked to a firm person — cannot open a matter.",
        );
    };

    // Open the matter through the shared command — the same boundary the CLI
    // (`cli::project`) and `POST /app/api/projects` use (#355). It owns the
    // reference checks, the conflict block, the attestation audit row, both DRI
    // designations in one transaction. The form is a thin adapter: it resolves
    // ids and renders the command's outcome. The required attestation checkbox
    // is the conflict control on every open; soft (non-blocking) findings
    // proceed on it and are recorded in the audit row, while a blocking
    // conflict is a hard stop.
    match store::projects::open_matter(
        &state.surreal,
        &store::projects::OpenMatterCommand {
            name: input.name.clone(),
            code: input.code.clone(),
            client_id: client.id,
            entity_id,
            description: nonblank(&input.description),
            attestation: input.attestation.as_deref() == Some("1"),
            acting_person_id: attester,
        },
    )
    .await
    {
        Ok(created) => {
            // Creating a matter lands on the matter. Its engagement letter is
            // the next step, not this one — the show page names that gap and
            // links to it, so lawyers choose the retainer deliberately.
            Redirect::to(&format!("/app/projects/{}", created.code)).into_response()
        }
        Err(e) => open_matter_form_error(session.as_deref(), &input, e),
    }
}

#[derive(Deserialize)]
struct ProjectParticipationInput {
    #[serde(default, rename = "_csrf")]
    _csrf_token: Option<String>,
    #[serde(default)]
    person_id: Option<Uuid>,
    /// The accountability control: `none`, `lawyer`, or `client`. Absent when
    /// the control was locked, which reads as "leave the markers alone".
    #[serde(default)]
    dri: Option<String>,
}

impl ProjectParticipationInput {
    /// The accountability request this submit carries.
    ///
    /// A missing field is [`DriRequest::Unchanged`]: a locked control posts
    /// nothing, and neither does any door that does not render one.
    fn dri_request(&self) -> store::participation::DriRequest {
        use store::participation::DriRequest;
        match self.dri.as_deref() {
            Some("lawyer") => DriRequest::Designate(store::projects::DriSide::Lawyer),
            Some("client") => DriRequest::Designate(store::projects::DriSide::Client),
            Some("none") => DriRequest::Clear,
            _ => DriRequest::Unchanged,
        }
    }
}

/// Turn a refused DRI change into the operator-facing sentence.
///
/// Designation is additive, so nothing here is a two-step: a submit either
/// changes the matter's accountability or is refused with a reason.
fn refuse_dri(
    project_code: &str,
    role_id: Option<Uuid>,
    input: &ProjectParticipationInput,
    error: &store::participation::DriError,
) -> Response {
    use store::participation::DriError as E;
    let message = match error {
        E::TierMismatch => {
            "Only a firm-side lawyer can be the lawyer DRI, and only a client can be the client DRI."
        }
        E::NotPermitted | E::ActorUnknown => {
            "You may not change this matter's accountability."
        }
        E::LawyerDriRequired => {
            "This matter always has a lawyer DRI. Designate another lawyer before removing this one."
        }
        E::Db(err) => {
            tracing::error!(error = %err, %project_code, "project participation: dri designation failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };
    refuse_participation(project_code, role_id, input, message)
}

/// The DRI actor behind this request.
///
/// This surface is admin-gated, so a session that reached it is already
/// entitled; naming the person is what puts them on the audit row. A session
/// with no `persons` row cannot be named, and the trail records the change with
/// no actor rather than not recording it.
fn dri_actor(session: Option<&SessionData>) -> store::participation::DriActor {
    session.and_then(|session| session.person_id).map_or(
        store::participation::DriActor::System,
        store::participation::DriActor::Person,
    )
}

/// Require a non-impersonating admin before changing a project's participation
/// ledger. This avoids exposing the firm-wide people directory to ordinary
/// lawyer and keeps project-scope grants with the administrative ACL owner.
///
/// This is the *firm-wide* ledger form. A matter's own lawyer DRIs govern their
/// side from the workbench (`webapp::lawyer_project_detail`), which shows only
/// the people already on that matter and so needs no directory read.
async fn project_participation_access(
    surreal: &store::surreal::SurrealDb,
    session: Option<&SessionData>,
    project_id: Uuid,
) -> Result<(), Response> {
    if !can_manage_project_participation(session) {
        return Err(not_found_response());
    }
    // Only admins reach this helper, and the lawyer access model gives admins
    // the documented all-project bypass. Keep the project lookup below so a
    // random identifier still returns a normal not-found response.
    match store::projects::find_by_id(surreal, project_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(not_found_response()),
        Err(e) => {
            tracing::error!(error = %e, project_id = %project_id, "project participation: project lookup failed");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response())
        }
    }
}

async fn project_show_path(surreal: &store::surreal::SurrealDb, project_id: Uuid) -> String {
    store::projects::find_by_id(surreal, project_id)
        .await
        .ok()
        .flatten()
        .map_or_else(
            || APP_PROJECTS_PATH.to_string(),
            |project| format!("/app/projects/{}", project.code),
        )
}

async fn participation_people(
    surreal: &store::surreal::SurrealDb,
) -> Result<Vec<store::persons::Person>, Response> {
    store::persons::list_directory(surreal, "", "", &[])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "project participation: people lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        })
}

/// The participation form's path for one matter — the page a refused add or
/// edit redirects back to (post/redirect/get), carrying its message and the
/// submitted values so nothing is retyped.
fn participation_form_path(project_code: &str, role_id: Option<Uuid>) -> String {
    match role_id {
        Some(role_id) => format!("/app/projects/{project_code}/people/{role_id}/edit"),
        None => format!("/app/projects/{project_code}/people/new"),
    }
}

/// Refuse a participation add/edit: redirect back to its form with the message
/// and the submitted person echoed.
fn refuse_participation(
    project_code: &str,
    role_id: Option<Uuid>,
    input: &ProjectParticipationInput,
    error: &str,
) -> Response {
    let mut query = String::new();
    push_query(&mut query, "error", error);
    if let Some(person_id) = input.person_id {
        push_query(&mut query, "person_id", &person_id.to_string());
    }
    if let Some(dri) = input.dri.as_deref() {
        push_query(&mut query, "dri", dri);
    }
    let path = participation_form_path(project_code, role_id);
    if query.is_empty() {
        Redirect::to(&path).into_response()
    } else {
        Redirect::to(&format!("{path}?{query}")).into_response()
    }
}

async fn project_participation_create(
    State(surreal): State<store::surreal::SurrealDb>,
    Path(project_code): Path<String>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<ProjectParticipationInput>,
) -> Response {
    let session = session.as_deref();
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    if let Err(response) = project_participation_access(&surreal, session, id).await {
        return response;
    }
    let people = match participation_people(&surreal).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let Some(person_id) = input.person_id else {
        return refuse_participation(
            &project_code,
            None,
            &input,
            "Choose a person to assign to this matter.",
        );
    };
    if !people.iter().any(|p| p.id == person_id) {
        return refuse_participation(&project_code, None, &input, "That person was not found.");
    }
    // Route the write through the shared command — the same boundary
    // `POST /app/api/projects/{id}/participants` uses (#355). It owns the
    // participation validation, the one-row-per-person+matter rule, and the
    // insert; the form resolves the person field and renders the outcome.
    // The form asks only who; the command derives the participation from that
    // person's tier, so this handler never names one.
    match store::participation::add_participant(
        &surreal,
        &store::participation::AddParticipantCommand {
            project_id: id,
            person_id,
            dri: input.dri_request(),
            actor: dri_actor(session),
        },
    )
    .await
    {
        Ok(_) => Redirect::to(&project_show_path(&surreal, id).await).into_response(),
        Err(e) => {
            use store::participation::AddParticipantError as E;
            let message = match e {
                E::Duplicate => "That person is already assigned to this matter.",
                E::PersonNotFound => "That person was not found.",
                E::ProjectNotFound => "That matter was not found.",
                E::Dri(error) => return refuse_dri(&project_code, None, &input, &error),
                E::Db(err) => {
                    tracing::error!(error = %err, project_id = %id, "project participation: add failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
                }
            };
            refuse_participation(&project_code, None, &input, message)
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn project_participation_update(
    State(surreal): State<store::surreal::SurrealDb>,
    Path((project_code, role_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<ProjectParticipationInput>,
) -> Response {
    let session = session.as_deref();
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    if let Err(response) = project_participation_access(&surreal, session, id).await {
        return response;
    }
    let people = match participation_people(&surreal).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let Some(person_id) = input.person_id else {
        return refuse_participation(
            &project_code,
            Some(role_id),
            &input,
            "Choose a person to assign to this matter.",
        );
    };
    if !people.iter().any(|p| p.id == person_id) {
        return refuse_participation(
            &project_code,
            Some(role_id),
            &input,
            "That person was not found.",
        );
    }
    // Route the write through the shared command (#355) — it owns the
    // participation validation, the duplicate rule, and the lawyer-DRI-lockout
    // invariant; the form resolves the person field and renders the outcome.
    // Re-pointing a row re-derives which side of the matter it sits on, inside
    // the command — this handler resolves the person and nothing else.
    match store::participation::update_participant(
        &surreal,
        &store::participation::UpdateParticipantCommand {
            project_id: id,
            role_id,
            person_id,
            dri: input.dri_request(),
            actor: dri_actor(session),
        },
    )
    .await
    {
        Ok(_) => Redirect::to(&project_show_path(&surreal, id).await).into_response(),
        Err(e) => {
            use store::participation::UpdateParticipantError as E;
            let message = match e {
                E::NotFound => return not_found_response(),
                E::PersonNotFound => "That person was not found.",
                E::Duplicate => "That person is already assigned to this matter.",
                E::DriLockout => PROJECT_PARTICIPATION_DRI_LOCKOUT_ERROR,
                E::Dri(error) => return refuse_dri(&project_code, Some(role_id), &input, &error),
                E::Db(err) => {
                    tracing::error!(error = %err, project_id = %id, role_id = %role_id, "project participation: update failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
                }
            };
            refuse_participation(&project_code, Some(role_id), &input, message)
        }
    }
}

async fn project_participation_delete(
    State(surreal): State<store::surreal::SurrealDb>,
    Path((project_code, role_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let session = session.as_deref();
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    if let Err(response) = project_participation_access(&surreal, session, id).await {
        return response;
    }
    // Route the removal through the shared command (#355) — it owns the
    // lawyer-DRI-lockout invariant (the accountability marker rides this row, so
    // removing the DRI's row would strand the matter's accountable lawyer).
    match store::participation::remove_participant(&surreal, id, role_id, dri_actor(session)).await
    {
        Ok(()) => Redirect::to(&project_show_path(&surreal, id).await).into_response(),
        Err(store::participation::RemoveParticipantError::NotFound) => not_found_response(),
        Err(store::participation::RemoveParticipantError::DriLockout) => {
            delete_refused_response(PROJECT_PARTICIPATION_DRI_LOCKOUT_ERROR, APP_PROJECTS_PATH)
        }
        Err(store::participation::RemoveParticipantError::Dri(error)) => {
            // Removing a row that carries a marker is a DRI change, so it can
            // be refused for the same reasons a designation can.
            tracing::warn!(error = %error, project_id = %id, role_id = %role_id, "project participation: dri removal refused");
            delete_refused_response(&error.to_string(), APP_PROJECTS_PATH)
        }
        Err(store::participation::RemoveParticipantError::Db(e)) => {
            tracing::error!(error = %e, project_id = %id, role_id = %role_id, "project participation delete failed");
            delete_refused_response(PROJECT_PARTICIPATION_DELETE_ERROR, APP_PROJECTS_PATH)
        }
    }
}

/// The two matter-workbench accountability controls, on people already assigned
/// to the matter.
///
/// These are deliberately *not* behind [`project_participation_access`]. That
/// gate protects the firm-wide participation form, which reads the whole people
/// directory; this pair names one row that is already on the matter, so it needs
/// no directory read and admits the lawyer tier. Who may actually move the
/// marker is `store::participation`'s decision, from the actor these handlers
/// pass it: a matter's lawyer DRIs govern their own side, and the client side
/// takes the lawyer tier.
///
/// `role_id` names the participation row; the side follows from the person on
/// it, so neither handler takes one.
async fn matter_dri_designate(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
    Path((project_code, role_id)): Path<(String, Uuid)>,
) -> Response {
    let session = session.as_deref();
    if !is_lawyer_tier(session) {
        return not_found_response();
    }
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    let Some(row) = participation_row(&surreal, id, role_id).await else {
        return not_found_response();
    };
    // The side is the person's tier, not a field the form posts — the same
    // derivation `participation_for_role` makes everywhere else.
    let Some(person) = (match store::persons::find_by_id(&surreal, row.person_id).await {
        Ok(person) => person,
        Err(e) => {
            tracing::error!(error = %e, project_id = %id, "matter dri: person lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    }) else {
        return not_found_response();
    };
    let side = if person.role.is_lawyer_tier() {
        store::projects::DriSide::Lawyer
    } else {
        store::projects::DriSide::Client
    };
    matter_dri_write(
        &surreal,
        id,
        role_id,
        row.person_id,
        store::participation::DriRequest::Designate(side),
        dri_actor(session),
    )
    .await
}

async fn matter_dri_remove(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
    Path((project_code, role_id)): Path<(String, Uuid)>,
) -> Response {
    let session = session.as_deref();
    if !is_lawyer_tier(session) {
        return not_found_response();
    }
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    let Some(row) = participation_row(&surreal, id, role_id).await else {
        return not_found_response();
    };
    matter_dri_write(
        &surreal,
        id,
        role_id,
        row.person_id,
        store::participation::DriRequest::Clear,
        dri_actor(session),
    )
    .await
}

/// One participation row, confirmed to belong to this matter.
async fn participation_row(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    role_id: Uuid,
) -> Option<store::projects::PersonProjectRole> {
    store::projects::participation_by_id(surreal, role_id)
        .await
        .ok()
        .flatten()
        .filter(|row| row.project_id == project_id)
}

/// Send one accountability change through the shared command and render the
/// outcome back onto the workbench.
async fn matter_dri_write(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    role_id: Uuid,
    person_id: Uuid,
    dri: store::participation::DriRequest,
    actor: store::participation::DriActor,
) -> Response {
    let show_path = project_show_path(surreal, project_id).await;
    let result = store::participation::update_participant(
        surreal,
        &store::participation::UpdateParticipantCommand {
            project_id,
            role_id,
            person_id,
            dri,
            actor,
        },
    )
    .await;
    // The refusal rides back to the matter it was refused on, not to the
    // matter list — the operator is looking at the accountability panel and
    // that is where the sentence has to appear.
    let refused = |message: &str| {
        Redirect::to(&format!(
            "{show_path}?error={}",
            encode_query_value(message)
        ))
        .into_response()
    };
    match result {
        Ok(_) => Redirect::to(&show_path).into_response(),
        Err(store::participation::UpdateParticipantError::NotFound) => not_found_response(),
        Err(store::participation::UpdateParticipantError::Dri(error)) => {
            refused(&error.to_string())
        }
        Err(e) => {
            tracing::error!(error = %e, %project_id, %role_id, "matter dri: write failed");
            refused(PROJECT_PARTICIPATION_DELETE_ERROR)
        }
    }
}

async fn projects_update_lawyer_only(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
    Path(code): Path<String>,
    Form(input): Form<ProjectInput>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found_response();
    }
    // The descriptive update owns name, entity, the scope narrative, the two
    // Slack channel links, and the source repository URL only. The edit form no
    // longer renders a status control: changing a matter's lifecycle
    // (open/closed/archived) and its coupled retention `closed_at` is a
    // transition with firm-policy semantics, handled by dedicated lifecycle
    // commands (navigator#770), not this general edit. So `status` is neither
    // posted by the form nor forwarded here. The form always sends
    // `description`, the two Slack fields, and the repository URL, so pass each
    // as `Some` to keep the blank-clears behavior.
    let Some(project) = store::projects::find_by_code(&surreal, &code)
        .await
        .ok()
        .flatten()
    else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };
    let project_id = project.id;
    let command = store::projects::UpdateProjectCommand {
        // The form always posts every field, so it is a full replacement even
        // though the command is a patch. A blank name still fails the command's
        // own refusal rather than clearing the column.
        name: Some(input.name),
        entity_id: input.entity_id,
        description: Some(input.description),
        internal_slack_channel_url: Some(input.internal_slack_channel_url),
        external_slack_channel_url: Some(input.external_slack_channel_url),
        repository_url: Some(input.repository_url),
        private_notion_page_url: Some(input.private_notion_page_url),
        shared_notion_page_url: Some(input.shared_notion_page_url),
    };
    match store::projects::update_project(&surreal, project_id, &command).await {
        Ok(_) => Redirect::to("/app/projects").into_response(),
        Err(store::projects::ProjectCommandError::NotFound) => {
            (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
        }
        Err(store::projects::ProjectCommandError::Invalid(message)) => {
            (StatusCode::BAD_REQUEST, message).into_response()
        }
        // `Referenced` is a delete-only outcome; a descriptive update never
        // raises it, but the enum is shared, so treat it as the same
        // server-side fault rather than leaving the match non-exhaustive.
        Err(store::projects::ProjectCommandError::Db(e)) => {
            tracing::error!(error = %e, %project_id, "projects_update: failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
        Err(e @ store::projects::ProjectCommandError::Referenced(_)) => {
            tracing::error!(error = %e, %project_id, "projects_update: unexpected referenced");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
    }
}

async fn projects_delete_lawyer_only(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
    Path(project_code): Path<String>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found_response();
    }
    let Some(id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found_response();
    };
    match store::projects::delete_project_with_surreal(&surreal, id).await {
        // `NotFound` lands with `Ok` on purpose: already gone is what the
        // caller wanted, so a double-clicked delete or a second tab must not
        // error. The API renders that same outcome as a 404 — only this
        // browser door treats it as done.
        Ok(_) | Err(store::projects::ProjectCommandError::NotFound) => {
            delete_response("/app/projects")
        }
        // Dependents still reference the matter — the row stays put.
        Err(store::projects::ProjectCommandError::Referenced(detail)) => delete_refused_response(
            &format!("Couldn't delete this matter — {detail}."),
            "/app/projects",
        ),
        Err(e) => {
            tracing::error!(error = %e, project_id = %id, "projects_delete: failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
    }
}

// --- CSV exports -----------------------------------------------------
//
// One endpoint per CRUD admin resource. Returns an RFC 4180 CSV with
// the columns the admin list pages already show, plus the resolved
// names of foreign-key references (entity type / jurisdiction /
// related entity) so the spreadsheet stays readable without joining
// back to other exports.

async fn people_csv(State(surreal): State<store::surreal::SurrealDb>) -> crate::admin_csv::CsvBody {
    // Ordered by name, not id: the CSV is a human artifact and an id
    // ordering carries no meaning to its reader.
    let rows_raw = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap_or_default();
    let rows: Vec<Vec<String>> = rows_raw
        .into_iter()
        .map(|p| vec![p.id.to_string(), p.name, p.email])
        .collect();
    crate::admin_csv::CsvBody {
        filename: "people.csv",
        headers: vec!["id", "name", "email"],
        rows,
    }
}

async fn entities_csv(
    State(surreal): State<store::surreal::SurrealDb>,
) -> crate::admin_csv::CsvBody {
    let mut rows_raw = store::entities::all(&surreal).await.unwrap_or_default();
    // The CSV has always been id-ordered, and `entities::all` reads by
    // name, so the order is restored here rather than adding a second
    // ordering to the store for one export.
    rows_raw.sort_by_key(|row| row.id);
    // A failed reference read degrades to empty cells, matching the
    // other lookups.
    let types = store::entity_types::list(&surreal, &[])
        .await
        .unwrap_or_default();
    let jurs = store::jurisdictions::list_all(&surreal)
        .await
        .unwrap_or_default();
    let by_type = |id: Uuid| {
        types
            .iter()
            .find(|t| t.id == id)
            .map_or(String::new(), |t| t.name.clone())
    };
    let by_jur = |id: Uuid| {
        jurs.iter()
            .find(|j| j.id == id)
            .map_or(String::new(), |j| j.name.clone())
    };
    let rows: Vec<Vec<String>> = rows_raw
        .into_iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                e.name,
                by_type(e.entity_type_id),
                by_jur(e.jurisdiction_id),
            ]
        })
        .collect();
    crate::admin_csv::CsvBody {
        filename: "entities.csv",
        headers: vec!["id", "name", "entity_type", "jurisdiction"],
        rows,
    }
}

async fn projects_csv(
    State(surreal): State<store::surreal::SurrealDb>,
    session: Option<Extension<SessionData>>,
) -> crate::admin_csv::CsvBody {
    let (person_id, role) = match session.as_deref() {
        Some(s) => (s.person_id, s.role),
        None => (None, store::persons::Role::Client),
    };
    let rows_raw = store::access::visible_projects_as_lawyer(&surreal, person_id, role)
        .await
        .unwrap_or_default();
    let entities = store::entities::all(&surreal).await.unwrap_or_default();
    let by_entity = |id: Uuid| {
        entities
            .iter()
            .find(|e| e.id == id)
            .map_or(String::new(), |e| e.name.clone())
    };
    let rows: Vec<Vec<String>> = rows_raw
        .into_iter()
        .map(|p| {
            vec![
                p.id.to_string(),
                p.code,
                p.name,
                p.status,
                by_entity(p.entity_id),
            ]
        })
        .collect();
    crate::admin_csv::CsvBody {
        filename: "projects.csv",
        headers: vec!["id", "code", "name", "status", "entity_name"],
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admin_gated_message, bootstrap_company_from_lookup, encode_query_value, is_admin,
        participation_people, project_participation_access,
    };
    use crate::session::SessionData;
    use store::persons::Role;
    use uuid::Uuid;

    /// The encoder's contract, pinned directly rather than only through the
    /// one flow that happened to expose it.
    ///
    /// Every byte it emits must be legal in a `Location` header. Before this
    /// was fixed a conflict block (findings joined with a newline) and any
    /// refusal carrying an em dash passed through unchanged and turned a
    /// deliberate refusal into a `500` — latent in the pre-existing `?error=`
    /// flashes, not only in the new post/redirect/get paths.
    #[test]
    fn encode_query_value_emits_only_header_safe_bytes() {
        // Query syntax that would otherwise split the query.
        assert_eq!(
            encode_query_value("a?b&c#d%e+f g"),
            "a%3Fb%26c%23d%25e%2Bf%20g"
        );
        // Control characters: the newline joining conflict findings, and the
        // CR that would otherwise permit header splitting outright.
        assert_eq!(encode_query_value("one\ntwo"), "one%0Atwo");
        assert_eq!(encode_query_value("a\r\nb"), "a%0D%0Ab");
        assert_eq!(encode_query_value("\u{0}"), "%00");
        // Printable ASCII that is nonetheless illegal in a URI query. The
        // playbook positions textarea rides a refusal as `?positions=`, and its
        // `|` field delimiter is the common case — left raw, the `Location`
        // is not a URI at all and a strict client refuses the redirect.
        assert_eq!(
            encode_query_value("Term | 1 year | 2 years"),
            "Term%20%7C%201%20year%20%7C%202%20years",
        );
        assert_eq!(
            encode_query_value("a\"b<c>d[e]f\\g^h`i{j}k"),
            "a%22b%3Cc%3Ed%5Be%5Df%5Cg%5Eh%60i%7Bj%7Dk",
        );
        // Non-ASCII: an em dash is three UTF-8 bytes, each encoded.
        assert_eq!(
            encode_query_value("no \u{2014} yes"),
            "no%20%E2%80%94%20yes"
        );
        // DEL, the boundary on the upper side.
        assert_eq!(encode_query_value("\u{7f}"), "%7F");
        // Ordinary printable ASCII is untouched.
        assert_eq!(
            encode_query_value("Plain-Text_123.txt"),
            "Plain-Text_123.txt"
        );

        // The invariant behind every case above: nothing outside printable
        // ASCII survives, so no output can break a `Location` header.
        let hostile = "findings:\nfirst \u{2014} second\r\n\u{0}\u{7f}\u{e9}";
        let encoded = encode_query_value(hostile);
        assert!(
            encoded.bytes().all(|b| (0x21..0x7f).contains(&b)),
            "every emitted byte must be printable ASCII, got: {encoded}"
        );
    }

    const GENERIC: &str =
        "Couldn't open this matter. The error has been logged; an admin can review the details.";

    #[test]
    fn bootstrap_company_defaults_to_shook_and_ignores_blank_values() {
        assert_eq!(
            bootstrap_company_from_lookup(|_| None),
            super::DEFAULT_BOOTSTRAP_COMPANY
        );
        assert_eq!(
            bootstrap_company_from_lookup(|_| Some(" \t ".to_string())),
            super::DEFAULT_BOOTSTRAP_COMPANY
        );
        assert_eq!(
            bootstrap_company_from_lookup(|_| Some("  Rebrand Law PLLC  ".to_string())),
            "Rebrand Law PLLC"
        );
    }

    #[test]
    fn admin_sees_detail_and_non_admin_gets_the_generic_line() {
        let detail = "Couldn't open this matter — a referenced person no longer exists.";
        // Admin: the diagnostic detail, verbatim.
        assert_eq!(admin_gated_message(true, detail, GENERIC), detail);
        // Non-admin: the generic line, with the detail withheld.
        let hidden = admin_gated_message(false, detail, GENERIC);
        assert_eq!(hidden, GENERIC);
        assert!(!hidden.contains("referenced person"));
    }

    #[test]
    fn is_admin_is_true_for_owner_and_admin() {
        assert!(is_admin(Some(&SessionData::fresh(
            "o@example.com",
            Role::Owner
        ))));
        assert!(is_admin(Some(&SessionData::fresh(
            "a@example.com",
            Role::Admin
        ))));
        assert!(!is_admin(Some(&SessionData::fresh(
            "s@example.com",
            Role::Lawyer
        ))));
        assert!(!is_admin(Some(&SessionData::fresh(
            "c@example.com",
            Role::Client
        ))));
        assert!(!is_admin(None));
    }

    #[tokio::test]
    async fn participation_people_returns_internal_on_database_failure() {
        // The directory is read from SurrealDB, so the failure has to happen
        // *there*: any other unreachable handle leaves this reading a healthy
        // engine and returning an empty list.
        let surreal = store::surreal::test_support::unreachable();
        let response = participation_people(&surreal).await.unwrap_err();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn project_participation_access_returns_internal_on_project_lookup_failure() {
        let surreal = store::surreal::test_support::unreachable();
        let session = SessionData::fresh("admin@example.com", Role::Admin);
        let response = project_participation_access(&surreal, Some(&session), Uuid::now_v7())
            .await
            .unwrap_err();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
