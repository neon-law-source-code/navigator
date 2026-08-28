#![allow(clippy::doc_markdown, clippy::too_many_lines)]
//! Integration tests for the contract-review workbench REST doors:
//! `POST /app/api/contract-reviews/{id}/{findings/{idx},summary,approve,reject}`.
//!
//! The write engines (`admin_contract_reviews::{save_review_finding,
//! save_review_summary, approve_review, reject_review}`) are shared with the
//! lawyer review surface, so this focuses on what the REST adapters add: the tier
//! gate (client 403, anon 401), the matter-scope gate (a non-participant lawyer
//! is a bare 404, admin bypasses), the approval guard (422 before every finding
//! is acted on), and the live accept → summary → approve / reject flows.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::SessionData;
use portal::AppState;
use store::persons::Role;
use store::playbooks::{NewPlaybook, Position};
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{DispatchingRuntime, InMemoryRuntime, IntakeArtifact};

struct Harness {
    app: axum::Router,
    admin_state: portal::admin::AdminState,
    surreal: store::surreal::SurrealDb,
    admin_bearer: String,
}

fn bearer(person_id: Option<Uuid>, role: Role) -> String {
    let mut s = SessionData::fresh("api-cr-sub", role);
    s.person_id = person_id;
    format!(
        "Bearer {}",
        portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY).encode(&s)
    )
}

async fn harness() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("nav-api-cr-{}", Uuid::now_v7())))
            .await
            .unwrap(),
    );
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let inner = Arc::new(InMemoryRuntime::new());
    let runtime: Arc<dyn workflows::StateMachineRuntime> = Arc::new(
        DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    let admin_state = portal::admin::AdminState {
        surreal: surreal.clone(),
        workflow_runtime: runtime.clone(),
        signature_provider: Arc::new(portal::signature::StubSignatureProvider::new()),
        retainer_intake_questionnaire: workflows::retainer_intake_questionnaire(),
        questionnaire_runtime: inner.clone(),
        storage: storage.clone(),
        assets_storage: storage.clone(),
        forms_registry: Arc::new(forms::registry().unwrap()),
        email: email.clone(),
        billing_provider: Arc::new(portal::billing::StubBillingProvider::new()),
        contract_reviewer: Arc::new(portal::contract_review::StubContractReviewer),
        bootstrap_owner_email: None,
        bootstrap_company: portal::admin::DEFAULT_BOOTSTRAP_COMPANY.into(),
        sessions: portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY),
        secure_cookies: false,
    };
    let state = AppState {
        storage: storage.clone(),
        workflow_runtime: runtime,
        questionnaire_runtime: inner,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    Harness {
        app,
        admin_state,
        surreal,
        admin_bearer: bearer(None, Role::Admin),
    }
}

