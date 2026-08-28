//! Integration tests for `GET /lawyer/notations/{id}/sign` — the door to the
//! signing ceremony.
//!
//! Navigator does not host the ceremony: the route mints a single-use
//! recipient-view URL and redirects the signer to the provider's own site
//! (#1010). These tests drive the composed router so they cover the whole
//! handler — the notation and client lookups, the "not sent yet" guard, and the
//! redirect itself — rather than only the pure URL check that
//! `portal::esign_view`'s unit tests cover.
//!
//! Worth stating because it is the point of the design: **nothing here proves
//! the signature completes.** Completion arrives on
//! `POST /webhook/esignature/{secret}` (see `esignature_loop.rs`), which is
//! deliberate — the signer may finish on their phone and never come back to
//! this browser session at all.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const TEMPLATE_CODE: &str = "onboarding__letter";
const ENVELOPE_ID: &str = "env-refer-out-1";

/// A composed router over a seeded store, the notation id its client is
/// bound to, and the handle that owns them. The stub `SignatureProvider`
/// is the default in `test_support::app_state`, so `create_recipient_view`
/// returns a deterministic `https://stub.docusign.local/...` URL with no
/// network call.
///
/// The handle is returned rather than re-derived by the caller: each
/// `mem_surreal()` opens its own engine, so a second call would hand back
/// an empty store in which this notation does not exist.
async fn app_with_notation() -> (axum::Router, Uuid, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-esign-redirect-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts the retainer template");

    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-retainer-{}", Uuid::now_v7()),
            name: "Libra retainer".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(
            tmpl.id,
            client.id,
            project.id,
            "sent_for_signature__pending",
        ),
    )
    .await
    .unwrap()
    .id;

    let state = AppState {
        storage,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    (app, notation_id, surreal)
}

async fn get_sign(app: &axum::Router, notation_id: Uuid) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/lawyer/notations/{notation_id}/sign"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_sent_notation_redirects_the_signer_to_the_provider() {
    let (app, notation_id, surreal) = app_with_notation().await;
    store::signatures::record_request(
        &surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        ENVELOPE_ID,
    )
    .await
    .unwrap();

    let response = get_sign(&app, notation_id).await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "the signer is sent to the provider, not shown a page here"
    );
    let location = response
        .headers()
        .get(header::LOCATION)
        .expect("a redirect carries a Location")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        location.starts_with("https://stub.docusign.local/signing/"),
        "redirects to the provider's own site: {location}"
    );
    assert!(
        location.contains(ENVELOPE_ID),
        "the recipient view is for this envelope: {location}"
    );
    // No HTML at all — the whole point of #1010. A body here would mean
    // Navigator is still hosting some part of the ceremony.
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&body).contains("<iframe"),
        "no iframe survives the referral-out"
    );
}

#[tokio::test]
async fn a_notation_not_yet_sent_for_signature_conflicts() {
    // No `signatures` row: there is no envelope, so there is nothing to sign
    // and no URL to mint. This must not become a redirect to an empty ceremony.
    let (app, notation_id, _surreal) = app_with_notation().await;
    let response = get_sign(&app, notation_id).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        response.headers().get(header::LOCATION).is_none(),
        "a conflict is not a redirect"
    );
}

#[tokio::test]
async fn an_unknown_notation_is_not_found() {
    let (app, _, _surreal) = app_with_notation().await;
    let response = get_sign(&app, Uuid::now_v7()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers().get(header::LOCATION).is_none(),
        "a miss is not a redirect"
    );
}
