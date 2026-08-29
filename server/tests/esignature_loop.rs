#![allow(clippy::doc_markdown)]
//! End-to-end integration test for the closed signature loop, driven
//! through the **real** `DocuSignSignatureProvider` against a mocked
//! DocuSign HTTP endpoint (wiremock).
//!
//! The loop:
//!   1. Walk the retainer questionnaire to the end. The post-intake
//!      workflow renders the PDF and calls `send_for_signature` on the
//!      real DocuSign provider, which POSTs an envelope-create to the
//!      mocked endpoint and gets back `envelopeId`.
//!   2. That envelope id is persisted on the notation
//!      (`signature_request_id`) — the correlation the webhook needs.
//!   3. The provider's completion callback is POSTed to
//!      `/webhook/esignature/:secret` with a valid HMAC over the raw
//!      body; the notation reaches END.
//!
//! Proves the whole loop including the correlation: the envelope id the
//! provider returned is the one the webhook resolves back to a notation.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::signature::DocuSignSignatureProvider;
use portal::webhook_auth::sign_hmac_sha256_b64;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__letter";
const HMAC_KEY: &str = "loop-test-hmac-key";
const ENVELOPE_ID: &str = "env-loop-1";

fn completion_body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "event": "envelope-completed",
        "data": {
            "envelopeId": ENVELOPE_ID,
            "envelopeSummary": { "status": "completed" },
        },
    }))
    .unwrap()
}

fn urlencoding(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn full_signature_loop_reaches_end_through_real_provider_and_webhook() {
    // 1. Mock DocuSign's envelope-create + document-download endpoints.
    let docusign = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2.1/accounts/acct-guid/envelopes"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({"envelopeId": ENVELOPE_ID, "status": "sent"})),
        )
        .mount(&docusign)
        .await;
    // The completion webhook downloads the signed PDF + Certificate of
    // Completion to archive them.
    Mock::given(method("GET"))
        .and(path(format!(
            "/v2.1/accounts/acct-guid/envelopes/{ENVELOPE_ID}/documents/combined"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-signed-retainer".to_vec()))
        .mount(&docusign)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/v2.1/accounts/acct-guid/envelopes/{ENVELOPE_ID}/documents/certificate"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-certificate".to_vec()))
        .mount(&docusign)
        .await;

    let surreal = mem_surreal().await;
    // `generate_pdf__retainer_pdf` is worker-dispatched and web reads
    // the template body + rendered PDF back from storage, so seed and
    // AppState share one storage handle.
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-esignature-loop-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    // Hold a handle so we can assert the archived documents after the
    // shared `storage` is moved into AppState below.
    let storage_handle = storage.clone();
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts onboarding__letter");
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();
    let proj = store::projects::create(
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
    let nid = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, libra.id, proj.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    let runtime = Arc::new(InMemoryRuntime::new());
    let provider = DocuSignSignatureProvider::new(
        docusign.uri(),
        "acct-guid",
        "TOKEN",
        "signer@example.com",
        "Signer",
    );
    // The workflow timeline runs through `DispatchingRuntime` (renders +
    // persists the retainer PDF); web reads it back for the signature
    // send — all against the shared `storage` created above.
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        workflows::DispatchingRuntime::new(runtime.clone(), email.clone(), storage.clone()),
    );
    let state = AppState {
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime.clone(),
        signature_provider: Arc::new(provider),
        esignature_hmac_key: Some(HMAC_KEY.to_string()),
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // 2. Walk the full questionnaire — the final POST drives the
    //    workflow through the real provider's send_for_signature.
    for value in [
        "Libra Holdings LLC",
        "500 Innovation Way Reno NV 89501",
        "Libra",
        "Firm Principal",
        "Estate plan",
        "2026-09-01",
        "Draft and file the matter documents.",
        "nevada",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/lawyer/notations/{nid}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", urlencoding(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::SEE_OTHER,
            "walk step status {value}: {}",
            resp.status()
        );
    }

    // The generic retainer leaves its fee terms to the custom-clause slot,
    // and dispatch refuses an engagement agreement whose slot is empty
    // (NRPC 1.5(b), Cal. B&P § 6148). Write the clause the lawyer would.
    store::notation_clauses::append(
        &surreal,
        nid,
        "**Fees.** Billed at $400 per hour; expenses passed through at cost.",
        None,
    )
    .await
    .unwrap();

    // The client's final answer parks at `lawyer_review` — no render on the
    // completion request. Lawyer then approves (renders + persists the PDF at
    // `generate_pdf__*`) and sends (creates the real provider's envelope).
    for action in ["approve-send", "send"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/lawyer/notations/{nid}/{action}"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Both actions are post/redirect/get onto the review screen since it
        // moved to Dioxus: a refresh after dispatching an envelope re-reads the
        // screen instead of re-posting the send.
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{action} status");
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some(format!("/lawyer/notations/{nid}/review").as_str()),
            "{action} redirect target",
        );
    }

    // The real provider returned ENVELOPE_ID and it was persisted.
    let row = store::notations::find_by_id(&surreal, nid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "sent_for_signature__pending");
    assert_eq!(
        store::signatures::request_id_for_notation(&surreal, nid)
            .await
            .unwrap()
            .as_deref(),
        Some(ENVELOPE_ID)
    );

    // End-to-end anchor proof: the retainer template's signature blocks
    // travelled template -> Typst -> rendered PDF -> DocuSign envelope.
    // The captured POST body carries the client and firm anchor strings.
    let envelope_post = docusign
        .received_requests()
        .await
        .expect("mock recorded requests")
        .into_iter()
        .find(|r| r.url.path().ends_with("/envelopes"))
        .expect("an envelope-create POST was made");
    let posted = String::from_utf8_lossy(&envelope_post.body);
    assert!(
        posted.contains("nlsig-client-signature-1"),
        "client signature anchor must reach DocuSign: {posted}"
    );
    assert!(
        posted.contains("nlsig-firm-signature-1"),
        "firm countersignature anchor must reach DocuSign"
    );

    // 3. Provider posts a validly-signed completion callback → END.
    let body = completion_body();
    let signature = sign_hmac_sha256_b64(HMAC_KEY.as_bytes(), &body);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/esignature/any-token")
                .header("content-type", "application/json")
                .header("x-docusign-signature-1", signature)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "body: {}",
        body_string(resp).await
    );

    let row = store::notations::find_by_id(&surreal, nid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "END");
    let final_state =
        StateMachineRuntime::current_state(runtime.as_ref(), MachineKind::Workflow, nid)
            .await
            .unwrap();
    assert_eq!(final_state.as_str(), "END");

    // 4. The completion webhook archived the executed document set —
    // the signed retainer + Certificate of Completion — to storage.
    let signed = storage_handle
        .get(&store::notations::signed_document_storage_key(nid))
        .await
        .expect("signed retainer archived")
        .bytes;
    assert_eq!(signed, b"%PDF-signed-retainer");
    let cert = storage_handle
        .get(&store::notations::certificate_of_completion_storage_key(
            nid,
        ))
        .await
        .expect("certificate of completion archived")
        .bytes;
    assert_eq!(cert, b"%PDF-certificate");
}
