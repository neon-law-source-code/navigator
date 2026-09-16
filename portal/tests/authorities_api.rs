//! `POST /app/api/authorities` — the global citation-apparatus write door
//! (ENG-712). An Authority carries no `project_id`, so these tests seed no
//! matter — unlike `document_upload.rs`, which does.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request};
use axum::Router;
use base64::Engine as _;
use cloud::{StorageError, StorageService, StoredObject};
use portal::api::ApiState;
use serde_json::{json, Value};
use store::persons::{NewPerson, Role};
use store::surreal::SurrealDb;
use store::test_support::{ensure_person, mem_surreal};
use tower::ServiceExt;

/// A storage backend whose `put` always fails, so a test can force
/// [`store::assets::ingest_content`] to error without touching the real
/// filesystem backend `portal::test_support::app_state` wires by default.
struct FailingStorage;

#[async_trait::async_trait]
impl StorageService for FailingStorage {
    async fn put(
        &self,
        _key: &str,
        _bytes: &[u8],
        _content_type: &str,
    ) -> Result<(), StorageError> {
        Err(StorageError::Unsupported("synthetic asset-write failure"))
    }
    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        Err(StorageError::NotFound(key.to_string()))
    }
    async fn delete(&self, _key: &str) -> Result<(), StorageError> {
        Ok(())
    }
    async fn signed_url(&self, _key: &str, _expires_in: Duration) -> Result<String, StorageError> {
        Err(StorageError::Unsupported("synthetic asset-write failure"))
    }
}

struct Fixture {
    app: Router,
    lawyer_session: portal::SessionData,
    surreal: SurrealDb,
    storage: Arc<dyn StorageService>,
}

/// Build a fixture on a caller-supplied store, optionally overriding the
/// storage backend so a test can force an asset-write failure without
/// touching the real `FsStorage` backend `portal::test_support::app_state`
/// wires by default. Taking the store as a parameter (rather than minting
/// a fresh one) lets a test build two fixtures — one with a failing
/// storage, one without — that read and write the *same* rows, which is
/// what proves a failed write left nothing behind for a later one to find.
async fn fixture_on(db: SurrealDb, storage: Option<Arc<dyn StorageService>>) -> Fixture {
    let lawyer = ensure_person(
        &db,
        &NewPerson {
            name: "Synthetic Lawyer".into(),
            email: "synthetic-lawyer@example.com".into(),
            role: Role::Lawyer,
            ..Default::default()
        },
    )
    .await;
    let state = portal::test_support::app_state(db).await;
    let storage = storage.unwrap_or_else(|| state.storage.clone());
    let api_state = ApiState {
        surreal: state.surreal.clone(),
        email: state.email.clone(),
        bootstrap_owner_email: state.bootstrap_owner_email.clone(),
        bootstrap_company: "Synthetic Firm".into(),
        questionnaire_runtime: state.questionnaire_runtime.clone(),
        storage: storage.clone(),
        workflow_runtime: state.workflow_runtime.clone(),
        assets_storage: state.assets_storage.clone(),
        forms_registry: state.forms_registry.clone(),
        signature_provider: state.signature_provider.clone(),
        contract_reviewer: state.contract_reviewer.clone(),
        integration_providers: state.integration_providers.clone(),
    };
    let session = portal::SessionData {
        person_id: Some(lawyer.id),
        ..portal::SessionData::fresh("synthetic-lawyer-sub", Role::Lawyer)
    };
    Fixture {
        app: portal::api::routes().with_state(api_state),
        lawyer_session: session,
        surreal: state.surreal,
        storage,
    }
}

async fn fixture() -> Fixture {
    fixture_on(mem_surreal().await, None).await
}

fn request_body(citation: &str, archive: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "class": "case_law",
        "citation": citation,
        "title": "Example v. Example",
        "archive_base64": base64::engine::general_purpose::STANDARD.encode(archive),
        "content_type": "application/pdf",
    }))
    .expect("serialize synthetic authority request")
}

async fn post(app: &Router, session: Option<&portal::SessionData>, body: Vec<u8>) -> (u16, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/app/api/authorities")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        builder = builder.extension(session.clone());
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).expect("build request"))
        .await
        .expect("run request");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let json: Value = serde_json::from_slice(&bytes).expect("response is JSON");
    (status, json)
}

