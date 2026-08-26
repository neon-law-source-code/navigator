#![allow(clippy::doc_markdown)]
//! Server-side e2e for the `navigator` CLI's bearer path.
//!
//! Proves the CLI drives the **existing** matter-open routes over an
//! `Authorization: Bearer <SessionData>` credential — the same blob the
//! browser cookie carries. CI-safe: the `StubSignatureProvider` records the
//! send, so nothing reaches DocuSign.
//!
//! Covers:
//!   1. A real minted `SessionData` bearer opens a retainer through the
//!      walker and parks at `lawyer_review`. `approve-send` renders then
//!      parks the PDF at `generate_pdf__retainer_pdf` (no envelope yet);
//!      the separate `send` then dispatches exactly one envelope. A `send`
//!      attempted before the PDF is rendered returns `409`.
//!   2. `GET /lawyer/notations/:id/review?format=json` returns the
//!      workflow state, signature request id, and `document_ready` (the
//!      `notation status` command's contract).
//!   3. An **expired** session bearer is rejected (the matter-open POST
//!      does not create a project).
//!   4. `GET /auth/cli/whoami` echoes the bearer caller's identity.
//!   5. `GET /auth/cli/start` refuses a non-loopback `redirect`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{now_unix_secs, SessionData};
use portal::signature::StubSignatureProvider;
use portal::{AppState, AuthConfig, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

const SESSION_KEY: &str = "cli-bearer-test-key-not-for-production";

async fn build_app(
    tag: &str,
) -> (
    axum::Router,
    store::surreal::SurrealDb,
    Arc<StubSignatureProvider>,
) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-cli-bearer-repos-{tag}-{}",
        uuid::Uuid::now_v7(),
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-cli-bearer-{tag}")))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(DispatchingRuntime::new(
        runtime.clone(),
        email.clone(),
        storage.clone(),
    ));
    let stub = Arc::new(StubSignatureProvider::new());
    let state = AppState {
        // Auth ENFORCED via HS256 so the Bearer path is exercised for
        // real: a session blob must reach the handler through
        // `inject_bearer_session`, not through a disabled pass-through.
        auth: AuthConfig::new(false, Some("unused-hs256-secret")),
        sessions: SessionStore::new(SESSION_KEY),
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: stub.clone(),
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        stub,
    )
}

