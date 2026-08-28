//! Inbound contract-review pipeline (web seam): upload a contract, run the
//! playbook deviation analysis web-side (the deterministic
//! `StubContractReviewer`), open a `contract_reviews` row with findings, and
//! land the matter at `lawyer_review`.
//!
//! Drives [`portal::contract_review_walk::drive_contract_review`] directly (the
//! same public entry the multipart upload route calls) against a real
//! store + the `DispatchingRuntime`, so the `document_intake` side effect
//! files the contract blob exactly as it does in the app.

use std::sync::Arc;

use uuid::Uuid;

use store::playbooks::{NewPlaybook, Position};
use store::test_support::mem_surreal;
use workflows::{DispatchingRuntime, InMemoryRuntime, IntakeArtifact};

async fn admin_state(surreal: store::surreal::SurrealDb) -> portal::admin::AdminState {
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-contract-review-test"))
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
    portal::admin::AdminState {
        surreal: surreal.clone(),
        workflow_runtime: runtime.clone(),
        signature_provider: Arc::new(portal::signature::StubSignatureProvider::new()),
        retainer_intake_questionnaire: workflows::retainer_intake_questionnaire(),
        questionnaire_runtime: inner,
        assets_storage: storage.clone(),
        forms_registry: Arc::new(forms::registry().unwrap()),
        storage,
        email,
        billing_provider: Arc::new(portal::billing::StubBillingProvider::new()),
        contract_reviewer: Arc::new(portal::contract_review::StubContractReviewer),
        bootstrap_owner_email: None,
        bootstrap_company: portal::admin::DEFAULT_BOOTSTRAP_COMPANY.into(),
        sessions: portal::SessionStore::new("test-session-key-not-for-production"),
        secure_cookies: false,
    }
}

/// Seed an Entity + Project + client Person + the contract-review template,
/// returning `(project_id, person_id, entity_id)`.
async fn seed_matter(surreal: &store::surreal::SurrealDb) -> (Uuid, Uuid, Uuid) {
    let entity_id = store::test_support::seed_entity(surreal).await;
    let project_id = store::projects::create(
        surreal,
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
        surreal,
        &store::persons::NewPerson::new(
            "Aquarius",
            format!("aquarius-{}@example.com", Uuid::now_v7()),
        ),
    )
    .await
    .unwrap()
    .id;
    let _ = store::templates::save_version(
        surreal,
        None,
        "memo__contract_review",
        store::templates::Version {
            title: "Inbound Contract Review".into(),
            respondent_type: "person_and_entity".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap();
    (project_id, person_id, entity_id)
}

fn sample_positions() -> Vec<Position> {
    vec![
        Position {
            topic: "Limitation of liability".into(),
            preferred: "Mutual cap at 12 months' fees".into(),
            fallback: "Cap at 2x fees paid".into(),
            walkaway: "Uncapped liability".into(),
            severity: store::playbooks::SEVERITY_HIGH.into(),
        },
        Position {
            topic: "Governing law".into(),
            preferred: "Nevada".into(),
            fallback: "Delaware".into(),
            walkaway: "A jurisdiction with no nexus".into(),
            severity: store::playbooks::SEVERITY_MEDIUM.into(),
        },
    ]
}

#[tokio::test]
async fn upload_runs_analysis_and_parks_at_lawyer_review() {
    let surreal = mem_surreal().await;
    let state = admin_state(surreal.clone()).await;
    let (project_id, person_id, entity_id) = seed_matter(&surreal).await;
    let scoped_template = store::templates::save_version(
        &surreal,
        Some(project_id),
        "memo__contract_review",
        store::templates::Version {
            title: "Project Contract Review".into(),
            respondent_type: "person_and_entity".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();

    let positions = sample_positions();
    let playbook_id = store::playbooks::create(
        &surreal,
        &NewPlaybook {
            entity_id,
            name: "Vendor MSA playbook",
            positions: &positions,
        },
    )
    .await
    .unwrap();

    let deps = portal::contract_review_walk::ReviewDeps {
        surreal: &state.surreal,
        workflow_runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        contract_reviewer: state.contract_reviewer.as_ref(),
    };
    let review_id = portal::contract_review_walk::drive_contract_review(
        &deps,
        project_id,
        person_id,
        "vendor-msa.txt",
        "MASTER SERVICES AGREEMENT\nLiability is uncapped. Governed by the laws of Mars.",
        IntakeArtifact::Text {
            text: "MASTER SERVICES AGREEMENT\nLiability is uncapped.".into(),
        },
    )
    .await
    .expect("pipeline runs");

    // The review row carries the playbook, a risk summary, and one finding
    // per playbook position — every one un-accepted (the attorney must act).
    let review = store::contract_reviews::by_id(&surreal, review_id)
        .await
        .unwrap()
        .expect("review row exists");
    assert_eq!(review.playbook_id, playbook_id);
    assert_eq!(review.status, store::contract_reviews::STATUS_ANALYZED);
    let findings = store::contract_reviews::findings_of(&review).unwrap();
    assert_eq!(findings.len(), 2, "one finding per playbook position");
    assert!(findings.iter().all(|f| !f.accepted));
    assert!(review.risk_summary.is_some());

    // The inbound contract was filed into the project as an assets row.
    assert!(
        review.asset_id.is_some(),
        "the filed inbound-contract document is linked"
    );

    // The notation reached the attorney gate.
    let notation_row = store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("notation exists");
    assert_eq!(notation_row.state, "lawyer_review");
    assert_eq!(notation_row.entity_id, Some(entity_id));
    assert_eq!(
        notation_row.template_id, scoped_template.id,
        "contract review must pin the current project-scoped template"
    );
}

#[tokio::test]
async fn upload_without_a_playbook_is_rejected() {
    let surreal = mem_surreal().await;
    let state = admin_state(surreal.clone()).await;
    let (project_id, person_id, _entity_id) = seed_matter(&surreal).await;

    let deps = portal::contract_review_walk::ReviewDeps {
        surreal: &state.surreal,
        workflow_runtime: state.workflow_runtime.as_ref(),
        storage: &state.storage,
        contract_reviewer: state.contract_reviewer.as_ref(),
    };
    let err = portal::contract_review_walk::drive_contract_review(
        &deps,
        project_id,
        person_id,
        "vendor-msa.txt",
        "contract body",
        IntakeArtifact::Text {
            text: "contract body".into(),
        },
    )
    .await
    .expect_err("no playbook on file");
    assert!(matches!(
        err,
        portal::contract_review_walk::ContractReviewError::NoPlaybook
    ));

    // No notation was opened — we fail before touching the workflow.
    let count = store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 0);
}
