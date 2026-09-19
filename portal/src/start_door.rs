//! The authenticated visitor's service start door.
//!
//! A service card names the only services this route can open. The visitor's
//! ordinary session supplies the client Person; the boot-resolved lawyer DRI
//! supplies the firm-side participant. The route performs the same immediate
//! commits as the existing lawyer walk and compensates them on refusal.

use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, RequestExt, Router};
use dioxus_server::{render_handler, FullstackState, ServeConfig};
use serde::Deserialize;
use uuid::Uuid;

use crate::csrf::CsrfMode;
use crate::retainer_walk::{open_intake_matter, IntakeClient, OpenIntakeError};
use crate::{gated, AppState, SessionData};

/// The brand-mounted start route.
pub const START_PATH: &str = "/start/{service_id}";
const MAX_BODY_BYTES: usize = 8 * 1024;
const ON_CALL_ENV: &str = "NAVIGATOR_ON_CALL_LAWYER_EMAIL";

/// The client intake route reuses start copy after the POST/redirect handoff.
/// It is parsed from the same locale catalog as the host route.
pub(crate) fn bundled_start_copy() -> Option<views::locales::services::StartDoorCopy> {
    static COPY: std::sync::OnceLock<Option<views::locales::services::StartDoorCopy>> =
        std::sync::OnceLock::new();
    COPY.get_or_init(|| {
        let yaml = include_str!("../../neon/locales/en/neon/services-catalog.yaml");
        views::locales::services::ServicesCatalog::parse(yaml)
            .ok()
            .map(|catalog| catalog.start)
    })
    .clone()
}

#[derive(Clone)]
struct DoorState {
    app: AppState,
    catalog: Option<views::locales::services::ServicesCatalog>,
}

#[derive(Debug, Deserialize, Default)]
struct StartForm {
    #[serde(default)]
    _csrf: String,
    /// Accepted but ignored. The route maps the service id to its template
    /// from the server-owned catalog.
    #[serde(default, rename = "template")]
    _template: Option<String>,
}

