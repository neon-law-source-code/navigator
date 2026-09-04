#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/intake` — the REST door
//! that emails a notation's client their self-serve intake link.
//!
//! The command (`send_intake`) is shared with the lawyer form. These tests
//! cover what the REST adapter adds: the tier gate (LawyerSession → 401/403),
//! the matter-scope gate (a lawyer who does not participate in the notation's
//! matter gets a bare 404, admin bypasses), and a live 200 reporting the
//! recipient. The test email service is a capturing stub, so a dispatch never
//! fails the command.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{InMemoryRuntime, StateMachineRuntime};

const VALID_TEMPLATE: &str =
    include_str!("../../templates/notations/neon_law/shared/onboarding_letter.md");
const KEY: &str = "api-notation-intake-test-key";
const CLIENT_EMAIL: &str = "libra@example.com";

// These tests mutate one process-wide repository root and invoke git in that
// shared directory. LLVM-instrumented macOS test processes can otherwise race
// over inherited file descriptors, surfacing an intermittent EBADF.
static REPO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn repo_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("navigator-api-notation-intake-repos");
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &root);
    root
}

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    runtime: Arc<dyn StateMachineRuntime>,
    storage: Arc<dyn cloud::StorageService>,
}

async fn build_app() -> Harness {
    repo_root();
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-intake-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Harness {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        runtime,
        storage,
    }
}

async fn open_project(surreal: &store::surreal::SurrealDb) -> store::projects::Project {
    store::test_support::seed_project(surreal, "Matter").await
}

async fn bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: Role,
    project: Option<uuid::Uuid>,
) -> String {
    let actor = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    if let Some(project_id) = project {
        store::projects::add_participation(surreal, project_id, actor.id, "lawyer")
            .await
            .unwrap();
    }
    let mut s = SessionData::fresh("api-intake-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Open a notation on `project` bound to `CLIENT_EMAIL`, returning its id.
/// Opened as an admin so it needs no participation seeding.
async fn open_notation(h: &Harness, project: &store::projects::Project) -> uuid::Uuid {
    commit_template(&project.code, "amendment", VALID_TEMPLATE.as_bytes());
    let admin = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::with_role(
            "opener@example.com",
            "opener@example.com",
            Role::Admin,
        ),
    )
    .await
    .unwrap();
    portal::project_notation::create_project_notation(
        &h.surreal,
        h.runtime.as_ref(),
        &h.storage,
        Some(admin.id),
        Role::Admin,
        project.id,
        "amendment",
        CLIENT_EMAIL,
    )
    .await
    .expect("notation opens")
    .notation_id
}

async fn post_intake(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/intake"))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = open_notation(&h, &project).await;

    let resp = post_intake(&h.app, None, notation_id).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = open_notation(&h, &project).await;
    let client = bearer(&h.surreal, "client@example.com", Role::Client, None).await;

    let resp = post_intake(&h.app, Some(&client), notation_id).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = open_notation(&h, &project).await;
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post_intake(&h.app, Some(&outsider), notation_id).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn participant_lawyer_dispatches_the_intake_and_reports_the_recipient() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = open_notation(&h, &project).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_intake(&h.app, Some(&lawyer), notation_id).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["notation_id"], notation_id.to_string());
    assert_eq!(json["recipient"], CLIENT_EMAIL);
}

#[tokio::test]
async fn admin_bypasses_scope_and_dispatches() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = open_notation(&h, &project).await;
    let admin = bearer(&h.surreal, "admin@example.com", Role::Admin, None).await;

    let resp = post_intake(&h.app, Some(&admin), notation_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Commit `templates/<code>.md` to the project's repo and return HEAD.
fn commit_template(project_code: &str, code: &str, body: &[u8]) -> String {
    let repo = repos::RepoStore::from_env().unwrap();
    repo.ensure_code(project_code).unwrap();
    repo.commit_as_code(
        project_code,
        repos::Author {
            name: "Lawyer",
            email: "lawyer@example.com",
        },
        "add template",
        &[(&format!("templates/{code}.md"), body)],
    )
    .unwrap();
    repo.head_oid_code(project_code).unwrap().unwrap()
}
