//! Cucumber runner for `features/portal_projects_writes.feature`.
//!
//! Exercises the lawyer write surface under `/app/projects/*` and
//! proves those write URLs are absent from the client portal. The
//! lightweight client detail at
//! `/app/projects/:code` continues to render without admin chrome
//! (no Edit / Upload buttons).

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

const CSRF: &str = "test-csrf";

#[derive(Default, World)]
#[world(init = Self::default)]
struct WritesWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    project_codes: HashMap<String, String>,
    /// `person_project_role` row id, keyed by `"{project_name}:{email}"` —
    /// what the DRI controls address a participant by.
    participation_role_ids: HashMap<String, Uuid>,
    last_status: Option<StatusCode>,
    last_body: String,
    last_location: Option<String>,
}

impl std::fmt::Debug for WritesWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritesWorld")
            .field("last_status", &self.last_status)
            .field("last_location", &self.last_location)
            .finish_non_exhaustive()
    }
}

impl WritesWorld {
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
async fn build_app(world: &mut WritesWorld) {
    let surreal = features::shared_surreal().await;
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-projects-writes").await;
    // The admin create path always opens a matter on a retainer, so it
    // needs the canonical retainer template present to bind.
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .expect("seed canonical");
    let sessions = SessionStore::new("test-session-key-not-for-production");
    let state = app_state(
        runtime,
        storage.clone(),
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
async fn seed_person(world: &mut WritesWorld, email: String, role: String) {
    let role = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
        "clerk" => store::persons::Role::Clerk,
        _ => store::persons::Role::Client,
    };
    // `seed_canonical` already plants the firm principal (`nick@neonlaw.com`,
    // admin). When a scenario re-declares that person, reuse the existing row
    // (and set its session subject) rather than hitting the unique-email
    // constraint.
    let surreal = features::shared_surreal().await;
    let id = if let Some(existing) = store::persons::find_by_email_ci(&surreal, &email)
        .await
        .expect("lookup person")
    {
        store::persons::link_oidc_subject(
            &surreal,
            existing.id,
            &format!("rauthy-{email}-subject"),
        )
        .await
        .expect("link the existing person's IdP identity");
        store::persons::set_role(&surreal, existing.id, role)
            .await
            .expect("update existing person");
        existing.id
    } else {
        store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson {
                oidc_subject: Some(format!("rauthy-{email}-subject")),
                ..store::persons::NewPerson::with_role(email.clone(), email.clone(), role)
            },
        )
        .await
        .id
    };
    world.persons.insert(email, id);
}

/// Seed a bare `Role::Client` person — the matter-open form opens a
/// matter *for* an existing client, so the client of record must exist
/// before the create POST. Returns the new person id.
async fn seed_client(surreal: &store::surreal::SurrealDb, email: &str) -> Uuid {
    store::test_support::ensure_person(
        surreal,
        &store::persons::NewPerson::with_role(email, email, store::persons::Role::Client),
    )
    .await
    .id
}

#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as a participant$"#)]
async fn seed_project_with_participant(
    world: &mut WritesWorld,
    project_name: String,
    participant_email: String,
) {
    let project_id = ensure_project(world, &project_name).await;
    let person_id = *world
        .persons
        .get(&participant_email)
        .expect("participant person was seeded earlier");
    let surreal = features::shared_surreal().await;
    // Derive the participation from the person's tier, exactly as the
    // matter-people form does at its write seam (`participation_for_role`).
    // Hard-coding `client` here would put a firm tier on the client side of the
    // matter and quietly deny them the workbench they are being seeded onto.
    let person = store::persons::find_by_id(&surreal, person_id)
        .await
        .expect("lookup participant")
        .expect("participant exists");
    let row = store::projects::add_participation(
        &surreal,
        project_id,
        person_id,
        store::projects::participation_for_role(person.role),
    )
    .await
    .expect("insert SurrealDB person_project_role");
    world
        .participation_role_ids
        .insert(format!("{project_name}:{participant_email}"), row.id);
}

#[given(regex = r#"^a project "([^"]+)" with no participants$"#)]
async fn seed_project_no_participants(world: &mut WritesWorld, project_name: String) {
    ensure_project(world, &project_name).await;
}

async fn ensure_project(world: &mut WritesWorld, project_name: &str) -> Uuid {
    if let Some(id) = world.projects.get(project_name) {
        return *id;
    }
    let surreal = features::shared_surreal().await;
    let code = format!("test-{}", Uuid::now_v7().simple());
    world
        .project_codes
        .insert(project_name.to_string(), code.clone());
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

#[when(regex = r#"^"([^"]+)" submits "([^"]*)" to (/[^ ]+)$"#)]
async fn submit_to_path(world: &mut WritesWorld, email: String, body: String, path: String) {
    let cookie = session_cookie_for(world, &email).await;
    // The admin create form opens a matter *for* an existing client: it
    // requires `entity_id`, `client_dri_person_id`
    // (a real `Role::Client` person), and the required
    // conflict `attestation` (the opening attorney affirms they have checked
    // for and cleared conflicts — the shared command refuses the open
    // without it). When the scenario posts a bare create form, seed those
    // prerequisites and append them, so the static feature body stays focused
    // on the access contract it's testing rather than the matter-open plumbing.
    let mut body = body;
    if path == "/app/projects" && body.contains("name=") {
        if !body.contains("entity_id=") {
            let entity_id =
                store::test_support::seed_entity(&features::shared_surreal().await).await;
            body = format!("{body}&entity_id={entity_id}");
        }
        if !body.contains("client_dri_person_id=") {
            let client_id = seed_client(
                &features::shared_surreal().await,
                "client-of-record@example.com",
            )
            .await;
            body = format!("{body}&client_dri_person_id={client_id}");
        }
        // The matter code is required and never derived — it is also the
        // matter's shared-drive folder name (#938). Same reasoning as the
        // fields above: supply it here so the feature body stays about the
        // access contract, not the matter-open plumbing. Matched on the field
        // boundary rather than a bare `contains("code=")`.
        if !body.starts_with("code=") && !body.contains("&code=") {
            body = format!("{body}&code=access-contract-matter");
        }
        if !body.contains("attestation=") {
            body = format!("{body}&attestation=1");
        }
    }
    let body_with_csrf = if body.is_empty() {
        format!("_csrf={CSRF}")
    } else {
        format!("{body}&_csrf={CSRF}")
    };
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body_with_csrf))
                .unwrap(),
        )
        .await
        .unwrap();
    capture(world, resp).await;
}

