#![allow(clippy::too_many_lines)]
//! Integration tests for the client "download all my documents" export
//! (`GET /app/projects/:project_code/documents.zip`).
//!
//! Covers what Task 3 promises:
//!   1. A scoped participant downloads a real ZIP whose entries are the
//!      matter's current files, by their human filenames, with bytes
//!      intact — never a packfile or bundle.
//!   2. A non-participant gets 404 — the matter doesn't exist for them.
//!
//! Documents are filed through the same `matter_documents` seam the
//! portal upload uses; the export reads them back from the durable
//! system of record — the `assets` rows plus their bytes in
//! `cloud::StorageService` — exactly as the per-document download does.
//!
//! Crucially, this binary **never sets `NAVIGATOR_GIT_REPO_ROOT`**, so it
//! exercises the topology the shipped GKE `web` actually runs: no mounted
//! repo volume. Before #542 the export shelled git here and handed back
//! an empty archive on every request; these tests would fail. The read
//! path no longer touches git, so a filed document is always returned.

use std::io::Read;
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
    project_id: Uuid,
    project_code: String,
    member_cookie: String,
    stranger_cookie: String,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
}

async fn build_fixture() -> Fixture {
    build_fixture_with(&[
        ("will.pdf", b"the last will and testament".as_slice()),
        ("trust.pdf", b"the revocable living trust".as_slice()),
    ])
    .await
}

/// Build the app + a scoped `Libra` participant + a `stranger`
/// non-participant, filing `docs` into the matter through the production
/// `record_document` seam. No `NAVIGATOR_GIT_REPO_ROOT` is set (see the
/// module docs), so filing skips the best-effort git commit and the
/// export must read the documents back from `assets` + storage.
async fn build_fixture_with(docs: &[(&str, &[u8])]) -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("nav-export-{}", Uuid::now_v7())))
            .await
            .unwrap(),
    );

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let stranger = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Aries", "aries@example.com", Role::Client),
    )
    .await
    .unwrap();
    let proj = store::test_support::seed_project(&surreal, "Libra estate plan").await;
    store::projects::add_participation(&surreal, proj.id, libra.id, "client")
        .await
        .unwrap();

    // File each document the production way (durable persist to storage +
    // the `assets` row; the git commit is skipped with no repo root).
    for (filename, bytes) in docs {
        let args = IngestArgs {
            project_id: proj.id,
            source: source::UPLOAD,
            filename,
            kind: "unclassified",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            // Every fixture doc must be client-visible for the export
            // tests below, which assert on the *client* zip's contents.
            visibility: store::documents::visibility::CLIENT,
        };
        portal::matter_documents::record_document(
            &surreal,
            &storage,
            repos::Author {
                name: "Libra",
                email: "libra@example.com",
            },
            &args,
            bytes,
        )
        .await
        .unwrap();
    }

    let sessions = SessionStore::new(KEY);
    let mut member = SessionData::fresh("libra-sub", Role::Client);
    member.person_id = Some(libra.id);
    let member_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&member));
    let mut other = SessionData::fresh("aries-sub", Role::Client);
    other.person_id = Some(stranger.id);
    let stranger_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&other));

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
        project_id: proj.id,
        project_code: proj.code.clone(),
        member_cookie,
        stranger_cookie,
        surreal,
        storage,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_client_downloads_a_zip_of_their_current_documents() {
    let f = build_fixture().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &f.member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/zip")
    );
    assert!(resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .contains("libra-estate-plan-documents.zip"));

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(archive.len(), 2);

    let mut got = std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        got.insert(name, content);
    }
    assert_eq!(
        got.get("will.pdf").map(Vec::as_slice),
        Some(b"the last will and testament".as_slice())
    );
    assert_eq!(
        got.get("trust.pdf").map(Vec::as_slice),
        Some(b"the revocable living trust".as_slice())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_filenames_and_shared_storage_key_yield_distinct_entries() {
    // Two documents with the *same* filename and the *same* bytes. Bytes
    // are content-addressed and deduped, so both `assets` rows point at
    // one `storage_key` — the export must still emit two entries, under
    // de-collided names, not collapse them to one.
    let f = build_fixture_with(&[
        (
            "statement.pdf",
            b"identical bank statement bytes".as_slice(),
        ),
        (
            "statement.pdf",
            b"identical bank statement bytes".as_slice(),
        ),
    ])
    .await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &f.member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(archive.len(), 2, "both documents must appear as entries");

    let mut names = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"identical bank statement bytes");
        names.push(name);
    }
    names.sort();
    assert_eq!(names, vec!["statement (2).pdf", "statement.pdf"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expunged_document_is_skipped_not_a_500() {
    let f = build_fixture_with(&[
        ("will.pdf", b"the last will and testament".as_slice()),
        ("trust.pdf", b"the revocable living trust".as_slice()),
    ])
    .await;

    // A governed expunge deletes a document's bytes from object storage but
    // keeps its `assets` row for audit. Simulate that: drop one blob.
    let assets = store::assets::for_project(&f.surreal, f.project_id)
        .await
        .unwrap();
    let expunged = assets
        .iter()
        .find(|a| a.filename.as_deref() == Some("will.pdf"))
        .expect("will.pdf asset row");
    f.storage.delete(&expunged.storage_key).await.unwrap();

    // The export must not 500 the whole matter — it returns the remaining
    // document, skipping the one whose bytes are gone.
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &f.member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    assert_eq!(
        archive.len(),
        1,
        "only the surviving document is in the archive"
    );
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name(), "trust.pdf");
    let mut content = Vec::new();
    entry.read_to_end(&mut content).unwrap();
    assert_eq!(content, b"the revocable living trust");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn internal_document_is_excluded_from_the_client_zip_but_included_for_lawyer() {
    // #782: this archive hands back full document bytes, so it carries the
    // same exposure the portal matter-detail listing did if left ungated —
    // an internal document (the review-memo shape) must not reach the
    // client's zip, but the lawyer zip is unfiltered.
    let f = build_fixture_with(&[("will.pdf", b"the last will and testament".as_slice())]).await;

    let args = IngestArgs {
        project_id: f.project_id,
        source: source::UPLOAD,
        filename: "review-memo.pdf",
        kind: "memo",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    portal::matter_documents::record_document(
        &f.surreal,
        &f.storage,
        repos::Author {
            name: "Lawyer",
            email: "lawyer@example.com",
        },
        &args,
        b"attorney work product",
    )
    .await
    .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &f.member_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["will.pdf"],
        "the internal review memo must not appear in the client's zip"
    );

    // The firm side of the same matter. Since ENG-81 the acting admin has to be
    // on it — a bare person id used to ride the bypass — so this still tests
    // what it is about: the firm gets the internal memo the client's zip omits.
    let admin_person = store::persons::create(
        &f.surreal,
        &store::persons::NewPerson::with_role(
            "Export Admin",
            "export-admin@neonlaw.com",
            Role::Admin,
        ),
    )
    .await
    .expect("seed the acting admin");
    store::projects::add_participation(&f.surreal, f.project_id, admin_person.id, "attorney")
        .await
        .expect("put the acting admin on the matter");
    let admin_sessions = SessionStore::new(KEY);
    let mut admin = SessionData::fresh("lawyer-sub", Role::Admin);
    admin.person_id = Some(admin_person.id);
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={}", admin_sessions.encode(&admin));
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["review-memo.pdf", "will.pdf"],
        "the lawyer zip stays unfiltered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_participant_gets_404() {
    let f = build_fixture().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/documents.zip", f.project_code))
                .header("cookie", &f.stranger_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
