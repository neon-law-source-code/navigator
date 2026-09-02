#![allow(clippy::doc_markdown)]
//! HTTP-level coverage for the web surfaces the `navigator` formation CLI
//! drives: the questionnaire walker's machine-readable step
//! (`GET …/step?format=json`), the idempotent approve, and the
//! template-neutral document download (`…/documents/document`).
//!
//! It walks the `nv__llc_formation` (Nevada LLC) questionnaire over real
//! HTTP with an admin `SessionData` bearer — the same blob the CLI
//! presents — so the JSON contract the CLI parses is pinned here, fast,
//! without spawning the binary (the binary round-trip lives in
//! `cli/tests/llc_formation_e2e.rs`). CI-safe: the `StubSignatureProvider`
//! records the send, so nothing reaches DocuSign.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, AuthConfig, SessionStore};
use serde_json::Value;
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

const SESSION_KEY: &str = "nest-cli-surface-key-not-for-production";

async fn build_app() -> axum::Router {
    let repo_root =
        std::env::temp_dir().join(format!("navigator-nest-cli-repos-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-nest-cli-surface"))
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
    let state = AppState {
        // Auth ENFORCED so the bearer path is exercised for real: the
        // session blob reaches the handler via `inject_bearer_session`,
        // and the document download's required `SessionData` extension is
        // actually populated.
        auth: AuthConfig::new(false, Some("unused-hs256-secret")),
        sessions: SessionStore::new(SESSION_KEY),
        // The blank NV packet is pulled from the assets lane and
        // verified against its pin at fill time; stage synthetic blanks
        // with matching pins on this test's storage root.
        assets_storage: storage.clone(),
        forms_registry: portal::test_support::stage_blank_forms(storage.as_ref()).await,
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: Arc::new(portal::signature::StubSignatureProvider::new()),
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR))
}

