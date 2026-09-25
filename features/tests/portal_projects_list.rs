//! Cucumber runner for `features/portal_projects_list.feature`.
//!
//! Exercises `GET /app/projects` — the matter list — proving Owner/Admin
//! read every matter (`store::projects::all`) while an ordinary Lawyer stays
//! scoped to their own participation ledger
//! (`store::access::visible_projects_as_lawyer`). The runner shape mirrors
//! `portal_projects_detail.rs`: forge a session cookie, send the request,
//! assert on the response.

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
struct ListWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for ListWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl ListWorld {
    fn sessions(&self) -> &SessionStore {
        self.sessions.as_ref().expect("sessions not built")
    }

    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
}

#[given("the Neon Law Navigator app is running")]
async fn build_app(world: &mut ListWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-projects-list").await;
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
async fn seed_person(world: &mut ListWorld, email: String, role: String) {
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
}

#[given(regex = r#"^a project "([^"]+)" with no participants$"#)]
async fn seed_project_no_participants(world: &mut ListWorld, project_name: String) {
    ensure_project(world, &project_name).await;
}

#[given(regex = r#"^a project "([^"]+)" with an onboarding document$"#)]
async fn seed_project_with_onboarding(world: &mut ListWorld, project_name: String) {
    let project_id = ensure_project(world, &project_name).await;
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("portal-projects-list-onboarding").await;
    let content = format!("# Onboarding\n\nFixture engagement letter for {project_name}.\n");
    store::documents::ingest_bytes_exactly_once(
        &surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id,
            source: "generated",
            filename: "onboarding.md",
            kind: "onboarding",
            content_type: "text/markdown",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::CLIENT,
        },
        content.as_bytes(),
    )
    .await
    .expect("ingest onboarding document");
}

#[given(regex = r#"^a project "([^"]+)" is closed$"#)]
async fn close_project(world: &mut ListWorld, project_name: String) {
    let project_id = ensure_project(world, &project_name).await;
    let surreal = features::shared_surreal().await;
    store::projects::transition_project_with_reason(
        &surreal,
        project_id,
        store::projects::Transition::Close,
        Some(store::projects::ClosureReason::EngagementCompleted),
        None,
    )
    .await
    .expect("close project");
}

#[given(regex = r#"^a project "([^"]+)" is archived$"#)]
async fn archive_project(world: &mut ListWorld, project_name: String) {
    let project_id = ensure_project(world, &project_name).await;
    let surreal = features::shared_surreal().await;
    store::projects::transition_project_with_reason(
        &surreal,
        project_id,
        store::projects::Transition::Archive,
        None,
        None,
    )
    .await
    .expect("archive project");
}

#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as the supervising lawyer DRI$"#)]
async fn seed_lawyer_dri(world: &mut ListWorld, project_name: String, lawyer_email: String) {
    let project_id = ensure_project(world, &project_name).await;
    let lawyer_id = *world
        .persons
        .get(&lawyer_email)
        .expect("lawyer was seeded earlier");
    store::projects::designate_dri_in_surreal(
        &features::shared_surreal().await,
        project_id,
        lawyer_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .expect("designate lawyer DRI");
}

#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as a supervised clerk$"#)]
async fn seed_supervised_clerk(world: &mut ListWorld, project_name: String, clerk_email: String) {
    let project_id = ensure_project(world, &project_name).await;
    let clerk_id = *world
        .persons
        .get(&clerk_email)
        .expect("clerk was seeded earlier");
    store::projects::add_participation(
        &features::shared_surreal().await,
        project_id,
        clerk_id,
        "clerk",
    )
    .await
    .expect("insert clerk participation");
}

async fn ensure_project(world: &mut ListWorld, project_name: &str) -> Uuid {
    if let Some(id) = world.projects.get(project_name) {
        return *id;
    }
    let surreal = features::shared_surreal().await;
    let code = format!("test-{}", Uuid::now_v7().simple());
    let inserted = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code,
            name: project_name.to_string(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert project");
    world.projects.insert(project_name.to_string(), inserted.id);
    inserted.id
}

#[when(regex = r#"^"([^"]+)" opens the projects list$"#)]
async fn open_list(world: &mut ListWorld, email: String) {
    open_list_at(world, email, "/app/projects").await;
}

#[when(regex = r#"^"([^"]+)" opens the closed projects list$"#)]
async fn open_closed_list(world: &mut ListWorld, email: String) {
    open_list_at(world, email, "/app/projects/closed").await;
}

async fn open_list_at(world: &mut ListWorld, email: String, uri: &str) {
    let person_id = *world.persons.get(&email).expect("actor was seeded earlier");
    let role = store::persons::find_by_id(&features::shared_surreal().await, person_id)
        .await
        .expect("query person")
        .expect("person row exists")
        .role;
    let session = SessionData {
        sub: format!("rauthy-{email}-subject"),
        email: Some(email.clone()),
        person_id: Some(person_id),
        exp: portal::session::now_unix_secs() + 60,
        role,
        csrf_token: "test-csrf".into(),
        source: portal::session::SessionSource::Browser,
        provider: None,
        viewing_as_dri: None,
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
                .uri(uri)
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
async fn status_is(world: &mut ListWorld, code: u16) {
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
async fn body_contains(world: &mut ListWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "expected body to contain {needle:?}; body was: {}",
        truncated(&world.last_body)
    );
}

#[then(regex = r#"^the response body does not contain "([^"]+)"$"#)]
async fn body_does_not_contain(world: &mut ListWorld, needle: String) {
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
    ListWorld::cucumber()
        // Scenarios share the process-wide embedded SurrealDB from
        // `features::shared_surreal`; releasing engine handles in parallel
        // can drop an embedded runtime from another scenario's async task.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/portal_projects_list.feature")
        .await;
}
