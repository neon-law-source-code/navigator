#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/release-drafts` — the
//! attorney gate that releases an estate notation's drafts to client review.
//!
//! The command (`crate::estate::release_drafts`) is shared with the lawyer form.
//! These tests cover the tier gate (LawyerSession → 401/403), the matter-scope
//! gate (a non-participant lawyer gets a bare 404, admin bypasses), the
//! not-at-the-gate conflict (a notation with no `approved` edge → 409), and a
//! live 200 releasing a notation driven to `lawyer_review`.

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

/// The `/app/api/notations/{id}/release-drafts` door is a generic
/// `lawyer_review`-gate transition (the estate matter is its main user, but the
/// command fires the same `approved` signal + draft flip for any notation at
/// the gate). These tests exercise the door's plumbing — auth, matter scope,
/// and the gate transition — with a retainer, whose workflow
/// `advance_to_lawyer_review` can drive to `lawyer_review`; the estate-specific
/// draft-release semantics stay covered by the estate handler's own tests.
const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "api-release-drafts-test-key";

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    runtime: Arc<dyn StateMachineRuntime>,
}

async fn build_app() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-release-drafts-storage"))
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
    let mut s = SessionData::fresh("api-release-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Insert a notation on `project` bound to a fresh client. Returns its id; the
/// notation is at `BEGIN` (its workflow has not been started).
async fn seed_notation(h: &Harness, project_id: uuid::Uuid) -> uuid::Uuid {
    let client = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::with_role(
            "libra@example.com",
            "libra@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let tmpl = store::templates::resolve(&h.surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed_canonical inserts the catalog retainer");
    store::notations::create(
        &h.surreal,
        &store::notations::NewNotation::new(tmpl.id, client.id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id
}

async fn post_release(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/release-drafts"))
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
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_notation(&h, project.id).await;

    let resp = post_release(&h.app, None, notation_id).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_notation(&h, project.id).await;
    let client = bearer(&h.surreal, "client@example.com", Role::Client, None).await;

    let resp = post_release(&h.app, Some(&client), notation_id).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_notation(&h, project.id).await;
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post_release(&h.app, Some(&outsider), notation_id).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_notation_not_at_the_review_gate_is_409() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    // Fresh notation at BEGIN — its workflow was never driven to lawyer_review,
    // so there is no `approved` edge to fire.
    let notation_id = seed_notation(&h, project.id).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_release(&h.app, Some(&lawyer), notation_id).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["error"], "not_at_review_gate");
}

#[tokio::test]
async fn participant_lawyer_releases_drafts_from_the_gate() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_notation(&h, project.id).await;
    // Drive the workflow to the lawyer_review gate.
    portal::retainer_walk::advance_to_lawyer_review(
        &h.surreal,
        h.runtime.as_ref(),
        notation_id,
        None,
    )
    .await
    .expect("notation drives to lawyer_review");
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_release(&h.app, Some(&lawyer), notation_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["notation_id"], notation_id.to_string());
    // The gate advances the notation off lawyer_review.
    assert_ne!(json["state"].as_str().unwrap(), "lawyer_review");
}