#[tokio::test]
async fn a_lawyer_creates_an_authority_and_the_archive_is_retrievable() {
    let fixture = fixture().await;
    let archive = b"synthetic opinion text";
    let (status, body) = post(
        &fixture.app,
        Some(&fixture.lawyer_session),
        request_body("410 U.S. 113 (1973)", archive),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["citation"], "410 U.S. 113 (1973)");
    let asset_id: uuid::Uuid = body["archived_asset_id"]
        .as_str()
        .expect("archived_asset_id is present")
        .parse()
        .expect("archived_asset_id is a uuid");

    // Retrievable through the ordinary Asset path.
    let fetched = store::assets::fetch(&fixture.surreal, &fixture.storage, asset_id)
        .await
        .expect("fetch archived bytes");
    assert_eq!(fetched, archive);
}

#[tokio::test]
async fn a_clerk_and_a_client_are_forbidden() {
    let fixture = fixture().await;
    for role in [Role::Clerk, Role::Client] {
        let session = portal::SessionData::fresh("synthetic-sub", role);
        let (status, _) = post(
            &fixture.app,
            Some(&session),
            request_body("1 U.S. 1", b"bytes"),
        )
        .await;
        assert_eq!(status, 403, "{role:?} must be forbidden");
    }
}

#[tokio::test]
async fn an_anonymous_caller_is_unauthenticated() {
    let fixture = fixture().await;
    let (status, _) = post(&fixture.app, None, request_body("2 U.S. 2", b"bytes")).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn repeating_the_same_citation_is_idempotent_and_keeps_the_original_archive() {
    let fixture = fixture().await;
    let (status_a, body_a) = post(
        &fixture.app,
        Some(&fixture.lawyer_session),
        request_body("3 U.S. 3", b"first archive"),
    )
    .await;
    assert_eq!(status_a, 200);

    let (status_b, body_b) = post(
        &fixture.app,
        Some(&fixture.lawyer_session),
        request_body("3 U.S. 3", b"a different second archive"),
    )
    .await;
    assert_eq!(status_b, 200);

    assert_eq!(body_a["id"], body_b["id"], "the citation is the identity");
    assert_eq!(
        body_a["archived_asset_id"], body_b["archived_asset_id"],
        "a repeat citation must never replace the original archive"
    );
}

#[tokio::test]
async fn an_unrecognized_class_is_a_typed_400() {
    let fixture = fixture().await;
    let body = json!({
        "class": "not_a_real_class",
        "citation": "4 U.S. 4",
        "title": "Example",
        "archive_base64": base64::engine::general_purpose::STANDARD.encode(b"bytes"),
    });
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.lawyer_session),
        serde_json::to_vec(&body).unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "invalid_class");
}

#[tokio::test]
async fn an_unreadable_archive_is_a_typed_400() {
    let fixture = fixture().await;
    for archive_base64 in ["not valid base64 !!!", ""] {
        let body = json!({
            "class": "case_law",
            "citation": "5 U.S. 5",
            "title": "Example",
            "archive_base64": archive_base64,
        });
        let (status, json) = post(
            &fixture.app,
            Some(&fixture.lawyer_session),
            serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(status, 400, "archive_base64 = {archive_base64:?}");
        assert_eq!(json["error"], "archive_unreadable");
    }
}

#[tokio::test]
async fn an_asset_write_failure_records_no_authority() {
    // Both fixtures share one store: a row the failing attempt left behind
    // would be sitting there for the working attempt's find-or-create to
    // find, which is exactly what this test rules out.
    let db = mem_surreal().await;
    let failing = fixture_on(
        db.clone(),
        Some(Arc::new(FailingStorage) as Arc<dyn StorageService>),
    )
    .await;
    let (status, _) = post(
        &failing.app,
        Some(&failing.lawyer_session),
        request_body("6 U.S. 6", b"bytes that never reach storage"),
    )
    .await;
    assert_eq!(status, 500);

    let working = fixture_on(db, None).await;
    let (status, body) = post(
        &working.app,
        Some(&working.lawyer_session),
        request_body("6 U.S. 6", b"bytes that do reach storage"),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        body["archived_asset_id"].is_string(),
        "a fresh create must carry a real archived_asset_id — an Authority row already sitting \
         there from the failed attempt would have been found instead of created"
    );
}
