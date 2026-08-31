//! Cucumber runner for `features/portal_admin_firm_surface.feature`.
//!
//! Verifies firm-administration listings at `/app/admin/*` and the lawyer
//! workbench at `/app/lawyer/*` for Owner/Admin/Lawyer. embedded Rego policy
//! is `passthrough` in this suite — the client-blocked scenario lives in a
//! live-KIND smoke test, not here.

// Cucumber's step-attribute macros want `async fn` everywhere.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{policy::PolicyClient, SessionStore};
use tower::ServiceExt;
use uuid::Uuid;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct FirmWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    roles: HashMap<String, store::persons::Role>,
    last_status: Option<StatusCode>,
    last_body: String,
    current_cookie: Option<String>,
}

impl std::fmt::Debug for FirmWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirmWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl FirmWorld {
    fn sessions(&self) -> &SessionStore {
        self.sessions.as_ref().expect("sessions not built")
    }
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    fn session_cookie_for(&self, email: &str) -> String {
        let person_id = *self.persons.get(email).expect("actor seeded");
        let role = *self.roles.get(email).expect("actor role seeded");
        let session = SessionData {
            sub: format!("rauthy-{email}-subject"),
            email: Some(email.to_string()),
            person_id: Some(person_id),
            exp: portal::session::now_unix_secs() + 60,
            role,
            csrf_token: "test-csrf".into(),
            source: portal::session::SessionSource::Browser,
            provider: None,
            impersonation: None,
            scope: None,
        };
        format!("{SESSION_COOKIE_NAME}={}", self.sessions().encode(&session))
    }
}

#[given("the Neon Law Navigator app is running")]
async fn build_app(world: &mut FirmWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-admin-firm").await;
    let sessions = SessionStore::new("test-session-key-not-for-production");
    let state = app_state(
        runtime,
        storage,
        PolicyClient::passthrough(),
        None,
        sessions.clone(),
    )
    .await;
    world.sessions = Some(sessions);
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[given(regex = r#"^a seeded person "([^"]+)" with role "([^"]+)"$"#)]
async fn seed_person(world: &mut FirmWorld, email: String, role: String) {
    let role = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
        "clerk" => store::persons::Role::Clerk,
        _ => store::persons::Role::Client,
    };
    let inserted = store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson {
            oidc_subject: Some(format!("rauthy-{email}-subject")),
            ..store::persons::NewPerson::with_role(email.clone(), email.clone(), role)
        },
    )
    .await;
    world.persons.insert(email, inserted.id);
    world.roles.insert(inserted.email, inserted.role);
}

#[given(regex = r#"^a Clerk project "([^"]+)" assigned to "([^"]+)" and supervised by "([^"]+)"$"#)]
async fn seed_clerk_project(
    world: &mut FirmWorld,
    name: String,
    clerk_email: String,
    lawyer_email: String,
) {
    let clerk_id = *world.persons.get(&clerk_email).expect("clerk seeded");
    let lawyer_id = *world.persons.get(&lawyer_email).expect("lawyer seeded");
    let surreal = features::shared_surreal().await;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("clerk-project-{}", Uuid::now_v7()),
            name,
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&features::shared_surreal().await).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert clerk project");
    // The Clerk lens only shows a matter whose lawyer DRI currently holds the lawyer tier.
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        lawyer_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .expect("designate supervising lawyer");
    store::projects::add_participation(&surreal, project.id, clerk_id, "clerk")
        .await
        .expect("assign Clerk to project");
}

#[when(regex = r#"^"([^"]+)" opens (.+)$"#)]
async fn open(world: &mut FirmWorld, email: String, path: String) {
    let cookie = world.session_cookie_for(&email);
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

#[when(regex = r#"^"([^"]+)" POSTs to impersonate "([^"]+)"$"#)]
async fn post_impersonate(world: &mut FirmWorld, actor_email: String, target_email: String) {
    let target_id = *world.persons.get(&target_email).expect("target seeded");
    let cookie = world.session_cookie_for(&actor_email);
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{target_id}/impersonate"))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.current_cookie = session_cookie_pair(&resp);
    world.last_body = body_string(resp).await;
}

#[when(regex = r"^the browser opens (.+) with its current session$")]
async fn browser_open_current_session(world: &mut FirmWorld, path: String) {
    let cookie = world
        .current_cookie
        .as_deref()
        .expect("browser session cookie set");
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut FirmWorld, code: u16) {
    let actual = world.last_status.expect("no response");
    assert_eq!(
        actual.as_u16(),
        code,
        "expected {code}, got {} (body: {})",
        actual,
        truncate(&world.last_body)
    );
}

#[then(regex = r#"^the response body contains "([^"]+)"$"#)]
async fn body_contains(world: &mut FirmWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "body did not contain {needle:?}; body: {}",
        truncate(&world.last_body)
    );
}

#[then(regex = r#"^the response body does not contain "([^"]+)"$"#)]
async fn body_does_not_contain(world: &mut FirmWorld, needle: String) {
    assert!(
        !world.last_body.contains(&needle),
        "body unexpectedly contained {needle:?}: {}",
        truncate(&world.last_body)
    );
}

#[then(regex = r#"^the browser session role is "([^"]+)"$"#)]
async fn browser_session_role_is(world: &mut FirmWorld, expected: String) {
    let cookie = world
        .current_cookie
        .as_deref()
        .expect("browser session cookie set");
    let session = decode_session_cookie_pair(world.sessions(), cookie);
    assert_eq!(session.role.as_str(), expected);
}

fn session_cookie_pair(resp: &axum::http::Response<Body>) -> Option<String> {
    resp.headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(SESSION_COOKIE_NAME))
        .map(|v| v.split(';').next().unwrap().to_string())
}

fn decode_session_cookie_pair(sessions: &SessionStore, cookie: &str) -> SessionData {
    let value = cookie
        .strip_prefix(&format!("{SESSION_COOKIE_NAME}="))
        .expect("navigator session cookie pair");
    sessions.decode(value).expect("valid signed session")
}

fn truncate(s: &str) -> String {
    if s.len() <= 400 {
        s.to_string()
    } else {
        format!("{}…", &s[..400])
    }
}

#[tokio::main]
async fn main() {
    FirmWorld::cucumber()
        // Every scenario composes a Dioxus-backed router. Serial execution
        // keeps its pinned Tokio worker pools bounded while each scenario's
        // world and store are torn down.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/portal_admin_firm_surface.feature")
        .await;
}
