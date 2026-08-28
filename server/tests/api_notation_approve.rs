#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/approval` — the REST door
//! that approves a notation parked at `lawyer_review` (fires `approved`, parks
//! at the generate-PDF step).
//!
//! The command core (`render_and_park`) is shared with the lawyer review screen
//! (covered in `retainer_walk_handler.rs`). These tests cover what the REST
//! adapter adds: the tier gate (LawyerSession → 401/403), the matter-scope gate
//! (a lawyer who does not participate in the notation's matter gets a bare 404,
//! admin bypasses), and a live 200 advancing a `lawyer_review` notation. The
//! fixture uses the shared `advance_to_lawyer_review` driver to reach the gate.

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

/// A seeded catalog retainer — has a post-questionnaire workflow spec, so
/// `advance_to_lawyer_review` can drive a fresh notation to the gate.
const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "api-notation-approve-test-key";

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    runtime: Arc<dyn StateMachineRuntime>,
}

async fn build_app() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-approve-storage"))
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
    let mut s = SessionData::fresh("api-approve-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Insert a retainer notation on `project` bound to a fresh client, then drive
/// it to the `lawyer_review` gate via the shared workflow driver.
async fn seed_lawyer_review_notation(h: &Harness, project_id: uuid::Uuid) -> uuid::Uuid {
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
        .expect("seed_canonical inserts onboarding__letter");
    let notation_id = store::notations::create(
        &h.surreal,
        &store::notations::NewNotation::new(tmpl.id, client.id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    portal::retainer_walk::advance_to_lawyer_review(
        &h.surreal,
        h.runtime.as_ref(),
        notation_id,
        None,
    )
    .await
    .expect("drives to lawyer_review");
    // Precondition: the fixture parked the notation at the lawyer_review gate.
    let row = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "lawyer_review",
        "fixture should park at lawyer_review"
    );
    notation_id
}

async fn post_approval(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/approval"))
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
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;

    let resp = post_approval(&h.app, None, notation_id).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let client = bearer(&h.surreal, "client@example.com", Role::Client, None).await;

    let resp = post_approval(&h.app, Some(&client), notation_id).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post_approval(&h.app, Some(&outsider), notation_id).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn participant_lawyer_approves_and_advances_past_lawyer_review() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_approval(&h.app, Some(&lawyer), notation_id).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["notation_id"], notation_id.to_string());
    // `approved` moves the notation off the lawyer_review gate to the
    // generate-PDF step — the door reports the new state.
    let state = json["state"].as_str().expect("a state");
    assert_ne!(state, "lawyer_review", "approval advances past the gate");
    assert!(
        state.starts_with("generate_pdf"),
        "expected a generate_pdf state, got `{state}`"
    );
}

#[tokio::test]
async fn admin_bypasses_scope_and_approves() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let admin = bearer(&h.surreal, "admin@example.com", Role::Admin, None).await;

    let resp = post_approval(&h.app, Some(&admin), notation_id).await;
    assert_eq!(resp.status(), StatusCode::OK);
}