/// Build the service door beside the firm's public Dioxus pages.
pub fn routes(
    state: &AppState,
    catalog: Option<views::locales::services::ServicesCatalog>,
) -> Router {
    let door_state = DoorState {
        app: state.clone(),
        catalog,
    };
    let render = Router::<FullstackState>::new()
        .route(
            START_PATH,
            get(render_handler)
                .layer(from_fn(inject_public_context))
                .layer(from_fn_with_state(door_state.clone(), inject_start_content))
                .layer(from_fn(inject_start_csrf))
                .layer(from_fn(crate::dioxus_app::dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::start_door::StartDoorEntry,
        ));
    let command = Router::new()
        .route(START_PATH, post(post_start))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .route_layer(from_fn_with_state(
            (state.sessions.clone(), CsrfMode::Strict),
            crate::csrf::require_csrf,
        ))
        .route_layer(from_fn_with_state(
            state.rate_limit.clone(),
            crate::rate_limit::enforce,
        ))
        .with_state(door_state);
    gated(state, render).merge(gated(state, command))
}

async fn inject_public_context(mut req: Request, next: Next) -> Response {
    let utility = crate::dioxus_app::public_utility_links(req.extensions().get::<SessionData>());
    req.extensions_mut()
        .insert(webapp::public_chrome::firm_public_chrome(utility));
    next.run(req).await
}

async fn inject_start_csrf(mut req: Request, next: Next) -> Response {
    let token = req
        .extensions()
        .get::<SessionData>()
        .map_or_else(String::new, |session| session.csrf_token.clone());
    req.extensions_mut()
        .insert(webapp::start_door::StartDoorCsrf(token));
    next.run(req).await
}

async fn inject_start_content(
    State(state): State<DoorState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Ok(Path(service_id)) = req.extract_parts::<Path<String>>().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(catalog) = state.catalog.as_ref() else {
        insert_refusal(&mut req, service_id, None);
        return next.run(req).await;
    };
    let Some(service) = catalog
        .services
        .iter()
        .find(|service| service.id == service_id && service.template.is_some())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let brand = views::brand::brand_key();
    let can_start = configured_lawyer(&state.app, brand, &service_id)
        .await
        .is_some()
        && store::firms::firm_id_for_brand_key(&state.app.surreal, brand.as_str())
            .await
            .ok()
            .flatten()
            .is_some();
    req.extensions_mut()
        .insert(webapp::start_door::InjectedStartDoor(
            webapp::start_door::StartDoorContent {
                service_id,
                service_name: service.name.clone(),
                disclosure: catalog.start.disclosure.clone(),
                refusal: catalog.start.refusal.clone(),
                start_label: catalog.start.label.clone(),
                can_start,
            },
        ));
    next.run(req).await
}

fn insert_refusal(
    req: &mut Request,
    service_id: String,
    catalog: Option<&views::locales::services::ServicesCatalog>,
) {
    let (disclosure, refusal, start_label) = catalog.map_or_else(
        || {
            (
                String::new(),
                bundled_start_copy()
                    .map(|copy| copy.refusal)
                    .unwrap_or_default(),
                String::new(),
            )
        },
        |catalog| {
            (
                catalog.start.disclosure.clone(),
                catalog.start.refusal.clone(),
                catalog.start.label.clone(),
            )
        },
    );
    req.extensions_mut()
        .insert(webapp::start_door::InjectedStartDoor(
            webapp::start_door::StartDoorContent {
                service_id,
                service_name: String::new(),
                disclosure,
                refusal,
                start_label,
                can_start: false,
            },
        ));
}

async fn configured_lawyer(
    state: &AppState,
    brand: views::brand::BrandKey,
    service_id: &str,
) -> Option<Uuid> {
    let configured = state
        .on_call_lawyer_email
        .as_deref()
        .map(|email| (email, "configured"));
    let bootstrap_owner = state
        .bootstrap_owner_email
        .as_deref()
        .map(|email| (email, "bootstrap_owner"));
    let Some((email, on_call_source)) = configured.or(bootstrap_owner) else {
        tracing::error!(
            target: "audit",
            audit = true,
            service_id,
            brand = brand.as_str(),
            outcome = "configuration_missing",
            "{ON_CALL_ENV} and NAVIGATOR_BOOTSTRAP_OWNER_EMAIL are not configured"
        );
        return None;
    };
    tracing::info!(
        target: "audit",
        audit = true,
        service_id,
        brand = brand.as_str(),
        on_call_source,
        outcome = "configuration_selected",
        "start door lawyer source selected"
    );
    let person = match store::persons::find_by_email_ci(&state.surreal, email).await {
        Ok(person) => person,
        Err(error) => {
            tracing::error!(
                target: "audit",
                audit = true,
                service_id,
                brand = brand.as_str(),
                on_call_source,
                outcome = "configuration_lookup_failed",
                error = %error,
                "start door lawyer source could not resolve its Person"
            );
            return None;
        }
    };
    let Some(person) = person else {
        tracing::error!(
            target: "audit",
            audit = true,
            service_id,
            brand = brand.as_str(),
            on_call_source,
            outcome = "configuration_person_missing",
            "start door lawyer source does not name a Person"
        );
        return None;
    };
    if !person.role.is_lawyer_tier()
        || !store::persons::is_admitted(&state.surreal, person.id)
            .await
            .ok()
            .unwrap_or(false)
    {
        tracing::error!(
            target: "audit",
            audit = true,
            service_id,
            brand = brand.as_str(),
            on_call_source,
            outcome = "configuration_person_not_admitted_lawyer",
            "start door lawyer source does not name an admitted lawyer-tier Person"
        );
        return None;
    }
    Some(person.id)
}

#[allow(clippy::too_many_lines)]
async fn post_start(
    State(state): State<DoorState>,
    Extension(session): Extension<SessionData>,
    Path(service_id): Path<String>,
    Form(_body): Form<StartForm>,
) -> Response {
    let Some(catalog) = state.catalog.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(service) = catalog
        .services
        .iter()
        .find(|service| service.id == service_id && service.template.is_some())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let brand = views::brand::brand_key();
    if session.role.is_lawyer_tier() {
        return Redirect::to("/app/lawyer/retainers/new").into_response();
    }
    if session.role != store::persons::Role::Client {
        audit(&session, None, None, &service_id, brand, "rejected_role");
        return (StatusCode::FORBIDDEN, catalog.start.refusal.clone()).into_response();
    }
    let Some(person_id) = session.person_id else {
        audit(&session, None, None, &service_id, brand, "rejected_person");
        return (StatusCode::FORBIDDEN, catalog.start.refusal.clone()).into_response();
    };
    let Some(lawyer_dri_id) = configured_lawyer(&state.app, brand, &service_id).await else {
        audit(
            &session,
            None,
            None,
            &service_id,
            brand,
            "rejected_configuration",
        );
        return (StatusCode::OK, catalog.start.refusal.clone()).into_response();
    };
    let Some(template_code) = service.template.as_deref() else {
        audit(
            &session,
            None,
            None,
            &service_id,
            brand,
            "rejected_template",
        );
        return (StatusCode::OK, catalog.start.refusal.clone()).into_response();
    };
    let Ok(Some(template)) =
        store::templates::resolve(&state.app.surreal, None, template_code).await
    else {
        audit(
            &session,
            None,
            None,
            &service_id,
            brand,
            "rejected_template",
        );
        return (StatusCode::OK, catalog.start.refusal.clone()).into_response();
    };
    let snapshot = match workflows::notation_session::questionnaire_snapshot_for_template(
        &state.app.surreal,
        Some(&state.app.storage),
        &template,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::error!(error = %error, "start door: questionnaire snapshot failed");
            audit(
                &session,
                None,
                None,
                &service_id,
                brand,
                "rejected_snapshot",
            );
            return (StatusCode::OK, catalog.start.refusal.clone()).into_response();
        }
    };
    match open_intake_matter(
        &state.app.surreal,
        &template,
        snapshot,
        IntakeClient::Existing(person_id),
        lawyer_dri_id,
        brand,
        "pitch",
        false,
    )
    .await
    {
        Ok(opened) => {
            audit(
                &session,
                Some(opened.project.id),
                Some(opened.rows.notation_id),
                &service_id,
                brand,
                "accepted",
            );
            let person_id = person_id.to_string();
            let project_id = opened.project.id.to_string();
            let notation_id = opened.rows.notation_id.to_string();
            telemetry::record_funnel_event(telemetry::FunnelEvent::Started {
                person_id: &person_id,
                project_id: &project_id,
                notation_id: &notation_id,
                service_id: &service_id,
                brand: brand.as_str(),
            });
            Redirect::to(&format!(
                "/app/projects/{}/intake/{}?started=1",
                opened.project.code, opened.rows.notation_id
            ))
            .into_response()
        }
        Err(OpenIntakeError::Conflict) => {
            audit(
                &session,
                None,
                None,
                &service_id,
                brand,
                "rejected_conflict",
            );
            (StatusCode::OK, catalog.start.refusal.clone()).into_response()
        }
        Err(OpenIntakeError::BrandNotWorn) => {
            audit(&session, None, None, &service_id, brand, "rejected_brand");
            (StatusCode::OK, catalog.start.refusal.clone()).into_response()
        }
        Err(OpenIntakeError::Internal) => {
            audit(
                &session,
                None,
                None,
                &service_id,
                brand,
                "rejected_internal",
            );
            (StatusCode::OK, catalog.start.refusal.clone()).into_response()
        }
    }
}

