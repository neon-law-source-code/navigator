#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/review-documents/{id}/comments` — the
//! first client-writable `/api` door.
//!
//! The command (`create_review_comment`) is shared with the browser review
//! form. These tests cover the door's distinctive authz: it admits any
//! authenticated caller (401 only for anon), then enforces **client-lens**
//! matter scope — so a matter's client-side participant can comment (201),
//! while a firm-side-only lawyer and a non-participant both get a bare 404,
//! exactly as on the read-only review surface. Plus validation (400) and the
//! draft-hidden rule.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::review_documents::{STATUS_DRAFT, STATUS_PENDING_REVIEW};
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;

const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "api-review-comments-test-key";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-review-comments-storage"))
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

/// Seed a person of `role`, optionally with a `participation` row on `project`,
/// and return `(person_id, bearer_header)`. Pass `participation: "client"` for
/// a client-lens participant, `"lawyer"` for a firm-side one.
async fn actor(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: Role,
    project: Option<(uuid::Uuid, &str)>,
) -> (uuid::Uuid, String) {
    let p = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    if let Some((project_id, participation)) = project {
        store::projects::add_participation(surreal, project_id, p.id, participation)
            .await
            .unwrap();
    }
    let mut s = SessionData::fresh("api-review-sub", role);
    s.person_id = Some(p.id);
    (
        p.id,
        format!("Bearer {}", SessionStore::new(KEY).encode(&s)),
    )
}

/// Insert a review document on a notation in `project`, at `status`.
async fn seed_review_document(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
    status: &str,
) -> uuid::Uuid {
    let tmpl = store::templates::resolve(surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .unwrap();
    let author = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "doc-owner@example.com",
            "doc-owner@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let notation_id = store::notations::create(
        surreal,
        &store::notations::NewNotation::new(tmpl.id, author.id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    let doc_id = store::review_documents::create(
        surreal,
        &store::review_documents::NewReviewDocument {
            notation_id,
            kind: "will",
            title: "Draft",
            body_html: "<p>Body under review.</p>",
        },
    )
    .await
    .unwrap();
    // review_documents::create always inserts a draft; advance it when the test
    // needs a client-visible (non-draft) document.
    if status != STATUS_DRAFT {
        store::review_documents::set_status(surreal, doc_id, status)
            .await
            .unwrap();
    }
    doc_id
}

async fn post_comment(
    app: &axum::Router,
    auth: Option<&str>,
    doc_id: uuid::Uuid,
    body: &str,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/review-documents/{doc_id}/comments"))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(
            req.body(Body::from(
                serde_json::json!({
                    "anchor_start": 3,
                    "anchor_end": 12,
                    "quoted_text": "the clause",
                    "body": body
                })
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let doc_id = seed_review_document(&surreal, project.id, STATUS_PENDING_REVIEW).await;

    let resp = post_comment(&app, None, doc_id, "hi").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_participant_creates_a_comment() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let doc_id = seed_review_document(&surreal, project.id, STATUS_PENDING_REVIEW).await;
    let (_pid, client) = actor(
        &surreal,
        "client@example.com",
        Role::Client,
        Some((project.id, "client")),
    )
    .await;

    let resp = post_comment(&app, Some(&client), doc_id, "Can we cap this?").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(json["comment_id"].as_str().is_some());
    assert!(json["communication_id"].as_str().is_some());
}

#[tokio::test]
async fn firm_side_lawyer_is_404_client_lens_gate() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let doc_id = seed_review_document(&surreal, project.id, STATUS_PENDING_REVIEW).await;
    // Lawyer tier, firm-side participation on the matter — NOT client-lens. The
    // review surface is client-lens, so this lawyer sees 404 exactly as on the
    // portal (firm-side lawyer comment through a different surface).
    let (_pid, lawyer) = actor(
        &surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some((project.id, "lawyer")),
    )
    .await;

    let resp = post_comment(&app, Some(&lawyer), doc_id, "note").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_participant_is_404() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let doc_id = seed_review_document(&surreal, project.id, STATUS_PENDING_REVIEW).await;
    // A client with no participation on this matter.
    let (_pid, outsider) = actor(&surreal, "outsider@example.com", Role::Client, None).await;

    let resp = post_comment(&app, Some(&outsider), doc_id, "note").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_draft_document_is_never_disclosed() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    // Draft status → hidden from the client review surface.
    let doc_id = seed_review_document(&surreal, project.id, STATUS_DRAFT).await;
    let (_pid, client) = actor(
        &surreal,
        "client@example.com",
        Role::Client,
        Some((project.id, "client")),
    )
    .await;

    let resp = post_comment(&app, Some(&client), doc_id, "note").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_blank_body_is_400() {
    let (app, surreal) = build_app().await;
    let project = open_project(&surreal).await;
    let doc_id = seed_review_document(&surreal, project.id, STATUS_PENDING_REVIEW).await;
    let (_pid, client) = actor(
        &surreal,
        "client@example.com",
        Role::Client,
        Some((project.id, "client")),
    )
    .await;

    let resp = post_comment(&app, Some(&client), doc_id, "   ").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
