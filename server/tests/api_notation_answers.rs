#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/answers` — the REST door
//! that answers a notation's current questionnaire step.
//!
//! The write engine (`workflows::answer_step_with_reference`) is shared with
//! the lawyer retainer walk, so these tests focus on what the REST adapter
//! adds: the tier gate (LawyerSession → 401/403), the matter-scope gate (a
//! lawyer who does not participate in the notation's matter gets a bare 404,
//! admin bypasses), the out-of-order-step contract (409 question_mismatch),
//! and a live 200 proving the step JSON.

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
use workflows::{InMemoryRuntime, NextStep, StartOutcome, StateMachineRuntime};

const VALID_TEMPLATE: &str = include_str!("../../templates/neon_law/shared/engagement_letter.md");
const KEY: &str = "api-notation-answers-test-key";

// Every case configures the process-wide repo root and commits through the
// same filesystem-backed test forge. Keep that environment/filesystem seam
// single-owner while each request is exercised.
static REPO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn repo_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("navigator-api-notation-answers-repos");
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
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-answers-storage"))
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

/// A `Bearer` header for a session of `role`, optionally a firm-side
/// participant of `project`.
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
    let mut s = SessionData::fresh("api-answer-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Open a notation on `project` (committing its template first) and return its
/// id plus the code of the first question the questionnaire asks. Opened as an
/// admin so it needs no participation seeding — the tests scope the *answer*.
async fn open_notation(h: &Harness, project: &store::projects::Project) -> (uuid::Uuid, String) {
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
    let StartOutcome { notation_id, next } = portal::project_notation::create_project_notation(
        &h.surreal,
        h.runtime.as_ref(),
        &h.storage,
        Some(admin.id),
        Role::Admin,
        project.id,
        "amendment",
        "libra@example.com",
    )
    .await
    .expect("notation opens");
    let code = match next {
        NextStep::NeedsAnswer { question } => question.code,
        NextStep::QuestionnaireComplete => panic!("expected a first question"),
    };
    (notation_id, code)
}

async fn post_answer(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
    question_code: &str,
    value: &str,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/answers"))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(
            req.body(Body::from(
                serde_json::json!({ "question_code": question_code, "value": value }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, code) = open_notation(&h, &project).await;

    let resp = post_answer(&h.app, None, notation_id, &code, "Ada Lovelace").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, code) = open_notation(&h, &project).await;
    let client = bearer(&h.surreal, "client@example.com", Role::Client, None).await;

    let resp = post_answer(&h.app, Some(&client), notation_id, &code, "Ada Lovelace").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, code) = open_notation(&h, &project).await;
    // Lawyer tier, but not a participant of this notation's matter.
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post_answer(&h.app, Some(&outsider), notation_id, &code, "Ada Lovelace").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn participant_lawyer_answers_the_step_and_gets_the_next() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, code) = open_notation(&h, &project).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_answer(&h.app, Some(&lawyer), notation_id, &code, "Ada Lovelace").await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The body is a discriminated NotationStep: either the next question, or complete.
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let status = json["status"].as_str().expect("a status discriminator");
    assert!(
        status == "needs_answer" || status == "complete",
        "unexpected status `{status}`"
    );
    if status == "needs_answer" {
        assert!(
            json["question"]["code"].as_str().is_some(),
            "needs_answer carries the next question"
        );
    }
}

#[tokio::test]
async fn admin_bypasses_scope_and_answers() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, code) = open_notation(&h, &project).await;
    // Admin does NOT participate; the scope check bypasses for admin.
    let admin = bearer(&h.surreal, "admin@example.com", Role::Admin, None).await;

    let resp = post_answer(&h.app, Some(&admin), notation_id, &code, "Ada Lovelace").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn out_of_order_question_code_is_409() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let (notation_id, _code) = open_notation(&h, &project).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    // A code the questionnaire is not currently asking → 409 question_mismatch.
    let resp = post_answer(
        &h.app,
        Some(&lawyer),
        notation_id,
        "definitely_not_the_current_step",
        "whatever",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "question_mismatch");
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
