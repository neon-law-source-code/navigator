#![allow(clippy::doc_markdown)]
//! The questionnaire reaching END begins the notation's workflow — through
//! the REST door, not only through the lawyer's form walk.
//!
//! `POST /app/api/notations/{id}/answers` advances the same questionnaire
//! runtime `portal::retainer_walk` advances. It used to answer "complete" and
//! stop there, so the same final answer left the notation at `lawyer_review`
//! or at no machine at all depending on which door recorded it. Both doors
//! now hand a completed questionnaire to
//! `portal::retainer_walk::begin_post_questionnaire_workflow`, and this test
//! holds that: the last answer posted here parks the notation at the human
//! gate exactly as the form walk does.

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
use workflows::{InMemoryRuntime, StateMachineRuntime};

/// A seeded catalog template that declares both a questionnaire and a
/// post-questionnaire workflow, so a full walk has somewhere to hand off to.
const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "api-notation-answer-test-key";

/// The template's declared questionnaire, in order, with an answer for each
/// step — the same walk `workflows::notation_session`'s own full-walk test
/// uses.
const WALK: [(&str, &str); 8] = [
    ("entity", "Apollo LLC"),
    ("address__principal_office", "1 Example Way, Reno, NV 89501"),
    ("person__client", "Libra"),
    ("person__lawyer_dri", "Firm Principal"),
    ("project__engagement", "Apollo"),
    ("custom_datetime__engagement_start_date", "2026-09-01"),
    (
        "custom_text__engagement_scope",
        "Draft and file the Apollo formation documents.",
    ),
    ("custom_single_choice__governing_law", "nevada"),
];

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    runtime: Arc<dyn StateMachineRuntime>,
}

async fn build_app() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-notation-answer-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Harness {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        runtime,
    }
}

/// A lawyer session scoped to `project`, which is what the answer door
/// requires: the acting lawyer must participate in the notation's matter.
async fn lawyer_bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    project_id: uuid::Uuid,
) -> String {
    let actor = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project_id, actor.id, "lawyer")
        .await
        .unwrap();
    let mut session = SessionData::fresh("api-answer-sub", Role::Lawyer);
    session.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn post_answer(
    app: &axum::Router,
    auth: &str,
    notation_id: uuid::Uuid,
    code: &str,
    value: &str,
) -> axum::http::Response<Body> {
    let body = serde_json::json!({ "question_code": code, "value": value }).to_string();
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/notations/{notation_id}/answers"))
                .header("content-type", "application/json")
                .header("authorization", auth)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The final answer through the REST door completes the questionnaire *and*
/// begins the workflow, parking the notation at the `lawyer_review` gate.
#[tokio::test]
async fn the_last_answer_through_the_rest_door_begins_the_workflow() {
    let h = build_app().await;
    let project = store::test_support::seed_project(&h.surreal, "Matter").await;
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
    let started = workflows::start_notation(
        &h.surreal,
        h.runtime.as_ref(),
        None,
        TEMPLATE_CODE,
        client.id,
        project.id,
        None,
    )
    .await
    .expect("the seeded template starts");
    let notation_id = started.notation_id;
    let auth = lawyer_bearer(&h.surreal, "lawyer@example.com", project.id).await;

    // Every answer but the last goes in the same way, so the assertion below
    // is about the step that completes the questionnaire and not about the
    // door being usable at all.
    let (last, rest) = WALK.split_last().expect("the walk has steps");
    for (code, value) in rest {
        let response = post_answer(&h.app, &auth, notation_id, code, value).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "answering `{code}` should be accepted"
        );
    }

    // Precondition: nothing has begun yet. The questionnaire is still
    // walking, so the workflow machine has not been started.
    let before = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        before.state, "lawyer_review",
        "the workflow must not begin before the questionnaire ends"
    );

    let response = post_answer(&h.app, &auth, notation_id, last.0, last.1).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["status"], "complete",
        "the last answer completes the questionnaire"
    );

    let after = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.state, "lawyer_review",
        "completing the questionnaire begins the workflow and parks it at the human gate"
    );
}
