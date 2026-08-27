#![allow(clippy::too_many_lines)]
//! Visibility gate for the direct per-document routes
//! (`GET /app/projects/:project_code/documents/:doc_id` and `.../download`).
//!
//! #782: the client matter-detail listing and the "download all my
//! documents" ZIP are gated on `assets.visibility`, but a client could
//! still reach an *internal* asset on their own matter by its `doc_id` —
//! the detail page leaked its provenance and the download handed back its
//! bytes. Both handlers resolve the row through `load_doc_for_project`,
//! which now 404s an internal asset under the client lens while leaving
//! the lawyer lens unfiltered. These tests pin that split.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::documents::{source, IngestArgs};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";

struct Fixture {
    app: axum::Router,
    project_code: String,
    client_cookie: String,
    admin_cookie: String,
    client_doc_id: Uuid,
    internal_doc_id: Uuid,
}

/// Build the app with a scoped client participant, an admin, and two
/// filed documents on the matter: a client-visible `will.pdf` and an
/// internal `review-memo.pdf` (the attorney-work-product shape).
async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("nav-docacl-{}", Uuid::now_v7())))
            .await
            .unwrap(),
    );

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let proj = store::test_support::seed_project(&surreal, "Libra estate plan").await;
    store::projects::add_participation(&surreal, proj.id, libra.id, "client")
        .await
        .unwrap();

    for (filename, kind, bytes, visibility) in [
        (
            "will.pdf",
            "unclassified",
            b"the last will and testament".as_slice(),
            store::documents::visibility::CLIENT,
        ),
        (
            "review-memo.pdf",
            "memo",
            b"attorney work product".as_slice(),
            store::documents::visibility::INTERNAL,
        ),
    ] {
        let args = IngestArgs {
            project_id: proj.id,
            source: source::UPLOAD,
            filename,
            kind,
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility,
        };
        portal::matter_documents::record_document(
            &surreal,
            &storage,
            repos::Author {
                name: "Lawyer",
                email: "lawyer@example.com",
            },
            &args,
            bytes,
        )
        .await
        .unwrap();
    }

    let assets = store::assets::for_project(&surreal, proj.id).await.unwrap();
    let doc_id = |name: &str| {
        assets
            .iter()
            .find(|a| a.filename.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} asset row"))
            .id
    };
    let client_doc_id = doc_id("will.pdf");
    let internal_doc_id = doc_id("review-memo.pdf");

    let sessions = SessionStore::new(KEY);
    let mut member = SessionData::fresh("libra-sub", Role::Client);
    member.person_id = Some(libra.id);
    let client_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&member));
    // The matter surface scopes every tier by participation now, so the acting
    // admin needs a real person on this matter — a bare id used to ride the
    // bypass. What this test is about is the `internal` visibility split, and
    // the gate itself is pinned in `store::access`.
    let admin_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Documents Admin",
            "documents-admin@neonlaw.com",
            Role::Admin,
        ),
    )
    .await
    .expect("seed the acting admin");
    store::projects::add_participation(&surreal, proj.id, admin_person.id, "attorney")
        .await
        .expect("put the acting admin on the matter");
    let mut admin = SessionData::fresh("lawyer-sub", Role::Admin);
    admin.person_id = Some(admin_person.id);
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&admin));

    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let runtime = Arc::new(workflows::InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    Fixture {
        app,
        project_code: proj.code.clone(),
        client_cookie,
        admin_cookie,
        client_doc_id,
        internal_doc_id,
    }
}

async fn get(app: &axum::Router, uri: String, cookie: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_download_of_an_internal_document_is_404() {
    let f = build_fixture().await;

    // The client-visible document streams back its bytes.
    let resp = get(
        &f.app,
        format!(
            "/app/projects/{}/documents/{}/download",
            f.project_code, f.client_doc_id
        ),
        &f.client_cookie,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), b"the last will and testament");

    // The internal document, requested by its real id through the client
    // lens, is a 404 — its bytes never leave the building.
    let resp = get(
        &f.app,
        format!(
            "/app/projects/{}/documents/{}/download",
            f.project_code, f.internal_doc_id
        ),
        &f.client_cookie,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "an internal asset must not be downloadable through the client lens"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lawyer_download_of_an_internal_document_succeeds() {
    let f = build_fixture().await;

    // The lawyer lens is unfiltered — an admin downloads the internal memo.
    let resp = get(
        &f.app,
        format!(
            "/app/projects/{}/documents/{}/download",
            f.project_code, f.internal_doc_id
        ),
        &f.admin_cookie,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), b"attorney work product");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_detail_of_an_internal_document_does_not_leak_it() {
    let f = build_fixture().await;

    let resp = get(
        &f.app,
        format!(
            "/app/projects/{}/documents/{}",
            f.project_code, f.internal_doc_id
        ),
        &f.client_cookie,
    )
    .await;
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("Not found") && !body.contains("review-memo.pdf"),
        "the client detail page must not render an internal document's provenance"
    );
}
