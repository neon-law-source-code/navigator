#![allow(clippy::doc_markdown, clippy::too_many_lines)]
//! Commit 2: notation-PDF access is gated by **project participation**,
//! not notation ownership, and the project page surfaces each notation's
//! three PDFs (rendered / signed / certificate) by plain name.
//!
//! Proves:
//!   - a project *participant* who is not the notation owner can download
//!     the rendered + signed PDFs (200 — `FsStorage` streams through);
//!   - a non-participant gets 404 (no leakage, not 403);
//!   - admin bypasses;
//!   - the client project page lists the notation under "Your agreements"
//!     with a working signed-copy link.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    sessions: SessionStore,
}

async fn build() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("navigator-doc-acl-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .expect("canonical seed");

    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let inner = Arc::new(workflows::InMemoryRuntime::new());
    let workflow_runtime: Arc<dyn workflows::StateMachineRuntime> = Arc::new(
        workflows::DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime,
        questionnaire_runtime: inner,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        storage,
        sessions: SessionStore::new(KEY),
    }
}

fn cookie_for(sessions: &SessionStore, role: Role, person_id: Option<Uuid>) -> String {
    let mut s = SessionData::fresh("sub", role);
    s.person_id = person_id;
    format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&s))
}

async fn mk_person(surreal: &store::surreal::SurrealDb, email: &str, role: Role) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap()
    .id
}

async fn get(app: &axum::Router, uri: &str, cookie: &str) -> axum::http::Response<Body> {
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

#[tokio::test]
async fn notation_pdfs_are_gated_by_project_participation_and_listed_on_the_project() {
    let f = build().await;

    // A retainer template to hang the notation off (any seeded onboarding
    // template works; pick the retainer by code).
    let tmpl = store::templates::list_current(&f.surreal)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("at least one seeded template");

    // The notation owner, a co-client participant, and an outsider.
    let owner = mk_person(&f.surreal, "owner@example.com", Role::Client).await;
    let spouse = mk_person(&f.surreal, "spouse@example.com", Role::Client).await;
    let outsider = mk_person(&f.surreal, "outsider@example.com", Role::Client).await;

    let project = store::projects::create(
        &f.surreal,
        &store::projects::NewProject {
            code: format!("joint-estate-{}", Uuid::now_v7()),
            name: "Joint estate".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&f.surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let project_id = project.id;

    // Both owner and spouse participate; the outsider does not.
    for pid in [owner, spouse] {
        store::projects::add_participation(&f.surreal, project_id, pid, "client")
            .await
            .unwrap();
    }

    let notation_id = store::notations::create(
        &f.surreal,
        &store::notations::NewNotation::new(tmpl.id, owner, project_id, "END"),
    )
    .await
    .unwrap()
    .id;

    // Materialize the rendered + signed PDFs in storage.
    for key in [
        store::notations::document_pdf_storage_key(notation_id),
        store::notations::signed_document_storage_key(notation_id),
    ] {
        f.storage
            .put(&key, b"%PDF-1.7 fake", "application/pdf")
            .await
            .unwrap();
    }

    let doc_uri = format!("/app/notations/{notation_id}/documents/retainer");
    let signed_uri = format!("/app/notations/{notation_id}/documents/signed");
    let lawyer_doc_uri = format!("/app/lawyer/notations/{notation_id}/documents/retainer");

    // (1) The spouse (participant, NOT the owner) can download both PDFs.
    let spouse_cookie = cookie_for(&f.sessions, Role::Client, Some(spouse));
    assert_eq!(
        get(&f.app, &doc_uri, &spouse_cookie).await.status(),
        StatusCode::OK,
        "a co-client participant can fetch the rendered PDF",
    );
    assert_eq!(
        get(&f.app, &signed_uri, &spouse_cookie).await.status(),
        StatusCode::OK,
        "a co-client participant can fetch the signed PDF",
    );

    // (2) The outsider (no participation) gets 404 — not 403, no leakage.
    let outsider_cookie = cookie_for(&f.sessions, Role::Client, Some(outsider));
    assert_eq!(
        get(&f.app, &doc_uri, &outsider_cookie).await.status(),
        StatusCode::NOT_FOUND,
        "a non-participant must get 404, not the document",
    );

    // (3) Admin bypasses participation.
    let admin_cookie = cookie_for(&f.sessions, Role::Admin, None);
    assert_eq!(
        get(&f.app, &lawyer_doc_uri, &admin_cookie).await.status(),
        StatusCode::OK,
        "admin bypasses project scoping",
    );

    // (4) Once the provider confirms execution, the client project page
    // lists the notation under "Your agreements" with a working signed-copy
    // link.
    let provider_id = format!("env-{notation_id}");
    store::signatures::record_request(
        &f.surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        &provider_id,
    )
    .await
    .unwrap();
    assert!(store::signatures::stamp_signed(
        &f.surreal,
        store::signatures::SignatureProvider::DocuSign,
        &provider_id,
        "2026-09-02T00:00:00Z",
    )
    .await
    .unwrap());
    let page = get(
        &f.app,
        &format!("/app/projects/{}", project.code),
        &spouse_cookie,
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        page.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(
        html.contains("Your agreements"),
        "agreements section missing"
    );
    assert!(
        html.contains(&format!("/app/notations/{notation_id}/documents/signed")),
        "signed-copy download link missing from the project page",
    );
}
