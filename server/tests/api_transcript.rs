#![allow(clippy::doc_markdown)]
//! Integration tests for the transcript REST doors:
//! `POST /app/api/notations/{id}/transcript` (batch coverage) and
//! `POST /app/api/projects/{id}/notations/{nid}/transcript` (estate intake).
//!
//! The write engines (`retainer_walk::record_transcript_coverage` and
//! `transcript_intake::file_estate_transcript`) are shared with the lawyer/CLI
//! forms, so this focuses on what the REST adapters add: they take the transcript
//! as JSON text (not multipart), lawyer-tier only (client 403, anon 401),
//! matter-scope (out-of-scope 404), and the live coverage / estate-pipeline runs.

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
use workflows::{DispatchingRuntime, InMemoryRuntime, MachineKind, StateMachineRuntime};

const KEY: &str = "api-transcript-test-key";

/// A questionnaire the batch coverage engine can partially cover (mirrors
/// `transcript_coverage.rs`).
const QUESTIONNAIRE: &[u8] = br"---
questionnaire:
  BEGIN:
    _: custom_yes_no__recording_consent
  custom_yes_no__recording_consent:
    _: custom_text__testator_name
  custom_text__testator_name:
    _: custom_text__note
  custom_text__note:
    _: END
  END: {}
---

# Transcript walk
";

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    runtime: Arc<dyn StateMachineRuntime>,
    admin: String,
    client: String,
    outsider: String,
}

fn bearer(person_id: Option<Uuid>, role: Role) -> String {
    let mut s = SessionData::fresh("api-transcript-sub", role);
    s.person_id = person_id;
    format!("Bearer {}", SessionStore::new(KEY).encode(&s))
}

async fn harness() -> Harness {
    let repo_root = std::env::temp_dir().join(format!("nav-api-tx-repos-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("nav-api-tx-{}", Uuid::now_v7())))
            .await
            .unwrap(),
    );
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .unwrap();
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let inner = Arc::new(InMemoryRuntime::new());
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Client", "client@example.com", Role::Client),
    )
    .await
    .unwrap();
    let outsider = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Outsider", "outsider@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: inner,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Harness {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        storage,
        runtime,
        admin: bearer(None, Role::Admin),
        client: bearer(Some(client.id), Role::Client),
        outsider: bearer(Some(outsider.id), Role::Lawyer),
    }
}

/// A notation on a template that carries [`QUESTIONNAIRE`] — for the coverage
/// door, which reads the questionnaire but needs no started machine.
async fn seed_coverage_notation(h: &Harness) -> Uuid {
    let blob =
        store::assets::ingest_content(&h.surreal, &h.storage, QUESTIONNAIRE, "text/markdown")
            .await
            .unwrap();
    let template = store::templates::save_version(
        &h.surreal,
        None,
        "test__transcript_walk",
        store::templates::Version {
            title: "Transcript walk".into(),
            respondent_type: "person".into(),
            asset_id: Some(blob),
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let person = store::persons::create(
        &h.surreal,
        &store::persons::NewPerson::new("Respondent", format!("r-{}@example.com", Uuid::now_v7())),
    )
    .await
    .unwrap();
    let project = seed_project(h).await;
    store::notations::create(
        &h.surreal,
        &store::notations::NewNotation::new(template.id, person.id, project, "BEGIN"),
    )
    .await
    .unwrap()
    .id
}

/// An estate notation with its workflow started at BEGIN — for the intake door,
/// whose `transcript_uploaded` signal needs a running machine.
async fn seed_estate_notation(h: &Harness) -> (Uuid, Uuid) {
    let notation_id = store::test_support::seed_notation(&h.surreal).await;
    let project_id = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .unwrap()
        .project_id;
    let yaml = workflows::bundled_spec_yaml("onboarding__letter").expect("letter spec bundled");
    let spec = workflows::workflow_spec_from_yaml(yaml).expect("letter spec parses");
    StateMachineRuntime::start(
        h.runtime.as_ref(),
        MachineKind::Workflow,
        notation_id,
        &spec,
    )
    .await
    .expect("start estate workflow");
    (project_id, notation_id)
}

async fn seed_project(h: &Harness) -> Uuid {
    store::projects::create(
        &h.surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&h.surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
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

// ---------- batch transcript coverage ----------

#[tokio::test]
async fn coverage_runs_against_the_notation_questionnaire() {
    let h = harness().await;
    let notation_id = seed_coverage_notation(&h).await;
    let resp = post(
        &h,
        &format!("/app/api/notations/{notation_id}/transcript"),
        Some(&h.admin),
        serde_json::json!({
            "transcript": "The client gave their consent to record. The testator is Jane Doe."
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("covered").is_some() && json.get("uncovered").is_some());
}

#[tokio::test]
async fn coverage_rejects_the_unauthorized() {
    let h = harness().await;
    let notation_id = seed_coverage_notation(&h).await;
    let uri = format!("/app/api/notations/{notation_id}/transcript");
    let body = serde_json::json!({ "transcript": "hello" });

    assert_eq!(
        post(&h, &uri, Some(&h.client), body.clone()).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&h, &uri, None, body.clone()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&h, &uri, Some(&h.outsider), body).await.status(),
        StatusCode::NOT_FOUND,
        "a lawyer off the matter gets a non-disclosing 404"
    );
}

#[tokio::test]
async fn coverage_with_an_empty_transcript_is_400() {
    let h = harness().await;
    let notation_id = seed_coverage_notation(&h).await;
    let resp = post(
        &h,
        &format!("/app/api/notations/{notation_id}/transcript"),
        Some(&h.admin),
        serde_json::json!({ "transcript": "   " }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------- estate transcript intake ----------

#[tokio::test]
async fn filing_an_estate_transcript_drives_the_pipeline() {
    let h = harness().await;
    let (project_id, notation_id) = seed_estate_notation(&h).await;
    let resp = post(
        &h,
        &format!("/app/api/projects/{project_id}/notations/{notation_id}/transcript"),
        Some(&h.admin),
        serde_json::json!({ "transcript_text": "Consent given. Testator: Capricorn. Executor: Aries. Successor trustee: Gemini. Residuary beneficiary: Leo. Health-care agent: Virgo. Financial agent: Libra." }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let state = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .unwrap()
        .state;
    assert_ne!(
        state, "BEGIN",
        "the transcript filing advanced the notation past BEGIN"
    );
}

#[tokio::test]
async fn estate_intake_rejects_the_unauthorized_and_mismatches() {
    let h = harness().await;
    let (project_id, notation_id) = seed_estate_notation(&h).await;
    let uri = format!("/app/api/projects/{project_id}/notations/{notation_id}/transcript");
    let body = serde_json::json!({ "transcript_text": "Consent given." });

    assert_eq!(
        post(&h, &uri, Some(&h.client), body.clone()).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&h, &uri, None, body.clone()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&h, &uri, Some(&h.outsider), body.clone())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // A notation that does not belong to the named project → 404.
    let other_project = seed_project(&h).await;
    let mismatched =
        format!("/app/api/projects/{other_project}/notations/{notation_id}/transcript");
    assert_eq!(
        post(&h, &mismatched, Some(&h.admin), body).await.status(),
        StatusCode::NOT_FOUND
    );
}
