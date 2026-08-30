#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/notations/{id}/signature` — the REST door
//! that dispatches a notation's rendered document for signature.
//!
//! The command core (`dispatch_signature`) is shared with the lawyer review
//! screen. These tests cover what the REST adapter adds: the tier gate
//! (LawyerSession → 401/403), the matter-scope gate (a non-participant lawyer
//! gets a bare 404, admin bypasses), the readiness gate (send before the PDF
//! is rendered → 409 document_not_ready), and a live 200 after an approval
//! rendered the PDF (a `DispatchingRuntime` runs the generate-PDF worker; a
//! `StubSignatureProvider` records the dispatched envelope).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "api-notation-send-test-key";
/// The fee terms a lawyer writes into the engagement agreement's
/// custom-clause slot before it can be sent for signature.
const FEE_CLAUSE: &str = "**Fees.** This matter is billed at $400 per hour against the \
                          engagement's rate sheet; expenses are passed through at cost.";

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    /// The workflow-machine runtime (a `DispatchingRuntime`, so the generate-PDF
    /// worker actually renders on approval) — also what drives the fixture to
    /// the `lawyer_review` gate.
    workflow_runtime: Arc<dyn StateMachineRuntime>,
    /// The stub the router dispatches through, so a test can assert the
    /// provider was never called.
    signature_provider: Arc<portal::signature::StubSignatureProvider>,
}

async fn build_app() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-send-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let questionnaire_runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    // The generate_pdf__* step is worker-dispatched; wrap the in-memory runtime
    // in DispatchingRuntime so approve renders + persists the PDF that send
    // gates on.
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(DispatchingRuntime::new(
        questionnaire_runtime.clone(),
        email.clone(),
        storage.clone(),
    ));
    let signature_provider = Arc::new(portal::signature::StubSignatureProvider::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: workflow_runtime.clone(),
        questionnaire_runtime,
        email,
        signature_provider: signature_provider.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Harness {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        workflow_runtime,
        signature_provider,
    }
}

async fn open_project(surreal: &store::surreal::SurrealDb) -> store::projects::Project {
    store::test_support::seed_project(surreal, "Matter").await
}

async fn bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: Role,
    project: Option<uuid::Uuid>,
) -> String {
    let actor = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    if let Some(project_id) = project {
        store::projects::add_participation(surreal, project_id, actor.id, "lawyer")
            .await
            .unwrap();
    }
    let mut s = SessionData::fresh("api-send-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// Insert a retainer notation on `project` bound to a fresh client, driven to
/// the `lawyer_review` gate (no PDF rendered yet — that needs an approval).
async fn seed_lawyer_review_notation(h: &Harness, project_id: uuid::Uuid) -> uuid::Uuid {
    let client = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::with_role(
            "libra@example.com",
            "libra@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let tmpl = store::templates::resolve(&h.surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed_canonical inserts onboarding__letter");
    let notation_id = store::notations::create(
        &h.surreal,
        &store::notations::NewNotation::new(tmpl.id, client.id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    portal::retainer_walk::advance_to_lawyer_review(
        &h.surreal,
        h.workflow_runtime.as_ref(),
        notation_id,
        None,
    )
    .await
    .expect("drives to lawyer_review");
    // The generic retainer leaves its fee terms to the custom-clause slot,
    // and dispatch refuses an engagement agreement whose slot is empty
    // (NRPC 1.5(b), Cal. B&P § 6148). Write the clause the lawyer would.
    store::notation_clauses::append(&h.surreal, notation_id, FEE_CLAUSE, None)
        .await
        .expect("append the fee clause");
    notation_id
}

async fn post(
    app: &axum::Router,
    auth: Option<&str>,
    notation_id: uuid::Uuid,
    action: &str,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/notations/{notation_id}/{action}"))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;

    let resp = post(&h.app, None, notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_is_403() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let client = bearer(&h.surreal, "client@example.com", Role::Client, None).await;

    let resp = post(&h.app, Some(&client), notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lawyer_outside_the_matter_scope_is_404() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = post(&h.app, Some(&outsider), notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_before_approve_is_409_document_not_ready() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    // No approval yet → the worker never rendered the PDF → 409, no envelope.
    let resp = post(&h.app, Some(&lawyer), notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "document_not_ready");
}

#[tokio::test]
async fn approve_then_send_dispatches_and_returns_the_request_id() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    // Approve first — the DispatchingRuntime's generate-PDF worker renders and
    // persists the PDF that send gates on.
    let approve = post(&h.app, Some(&lawyer), notation_id, "approval").await;
    assert_eq!(approve.status(), StatusCode::OK, "approval renders the PDF");

    // Now dispatch for signature.
    let resp = post(&h.app, Some(&lawyer), notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["notation_id"], notation_id.to_string());
    assert!(
        json["signature_request_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the response carries a non-empty signature_request_id: {json}"
    );
}

/// The engagement agreement's fee terms arrive as custom clauses, and
/// `splice` will happily substitute the marker with nothing — so without a
/// guard a retainer dispatches for signature with no fee terms in it. NRPC
/// 1.5(b) requires the basis of the fee be communicated, and Cal. B&P
/// § 6148 requires it in writing for a matter likely to exceed $1,000, so
/// that dispatch must not happen. The provider is never reached.
#[tokio::test]
async fn a_retainer_with_no_clauses_is_refused_and_never_reaches_the_provider() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    // Undo the fixture's clause: this is the empty-slot case.
    for clause in store::notation_clauses::for_notation(&h.surreal, notation_id)
        .await
        .unwrap()
    {
        store::notation_clauses::delete(&h.surreal, clause.id)
            .await
            .unwrap();
    }
    let lawyer = bearer(
        &h.surreal,
        "acting-lawyer@example.com",
        Role::Lawyer,
        Some(project.id),
    )
    .await;

    let approve = post(&h.app, Some(&lawyer), notation_id, "approval").await;
    assert_eq!(approve.status(), StatusCode::OK, "approval renders the PDF");

    let resp = post(&h.app, Some(&lawyer), notation_id, "signature").await;
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an engagement agreement with no fee terms must not dispatch"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "clauses_required");
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|m| m.contains("fee terms have not been written")),
        "the refusal names what is missing: {json}"
    );

    assert!(
        h.signature_provider.calls().is_empty(),
        "the signature provider was never called: {:?}",
        h.signature_provider.calls()
    );

    // The walk is resumable: writing the clause and sending again works.
    store::notation_clauses::append(&h.surreal, notation_id, FEE_CLAUSE, None)
        .await
        .unwrap();
    let resent = post(&h.app, Some(&lawyer), notation_id, "signature").await;
    assert_eq!(
        resent.status(),
        StatusCode::OK,
        "the refusal parks the walk where a lawyer can fix it and retry"
    );
    assert_eq!(
        h.signature_provider.calls().len(),
        1,
        "exactly one envelope, dispatched after the clause was written"
    );
}

#[tokio::test]
async fn admin_bypasses_scope_and_sends() {
    let h = build_app().await;
    let project = open_project(&h.surreal).await;
    let notation_id = seed_lawyer_review_notation(&h, project.id).await;
    let admin = bearer(&h.surreal, "admin@example.com", Role::Admin, None).await;

    let approve = post(&h.app, Some(&admin), notation_id, "approval").await;
    assert_eq!(approve.status(), StatusCode::OK);
    let resp = post(&h.app, Some(&admin), notation_id, "signature").await;
    assert_eq!(resp.status(), StatusCode::OK);
}
