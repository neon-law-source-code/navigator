#![allow(clippy::doc_markdown)]
//! Integration tests for the notation-review REST doors:
//! `POST /app/api/notations/{id}/request-changes` and `.../reask`.
//!
//! The write engines (`retainer_walk::request_notation_changes` and
//! `resubmit_reask`) are shared with the lawyer review controls, so these tests
//! focus on what the REST adapters add: the tier gate (LawyerSession → 401/403),
//! the actor requirement (a bearer with no linked Person is 403), the
//! matter-scope gate (a lawyer outside the matter is a bare 404), the state
//! guards (409 off-gate), the completeness guards (400), and the live 204s
//! proving the notation moves reask__client ↔ lawyer_review.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::SessionData;
use portal::AppState;
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__letter";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    notation_id: Uuid,
    /// A lawyer who participates in the notation's matter — the authorised actor.
    acting: String,
    /// A lawyer with no participation in this matter.
    outsider: String,
    /// A client-tier caller.
    client: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-reask-sub", role);
    session.person_id = Some(person_id);
    format!(
        "Bearer {}",
        portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY).encode(&session)
    )
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("navigator-api-reask-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts onboarding__letter");
    let client_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let project = store::projects::create(
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
    // The acting lawyer participates in the matter; the outsider does not.
    let acting_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Acting Lawyer", "acting@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, acting_person.id, "lawyer")
        .await
        .unwrap();
    let outsider_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Outsider", "outsider@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, client_person.id, project.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(DispatchingRuntime::new(
        runtime.clone(),
        email.clone(),
        storage.clone(),
    ));
    let state = AppState {
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        notation_id,
        acting: bearer(acting_person.id, Role::Lawyer),
        outsider: bearer(outsider_person.id, Role::Lawyer),
        client: bearer(client_person.id, Role::Client),
    }
}

/// Drive the eight-question retainer intake to the `lawyer_review` gate over
/// the lawyer walk, mirroring `reask_handler.rs`.
async fn walk_to_lawyer_review(fx: &Fixture) {
    for value in [
        "Libra%20Holdings%20LLC",
        "500%20Innovation%20Way%20Reno%20NV%2089501",
        "Libra",
        "Firm%20Principal",
        "Estate%20plan",
        "2026-09-01",
        "Draft%20and%20file%20the%20matter%20documents.",
        "nevada",
    ] {
        let resp = fx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/lawyer/notations/{}/step", fx.notation_id))
                    .header("authorization", &fx.acting)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={value}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::SEE_OTHER || resp.status() == StatusCode::OK,
            "walk step {value} returned {}",
            resp.status()
        );
    }
    assert_eq!(state_of(fx).await, "lawyer_review");
}

async fn state_of(fx: &Fixture) -> String {
    store::notations::find_by_id(&fx.surreal, fx.notation_id)
        .await
        .unwrap()
        .unwrap()
        .state
}

async fn post_json(
    fx: &Fixture,
    path_suffix: &str,
    auth: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!(
            "/app/api/notations/{}/{path_suffix}",
            fx.notation_id
        ))
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_participant_lawyer_requests_changes_then_reasks() {
    let fx = build_fixture().await;
    walk_to_lawyer_review(&fx).await;

    let resp = post_json(
        &fx,
        "request-changes",
        Some(&fx.acting),
        serde_json::json!({ "flagged": ["person__client"], "note": "Confirm the legal name" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        state_of(&fx).await,
        "reask__client",
        "request-changes parks the matter at reask__client"
    );

    let resp = post_json(
        &fx,
        "reask",
        Some(&fx.acting),
        serde_json::json!({ "answers": { "person__client": "Libra Jones" } }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        state_of(&fx).await,
        "lawyer_review",
        "reask loops the matter back to review"
    );
}

#[tokio::test]
async fn request_changes_rejects_the_unauthorized() {
    let fx = build_fixture().await;
    walk_to_lawyer_review(&fx).await;
    let body = serde_json::json!({ "flagged": ["person__client"] });

    let client = post_json(&fx, "request-changes", Some(&fx.client), body.clone()).await;
    assert_eq!(
        client.status(),
        StatusCode::FORBIDDEN,
        "client tier is refused"
    );

    let anon = post_json(&fx, "request-changes", None, body.clone()).await;
    assert_eq!(
        anon.status(),
        StatusCode::UNAUTHORIZED,
        "anonymous is refused"
    );

    let outsider = post_json(&fx, "request-changes", Some(&fx.outsider), body).await;
    assert_eq!(
        outsider.status(),
        StatusCode::NOT_FOUND,
        "a lawyer outside the matter gets a non-disclosing 404"
    );
    assert_eq!(
        state_of(&fx).await,
        "lawyer_review",
        "no refusal moved the matter"
    );
}

#[tokio::test]
async fn request_changes_off_gate_is_409() {
    // Still at BEGIN — not awaiting review.
    let fx = build_fixture().await;
    let resp = post_json(
        &fx,
        "request-changes",
        Some(&fx.acting),
        serde_json::json!({ "flagged": ["person__client"] }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn request_changes_with_no_flags_is_400() {
    let fx = build_fixture().await;
    walk_to_lawyer_review(&fx).await;
    let resp = post_json(
        &fx,
        "request-changes",
        Some(&fx.acting),
        serde_json::json!({ "flagged": [], "note": "fix something" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        state_of(&fx).await,
        "lawyer_review",
        "a flagless request moves nothing"
    );
}

#[tokio::test]
async fn reask_off_gate_is_409() {
    // Still at BEGIN — not awaiting re-collection.
    let fx = build_fixture().await;
    let resp = post_json(
        &fx,
        "reask",
        Some(&fx.acting),
        serde_json::json!({ "answers": { "person__client": "Libra Jones" } }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn reask_with_a_blank_flagged_answer_is_400() {
    let fx = build_fixture().await;
    walk_to_lawyer_review(&fx).await;
    // Park at reask__client with person__client flagged.
    let parked = post_json(
        &fx,
        "request-changes",
        Some(&fx.acting),
        serde_json::json!({ "flagged": ["person__client"] }),
    )
    .await;
    assert_eq!(parked.status(), StatusCode::NO_CONTENT);

    // Resubmit without re-collecting the flagged answer: refused whole.
    let resp = post_json(
        &fx,
        "reask",
        Some(&fx.acting),
        serde_json::json!({ "answers": {} }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        state_of(&fx).await,
        "reask__client",
        "an incomplete resubmit leaves the matter parked"
    );
}