fn audit(
    session: &SessionData,
    project_id: Option<Uuid>,
    notation_id: Option<Uuid>,
    service_id: &str,
    brand: views::brand::BrandKey,
    outcome: &str,
) {
    match (project_id, notation_id) {
        (Some(project_id), Some(notation_id)) => tracing::info!(
            target: "audit", audit = true, person_id = ?session.person_id,
            %project_id, %notation_id, service_id, brand = brand.as_str(), outcome,
            "start door"
        ),
        (Some(project_id), None) => tracing::info!(
            target: "audit", audit = true, person_id = ?session.person_id,
            %project_id, service_id, brand = brand.as_str(), outcome, "start door"
        ),
        (None, Some(notation_id)) => tracing::info!(
            target: "audit", audit = true, person_id = ?session.person_id,
            %notation_id, service_id, brand = brand.as_str(), outcome, "start door"
        ),
        (None, None) => tracing::info!(
            target: "audit", audit = true, person_id = ?session.person_id,
            service_id, brand = brand.as_str(), outcome, "start door"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::future::Future;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;
    use tracing_subscriber::fmt::MakeWriter;

    const LAWYER_EMAIL: &str = "lawyer@neonlaw.com";

    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    async fn capture_output<F, Fut, T>(action: F) -> (T, String)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        crate::test_tracing::ensure_callsite_interest();
        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();
        let result = {
            let _guard = tracing::subscriber::set_default(subscriber);
            action().await
        };
        let output = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");
        (result, output)
    }

    fn catalog() -> views::locales::services::ServicesCatalog {
        views::locales::services::ServicesCatalog::parse(include_str!(
            "../../neon/locales/en/neon/services-catalog.yaml"
        ))
        .expect("the shipped service catalog parses")
    }

    async fn seeded_state() -> (AppState, store::surreal::SurrealDb) {
        let surreal = store::test_support::mem_surreal().await;
        let state = crate::test_support::app_state(surreal.clone()).await;
        store::seed::seed_canonical(&surreal, &state.storage)
            .await
            .expect("the canonical seed loads the service templates");
        let lawyer = store::persons::find_or_create(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Start Door Lawyer",
                LAWYER_EMAIL,
                store::persons::Role::Lawyer,
            ),
        )
        .await
        .expect("create configured lawyer");
        store::persons::set_admitted(&surreal, lawyer.id, true)
            .await
            .expect("admit configured lawyer");
        let mut state = state;
        state.on_call_lawyer_email = Some(LAWYER_EMAIL.to_string());
        (state, surreal)
    }

    async fn client(
        surreal: &store::surreal::SurrealDb,
        name: &str,
        email: &str,
    ) -> store::persons::Person {
        let person = store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
        )
        .await
        .expect("create synthetic client");
        store::persons::set_admitted(surreal, person.id, true)
            .await
            .expect("admit synthetic client");
        store::persons::find_by_id(surreal, person.id)
            .await
            .expect("read synthetic client")
            .expect("synthetic client remains present")
    }

    async fn person_with_role(
        surreal: &store::surreal::SurrealDb,
        name: &str,
        email: &str,
        role: store::persons::Role,
    ) -> store::persons::Person {
        let person = store::persons::find_or_create(
            surreal,
            &store::persons::NewPerson::with_role(name, email, role),
        )
        .await
        .expect("create synthetic person");
        store::persons::set_admitted(surreal, person.id, true)
            .await
            .expect("admit synthetic person");
        store::persons::find_by_id(surreal, person.id)
            .await
            .expect("read synthetic person")
            .expect("synthetic person remains present")
    }

    fn session(person: &store::persons::Person) -> SessionData {
        let mut session = SessionData::fresh(person.email.clone(), person.role);
        session.email = Some(person.email.clone());
        session.person_id = Some(person.id);
        session
    }

    async fn post_direct(
        state: &AppState,
        catalog: views::locales::services::ServicesCatalog,
        session: SessionData,
        service_id: &str,
        template: Option<&str>,
    ) -> Response {
        post_start(
            State(DoorState {
                app: state.clone(),
                catalog: Some(catalog),
            }),
            Extension(session),
            Path(service_id.to_string()),
            Form(StartForm {
                _csrf: String::new(),
                _template: template.map(ToOwned::to_owned),
            }),
        )
        .await
    }

    fn location(response: &Response) -> String {
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .unwrap_or_default()
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read response body");
        String::from_utf8(bytes.to_vec()).expect("response body is UTF-8")
    }

    fn session_cookie(state: &AppState, session: &SessionData) -> String {
        format!(
            "{}={}",
            crate::session::SESSION_COOKIE_NAME,
            state.sessions.encode(session)
        )
    }

    #[tokio::test]
    async fn t1_client_start_creates_a_pitch_matter_with_both_dris() {
        let (state, surreal) = seeded_state().await;
        let client = client(&surreal, "Start Door Client", "start-client@example.com").await;
        let lawyer = store::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
            .await
            .expect("find configured lawyer")
            .expect("canonical seed includes the configured lawyer");

        let (response, output) = capture_output(|| async {
            post_direct(
                &state,
                catalog(),
                session(&client),
                "llc-file",
                Some("onboarding__letter"),
            )
            .await
        })
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = location(&response);
        assert!(location.starts_with("/app/projects/"), "{location}");
        assert!(location.contains("/intake/"), "{location}");
        assert!(location.ends_with("?started=1"), "{location}");

        let projects = store::projects::all(&surreal).await.expect("list projects");
        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.status, "pitch");
        assert_eq!(project.brand, "neon");

        let roles = store::projects::participations_for_project(&surreal, project.id)
            .await
            .expect("list matter participation");
        assert_eq!(roles.len(), 2);
        assert!(roles.iter().any(|role| {
            role.person_id == client.id && role.participation == "client" && role.is_client_dri
        }));
        assert!(roles.iter().any(|role| {
            role.person_id == lawyer.id && role.participation == "lawyer" && role.is_lawyer_dri
        }));

        let notations = store::notations::list_by_project(&surreal, project.id)
            .await
            .expect("list matter notations");
        assert_eq!(notations.len(), 1);
        let template = store::templates::find_by_id(&surreal, notations[0].template_id)
            .await
            .expect("read notation template")
            .expect("notation template remains present");
        assert_eq!(template.code, "nv__llc_formation");

        let (engagements, closings) = store::projects::matter_lifecycle_sets(&surreal, &projects)
            .await
            .expect("compute matter lifecycle sets");
        assert!(!engagements.contains(&project.id));
        assert!(!closings.contains(&project.id));

        assert!(output.contains("funnel.started"), "funnel: {output}");
        assert!(
            output.contains(&format!("person_id=\"{}\"", client.id)),
            "funnel: {output}"
        );
        assert!(
            output.contains(&format!("project_id=\"{}\"", project.id)),
            "funnel: {output}"
        );
        assert!(
            output.contains("service_id=\"llc-file\""),
            "funnel: {output}"
        );
        assert!(output.contains("brand=\"neon\""), "funnel: {output}");
        assert!(
            !output.contains("Start Door Client"),
            "name leaked: {output}"
        );
        assert!(
            !output.contains("start-client@example.com"),
            "email leaked: {output}"
        );
        assert!(
            !output.contains(&project.code),
            "Project code leaked: {output}"
        );
        for forbidden in ["email=", "phone=", "name=", "address=", "project_code="] {
            assert!(!output.contains(forbidden), "{forbidden} leaked: {output}");
        }
    }

    #[tokio::test]
    async fn t2_service_mapping_controls_the_template_and_unmapped_service_writes_nothing() {
        let (state, surreal) = seeded_state().await;
        let client = client(
            &surreal,
            "Mapping Test Client",
            "mapping-client@example.com",
        )
        .await;
        let response = post_direct(
            &state,
            catalog(),
            session(&client),
            "llc-file",
            Some("onboarding__letter"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let notation = store::notations::list_all(&surreal)
            .await
            .expect("list mapped notation")
            .pop()
            .expect("mapped service opened one notation");
        let template = store::templates::find_by_id(&surreal, notation.template_id)
            .await
            .expect("read mapped template")
            .expect("mapped template exists");
        assert_eq!(template.code, "nv__llc_formation");

        let before_projects = store::projects::all(&surreal)
            .await
            .expect("list projects")
            .len();
        let before_notations = store::notations::list_all(&surreal)
            .await
            .expect("list notations")
            .len();
        let response = post_direct(
            &state,
            catalog(),
            session(&client),
            "nda",
            Some("nv__llc_formation"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            store::projects::all(&surreal)
                .await
                .expect("list projects")
                .len(),
            before_projects
        );
        assert_eq!(
            store::notations::list_all(&surreal)
                .await
                .expect("list notations")
                .len(),
            before_notations
        );
    }

    #[tokio::test]
    async fn bootstrap_owner_fallback_opens_start_door_with_owner_dri() {
        let (mut state, surreal) = seeded_state().await;
        let owner = person_with_role(
            &surreal,
            "Bootstrap Owner",
            "bootstrap-owner@example.com",
            store::persons::Role::Owner,
        )
        .await;
        let client = client(&surreal, "Fallback Client", "fallback-client@example.com").await;
        state.on_call_lawyer_email = None;
        state.bootstrap_owner_email = Some(owner.email.clone());

        let (response, output) = capture_output(|| async {
            post_direct(&state, catalog(), session(&client), "llc-file", None).await
        })
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let projects = store::projects::all(&surreal).await.expect("list projects");
        let roles = store::projects::participations_for_project(&surreal, projects[0].id)
            .await
            .expect("list matter participation");
        assert!(roles
            .iter()
            .any(|role| role.person_id == owner.id && role.is_lawyer_dri));
        assert!(
            output.contains("on_call_source=\"bootstrap_owner\""),
            "{output}"
        );
    }

    #[tokio::test]
    async fn bootstrap_owner_fallback_requires_lawyer_tier() {
        let (mut state, surreal) = seeded_state().await;
        let owner = person_with_role(
            &surreal,
            "Bootstrap Client",
            "bootstrap-client@example.com",
            store::persons::Role::Client,
        )
        .await;
        let client = client(
            &surreal,
            "Rejected Fallback Client",
            "rejected-fallback@example.com",
        )
        .await;
        state.on_call_lawyer_email = None;
        state.bootstrap_owner_email = Some(owner.email);

        let response = post_direct(&state, catalog(), session(&client), "llc-file", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await, catalog().start.refusal);
        assert!(store::projects::all(&surreal)
            .await
            .expect("list projects")
            .is_empty());
    }

    #[tokio::test]
    async fn configured_on_call_lawyer_wins_over_bootstrap_owner() {
        let (mut state, surreal) = seeded_state().await;
        let owner = person_with_role(
            &surreal,
            "Bootstrap Owner",
            "bootstrap-owner@example.com",
            store::persons::Role::Owner,
        )
        .await;
        let lawyer = store::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
            .await
            .expect("find configured lawyer")
            .expect("configured lawyer exists");
        let client = client(
            &surreal,
            "Configured Client",
            "configured-client@example.com",
        )
        .await;
        state.bootstrap_owner_email = Some(owner.email);

        let (response, output) = capture_output(|| async {
            post_direct(&state, catalog(), session(&client), "llc-file", None).await
        })
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let projects = store::projects::all(&surreal).await.expect("list projects");
        let roles = store::projects::participations_for_project(&surreal, projects[0].id)
            .await
            .expect("list matter participation");
        assert!(roles
            .iter()
            .any(|role| role.person_id == lawyer.id && role.is_lawyer_dri));
        assert!(!roles
            .iter()
            .any(|role| role.person_id == owner.id && role.is_lawyer_dri));
        assert!(output.contains("on_call_source=\"configured\""), "{output}");
    }

    #[tokio::test]
    async fn missing_start_door_lawyer_configuration_is_reported() {
        let (mut state, surreal) = seeded_state().await;
        let client = client(
            &surreal,
            "Configuration Client",
            "configuration-client@example.com",
        )
        .await;
        state.on_call_lawyer_email = None;
        state.bootstrap_owner_email = None;

        let (response, output) = capture_output(|| async {
            post_direct(&state, catalog(), session(&client), "llc-file", None).await
        })
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await, catalog().start.refusal);
        assert!(output.contains("configuration_missing"), "{output}");
        assert!(store::projects::all(&surreal)
            .await
            .expect("list projects")
            .is_empty());
        assert!(!output.contains("configuration-client@example.com"));
        assert!(!output.contains("Configuration Client"));
    }

    #[tokio::test]
    async fn t4_anonymous_get_redirects_and_authenticated_get_carries_verbatim_disclosure() {
        let (state, surreal) = seeded_state().await;
        let catalog = catalog();
        let app = routes(&state, Some(catalog.clone())).layer(CookieManagerLayer::new());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/start/llc-file")
                    .body(Body::empty())
                    .expect("anonymous request"),
            )
            .await
            .expect("anonymous response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&response), "/auth/login?return_to=/start/llc-file");

        let client = client(
            &surreal,
            "Disclosure Client",
            "disclosure-client@example.com",
        )
        .await;
        let session = session(&client);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/start/llc-file")
                    .header("cookie", session_cookie(&state, &session))
                    .body(Body::empty())
                    .expect("authenticated request"),
            )
            .await
            .expect("authenticated response");
        assert_eq!(response.status(), StatusCode::OK);
        let html = body(response).await;
        assert!(html.contains(&catalog.start.disclosure), "{html}");
    }

    #[tokio::test]
    async fn t5_strict_csrf_and_rate_limit_protect_the_cookie_post() {
        let (mut state, surreal) = seeded_state().await;
        let client = client(
            &surreal,
            "Protection Client",
            "protection-client@example.com",
        )
        .await;
        state.rate_limit = crate::rate_limit::RateLimit::new(1, Duration::from_mins(1));
        let session = session(&client);
        let app = routes(&state, Some(catalog())).layer(CookieManagerLayer::new());
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/start/llc-file")
                .header("cookie", session_cookie(&state, &session))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("template=nv__annual_report"))
                .expect("cookie POST")
        };
        let first = app
            .clone()
            .oneshot(request())
            .await
            .expect("first response");
        let second = app.oneshot(request()).await.expect("second response");
        assert_eq!(first.status(), StatusCode::FORBIDDEN);
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            store::projects::all(&surreal)
                .await
                .expect("list projects")
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn t6_blocking_conflict_compensates_rows_but_keeps_client_admitted() {
        let (state, surreal) = seeded_state().await;
        let opponent = client(&surreal, "Current Client", "current-client@example.com").await;
        let opponent_entity = store::test_support::seed_entity(&surreal).await;
        let existing = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: format!("existing-conflict-{}", Uuid::now_v7()),
                name: "Existing conflict matter".into(),
                status: "open".into(),
                brand: "neon".into(),
                entity_id: opponent_entity,
                ..Default::default()
            },
        )
        .await
        .expect("create blocking-conflict fixture");
        store::projects::designate_dri_in_surreal(
            &surreal,
            existing.id,
            opponent.id,
            store::projects::DriSide::Client,
        )
        .await
        .expect("designate current client");
        store::projects::designate_dri_in_surreal(
            &surreal,
            existing.id,
            store::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
                .await
                .expect("find lawyer")
                .expect("lawyer exists")
                .id,
            store::projects::DriSide::Lawyer,
        )
        .await
        .expect("designate current lawyer");

        let proposed = client(&surreal, "Proposed Client", "proposed-client@example.com").await;
        store::relationships::record(
            &surreal,
            &store::relationships::NewRelationship {
                from: store::relationships::Endpoint::Person,
                from_id: proposed.id,
                to: store::relationships::Endpoint::Person,
                to_id: opponent.id,
                kind: store::relationships::KIND_ADVERSE_TO.into(),
                confidence_pct: 100,
                source_kind: store::relationships::SOURCE_MANUAL.into(),
                source_id: None,
                detail: None,
            },
        )
        .await
        .expect("record blocking relationship");

        let before_notations = store::notations::list_all(&surreal)
            .await
            .expect("list initial notations")
            .len();
        let before_roles = store::projects::all_participations(&surreal)
            .await
            .expect("list initial participation")
            .len();
        let response = post_direct(&state, catalog(), session(&proposed), "llc-file", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await, catalog().start.refusal);
        assert_eq!(
            store::projects::all(&surreal)
                .await
                .expect("list projects")
                .len(),
            1
        );
        assert_eq!(
            store::notations::list_all(&surreal)
                .await
                .expect("list notations")
                .len(),
            before_notations
        );
        assert_eq!(
            store::projects::all_participations(&surreal)
                .await
                .expect("list participation")
                .len(),
            before_roles
        );
        assert!(store::persons::is_admitted(&surreal, proposed.id)
            .await
            .expect("read admission"));
    }
}
