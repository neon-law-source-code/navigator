//! `POST /app/api/assets` — the public-asset publish door (ENG-909). No
//! `project_id`, like `authorities_api.rs`'s tests: a public brand asset is
//! deployment-wide, not matter-scoped.

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

/// A storage backend that always fails. Standing in for `ApiState::storage`
/// (the private documents bucket) in
/// [`the_door_never_writes_to_the_documents_bucket`]: if the handler ever
/// reached that field instead of `assets_storage`, this would turn a
/// passing upload into a `500`.
struct FailingStorage;

#[async_trait::async_trait]
impl StorageService for FailingStorage {
    async fn put(
        &self,
        _key: &str,
        _bytes: &[u8],
        _content_type: &str,
    ) -> Result<(), StorageError> {
        Err(StorageError::Unsupported(
            "the documents bucket must never be written by this door",
        ))
    }
    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        Err(StorageError::NotFound(key.to_string()))
    }
    async fn delete(&self, _key: &str) -> Result<(), StorageError> {
        Ok(())
    }
    async fn signed_url(&self, _key: &str, _expires_in: Duration) -> Result<String, StorageError> {
        Err(StorageError::Unsupported("unused"))
    }
}

struct Fixture {
    app: Router,
    admin_session: portal::SessionData,
    assets_storage: Arc<dyn StorageService>,
}

/// A fresh public assets bucket backed by a unique temporary directory, so
/// each fixture writes into a store no other test (or prior run —
/// `portal::test_support::app_state`'s default root is a fixed, persistent
/// path) shares. Idempotency tests below rely on starting from empty.
async fn fresh_assets_bucket() -> Arc<dyn StorageService> {
    let root = std::env::temp_dir().join(format!(
        "navigator-assets-api-test-{}",
        uuid::Uuid::new_v4()
    ));
    Arc::new(
        cloud::FsStorage::new(root)
            .await
            .expect("a temp-dir assets bucket"),
    )
}

/// Build a fixture, optionally overriding `ApiState::storage` (the private
/// documents bucket, unused by this door) so a test can prove the door
/// never reaches it.
async fn fixture_on(db: SurrealDb, documents_storage: Option<Arc<dyn StorageService>>) -> Fixture {
    let admin = ensure_person(
        &db,
        &NewPerson {
            name: "Synthetic Admin".into(),
            email: "synthetic-admin@example.com".into(),
            role: Role::Admin,
            ..Default::default()
        },
    )
    .await;
    let state = portal::test_support::app_state(db).await;
    let assets_storage = fresh_assets_bucket().await;
    let api_state = ApiState {
        surreal: state.surreal.clone(),
        email: state.email.clone(),
        bootstrap_owner_email: state.bootstrap_owner_email.clone(),
        bootstrap_company: "Synthetic Firm".into(),
        questionnaire_runtime: state.questionnaire_runtime.clone(),
        storage: documents_storage.unwrap_or_else(|| state.storage.clone()),
        workflow_runtime: state.workflow_runtime.clone(),
        assets_storage: assets_storage.clone(),
        forms_registry: state.forms_registry.clone(),
        signature_provider: state.signature_provider.clone(),
        contract_reviewer: state.contract_reviewer.clone(),
        integration_providers: state.integration_providers.clone(),
    };
    let session = portal::SessionData {
        person_id: Some(admin.id),
        ..portal::SessionData::fresh("synthetic-admin-sub", Role::Admin)
    };
    Fixture {
        app: portal::api::routes().with_state(api_state),
        admin_session: session,
        assets_storage,
    }
}

async fn fixture() -> Fixture {
    fixture_on(mem_surreal().await, None).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    store::assets::sha256_hex(bytes)
}

fn request_body(key: &str, bytes: &[u8], content_type: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "key": key,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "content_type": content_type,
        "sha256": sha256_hex(bytes),
    }))
    .expect("serialize synthetic asset request")
}

