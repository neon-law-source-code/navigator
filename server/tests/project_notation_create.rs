#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/projects/:project_code/notations/new` —
//! the project-scoped `notation create` front door (issue #252, slice 2).
//!
//! It reads a template from the Project's git repo through the shared
//! `read-repo → validate → persist` engine, auto-saves an immutable
//! project-scoped version, and opens a notation pinned to it — proving the
//! whole HTTP path, not just the engine unit-tested in `store`/`workflows`.
//! Creation is matter-scoped, so the acting lawyer must participate in the
//! project (admin bypasses); the tests authenticate with a minted session.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{InMemoryRuntime, StateMachineRuntime};

/// A corpus template body — guaranteed to validate clean (CI checks it).
const VALID_TEMPLATE: &str = include_str!("../../templates/neon_law/shared/letter.md");

/// Signing key shared by the app's `SessionStore` and the bearers the tests
/// mint, so `inject_bearer_session` decodes them.
const KEY: &str = "project-notation-create-test-key";

// These tests mutate one process-wide repository root and invoke git in that
// shared directory. LLVM-instrumented macOS test processes can otherwise race
// over inherited file descriptors, surfacing an intermittent EBADF.
static REPO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One stable repo root for every test in this binary, so the handler's
/// `RepoStore::from_env()` reads the same root regardless of which test set
/// it (each test isolates by its own unique `project_id` sub-repo).
fn repo_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("navigator-project-notation-create-repos");
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &root);
    root
}

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    repo_root();
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-project-notation-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

async fn open_project(surreal: &store::surreal::SurrealDb) -> store::projects::Project {
    store::test_support::seed_project(surreal, "Matter").await
}

/// A `Bearer` header for a lawyer session, optionally scoped to `project`.
async fn lawyer_bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    project: Option<uuid::Uuid>,
) -> String {
    let lawyer = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, Role::Lawyer),
    )
    .await
    .unwrap();
    if let Some(project_id) = project {
        store::projects::add_participation(surreal, project_id, lawyer.id, "lawyer")
            .await
            .unwrap();
    }
    let mut s = SessionData::fresh("lawyer-sub", Role::Lawyer);
    s.person_id = Some(lawyer.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

async fn post_new(
    app: &axum::Router,
    bearer: &str,
    project_code: &str,
    code: &str,
    email: &str,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/notations/new"))
                .header("authorization", bearer)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "template_code={code}&client_email={email}"
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn creates_a_project_scoped_notation_from_the_repo_template() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let project_id = project.id;
    let head = commit_template(&project.code, "amendment", VALID_TEMPLATE.as_bytes());
    // Acting lawyer participates in the matter.
    let bearer = lawyer_bearer(&surreal, "acting-lawyer@example.com", Some(project_id)).await;

    let resp = post_new(
        &app,
        &bearer,
        &project.code,
        "amendment",
        "libra@example.com",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "expected a 303 redirect"
    );

    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.starts_with("/lawyer/notations/") && location.ends_with("/step"),
        "redirect to the step page, got `{location}`"
    );

    // The notation pinned the just-saved project-scoped version, whose
    // provenance is the repo commit it was read from.
    let notation = store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("a notation was created for the project");
    let pinned = store::templates::find_by_id(&surreal, notation.template_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pinned.code, "amendment");
    assert_eq!(pinned.project_id, Some(project_id));
    assert_eq!(pinned.source_commit_sha.as_deref(), Some(head.as_str()));

    // The client Person was created and is distinct from the acting lawyer.
    let person = store::persons::find_by_email_ci(&surreal, "libra@example.com")
        .await
        .unwrap()
        .expect("client person created");
    assert_eq!(notation.person_id, person.id);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_refused() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let project_id = project.id;
    commit_template(&project.code, "amendment", VALID_TEMPLATE.as_bytes());
    // Acting lawyer does NOT participate in this project.
    let bearer = lawyer_bearer(&surreal, "outsider@example.com", None).await;

    let resp = post_new(
        &app,
        &bearer,
        &project.code,
        "amendment",
        "libra@example.com",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "out-of-scope lawyer");

    // Nothing was created.
    assert!(store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unknown_project_is_404() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let bearer = lawyer_bearer(&surreal, "acting-lawyer@example.com", None).await;
    let resp = post_new(
        &app,
        &bearer,
        "no-such-matter",
        "amendment",
        "libra@example.com",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_invalid_repo_template_is_refused_with_422() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let project_id = project.id;
    // A line past the 120-char limit trips S101 (Error-severity).
    let invalid = format!("# Bad\n\n{}\n", "x".repeat(200));
    commit_template(&project.code, "bad", invalid.as_bytes());
    let bearer = lawyer_bearer(&surreal, "acting-lawyer@example.com", Some(project_id)).await;

    let resp = post_new(&app, &bearer, &project.code, "bad", "libra@example.com").await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Nothing was persisted for the bad code.
    assert!(store::templates::resolve(&surreal, Some(project_id), "bad")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn first_notation_on_a_matter_must_be_the_engagement_that_opens_it() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let project_id = project.id;
    let bearer = lawyer_bearer(&surreal, "acting-lawyer@example.com", Some(project_id)).await;

    // A filing as the matter's very first notation is refused — the
    // engagement opens the matter. `nv__annual_report` is seeded (kind:
    // filing) and resolves from the bundled catalog, not this fresh matter's
    // repo.
    let resp = post_new(
        &app,
        &bearer,
        &project.code,
        "nv__annual_report",
        "libra@example.com",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a non-retainer first notation is refused"
    );
    assert!(
        store::notations::list_by_project(&surreal, project_id)
            .await
            .unwrap()
            .is_empty(),
        "no notation was created",
    );
}

#[tokio::test]
async fn retainer_opens_as_the_first_notation_from_the_bundled_catalog() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let project_id = project.id;
    let bearer = lawyer_bearer(&surreal, "acting-lawyer@example.com", Some(project_id)).await;

    // The retainer isn't authored in this fresh matter's repo — it resolves
    // from the bundled firm catalog (proving the repo→catalog fallback) and
    // passes the engagement-first gate.
    let resp = post_new(
        &app,
        &bearer,
        &project.code,
        "onboarding__letter",
        "libra@example.com",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "the retainer opens as the matter's first notation"
    );
    assert!(
        !store::notations::list_by_project(&surreal, project_id)
            .await
            .unwrap()
            .is_empty(),
        "a notation was created",
    );
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
