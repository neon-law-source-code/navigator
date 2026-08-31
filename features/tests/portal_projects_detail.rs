//! Cucumber runner for `features/portal_projects_detail.feature`.
//!
//! Exercises `GET /app/projects/:code` end-to-end with row-level
//! scoping via [`store::access::visible_projects`]. The runner shape
//! mirrors `portal_landing.rs`: forge a session cookie, send the
//! request, assert on the response.

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
struct DetailWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    project_codes: HashMap<String, String>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for DetailWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetailWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl DetailWorld {
    fn sessions(&self) -> &SessionStore {
        self.sessions.as_ref().expect("sessions not built")
    }

    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    fn project_code(&self, name: &str) -> &str {
        self.project_codes
            .get(name)
            .expect("project was seeded earlier")
    }
}

#[given("the Neon Law Navigator app is running")]
async fn build_app(world: &mut DetailWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-projects-detail").await;
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
async fn seed_person(world: &mut DetailWorld, email: String, role: String) {
    let role = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
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
}

#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as a participant$"#)]
async fn seed_project_with_participant(
    world: &mut DetailWorld,
    project_name: String,
    participant_email: String,
) {
    let project_id = ensure_project(world, &project_name).await;
    let person_id = *world
        .persons
        .get(&participant_email)
        .expect("participant person was seeded earlier");
    store::projects::add_participation(
        &features::shared_surreal().await,
        project_id,
        person_id,
        "client",
    )
    .await
    .expect("insert SurrealDB person_project_role");
}

#[given(regex = r#"^a project "([^"]+)" with no participants$"#)]
async fn seed_project_no_participants(world: &mut DetailWorld, project_name: String) {
    ensure_project(world, &project_name).await;
}

async fn ensure_project(world: &mut DetailWorld, project_name: &str) -> Uuid {
    if let Some(id) = world.projects.get(project_name) {
        return *id;
    }
    let surreal = features::shared_surreal().await;
    let code = format!("test-{}", Uuid::now_v7().simple());
    let inserted = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: project_name.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&features::shared_surreal().await).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert project");
    world.projects.insert(project_name.to_string(), inserted.id);
    world.project_codes.insert(project_name.to_string(), code);
    inserted.id
}

#[when(regex = r#"^"([^"]+)" opens the detail page for "([^"]+)"$"#)]
async fn open_detail(world: &mut DetailWorld, email: String, project_name: String) {
    let person_id = *world.persons.get(&email).expect("actor was seeded earlier");
    let project_code = world.project_code(&project_name);
    let role = role_for(&features::shared_surreal().await, person_id).await;
    let session = SessionData {
        sub: format!("rauthy-{email}-subject"),
        email: Some(email.clone()),
        person_id: Some(person_id),
        exp: portal::session::now_unix_secs() + 60,
        role,
        csrf_token: "test-csrf".into(),
        source: portal::session::SessionSource::Browser,
        provider: None,
        impersonation: None,
        scope: None,
    };
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        world.sessions().encode(&session)
    );
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

async fn role_for(surreal: &store::surreal::SurrealDb, person_id: Uuid) -> store::persons::Role {
    store::persons::find_by_id(surreal, person_id)
        .await
        .expect("query person")
        .expect("person row exists")
        .role
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut DetailWorld, code: u16) {
    let actual = world.last_status.expect("no response captured");
    assert_eq!(
        actual.as_u16(),
        code,
        "expected {code}, got {} (body: {})",
        actual,
        truncated(&world.last_body)
    );
}

#[then(regex = r#"^the response body contains "([^"]+)"$"#)]
async fn body_contains(world: &mut DetailWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "expected body to contain {needle:?}; body was: {}",
        truncated(&world.last_body)
    );
}

#[then(regex = r#"^the response body does not contain "([^"]+)"$"#)]
async fn body_does_not_contain(world: &mut DetailWorld, needle: String) {
    assert!(
        !world.last_body.contains(&needle),
        "expected body not to contain {needle:?}; body was: {}",
        truncated(&world.last_body)
    );
}

fn truncated(s: &str) -> String {
    const LIMIT: usize = 400;
    if s.len() <= LIMIT {
        s.to_string()
    } else {
        format!("{}…", &s[..LIMIT])
    }
}

#[tokio::main]
async fn main() {
    DetailWorld::cucumber()
        .run_and_exit("tests/features/portal_projects_detail.feature")
        .await;
}