fn admin_bearer() -> String {
    let mut session = SessionData::fresh("cli-admin", Role::Admin);
    session.email = Some("nick@neonlaw.com".into());
    format!("Bearer {}", SessionStore::new(SESSION_KEY).encode(&session))
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_bytes(resp: axum::http::Response<Body>) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn enc(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

async fn get(app: &axum::Router, bearer: &str, uri: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post(
    app: &axum::Router,
    bearer: &str,
    uri: &str,
    body: String,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", bearer)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The whole formation surface the CLI drives, end to end over HTTP:
/// open → walk the seven Nest questions as JSON → complete → status →
/// idempotent approve → download the filled packet.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn nest_walker_json_step_and_document_download_drive_the_formation() {
    let app = build_app().await;
    let bearer = admin_bearer();

    // Open the matter through the questionnaire-walker entry; the redirect
    // names the notation.
    let resp = post(
        &app,
        &bearer,
        "/app/lawyer/retainers/new",
        format!(
            "client_email={}&retainer_template_code=nv__llc_formation",
            enc("libra@example.com"),
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "matter open redirects"
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let notation_id = location
        .strip_prefix("/app/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .expect("redirect names the notation's step")
        .to_string();
    let step_uri = format!("/app/lawyer/notations/{notation_id}/step");

    // The first JSON step is `person__client`, not complete.
    let first: Value = serde_json::from_str(
        &body_string(get(&app, &bearer, &format!("{step_uri}?format=json")).await).await,
    )
    .unwrap();
    assert_eq!(first["complete"], Value::Bool(false));
    assert_eq!(first["question"]["code"], "person__client");
    assert_eq!(first["question"]["choices"].as_array().unwrap().len(), 0);

    // Walk the seven questions. The scalar answers post `value=`; the
    // `people_list` posts the widget's `p0_*` parts — exactly the bodies
    // the browser form and the CLI both send.
    let scalars = [
        ("person__client", "Libra"),
        ("entity__company", "Bright Star Ventures"),
        ("person__registered_agent", "Neon Law Registered Agent"),
        ("custom_single_choice__management_structure", "members"),
        // managing_members (people_list) handled below, then:
        ("custom_datetime__formation_date", "2026-07-01"),
    ];
    let mut scalar_iter = scalars.iter();
    loop {
        let step: Value = serde_json::from_str(
            &body_string(get(&app, &bearer, &format!("{step_uri}?format=json")).await).await,
        )
        .unwrap();
        if step["complete"] == Value::Bool(true) {
            break;
        }
        let code = step["question"]["code"].as_str().unwrap().to_string();
        let answer_type = step["question"]["answer_type"].as_str().unwrap();
        let body = if answer_type == "people_list" {
            assert_eq!(code, "people__managing_members");
            [
                ("p0_name", "Libra"),
                ("p0_street", "1 Main St"),
                ("p0_city", "Las Vegas"),
                ("p0_state", "NV"),
                ("p0_zip", "89101"),
                ("p0_country", "USA"),
            ]
            .iter()
            .map(|(k, v)| format!("{k}={}", enc(v)))
            .collect::<Vec<_>>()
            .join("&")
        } else {
            let (expected_code, value) = scalar_iter.next().expect("a scalar answer is queued");
            assert_eq!(&code, expected_code, "questions arrive in spec order");
            // The radio surfaces its canonical choices for a terminal to show.
            if code == "custom_single_choice__management_structure" {
                let choices = step["question"]["choices"].as_array().unwrap();
                let values: Vec<&str> =
                    choices.iter().filter_map(|c| c["value"].as_str()).collect();
                assert_eq!(values, vec!["managers", "members"]);
            }
            format!("value={}", enc(value))
        };
        let resp = post(&app, &bearer, &step_uri, body).await;
        assert!(
            resp.status().is_success() || resp.status().is_redirection(),
            "answering {code} returned {}",
            resp.status(),
        );
    }

    // The client's final answer parks at the `lawyer_review` gate — a true
    // human gate, no render on the completion request, so `document_ready`
    // is still false.
    let review_status = |app: &axum::Router, bearer: &str| {
        let app = app.clone();
        let bearer = bearer.to_string();
        let uri = format!("/app/lawyer/notations/{notation_id}/review?format=json");
        async move {
            serde_json::from_str::<Value>(&body_string(get(&app, &bearer, &uri).await).await)
                .unwrap()
        }
    };
    let status = review_status(&app, &bearer).await;
    assert_eq!(status["state"], "lawyer_review");
    assert_eq!(status["document_ready"], Value::Bool(false));

    // Lawyer approves: this renders + persists the packet at
    // `generate_pdf__articles_pdf` — the first time a PDF exists.
    let resp = post(
        &app,
        &bearer,
        &format!("/app/lawyer/notations/{notation_id}/approve-send"),
        String::new(),
    )
    .await;
    // Post/redirect/get onto the review screen since it moved to Dioxus; the
    // state assertions below read it back through `?format=json`.
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "approve renders + parks the packet",
    );
    let status = review_status(&app, &bearer).await;
    assert!(
        status["state"]
            .as_str()
            .is_some_and(|s| s.starts_with("generate_pdf__")),
        "approve parks at the generate_pdf step, got {:?}",
        status["state"],
    );
    assert_eq!(status["document_ready"], Value::Bool(true));

    // Lawyer sends: drives the packet to the signature wait.
    let resp = post(
        &app,
        &bearer,
        &format!("/app/lawyer/notations/{notation_id}/send"),
        String::new(),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "send creates the envelope"
    );
    let status = review_status(&app, &bearer).await;
    assert_eq!(status["state"], "sent_for_signature__pending");

    // The template-neutral `document` slug downloads the filled packet.
    let resp = get(
        &app,
        &bearer,
        &format!("/app/lawyer/notations/{notation_id}/documents/document"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "document download succeeds");
    let bytes = body_bytes(resp).await;
    assert!(bytes.starts_with(b"%PDF"), "the download is a PDF");
}