async fn post(app: &Router, session: Option<&portal::SessionData>, body: Vec<u8>) -> (u16, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/app/api/assets")
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
async fn an_admin_uploads_an_asset_and_it_is_stored_and_read_back() {
    let fixture = fixture().await;
    let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
    let (status, body) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/rabbit.svg", bytes, "image/svg+xml"),
    )
    .await;

    assert_eq!(status, 201);
    assert_eq!(body["key"], "brand/rabbit.svg");
    assert_eq!(body["bytes"], bytes.len());
    assert_eq!(body["content_type"], "image/svg+xml");
    assert_eq!(body["sha256"], sha256_hex(bytes));
    assert_eq!(body["unchanged"], false);

    let stored = fixture
        .assets_storage
        .get("brand/rabbit.svg")
        .await
        .expect("the object is in the public assets bucket");
    assert_eq!(stored.bytes, bytes);
    assert_eq!(stored.content_type, "image/svg+xml");
}

#[tokio::test]
async fn an_owner_may_also_upload() {
    let fixture = fixture_on(mem_surreal().await, None).await;
    let owner_session = portal::SessionData::fresh("synthetic-owner-sub", Role::Owner);
    let (status, _) = post(
        &fixture.app,
        Some(&owner_session),
        request_body("brand/owner.svg", b"<svg/>", "image/svg+xml"),
    )
    .await;
    assert_eq!(status, 201);
}

#[tokio::test]
async fn a_lawyer_a_clerk_and_a_client_are_forbidden() {
    let fixture = fixture().await;
    for role in [Role::Lawyer, Role::Clerk, Role::Client] {
        let session = portal::SessionData::fresh("synthetic-sub", role);
        let (status, _) = post(
            &fixture.app,
            Some(&session),
            request_body("brand/forbidden.svg", b"bytes", "image/svg+xml"),
        )
        .await;
        assert_eq!(status, 403, "{role:?} must be forbidden");
    }
}