/// Seed a review parked at `lawyer_review` and return `(review_id, project_id)`.
async fn seed_review_at_lawyer_review(h: &Harness) -> (Uuid, Uuid) {
    let entity_id = store::test_support::seed_entity(&h.surreal).await;
    let project_id = store::projects::create(
        &h.surreal,
        &store::projects::NewProject {
            code: format!("contract-review-{}", Uuid::now_v7()),
            name: "Contract review".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id;
    let person_id = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::new("Aquarius", format!("aq-{}@example.com", Uuid::now_v7())),
    )
    .await
    .unwrap()
    .id;
    let _ = store::templates::save_version(
        &h.surreal,
        None,
        "memo__contract_review",
        store::templates::Version {
            title: "Inbound Contract Review".into(),
            respondent_type: "person_and_entity".into(),
            asset_id: None,
            form_code: None,
            kind: Some("memo".into()),
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let positions = vec![Position {
        topic: "Limitation of liability".into(),
        preferred: "Mutual cap at 12 months' fees".into(),
        fallback: "Cap at 2x fees paid".into(),
        walkaway: "Uncapped liability".into(),
        severity: store::playbooks::SEVERITY_HIGH.into(),
    }];
    store::playbooks::create(
        &h.surreal,
        &NewPlaybook {
            entity_id,
            name: "Vendor MSA playbook",
            positions: &positions,
        },
    )
    .await
    .unwrap();
    let deps = portal::contract_review_walk::ReviewDeps {
        surreal: &h.admin_state.surreal,
        workflow_runtime: h.admin_state.workflow_runtime.as_ref(),
        storage: &h.admin_state.storage,
        contract_reviewer: h.admin_state.contract_reviewer.as_ref(),
    };
    let review_id = portal::contract_review_walk::drive_contract_review(
        &deps,
        project_id,
        person_id,
        "vendor-msa.txt",
        "MASTER SERVICES AGREEMENT. Liability is uncapped.",
        IntakeArtifact::Text {
            text: "MASTER SERVICES AGREEMENT. Liability is uncapped.".into(),
        },
    )
    .await
    .unwrap();
    (review_id, project_id)
}

async fn post(
    h: &Harness,
    uri: &str,
    auth: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    h.app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn finding_count(h: &Harness, review_id: Uuid) -> usize {
    let review = store::contract_reviews::by_id(&h.surreal, review_id)
        .await
        .unwrap()
        .unwrap();
    store::contract_reviews::findings_of(&review)
        .unwrap_or_default()
        .len()
}

#[tokio::test]
async fn an_attorney_acts_on_findings_edits_the_summary_and_approves() {
    let h = harness().await;
    let (review_id, _project_id) = seed_review_at_lawyer_review(&h).await;

    // Accept every finding through the API.
    for idx in 0..finding_count(&h, review_id).await {
        let resp = post(
            &h,
            &format!("/app/api/contract-reviews/{review_id}/findings/{idx}"),
            Some(&h.admin_bearer),
            serde_json::json!({ "accept": true, "severity": "high", "attorney_note": "Push a cap." }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    let summary = post(
        &h,
        &format!("/app/api/contract-reviews/{review_id}/summary"),
        Some(&h.admin_bearer),
        serde_json::json!({ "risk_summary": "One high-severity deviation." }),
    )
    .await;
    assert_eq!(summary.status(), StatusCode::NO_CONTENT);

    let approve = post(
        &h,
        &format!("/app/api/contract-reviews/{review_id}/approve"),
        Some(&h.admin_bearer),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(approve.status(), StatusCode::NO_CONTENT);
    let review = store::contract_reviews::by_id(&h.surreal, review_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.status, store::contract_reviews::STATUS_APPROVED);
}

#[tokio::test]
async fn approving_before_acting_on_findings_is_422() {
    let h = harness().await;
    let (review_id, _project_id) = seed_review_at_lawyer_review(&h).await;
    let resp = post(
        &h,
        &format!("/app/api/contract-reviews/{review_id}/approve"),
        Some(&h.admin_bearer),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejecting_the_review_ends_it() {
    let h = harness().await;
    let (review_id, _project_id) = seed_review_at_lawyer_review(&h).await;
    let resp = post(
        &h,
        &format!("/app/api/contract-reviews/{review_id}/reject"),
        Some(&h.admin_bearer),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let review = store::contract_reviews::by_id(&h.surreal, review_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.status, store::contract_reviews::STATUS_REJECTED);
}

#[tokio::test]
async fn a_non_participant_lawyer_is_404() {
    let h = harness().await;
    let (review_id, _project_id) = seed_review_at_lawyer_review(&h).await;
    // A lawyer with a linked Person who does not participate in the matter.
    let stranger = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::with_role("Stranger", "stranger@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let resp = post(
        &h,
        &format!("/app/api/contract-reviews/{review_id}/summary"),
        Some(&bearer(Some(stranger.id), Role::Lawyer)),
        serde_json::json!({ "risk_summary": "let me in" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_client_is_403_and_anonymous_is_401() {
    let h = harness().await;
    let (review_id, _project_id) = seed_review_at_lawyer_review(&h).await;
    let client = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::with_role("Client", "client@example.com", Role::Client),
    )
    .await
    .unwrap();
    let uri = format!("/app/api/contract-reviews/{review_id}/reject");
    assert_eq!(
        post(
            &h,
            &uri,
            Some(&bearer(Some(client.id), Role::Client)),
            serde_json::json!({})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&h, &uri, None, serde_json::json!({})).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
