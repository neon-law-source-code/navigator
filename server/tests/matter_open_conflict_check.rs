#![allow(clippy::doc_markdown)]
//! Dev e2e for the conflict check the shared `open_matter` command runs for
//! `POST /app/projects`.
//!
//! Drives the real HTTP path against the in-memory workflow runtime, so
//! nothing reaches DocuSign. Since the form now routes through `open_matter`
//! (navigator#355), the conflict model is the command's: the attorney's
//! attestation is the control, not a separate acknowledgment step. Covers:
//!
//!   1. **Block** — the proposed client is directly `adverse_to` a current
//!      client → refused, no matter created. No attestation overrides a block.
//!   2. **Soft finding** — the proposed matter shares an entity with another
//!      client's open matter. It is refused without the attestation, but
//!      opens (`303`) with it; the finding is recorded on the
//!      `conflict_attestation` audit row rather than gating a second submit.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::signature::StubSignatureProvider;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

fn admin_bearer() -> String {
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let mut session = portal::SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    session.source = portal::session::SessionSource::Cli;
    format!("Bearer {}", sessions.encode(&session))
}

async fn build_app(tag: &str) -> (axum::Router, store::surreal::SurrealDb) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-conflict-repos-{tag}-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-conflict-{tag}")))
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
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: stub,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

/// Assert `resp` is the matter-open refusal redirect and its `?error=` carries
/// `needle`. Every refusal is post/redirect/get back to `/app/projects/new`
/// with the message and the submitted fields in the query.
fn assert_refused_with(resp: &axum::http::Response<Body>, needle: &str) {
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.starts_with("/app/projects/new?error="),
        "a refused open must redirect back to the form; got: {loc}",
    );
    let decoded = loc.replace("%20", " ").replace("%26", "&");
    assert!(
        decoded.contains(needle),
        "expected the refusal to say {needle:?}; got: {decoded}",
    );
}

fn enc(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

async fn seed_client(surreal: &store::surreal::SurrealDb, name: &str, email: &str) -> uuid::Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
    )
    .await
    .unwrap()
    .id
}

/// Open an existing matter for `client` against `entity_id`, so the graph
/// sees that entity / person as a party the firm already serves.
async fn seed_open_project(
    surreal: &store::surreal::SurrealDb,
    entity_id: uuid::Uuid,
    client: uuid::Uuid,
) {
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("existing-matter-{}", uuid::Uuid::now_v7()),
            name: "Existing matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // The conflict graph reads the client-DRI markers to decide who the firm
    // already serves.
    store::projects::designate_dri_in_surreal(
        surreal,
        project.id,
        client,
        store::projects::DriSide::Client,
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        surreal,
        project.id,
        store::test_support::dri_person(surreal).await,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
}

async fn post_projects(app: &axum::Router, body: String) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/projects")
                .header("authorization", admin_bearer())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn adverse_to_current_client_blocks_the_open() {
    let (app, surreal) = build_app("block").await;

    // The opponent is already a client of the firm (an open matter).
    let opponent = seed_client(&surreal, "Opposing Party", "opponent@example.com").await;
    let opp_entity = store::test_support::seed_entity(&surreal).await;
    seed_open_project(&surreal, opp_entity, opponent).await;

    // The proposed client is directly adverse to that current client.
    let proposed = seed_client(&surreal, "New Client", "newclient@example.com").await;
    store::relationships::record(
        &surreal,
        &store::relationships::NewRelationship {
            from: store::relationships::Endpoint::Person,
            from_id: proposed,
            to: store::relationships::Endpoint::Person,
            to_id: opponent,
            kind: store::relationships::KIND_ADVERSE_TO.into(),
            confidence_pct: 100,
            source_kind: store::relationships::SOURCE_MANUAL.into(),
            source_id: None,
            detail: None,
        },
    )
    .await
    .unwrap();

    let new_entity = store::test_support::seed_entity(&surreal).await;
    // Attested — so the refusal is the conflict block, not the attestation gate.
    // A block is a hard stop no attestation overrides.
    let body = format!(
        "name={}&code=matter-open-1&entity_id={new_entity}\
         &client_dri_person_id={proposed}\
         &scope_of_services={}\
         &attestation=1",
        enc("Adverse matter"),
        enc("Some work"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "adverse to a current client");
    // Nothing was created.
    assert!(
        store::projects::find_by_name(&surreal, "Adverse matter")
            .await
            .unwrap()
            .is_none(),
        "no matter should open on a hard block"
    );
}

#[tokio::test]
async fn shared_party_is_refused_without_attestation_then_opens_with_it() {
    let (app, surreal) = build_app("review").await;

    // The firm already runs a matter on this entity for another client — a
    // soft (non-blocking) entanglement the graph surfaces.
    let existing = seed_client(&surreal, "Existing Client", "existing@example.com").await;
    let shared_entity = store::test_support::seed_entity(&surreal).await;
    seed_open_project(&surreal, shared_entity, existing).await;

    let proposed = seed_client(&surreal, "Second Client", "second@example.com").await;
    let base = format!(
        "name={}&code=matter-open-2&entity_id={shared_entity}\
         &client_dri_person_id={proposed}\
         &scope_of_services={}",
        enc("Shared entity matter"),
        enc("Some work"),
    );

    // A soft finding does not block, but the attestation is still required on
    // every open: without it the submit is refused and opens nothing.
    let resp = post_projects(&app, base.clone()).await;
    assert_refused_with(
        &resp,
        "Attest that you have checked for conflicts, and that either none prevent opening this Project or this Project is not legal advice",
    );
    assert!(
        store::projects::find_by_name(&surreal, "Shared entity matter")
            .await
            .unwrap()
            .is_none(),
        "no matter should open without the attestation",
    );

    // With the attestation the softly-flagged matter opens (`303`); the finding
    // is recorded on the attestation audit row, not gated behind a second step.
    let resp = post_projects(&app, format!("{base}&attestation=1")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let project = store::projects::find_by_name(&surreal, "Shared entity matter")
        .await
        .unwrap()
        .expect("a softly-flagged matter opens on the attestation");

    let audit: Vec<_> = store::relationship_logs::for_subject(&surreal, "project", project.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|log| log.action == "conflict_attestation")
        .collect();
    assert_eq!(
        audit.len(),
        1,
        "every open writes exactly one conflict-attestation audit entry",
    );
    // The detail names what the attorney attested over — the soft finding.
    assert!(
        audit[0].detail.contains("findings"),
        "a flagged open records the findings it attested over: {}",
        audit[0].detail,
    );
}
