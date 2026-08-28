#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/projects/{id}/contract-review` — a
//! multipart, client-writable door that uploads a contract for deviation
//! review.
//!
//! The command (`drive_contract_review`) is shared with the lawyer/portal form.
//! These tests cover the REST adapter: the tier/participation gate (401 anon,
//! 404 non-participant), the no-playbook precondition (422), and a live 201 —
//! against the deterministic `StubContractReviewer`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::playbooks::{NewPlaybook, Position};
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::InMemoryRuntime;

const KEY: &str = "api-contract-review-test-key";
const BOUNDARY: &str = "----navcontractboundary";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-contract-review-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        contract_reviewer: Arc::new(portal::contract_review::StubContractReviewer),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

/// Seed a matter (entity + project + the contract-review template) and return
/// `(project_id, entity_id)`.
async fn seed_matter(surreal: &store::surreal::SurrealDb) -> (Uuid, Uuid) {
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
    let _ = store::templates::save_version(
        surreal,
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
    .unwrap();
    (project_id, entity_id)
}

async fn seed_playbook(surreal: &store::surreal::SurrealDb, entity_id: Uuid) {
    let positions = vec![Position {
        topic: "Limitation of liability".into(),
        preferred: "Mutual cap at 12 months' fees".into(),
        fallback: "Cap at 2x fees paid".into(),
        walkaway: "Uncapped liability".into(),
        severity: store::playbooks::SEVERITY_HIGH.into(),
    }];
    store::playbooks::create(
        surreal,
        &NewPlaybook {
            entity_id,
            name: "Vendor MSA playbook",
            positions: &positions,
        },
    )
    .await
    .unwrap();
}

/// A `Bearer` for a person of `role`, optionally a participant of `project`.
async fn bearer(
    surreal: &store::surreal::SurrealDb,
    role: Role,
    project: Option<(uuid::Uuid, &str)>,
) -> String {
    let email = format!("actor-{}@example.com", Uuid::now_v7());
    let actor = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email.clone(), email, role),
    )
    .await
    .unwrap();
    if let Some((project_id, participation)) = project {
        store::projects::add_participation(surreal, project_id, actor.id, participation)
            .await
            .unwrap();
    }
    let mut s = SessionData::fresh("api-contract-sub", role);
    s.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

/// A multipart body carrying only a `contract_text` part (the pasted contract).
fn text_body(text: &str) -> Vec<u8> {
    format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"contract_text\"\r\n\r\n{text}\r\n--{BOUNDARY}--\r\n"
    )
    .into_bytes()
}

async fn post(
    app: &axum::Router,
    auth: Option<&str>,
    project_id: uuid::Uuid,
    body: Vec<u8>,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/projects/{project_id}/contract-review"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn anonymous_is_401() {
    let (app, surreal) = build_app().await;
    let (project_id, entity_id) = seed_matter(&surreal).await;
    seed_playbook(&surreal, entity_id).await;

    let resp = post(
        &app,
        None,
        project_id,
        text_body("MSA. Liability uncapped."),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_participant_is_404() {
    let (app, surreal) = build_app().await;
    let (project_id, entity_id) = seed_matter(&surreal).await;
    seed_playbook(&surreal, entity_id).await;
    let outsider = bearer(&surreal, Role::Lawyer, None).await;

    let resp = post(&app, Some(&outsider), project_id, text_body("MSA.")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn no_playbook_is_422() {
    let (app, surreal) = build_app().await;
    let (project_id, _entity_id) = seed_matter(&surreal).await;
    // No playbook seeded on the entity.
    let lawyer = bearer(&surreal, Role::Lawyer, Some((project_id, "lawyer"))).await;

    let resp = post(&app, Some(&lawyer), project_id, text_body("MSA.")).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["error"], "no_playbook");
}

#[tokio::test]
async fn participant_lawyer_uploads_and_runs_the_review() {
    let (app, surreal) = build_app().await;
    let (project_id, entity_id) = seed_matter(&surreal).await;
    seed_playbook(&surreal, entity_id).await;
    let lawyer = bearer(&surreal, Role::Lawyer, Some((project_id, "lawyer"))).await;

    let resp = post(
        &app,
        Some(&lawyer),
        project_id,
        text_body(
            "MASTER SERVICES AGREEMENT\nLiability is uncapped. Governed by the laws of Mars.",
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let review_id = json["review_id"].as_str().expect("a review_id");
    // The review row exists with its analysis attached.
    let review = store::contract_reviews::by_id(&surreal, Uuid::parse_str(review_id).unwrap())
        .await
        .unwrap()
        .expect("the review row exists");
    let findings = store::contract_reviews::findings_of(&review).unwrap();
    assert!(
        !findings.is_empty(),
        "the deviation analysis produced findings"
    );
}

#[tokio::test]
async fn a_client_participant_may_also_upload() {
    let (app, surreal) = build_app().await;
    let (project_id, entity_id) = seed_matter(&surreal).await;
    seed_playbook(&surreal, entity_id).await;
    // Client-lens participation — the client submits their own contract.
    let client = bearer(&surreal, Role::Client, Some((project_id, "client"))).await;

    let resp = post(
        &app,
        Some(&client),
        project_id,
        text_body("MSA. Liability uncapped."),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}
