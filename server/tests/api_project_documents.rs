#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/projects/{id}/documents` — the REST door
//! that files a document into a matter.
//!
//! The write engine (`matter_documents::record_document`) is shared with the
//! lawyer upload control, so this focuses on what the REST adapter adds: it takes
//! the bytes base64-encoded (not multipart), lawyer-tier only (client 403, anon
//! 401), the matter-scope gate (a non-participant lawyer is 404), undecodable
//! base64 is 400, a `kind` outside the asset lane is 400 (not the 500 it used to be),
//! and a filed document actually lands.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-project-documents-test-key";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    project_id: Uuid,
    lawyer: String,
    outsider: String,
    client: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut s = SessionData::fresh("api-doc-sub", role);
    s.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-api-docs-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Lawyer", "lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer")
        .await
        .unwrap();
    let outsider = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Outsider", "outsider@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Client", "client@example.com", Role::Client),
    )
    .await
    .unwrap();
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        project_id: project.id,
        lawyer: bearer(lawyer.id, Role::Lawyer),
        outsider: bearer(outsider.id, Role::Lawyer),
        client: bearer(client.id, Role::Client),
    }
}

async fn upload(
    fx: &Fixture,
    auth: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/projects/{}/documents", fx.project_id))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

fn doc_body() -> serde_json::Value {
    // "test document" base64-encoded.
    serde_json::json!({
        "filename": "note.txt",
        "content_base64": "dGVzdCBkb2N1bWVudA==",
        "content_type": "text/plain"
    })
}

#[tokio::test]
async fn a_participant_lawyer_files_a_document() {
    let fx = build_fixture().await;
    let resp = upload(&fx, Some(&fx.lawyer), doc_body()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let document_id: Uuid = json["document_id"].as_str().unwrap().parse().unwrap();
    assert!(
        store::assets::find_by_id(&fx.surreal, document_id)
            .await
            .unwrap()
            .is_some(),
        "the filed document is a real asset"
    );
}

#[tokio::test]
async fn a_non_participant_lawyer_is_404() {
    let fx = build_fixture().await;
    let resp = upload(&fx, Some(&fx.outsider), doc_body()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_client_is_403_and_anonymous_is_401() {
    let fx = build_fixture().await;
    assert_eq!(
        upload(&fx, Some(&fx.client), doc_body()).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(&fx, None, doc_body()).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn undecodable_base64_is_400() {
    let fx = build_fixture().await;
    let resp = upload(
        &fx,
        Some(&fx.lawyer),
        serde_json::json!({ "filename": "note.txt", "content_base64": "!!! not base64 !!!" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// A `kind` the asset lane does not accept is the **caller's** error, so it is a
/// 400 carrying `invalid_kind` — not the 500 that `ApiError::Db` used to produce
/// by collapsing every ingest failure into "the database failed".
///
/// `review_queue_workbench` is the sharp case rather than gibberish: it is a real
/// `rules::kind::Kind`, valid in the Template lane, and refused only here. A door
/// that parsed the value without checking the lane would let it through.
#[tokio::test]
async fn a_kind_outside_the_asset_lane_is_400_naming_the_accepted_values() {
    let fx = build_fixture().await;
    let mut body = doc_body();
    body["kind"] = serde_json::json!("review_queue_workbench");
    let resp = upload(&fx, Some(&fx.lawyer), body).await;

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an unaccepted kind is the caller's mistake, not a server fault"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["error"].as_str(),
        Some("invalid_kind"),
        "the body carries the machine-readable error identifier"
    );

    // The message has to let an integrator fix the request without reading our
    // source: it names what they sent and what would have been accepted.
    let message = json["message"].as_str().expect("a message is present");
    assert!(
        message.contains("review_queue_workbench"),
        "the message names the rejected value, got: {message}"
    );
    for accepted in [
        "onboarding",
        "unclassified",
        "certificate_of_naturalization",
    ] {
        assert!(
            message.contains(accepted),
            "the message lists the accepted kind `{accepted}`, got: {message}"
        );
    }

    // The refusal is real: the store wrote nothing.
    assert!(
        store::assets::for_project(&fx.surreal, fx.project_id)
            .await
            .unwrap()
            .is_empty(),
        "a refused kind files no document"
    );
}

/// A value that is not a `Kind` at all is refused the same way — the door does
/// not silently coerce it to `unclassified`.
#[tokio::test]
async fn an_unknown_kind_is_400() {
    let fx = build_fixture().await;
    let mut body = doc_body();
    body["kind"] = serde_json::json!("not-a-kind");
    let resp = upload(&fx, Some(&fx.lawyer), body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"].as_str(), Some("invalid_kind"));
}

/// The other half of the classification, and the one that would catch an
/// over-eager 400: every kind the asset lane accepts still files a document.
/// Driven from `rules::kind` itself, so widening the lane cannot leave this
/// test asserting a stale vocabulary.
#[tokio::test]
async fn every_accepted_kind_still_files_a_document() {
    let fx = build_fixture().await;
    for kind in rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
    {
        let mut body = doc_body();
        body["kind"] = serde_json::json!(kind.as_str());
        // Distinct bytes per kind so each is a fresh ingest rather than a dedup.
        body["content_base64"] = serde_json::json!(base64_of(kind.as_str()));
        let resp = upload(&fx, Some(&fx.lawyer), body).await;
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "`{}` is an asset-lane kind and must be accepted",
            kind.as_str()
        );
    }
}

/// Minimal base64 encoder for the fixture bytes — the test needs distinct
/// content per kind, not a dependency.
fn base64_of(text: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
