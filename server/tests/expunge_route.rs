#![allow(clippy::too_many_lines)]
//! Integration tests for the admin governed-expunge surface
//! (`/app/lawyer/documents/:doc_id/expunge`).
//!
//! Covers what Task 1 promises:
//!   1. An admin sees the confirmation screen naming the document.
//!   2. An admin POST drives the primitive end-to-end — history
//!      rewritten, bytes deleted, audit row written — and redirects to the
//!      result state, which shows the audit-row id.
//!   3. A non-admin (client) session 404s on both the GET and the POST,
//!      and nothing is touched.
//!
//! The screen renders through Dioxus (#956 Phase 4) and the POST is
//! post/redirect/get, so the mutation assertions follow the redirect.
//!
//! The surface lives under the admin sub-router, so the test drives it
//! with a real signed session cookie + CSRF token, exactly like the rest
//! of `/app`. A matter repo is filed via the same `matter_documents`
//! seam the portal upload uses, so the `documents`/`blobs` rows and the
//! committed repo file are produced the production way.

use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::documents::{source, IngestArgs};
use store::expunge_records;
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";

/// One repo root for the whole test binary. `NAVIGATOR_GIT_REPO_ROOT`
/// is process-global, so per-test tempdirs would race across the
/// parallel tests (one test's value overwriting another's between the
/// commit and the later history rewrite). A single stable root sidesteps
/// the race; each test uses its own project id, so the repos never
/// collide under it.
static REPO_ROOT: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", dir.path());
    dir
});

// The route fixtures invoke git under the process-wide repository root.
// Serialize them so LLVM-instrumented macOS child processes do not race over
// inherited file descriptors and turn successful redirects into a 500.
static REPO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    project_id: Uuid,
    doc_id: Uuid,
    storage_key: String,
    admin_cookie: String,
    admin_csrf: String,
    client_cookie: String,
    client_csrf: String,
}

async fn build_fixture() -> Fixture {
    // The matter-documents seam commits into a real repo when
    // NAVIGATOR_GIT_REPO_ROOT is set; the binary-wide stable root is set
    // on first access here.
    LazyLock::force(&REPO_ROOT);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-expunge-route-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );

    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Nick", "nick@neonlaw.com", Role::Admin),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Aries", "aries@example.com", Role::Client),
    )
    .await
    .unwrap();
    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("aries-matter-{}", Uuid::now_v7()),
            name: "Aries matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // File a document the production way: persist (blob + document rows)
    // + commit into the matter repo.
    let bytes = b"privileged material";
    let args = IngestArgs {
        project_id: proj.id,
        source: source::UPLOAD,
        filename: "privileged.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    let ingested = portal::matter_documents::record_document(
        &surreal,
        &storage,
        repos::Author {
            name: "Aries",
            email: "aries@example.com",
        },
        &args,
        bytes,
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
    let mut admin_session = SessionData::fresh("nick-sub", Role::Admin);
    admin_session.person_id = Some(admin.id);
    let admin_csrf = admin_session.csrf_token.clone();
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&admin_session));

    let mut client_session = SessionData::fresh("aries-sub", Role::Client);
    client_session.person_id = Some(client.id);
    let client_csrf = client_session.csrf_token.clone();
    let client_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&client_session));

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
        doc_id,
        storage_key,
        admin_cookie,
        admin_csrf,
        client_cookie,
        client_csrf,
    }
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_sees_the_confirmation_screen() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let f = build_fixture().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/documents/{}/expunge", f.doc_id))
                .header("cookie", &f.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(html.contains("privileged.pdf"), "html: {html}");
    // Dioxus SSR escapes the apostrophe in the irreversibility warning.
    assert!(
        html.contains("rewrites the matter&#39;s history"),
        "html: {html}"
    );
    assert!(html.contains("value=\"sealing\""), "html: {html}");
    // The confirmation form keeps the axe e2e selector hook.
    assert!(html.contains("admin-form"), "html: {html}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_post_expunges_and_shows_the_audit_row() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let f = build_fixture().await;
    let form = format!(
        "_csrf={}&category=sealing&note=docket+24-CV-1",
        f.admin_csrf
    );
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/documents/{}/expunge", f.doc_id))
                .header("cookie", &f.admin_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    // Post/redirect/get: the audit-row id travels back in the query.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();

    // The audit row exists, scoped to this expunge.
    let rows = expunge_records::for_project(&f.surreal, f.project_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one audit row written");
    let row = &rows[0];
    assert_eq!(row.category, expunge_records::CATEGORY_SEALING);
    assert_eq!(row.path, "privileged.pdf");
    assert_eq!(row.note.as_deref(), Some("docket 24-CV-1"));
    assert_eq!(
        location,
        format!("/app/lawyer/documents/{}/expunge?record={}", f.doc_id, row.id)
    );

    // ...and the result state renders it.
    let result = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header("cookie", &f.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);
    let html = body_string(result).await;
    assert!(html.contains("Document expunged"), "html: {html}");
    assert!(
        html.contains(&row.id.to_string()),
        "audit id shown on the page: {html}"
    );

    // The bytes are gone from object storage.
    assert!(matches!(
        f.storage.get(&f.storage_key).await,
        Err(cloud::StorageError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_category_is_rejected_without_expunging() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let f = build_fixture().await;
    let form = format!("_csrf={}&category=whoops&note=", f.admin_csrf);
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/documents/{}/expunge", f.doc_id))
                .header("cookie", &f.admin_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    // A rejected category re-shows the form with its flash (PRG), and nothing
    // is expunged.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(
        location.starts_with(&format!("/app/lawyer/documents/{}/expunge?error=", f.doc_id)),
        "location: {location}"
    );
    let form_again = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header("cookie", &f.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(form_again.status(), StatusCode::OK);
    let html = body_string(form_again).await;
    assert!(
        html.contains("Choose one of the listed expunge categories."),
        "html: {html}"
    );
    // Nothing expunged, document bytes intact.
    assert_eq!(
        expunge_records::for_project(&f.surreal, f.project_id)
            .await
            .unwrap()
            .len(),
        0
    );
    assert!(f.storage.get(&f.storage_key).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_admin_cannot_see_or_run_the_expunge() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let f = build_fixture().await;

    // GET → 404 for a client.
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/documents/{}/expunge", f.doc_id))
                .header("cookie", &f.client_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // POST → 404 for a client, and nothing is touched.
    let form = format!("_csrf={}&category=sealing&note=", f.client_csrf);
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/documents/{}/expunge", f.doc_id))
                .header("cookie", &f.client_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        expunge_records::for_project(&f.surreal, f.project_id)
            .await
            .unwrap()
            .len(),
        0
    );
    assert!(f.storage.get(&f.storage_key).await.is_ok());
    // Document row still present.
    assert!(store::assets::find_by_id(&f.surreal, f.doc_id)
        .await
        .unwrap()
        .is_some());
}