#[tokio::test]
async fn an_anonymous_caller_is_unauthenticated() {
    let fixture = fixture().await;
    let (status, _) = post(
        &fixture.app,
        None,
        request_body("brand/anon.svg", b"bytes", "image/svg+xml"),
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_key_outside_the_allowed_prefixes_is_a_typed_400() {
    let fixture = fixture().await;
    for key in [
        "/brand/rabbit.svg",
        "../brand/rabbit.svg",
        "brand/../x.svg",
        "other/rabbit.svg",
        "documents/rabbit.svg",
    ] {
        let (status, json) = post(
            &fixture.app,
            Some(&fixture.admin_session),
            request_body(key, b"<svg/>", "image/svg+xml"),
        )
        .await;
        assert_eq!(status, 400, "key = {key:?}");
        assert_eq!(json["error"], "invalid_key");
    }
}

#[tokio::test]
async fn unreadable_content_is_a_typed_400() {
    let fixture = fixture().await;
    for content_base64 in ["not valid base64 !!!", ""] {
        let body = json!({
            "key": "brand/rabbit.svg",
            "content_base64": content_base64,
            "content_type": "image/svg+xml",
            "sha256": sha256_hex(b""),
        });
        let (status, json) = post(
            &fixture.app,
            Some(&fixture.admin_session),
            serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(status, 400, "content_base64 = {content_base64:?}");
        assert_eq!(json["error"], "content_unreadable");
    }
}

#[tokio::test]
async fn oversized_content_is_a_typed_400() {
    let fixture = fixture().await;
    // One byte past `assets_api::MAX_ASSET_UPLOAD_BYTES` (25 MB). The
    // module is `pub(crate)`, so this test restates the limit rather than
    // importing it — the same trade-off `authorities_api.rs`'s tests make.
    let bytes = vec![0u8; 25 * 1024 * 1024 + 1];
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/too-big.png", &bytes, "image/png"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "asset_too_large");
}

#[tokio::test]
async fn an_unsupported_content_type_is_a_typed_400() {
    let fixture = fixture().await;
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/rabbit.unknownext", b"bytes", "text/html"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "unsupported_content_type");
}

#[tokio::test]
async fn a_content_type_that_does_not_match_the_key_extension_is_a_typed_400() {
    let fixture = fixture().await;
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/rabbit.svg", b"<svg/>", "image/png"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "content_type_mismatch");
}

#[tokio::test]
async fn a_sha256_that_does_not_match_the_decoded_bytes_is_a_typed_400() {
    let fixture = fixture().await;
    let body = json!({
        "key": "brand/rabbit.svg",
        "content_base64": base64::engine::general_purpose::STANDARD.encode(b"<svg/>"),
        "content_type": "image/svg+xml",
        "sha256": sha256_hex(b"different bytes"),
    });
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        serde_json::to_vec(&body).unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "sha256_mismatch");
}

#[tokio::test]
async fn a_woff2_asset_without_the_signature_is_a_typed_400() {
    let fixture = fixture().await;
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body(
            "fonts/eb-garamond/EBGaramond-Regular.woff2",
            b"not a real font",
            "font/woff2",
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "invalid_font");
}

#[tokio::test]
async fn a_woff2_asset_with_the_signature_is_accepted() {
    let fixture = fixture().await;
    let mut bytes = b"wOF2".to_vec();
    bytes.extend_from_slice(b"synthetic font body");
    let (status, _) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body(
            "fonts/eb-garamond/EBGaramond-Regular.woff2",
            &bytes,
            "font/woff2",
        ),
    )
    .await;
    assert_eq!(status, 201);
}

#[tokio::test]
async fn a_license_file_outside_the_ofl_name_is_unsupported() {
    let fixture = fixture().await;
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body(
            "fonts/eb-garamond/license.txt",
            b"license text",
            "text/plain",
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "unsupported_content_type");
}

#[tokio::test]
async fn an_ofl_license_file_that_is_not_utf8_is_a_typed_400() {
    let fixture = fixture().await;
    let invalid_utf8 = vec![0xff, 0xfe, 0xfd];
    let (status, json) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("fonts/eb-garamond/OFL.txt", &invalid_utf8, "text/plain"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "invalid_license");
}

#[tokio::test]
async fn an_ofl_license_file_is_accepted() {
    let fixture = fixture().await;
    let (status, body) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body(
            "fonts/eb-garamond/OFL.txt",
            b"SIL Open Font License, Version 1.1",
            "text/plain",
        ),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(body["content_type"], "text/plain");
}

#[tokio::test]
async fn the_door_never_writes_to_the_documents_bucket() {
    // `storage` (the documents bucket) always fails; the door must still
    // succeed because it writes `assets_storage` exclusively.
    let failing = fixture_on(
        mem_surreal().await,
        Some(Arc::new(FailingStorage) as Arc<dyn StorageService>),
    )
    .await;
    let (status, body) = post(
        &failing.app,
        Some(&failing.admin_session),
        request_body(
            "brand/rabbit.svg",
            b"bytes never touching documents",
            "image/svg+xml",
        ),
    )
    .await;
    assert_eq!(status, 201, "body: {body}");
}

#[tokio::test]
async fn repeating_the_same_key_and_bytes_is_reported_unchanged() {
    let fixture = fixture().await;
    let bytes = b"stable brand bytes";
    let (status_a, body_a) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/stable.svg", bytes, "image/svg+xml"),
    )
    .await;
    assert_eq!(status_a, 201);
    assert_eq!(body_a["unchanged"], false);

    let (status_b, body_b) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/stable.svg", bytes, "image/svg+xml"),
    )
    .await;
    assert_eq!(status_b, 200);
    assert_eq!(body_b["unchanged"], true);
    assert_eq!(body_a["key"], body_b["key"]);
    assert_eq!(body_a["sha256"], body_b["sha256"]);
}

#[tokio::test]
async fn changed_bytes_at_the_same_key_are_republished() {
    let fixture = fixture().await;
    let (status_a, body_a) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/changes.svg", b"first", "image/svg+xml"),
    )
    .await;
    assert_eq!(status_a, 201);

    let (status_b, body_b) = post(
        &fixture.app,
        Some(&fixture.admin_session),
        request_body("brand/changes.svg", b"second", "image/svg+xml"),
    )
    .await;
    assert_eq!(status_b, 201);
    assert_eq!(body_b["unchanged"], false);
    assert_ne!(body_a["sha256"], body_b["sha256"]);

    let stored = fixture
        .assets_storage
        .get("brand/changes.svg")
        .await
        .unwrap();
    assert_eq!(stored.bytes, b"second");
}
