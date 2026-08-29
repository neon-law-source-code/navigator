#![allow(clippy::doc_markdown)]
//! Integration tests for editing, removing, and reordering a notation clause:
//! `PATCH`/`DELETE /app/api/notations/{id}/clauses/{clause_id}` and
//! `POST .../move`.
//!
//! The store commands (`update_body`/`delete`/`move_clause`) are the ones the
//! lawyer clause form already drives. These tests cover the REST adapter: the
//! tier gate (401/403), the matter-scope gate + the clause-belongs-to-notation
//! guard (both → 404), blank-body 400, and the live 200/204 outcomes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;

const TEMPLATE_CODE: &str = "onboarding__engagement_letter";
const KEY: &str = "api-clause-edits-test-key";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-clause-edits-storage"))
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
    let mut s = SessionData::fresh("api-clause-edit-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

async fn seed_notation(surreal: &store::surreal::SurrealDb, project_id: uuid::Uuid) -> uuid::Uuid {
    let email = format!("client-{}@example.com", uuid::Uuid::now_v7());
    let client = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email.clone(), email, Role::Client),
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

async fn append_clause(
    surreal: &store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
    body: &str,
) -> uuid::Uuid {
    store::notation_clauses::append(surreal, notation_id, body, None)
        .await
        .unwrap()
}

fn req(
    method: &str,
    uri: String,
    auth: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    b = b.header("content-type", "application/json");
    let body = body.map_or(Body::empty(), |v| Body::from(v.to_string()));
    b.body(body).unwrap()
}

async fn send(app: &axum::Router, r: Request<Body>) -> axum::http::Response<Body> {
    app.clone().oneshot(r).await.unwrap()
}

#[tokio::test]
async fn edit_authz_matrix_and_success() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let clause_id = append_clause(&surreal, notation_id, "Original.").await;
    let uri = format!("/app/api/notations/{notation_id}/clauses/{clause_id}");

    // anon → 401
    let r = send(
        &app,
        req(
            "PATCH",
            uri.clone(),
            None,
            Some(serde_json::json!({"body": "New."})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // client → 403
    let client = bearer(&surreal, "client@example.com", Role::Client, None).await;
    let r = send(
        &app,
        req(
            "PATCH",
            uri.clone(),
            Some(&client),
            Some(serde_json::json!({"body": "New."})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // non-participant lawyer → 404
    let outsider = bearer(&surreal, "outsider@example.com", Role::Lawyer, None).await;
    let r = send(
        &app,
        req(
            "PATCH",
            uri.clone(),
            Some(&outsider),
            Some(serde_json::json!({"body": "New."})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // participant lawyer blank body → 400
    let lawyer = bearer(
        &surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;
    let r = send(
        &app,
        req(
            "PATCH",
            uri.clone(),
            Some(&lawyer),
            Some(serde_json::json!({"body": "  "})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // participant lawyer edits → 200, body changed
    let r = send(
        &app,
        req(
            "PATCH",
            uri.clone(),
            Some(&lawyer),
            Some(serde_json::json!({"body": "Rewritten."})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let clauses = store::notation_clauses::for_notation(&surreal, notation_id)
        .await
        .unwrap();
    assert_eq!(clauses[0].body_markdown, "Rewritten.");
}

#[tokio::test]
async fn a_clause_from_another_notation_is_404() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let other_notation = seed_notation(&surreal, project.id).await;
    let other_clause = append_clause(&surreal, other_notation, "Elsewhere.").await;
    let lawyer = bearer(
        &surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    // The clause exists and the caller is in scope, but the clause belongs to a
    // different notation than the path names → 404.
    let uri = format!("/app/api/notations/{notation_id}/clauses/{other_clause}");
    let r = send(&app, req("DELETE", uri, Some(&lawyer), None)).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // It was not deleted.
    let clauses = store::notation_clauses::for_notation(&surreal, other_notation)
        .await
        .unwrap();
    assert_eq!(clauses.len(), 1);
}

#[tokio::test]
async fn participant_lawyer_deletes_a_clause() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let clause_id = append_clause(&surreal, notation_id, "To remove.").await;
    let lawyer = bearer(
        &surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let uri = format!("/app/api/notations/{notation_id}/clauses/{clause_id}");
    let r = send(&app, req("DELETE", uri, Some(&lawyer), None)).await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert!(store::notation_clauses::for_notation(&surreal, notation_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn participant_lawyer_moves_a_clause() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let notation_id = seed_notation(&surreal, project.id).await;
    let first = append_clause(&surreal, notation_id, "First.").await;
    let second = append_clause(&surreal, notation_id, "Second.").await;
    let lawyer = bearer(
        &surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    // Move the second clause up → it becomes first.
    let uri = format!("/app/api/notations/{notation_id}/clauses/{second}/move");
    let r = send(
        &app,
        req(
            "POST",
            uri,
            Some(&lawyer),
            Some(serde_json::json!({"direction": "up"})),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);

    let clauses = store::notation_clauses::for_notation(&surreal, notation_id)
        .await
        .unwrap();
    assert_eq!(clauses[0].id, second);
    assert_eq!(clauses[1].id, first);
}