#[when(regex = r#"^"([^"]+)" submits "([^"]*)" to the delete action for "([^"]+)"$"#)]
async fn submit_delete(world: &mut WritesWorld, email: String, body: String, project_name: String) {
    let project_code = world
        .project_codes
        .get(&project_name)
        .expect("project was seeded earlier")
        .clone();
    let cookie = session_cookie_for(world, &email).await;
    let body_with_csrf = if body.is_empty() {
        format!("_csrf={CSRF}")
    } else {
        format!("{body}&_csrf={CSRF}")
    };
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/delete"))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body_with_csrf))
                .unwrap(),
        )
        .await
        .unwrap();
    capture(world, resp).await;
}

/// `POST /app/projects/{code}/people/{role_id}/dri` — the matter-workbench
/// accountability control, exercised by the caller naming their own already-
/// seeded participation row. This is the exact mutation a live "Add Lawyer
/// DRI" hit the `project.brand` coercion bug on (#1093-adjacent): it writes
/// the Project row itself (`UPDATE $project SET updated_at = $now`) before
/// touching the participation row, so any project row missing `brand`
/// tripped it regardless of who was asking.
#[when(regex = r#"^"([^"]+)" designates themselves as lawyer DRI on "([^"]+)"$"#)]
async fn designate_self_as_lawyer_dri(
    world: &mut WritesWorld,
    email: String,
    project_name: String,
) {
    let project_code = world.project_code(&project_name).to_owned();
    let role_id = *world
        .participation_role_ids
        .get(&format!("{project_name}:{email}"))
        .expect("participant was seeded earlier");
    let cookie = session_cookie_for(world, &email).await;
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/people/{role_id}/dri"))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={CSRF}")))
                .unwrap(),
        )
        .await
        .unwrap();
    capture(world, resp).await;
}

#[when(regex = r#"^"([^"]+)" opens the edit page for "([^"]+)"$"#)]
async fn open_edit(world: &mut WritesWorld, email: String, project_name: String) {
    let project_code = world
        .project_codes
        .get(&project_name)
        .expect("project was seeded earlier")
        .clone();
    // One path for every tier — the edit form's own gate decides what comes
    // back, so the step reads the same for a lawyer and a client.
    get_path(world, &email, &format!("/app/projects/{project_code}/edit")).await;
}

