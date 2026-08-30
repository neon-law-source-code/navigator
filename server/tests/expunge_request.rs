#![allow(clippy::too_many_lines)]
//! Integration tests for client-initiated document deletion
//! (git-repos surfaces Task 2): a client requests deletion, a lawyer/admin
//! authorizes, and the document is scrubbed.
//!
//! Covers:
//!   1. The client POSTs a deletion request (the portal documents table is
//!      read-only — the route is not linked in the UI); an admin authorizes
//!      it and the bytes + audit row + request status all reflect a
//!      completed governed expunge (category `client_request`).
//!   2. A non-admin cannot authorize — the request stays pending and
//!      nothing is deleted.

use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::documents::{source, IngestArgs};
use store::expunge_records;
use store::expunge_requests::{STATUS_AUTHORIZED, STATUS_PENDING};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";

static REPO_ROOT: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", dir.path());
    dir
});

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    project_id: Uuid,
    project_code: String,
    doc_id: Uuid,
    storage_key: String,
    client_cookie: String,
    client_csrf: String,
    admin_cookie: String,
    admin_csrf: String,
}

async fn build_fixture() -> Fixture {
    LazyLock::force(&REPO_ROOT);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("nav-exreq-{}", Uuid::now_v7())))
            .await
            .unwrap(),
    );

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Nick", "nick@neonlaw.com", Role::Admin),
    )
    .await
    .unwrap();
    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-estate-{}", Uuid::now_v7()),
            name: "Libra estate plan".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, proj.id, libra.id, "client")
        .await
        .unwrap();

    // The client requests deletion of a document they can see on their own
    // matter page, so it must be client-visible — the visibility gate (#782)
    // hides `internal` work product from the client lens.
    let args = IngestArgs {
        project_id: proj.id,
        source: source::UPLOAD,
        filename: "old-draft.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::CLIENT,
    };
    let ingested = portal::matter_documents::record_document(
        &surreal,
        &storage,
        repos::Author {
            name: "Libra",
            email: "libra@example.com",
        },
        &args,
        b"a draft to delete",
    )
    .await
    .unwrap();
    let doc_id = ingested.asset_id;
    let storage_key = store::assets::find_by_id(&surreal, ingested.asset_id)
        .await
        .unwrap()
        .unwrap()
        .storage_key;

    let sessions = SessionStore::new(KEY);
    let mut client = SessionData::fresh("libra-sub", Role::Client);
    client.person_id = Some(libra.id);
    let client_csrf = client.csrf_token.clone();
    let client_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&client));
    let mut admin_session = SessionData::fresh("nick-sub", Role::Admin);
    admin_session.person_id = Some(admin.id);
    let admin_csrf = admin_session.csrf_token.clone();
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&admin_session));

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
        surreal,
        storage,
        project_id: proj.id,
        project_code: proj.code,
        doc_id,
        storage_key,
        client_cookie,
        client_csrf,
        admin_cookie,
        admin_csrf,
    }
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get(f: &Fixture, uri: String, cookie: &str) -> axum::http::Response<Body> {
    f.app
        .clone()
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

async fn post(f: &Fixture, uri: String, cookie: &str, form: String) -> axum::http::Response<Body> {
    f.app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_requests_then_admin_authorizes_and_document_is_scrubbed() {
    let f = build_fixture().await;

    // Client can reach their matter page (documents render read-only).
    let page = get(
        &f,
        format!("/app/projects/{}", f.project_code),
        &f.client_cookie,
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_string(page).await;
    assert!(html.contains("old-draft.pdf"));
    assert!(!html.contains("request-deletion"));

    // Client requests deletion via the governed route (not linked in UI).
    let resp = post(
        &f,
        format!(
            "/app/projects/{}/documents/{}/request-deletion",
            f.project_code, f.doc_id
        ),
        &f.client_cookie,
        format!("_csrf={}", f.client_csrf),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // A pending request now exists (the read-only documents table does not
    // surface deletion status in the portal UI).
    let pending = store::expunge_requests::pending_for_document(&f.surreal, f.doc_id)
        .await
        .unwrap();
    assert!(pending.is_some(), "a pending request should exist");

    // Admin sees it in the queue.
    let queue = get(&f, "/app/lawyer/expunge-requests".into(), &f.admin_cookie).await;
    assert_eq!(queue.status(), StatusCode::OK);
    let html = body_string(queue).await;
    assert!(html.contains("old-draft.pdf"));
    assert!(&html.contains("Authorize deletion"));
    // The Dioxus queue posts to the existing handlers through native forms: the
    // deny action and the session CSRF token must render so the actions work
    // without JavaScript.
    assert!(
        html.contains("/deny"),
        "the deny action form must render: {html}"
    );
    assert!(
        html.contains(&f.admin_csrf),
        "the session CSRF token must render in the action forms",
    );

    // Admin authorizes → the governed expunge runs.
    let request_id = pending.unwrap().id;
    let resp = post(
        &f,
        format!("/app/lawyer/expunge-requests/{request_id}/authorize"),
        &f.admin_cookie,
        format!("_csrf={}", f.admin_csrf),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Audit row written with the client_request category.
    let records = expunge_records::for_project(&f.surreal, f.project_id)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].category,
        expunge_records::CATEGORY_CLIENT_REQUEST
    );

    // Bytes gone from object storage.
    assert!(matches!(
        f.storage.get(&f.storage_key).await,
        Err(cloud::StorageError::NotFound(_))
    ));

    // Request marked authorized + linked to the audit row.
    let req = store::expunge_requests::by_id(&f.surreal, request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(req.status, STATUS_AUTHORIZED);
    assert_eq!(req.expunge_record_id, Some(records[0].id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_admin_cannot_authorize() {
    let f = build_fixture().await;

    // Stand up a pending request directly.
    let request_id = store::expunge_requests::create(
        &f.surreal,
        &store::expunge_requests::NewExpungeRequest {
            project_id: f.project_id,
            asset_id: f.doc_id,
            requested_by_person_id: store::persons::list_directory(&f.surreal, "", "", &[])
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.role == Role::Client)
                .unwrap()
                .id,
            note: None,
        },
    )
    .await
    .unwrap();

    // The client tries to authorize → 404 (admin-only), nothing deleted.
    let resp = post(
        &f,
        format!("/app/lawyer/expunge-requests/{request_id}/authorize"),
        &f.client_cookie,
        format!("_csrf={}", f.client_csrf),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let req = store::expunge_requests::by_id(&f.surreal, request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(req.status, STATUS_PENDING);
    assert!(f.storage.get(&f.storage_key).await.is_ok());
    assert_eq!(
        expunge_records::for_project(&f.surreal, f.project_id)
            .await
            .unwrap()
            .len(),
        0
    );
}
