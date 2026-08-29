#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/clauses` — append a custom
//! clause to a notation's document.
//!
//! The store command (`store::notation_clauses::append`) is the one the lawyer
//! clause form already drives, so these tests focus on the REST adapter: the
//! tier gate (LawyerSession → 401/403), the matter-scope gate (a non-participant
//! lawyer gets a bare 404, admin bypasses), validation (blank body → 400), and a
//! live 201 that returns the appended clause id.

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

const TEMPLATE_CODE: &str = "onboarding__engagement_letter";
const KEY: &str = "api-notation-clauses-test-key";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-clauses-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
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
    let mut s = SessionData::fresh("api-clause-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Insert a notation on `project` bound to a fresh client; returns its id.
async fn seed_notation(surreal: &store::surreal::SurrealDb, project_id: uuid::Uuid) -> uuid::Uuid {
    let client = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "libra@example.com",
            "libra@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let tmpl = store::templates::resolve(surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .unwrap();
    store::notations::create(
        surreal,
        &store::notations::NewNotation::new(tmpl.id, client.id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id
}

async fn post_clause(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
    body: &str,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/clauses"))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(
            req.body(Body::from(serde_json::json!({ "body": body }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;

    let resp = post_clause(&app, None, notation_id, "A clause.").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let client = bearer(&surreal, "client@example.com", Role::Client, None).await;

    let resp = post_clause(&app, Some(&client), notation_id, "A clause.").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let outsider = bearer(&surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post_clause(&app, Some(&outsider), notation_id, "A clause.").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_blank_body_is_400() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let lawyer = bearer(
        &surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_clause(&app, Some(&lawyer), notation_id, "   ").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn participant_lawyer_appends_a_clause() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let lawyer = bearer(
        &surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let resp = post_clause(
        &app,
        Some(&lawyer),
        notation_id,
        "Binding arbitration in Clark County.",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let clause_id = json["clause_id"].as_str().expect("a clause_id");
    // The clause is now attached to the notation.
    let clauses = store::notation_clauses::for_notation(&surreal, notation_id)
        .await
        .unwrap();
    assert_eq!(clauses.len(), 1);
    assert_eq!(clauses[0].id.to_string(), clause_id);
}

#[tokio::test]
async fn admin_bypasses_scope_and_appends() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let admin = bearer(&surreal, "admin@example.com", Role::Admin, None).await;

    let resp = post_clause(&app, Some(&admin), notation_id, "A clause.").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}