#[when(regex = r#"^"([^"]+)" opens the detail page for "([^"]+)"$"#)]
async fn open_detail(world: &mut WritesWorld, email: String, project_name: String) {
    let project_code = world.project_code(&project_name).to_owned();
    get_path(world, &email, &format!("/app/projects/{project_code}")).await;
}

async fn get_path(world: &mut WritesWorld, email: &str, path: &str) {
    let cookie = session_cookie_for(world, email).await;
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
    capture(world, resp).await;
}

async fn session_cookie_for(world: &mut WritesWorld, email: &str) -> String {
    let person_id = *world.persons.get(email).expect("actor seeded");
    let role = role_for(&features::shared_surreal().await, person_id).await;
    let session = SessionData {
        sub: format!("rauthy-{email}-subject"),
        email: Some(email.to_string()),
        person_id: Some(person_id),
        exp: portal::session::now_unix_secs() + 60,
        role,
        csrf_token: CSRF.into(),
        source: portal::session::SessionSource::Browser,
        provider: None,
        impersonation: None,
        scope: None,
    };
    format!(
        "{SESSION_COOKIE_NAME}={}",
        world.sessions().encode(&session)
    )
}

async fn role_for(surreal: &store::surreal::SurrealDb, person_id: Uuid) -> store::persons::Role {
    store::persons::find_by_id(surreal, person_id)
        .await
        .expect("query")
        .expect("row exists")
        .role
}

async fn capture(world: &mut WritesWorld, resp: axum::http::Response<Body>) {
    world.last_status = Some(resp.status());
    world.last_location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    world.last_body = body_string(resp).await;
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut WritesWorld, code: u16) {
    let actual = world.last_status.expect("no response captured");
    assert_eq!(
        actual.as_u16(),
        code,
        "expected {code}, got {actual} (body: {})",
        truncate(&world.last_body),
    );
}

#[then(regex = r#"^the response body contains "([^"]+)"$"#)]
async fn body_contains(world: &mut WritesWorld, needle: String) {
    let needle = needle.replace("\\\"", "\"");
    assert!(
        world.last_body.contains(&needle),
        "body did not contain {needle:?}; body: {}",
        truncate(&world.last_body),
    );
}

#[then(regex = r#"^the response body does not contain "([^"]+)"$"#)]
async fn body_does_not_contain(world: &mut WritesWorld, needle: String) {
    let needle = needle.replace("\\\"", "\"");
    assert!(
        !world.last_body.contains(&needle),
        "body unexpectedly contained {needle:?}; body: {}",
        truncate(&world.last_body),
    );
}

#[then(regex = r#"^the response location contains "([^"]+)"$"#)]
async fn location_contains(world: &mut WritesWorld, needle: String) {
    let loc = world
        .last_location
        .as_deref()
        .expect("no Location header on response");
    assert!(
        loc.contains(&needle),
        "expected Location to contain {needle:?}, got {loc:?}",
    );
}

/// The regression guard for the edit form's save action: it must post to the
/// matter's *code* — the segment `POST /app/projects/{project_code}` actually
/// matches — never to the row's raw id, which 404s the save and strands the
/// lawyer on an id-shaped URL instead of saving anything.
#[then(regex = r#"^the response body posts its save to "([^"]+)"$"#)]
async fn body_posts_save_to(world: &mut WritesWorld, project_name: String) {
    let code = world.project_code(&project_name).to_owned();
    let id = *world
        .projects
        .get(&project_name)
        .expect("project was seeded earlier");
    let wanted = format!(r#"action="/app/projects/{code}""#);
    assert!(
        world.last_body.contains(&wanted),
        "expected the form to post to {wanted:?}; body: {}",
        truncate(&world.last_body),
    );
    let by_id = format!(r#"action="/app/projects/{id}""#);
    assert!(
        !world.last_body.contains(&by_id),
        "the form posted to the row id instead of the matter code: {}",
        truncate(&world.last_body),
    );
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
    WritesWorld::cucumber()
        // Every scenario seeds the canonical template catalog into the one
        // embedded SurrealDB this Cucumber process shares. The seed is
        // idempotent after a write completes, but concurrent first writes
        // race on the current-template index.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/portal_projects_writes.feature")
        .await;
}
