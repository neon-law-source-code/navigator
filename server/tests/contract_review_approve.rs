//! Inbound contract review — the attorney review screen through memo
//! delivery, driven over the real HTTP routes.
//!
//! Sets up a review parked at `lawyer_review` (via the same public pipeline
//! entry the upload route uses), then as an admin: GETs the review screen,
//! accepts the finding, and approves — asserting the workflow reaches `END`,
//! the review is `approved`, and the memo PDF is filed into the Project
//! (`documents` row + storage).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

use portal::session::SessionData;
use portal::AppState;
use store::playbooks::{NewPlaybook, Position};
use store::test_support::mem_surreal;
use workflows::{DispatchingRuntime, InMemoryRuntime, IntakeArtifact};

struct Harness {
    app: axum::Router,
    admin_state: portal::admin::AdminState,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    admin_bearer: String,
}

async fn harness() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-contract-approve-test"))
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
        sessions: portal::SessionStore::new("test-session-key-not-for-production"),
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

    // An admin session blob, presented as a Bearer credential (the CLI
    // path) so it injects an admin `SessionData` and bypasses CSRF.
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let admin_bearer = sessions.encode(&SessionData::fresh(
        "nick@neonlaw.com",
        store::persons::Role::Admin,
    ));

    Harness {
        app,
        admin_state,
        surreal,
        storage,
        admin_bearer,
    }
}

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
        &store::persons::NewPerson::new(
            "Aquarius",
            format!("aquarius-{}@example.com", Uuid::now_v7()),
        ),
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

async fn post(h: &Harness, uri: &str, body: &'static str) -> axum::http::Response<Body> {
    h.app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", format!("Bearer {}", h.admin_bearer))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn attorney_accepts_finding_and_approves_delivering_the_memo() {
    let h = harness().await;
    let (review_id, project_id) = seed_review_at_lawyer_review(&h).await;

    // The review screen renders.
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/app/lawyer/contract-reviews/{review_id}"))
                .header("authorization", format!("Bearer {}", h.admin_bearer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Approving before acting on the finding is refused (no memo, still
    // at lawyer_review).
    let resp = post(
        &h,
        &format!("/app/lawyer/contract-reviews/{review_id}/approve"),
        "",
    )
    .await;
    // Post/redirect/get: the refusal comes back as an `?error=` flash on the
    // review screen, not an inline re-render.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(
        location.starts_with(&format!("/app/lawyer/contract-reviews/{review_id}?error=")),
        "location: {location}"
    );
    let notation_row = notation_for_project(&h.surreal, project_id).await;
    assert_eq!(notation_row.state, "lawyer_review");

    // Accept the one finding.
    let resp = post(
        &h,
        &format!("/app/lawyer/contract-reviews/{review_id}/findings/0"),
        "decision=accept&severity=high&suggested_redline=Add+a+mutual+cap.&attorney_note=Push+this.",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Now approve — assembles + delivers the memo, drives to END.
    let resp = post(
        &h,
        &format!("/app/lawyer/contract-reviews/{review_id}/approve"),
        "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // The review is approved and the workflow reached END.
    let review = store::contract_reviews::by_id(&h.surreal, review_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.status, store::contract_reviews::STATUS_APPROVED);
    let notation_row = notation_for_project(&h.surreal, project_id).await;
    assert_eq!(notation_row.state, "END");
    let findings = store::contract_reviews::findings_of(&review).unwrap();
    assert!(findings[0].accepted);
    assert_eq!(findings[0].attorney_note.as_deref(), Some("Push this."));

    // The memo PDF was filed into the Project as a documents row and to
    // storage.
    let memo = store::assets::latest_of_kind(&h.surreal, project_id, "memo")
        .await
        .unwrap()
        .expect("memo document row exists");
    assert_eq!(memo.filename.as_deref(), Some("review-memo.pdf"));
    assert!(h
        .storage
        .exists(&portal::admin_contract_reviews::memo_storage_key(
            notation_row.id
        ))
        .await
        .unwrap());

    // The per-finding decision was recorded as an immutable attribution
    // event (distinct machine kind).
    let events: Vec<_> = store::notation_events::for_notation(&h.surreal, notation_row.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.machine_kind == store::contract_reviews::MACHINE_CONTRACT_REVIEW)
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].condition, "finding_accepted");
}

#[tokio::test]
async fn rejecting_the_review_ends_without_a_memo() {
    let h = harness().await;
    let (review_id, project_id) = seed_review_at_lawyer_review(&h).await;

    let resp = post(
        &h,
        &format!("/app/lawyer/contract-reviews/{review_id}/reject"),
        "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let review = store::contract_reviews::by_id(&h.surreal, review_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review.status, store::contract_reviews::STATUS_REJECTED);
    let notation_row = notation_for_project(&h.surreal, project_id).await;
    assert_eq!(notation_row.state, "END");

    // No memo document was filed.
    let memo = store::assets::latest_of_kind(&h.surreal, project_id, "memo")
        .await
        .unwrap();
    assert!(memo.is_none());
}

async fn notation_for_project(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
) -> store::notations::Notation {
    store::notations::list_by_project(surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("notation exists")
}