/// A fresh admin session bearer, minted exactly as `/auth/cli/start`
/// would, signed with the test session key.
fn admin_bearer() -> String {
    let mut session = SessionData::fresh("cli-admin", Role::Admin);
    session.email = Some("nick@neonlaw.com".into());
    SessionStore::new(SESSION_KEY).encode(&session)
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn enc(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cli_bearer_opens_retainer_then_approve_parks_and_send_dispatches_once() {
    let (app, surreal, stub) = build_app("happy").await;
    let bearer = format!("Bearer {}", admin_bearer());

    // Open the retainer through the walker door — `POST /app/projects`
    // opens the matter and only the matter, so the retainer (and the
    // lifecycle this test is about) starts here. Driven with the same
    // minted `SessionData` bearer, which is the point of the test.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/lawyer/retainers/new")
                .header("authorization", &bearer)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "client_email={}&retainer_template_code=onboarding__retainer",
                    enc("nick@shook.family"),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let notation_id: uuid::Uuid = loc
        .trim_start_matches("/lawyer/notations/")
        .trim_end_matches("/step")
        .parse()
        .expect("redirect carries the notation id");

    // Walk the retainer questionnaire (client name, firm DRI, engagement
    // name, engagement start date, engagement scope, fee basis, then the
    // governing-law choice) — the last answer drives the workflow to the
    // `lawyer_review` gate, which is where this test's subject begins.
    for value in [
        "Nick Shook",
        "Firm Principal",
        "Shook estate",
        "2026-09-01",
        "Draft and file the matter documents.",
        "450 per hour",
        "nevada",
    ] {
        let step = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/lawyer/notations/{notation_id}/step"))
                    .header("authorization", &bearer)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", enc(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            step.status() == StatusCode::SEE_OTHER || step.status() == StatusCode::OK,
            "walking the questionnaire returned {}",
            step.status(),
        );
    }

    // Parked at lawyer_review — the gate is intact; no envelope yet.
    let notation = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("retainer notation inserted");
    assert_eq!(notation.state, "lawyer_review");
    // The walker's client signs embedded (captive, in the portal) rather
    // than being emailed a link.
    assert_eq!(notation.delivery, "embedded");
    assert!(stub.calls().is_empty());

    // The provenance is attributable: the walk's client is the matter's
    // client-side DRI (a first-class column on the project, not a
    // `client_dri` participation row).
    let person = store::persons::find_by_email_ci(&surreal, "nick@shook.family")
        .await
        .unwrap()
        .expect("client person exists");
    let participation =
        store::projects::participation_for_person(&surreal, person.id, notation.project_id)
            .await
            .unwrap()
            .expect("client participation inserted");
    assert!(participation.is_client_dri);

    // A small closure for the repeated bearer POST / GET shapes.
    let get_status = {
        let app = app.clone();
        let bearer = bearer.clone();
        move || {
            let app = app.clone();
            let bearer = bearer.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::builder()
                            .uri(format!(
                                "/lawyer/notations/{notation_id}/review?format=json"
                            ))
                            .header("authorization", &bearer)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                let json: serde_json::Value =
                    serde_json::from_str(&body_string(resp).await).unwrap();
                json
            }
        }
    };
    let post = |path: String| {
        let app = app.clone();
        let bearer = bearer.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", &bearer)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    // `notation status` JSON view reflects the parked state: no envelope,
    // and no PDF rendered yet (`document_ready:false`).
    let status_json = get_status().await;
    assert_eq!(status_json["state"], "lawyer_review");
    assert!(status_json["signature_request_id"].is_null());
    assert_eq!(status_json["document_ready"], false);

    // `send` BEFORE the PDF is rendered → 409 with a JSON reason, and no
    // envelope goes out. The readiness gate is what stops a send against a
    // worker that hasn't (or can't) render.
    let early_send = post(format!("/lawyer/notations/{notation_id}/send")).await;
    assert_eq!(early_send.status(), StatusCode::CONFLICT);
    let early_json: serde_json::Value =
        serde_json::from_str(&body_string(early_send).await).unwrap();
    assert_eq!(early_json["error"], "document_not_ready");
    assert!(early_json["reason"].is_string());
    assert!(stub.calls().is_empty(), "no envelope before send");

    // The generic retainer leaves its fee terms to the custom-clause slot,
    // and dispatch refuses an engagement agreement whose slot is empty
    // (NRPC 1.5(b), Cal. B&P § 6148). Write the clause the lawyer would.
    store::notation_clauses::append(
        &surreal,
        notation_id,
        "**Fees.** Billed at $400 per hour; expenses passed through at cost.",
        None,
    )
    .await
    .unwrap();

    // The lawyer approves → renders + parks at generate_pdf__retainer_pdf. The
    // in-process DispatchingRuntime renders + persists the PDF inline, so
    // the workflow waits at the document step with the PDF present — but
    // NO envelope has gone out yet.
    let approve = post(format!("/lawyer/notations/{notation_id}/approve-send")).await;
    // Approve and send are post/redirect/get onto the review screen since it
    // moved to Dioxus — a refresh re-reads it instead of re-posting.
    assert_eq!(approve.status(), StatusCode::SEE_OTHER);
    assert!(
        stub.calls().is_empty(),
        "approve renders + parks; it must NOT send"
    );
    let row = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "generate_pdf__retainer_pdf");
    assert!(
        store::signatures::request_id_for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .is_none()
    );

    // Status now shows the parked-and-rendered state: document_ready:true,
    // still no envelope.
    let parked_json = get_status().await;
    assert_eq!(parked_json["state"], "generate_pdf__retainer_pdf");
    assert_eq!(parked_json["document_ready"], true);
    assert!(parked_json["signature_request_id"].is_null());

    // The deliberate send → exactly one envelope, lands at
    // sent_for_signature__pending.
    let send = post(format!("/lawyer/notations/{notation_id}/send")).await;
    assert_eq!(send.status(), StatusCode::SEE_OTHER);
    assert_eq!(stub.calls().len(), 1, "exactly one envelope should be sent");
    let row = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "sent_for_signature__pending");
    assert!(
        store::signatures::request_id_for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .is_some()
    );

    // The status JSON now carries the signature request id.
    let sent_json = get_status().await;
    assert_eq!(sent_json["state"], "sent_for_signature__pending");
    assert!(sent_json["signature_request_id"].is_string());

    // `send` again is idempotent: it reuses the existing envelope, fires
    // no second send.
    let resend = post(format!("/lawyer/notations/{notation_id}/send")).await;
    assert_eq!(resend.status(), StatusCode::SEE_OTHER);
    assert_eq!(stub.calls().len(), 1, "resend must not double-send");
}

#[tokio::test]
async fn expired_session_bearer_is_rejected_with_no_matter() {
    let (app, surreal, _stub) = build_app("expired").await;

    let mut session = SessionData::fresh("cli-admin", Role::Admin);
    session.exp = now_unix_secs() - 60; // expired a minute ago
    let token = SessionStore::new(SESSION_KEY).encode(&session);

    let body = format!("name={}&status=open", enc("Expired matter"));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/projects")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    // The expired blob never resolves to a session; with auth enforced
    // and no AuthClaims injected, require_auth rejects it.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        store::projects::find_by_name(&surreal, "Expired matter")
            .await
            .unwrap()
            .is_none(),
        "an expired token must not open a matter",
    );
}

#[tokio::test]
async fn whoami_echoes_the_bearer_identity() {
    let (app, _surreal, _stub) = build_app("whoami").await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/cli/whoami")
                .header("authorization", format!("Bearer {}", admin_bearer()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(json["email"], "nick@neonlaw.com");
    assert_eq!(json["role"], "admin");
    assert!(json["exp"].is_number());
}

#[tokio::test]
async fn whoami_without_a_bearer_is_unauthorized() {
    let (app, _surreal, _stub) = build_app("whoami-none").await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/cli/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cli_start_refuses_a_non_loopback_redirect() {
    let (app, _surreal, _stub) = build_app("redirect-guard").await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/cli/start?redirect=http://evil.example/cb&state=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
