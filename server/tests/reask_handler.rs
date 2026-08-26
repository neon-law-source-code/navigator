#![allow(clippy::doc_markdown)]
//! Integration tests for the re-ask handlers (#252): the "send a reviewed
//! notation back for changes" half of `lawyer_review`, the lawyer-on-behalf
//! re-collection surface, and the resubmit that loops back to review.
//!
//! The browser e2e (`reask_flow.rs`) drives the same loop against a live
//! server but skips under a plain `cargo test` (it needs chromedriver + a
//! running web), so it contributes no coverage. These tests exercise the
//! three handlers directly through the router against a real test database
//! and the in-memory workflow runtime, covering the happy path and every
//! guard branch (404 / 409 / 400) without a browser.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{InMemoryRuntime, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__retainer";

/// Build the router over a real test database with the bundled
/// `onboarding__retainer` seeded, plus one notation at BEGIN. The workflow
/// and questionnaire share one `InMemoryRuntime` — the same in-process
/// topology the walker tests use.
async fn build_app_and_notation() -> (axum::Router, store::surreal::SurrealDb, uuid::Uuid) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("navigator-reask-storage-{}", uuid::Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed pass inserts onboarding__retainer");

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-retainer-{}", uuid::Uuid::now_v7()),
            name: "Libra retainer".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, libra.id, proj.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        workflows::DispatchingRuntime::new(runtime.clone(), email.clone(), storage.clone()),
    );
    let state = AppState {
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        notation_id,
    )
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn location(resp: &axum::http::Response<Body>) -> String {
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn get(app: &axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post_form(
    app: &axum::Router,
    uri: &str,
    body: &'static str,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Walk the three-question retainer intake to the `lawyer_review` gate — the
/// state from which changes can be requested. Mirrors the walker tests: the
/// final answer parks the matter at review rather than rendering.
async fn walk_to_lawyer_review(
    app: &axum::Router,
    nid: uuid::Uuid,
    surreal: &store::surreal::SurrealDb,
) {
    for value in [
        "Libra",
        "Firm%20Principal",
        "Estate%20plan",
        "2026-09-01",
        "Draft%20and%20file%20the%20matter%20documents.",
        "450%20per%20hour",
        "nevada",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/lawyer/notations/{nid}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={value}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::SEE_OTHER || resp.status() == StatusCode::OK,
            "walk step for {value} returned {}",
            resp.status()
        );
    }
    let row = store::notations::find_by_id(surreal, nid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "lawyer_review",
        "walk should park at lawyer_review"
    );
}

async fn notation_state(surreal: &store::surreal::SurrealDb, nid: uuid::Uuid) -> String {
    store::notations::find_by_id(surreal, nid)
        .await
        .unwrap()
        .unwrap()
        .state
}

#[tokio::test]
async fn reask_loop_parks_at_reask_then_returns_to_review() {
    let (app, surreal, nid) = build_app_and_notation().await;
    walk_to_lawyer_review(&app, nid, &surreal).await;

    // The lawyer sends the client back for changes on person__client with a note.
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/request-changes"),
        "q:person__client=on&note=Confirm+the+client%27s+legal+name",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), format!("/lawyer/notations/{nid}/reask"));
    assert_eq!(
        notation_state(&surreal, nid).await,
        "reask__client",
        "request-changes parks the matter at reask__client"
    );

    // The re-ask surface renders the flagged question and the reviewer note.
    let resp = get(&app, &format!("/lawyer/notations/{nid}/reask")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.contains("a:person__client"),
        "re-ask form should offer the flagged answer field: {html}"
    );
    // Dioxus SSR spells the apostrophe as `&#39;`, so match what the reader
    // sees rather than the raw bytes.
    assert!(
        html.replace("&#39;", "'")
            .contains("Confirm the client's legal name"),
        "re-ask surface should show the reviewer note: {html}"
    );

    // Re-collect the flagged answer and resubmit — back to lawyer_review.
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/reask"),
        "a:person__client=Libra+Jones",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), format!("/lawyer/notations/{nid}/review"));
    assert_eq!(
        notation_state(&surreal, nid).await,
        "lawyer_review",
        "resubmit loops the matter back to review, not to a dead-end"
    );

    // The corrected answer is on record as the latest person__client value.
    // Append-only: the last row for this state is the latest answer.
    let latest = store::answers::for_notation(&surreal, nid)
        .await
        .unwrap()
        .into_iter()
        .rfind(|a| a.state_name.as_deref() == Some("person__client"))
        .expect("a re-collected person__client answer landed");
    assert!(
        store::answers::display_value(&latest.value).contains("Libra Jones"),
        "latest person__client answer should be the correction: {:?}",
        latest.value
    );
}

#[tokio::test]
async fn request_changes_for_unknown_notation_returns_404() {
    let (app, _surreal, _nid) = build_app_and_notation().await;
    let resp = post_form(
        &app,
        &format!(
            "/lawyer/notations/{}/request-changes",
            uuid::Uuid::from_u128(9999)
        ),
        "q:person__client=on",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn request_changes_when_not_in_review_returns_409() {
    // A notation still at BEGIN is not awaiting review — nothing to send back.
    let (app, _surreal, nid) = build_app_and_notation().await;
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/request-changes"),
        "q:person__client=on",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn request_changes_without_any_flags_returns_400() {
    let (app, surreal, nid) = build_app_and_notation().await;
    walk_to_lawyer_review(&app, nid, &surreal).await;
    // A note but no flagged answers: there is nothing to re-collect.
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/request-changes"),
        "note=please+fix+something",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        notation_state(&surreal, nid).await,
        "lawyer_review",
        "a flagless request-changes must not move the matter"
    );
}

#[tokio::test]
async fn reask_get_for_unknown_notation_returns_404() {
    let (app, _surreal, _nid) = build_app_and_notation().await;
    let resp = get(
        &app,
        &format!("/lawyer/notations/{}/reask", uuid::Uuid::from_u128(9999)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reask_get_when_not_parked_redirects_to_review() {
    // Nothing parked for re-collection (still at BEGIN): lawyers are sent to
    // the review page rather than an empty re-ask form.
    let (app, _surreal, nid) = build_app_and_notation().await;
    let resp = get(&app, &format!("/lawyer/notations/{nid}/reask")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), format!("/lawyer/notations/{nid}/review"));
}

#[tokio::test]
async fn reask_post_for_unknown_notation_returns_404() {
    let (app, _surreal, _nid) = build_app_and_notation().await;
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{}/reask", uuid::Uuid::from_u128(9999)),
        "a:person__client=Libra+Jones",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reask_post_when_not_parked_returns_409() {
    // A notation not at reask__client is not awaiting re-collection.
    let (app, _surreal, nid) = build_app_and_notation().await;
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/reask"),
        "a:person__client=Libra+Jones",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn reask_post_rolls_back_and_400s_when_an_answer_cannot_be_saved() {
    // Flag a code whose question isn't seeded (a stale or hand-crafted flag):
    // re-collection can't persist it, so the whole resubmit is refused and
    // rolled back rather than landing a partial correction. Also exercises
    // the re-ask surface's label fallback to the bare code.
    let (app, surreal, nid) = build_app_and_notation().await;
    walk_to_lawyer_review(&app, nid, &surreal).await;
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/request-changes"),
        "q:zzz_stale_code=on",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // The re-ask surface renders, falling back to the bare code as its label.
    let resp = get(&app, &format!("/lawyer/notations/{nid}/reask")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.contains("a:zzz_stale_code"),
        "re-ask form offers the flagged field even with no seeded question: {html}"
    );

    // A non-empty answer clears the completeness guard but fails to persist —
    // the write is refused whole, the matter stays parked.
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/reask"),
        "a:zzz_stale_code=whatever",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        notation_state(&surreal, nid).await,
        "reask__client",
        "a failed re-collection must leave the matter parked, not resubmitted"
    );
}

#[tokio::test]
async fn reask_post_with_a_blank_flagged_answer_returns_400() {
    let (app, surreal, nid) = build_app_and_notation().await;
    walk_to_lawyer_review(&app, nid, &surreal).await;
    // Park the matter at reask__client with person__client flagged.
    let resp = post_form(
        &app,
        &format!("/lawyer/notations/{nid}/request-changes"),
        "q:person__client=on",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Resubmit without re-collecting the flagged answer: refused whole, so
    // the wrong value never slips back into review.
    let resp = post_form(&app, &format!("/lawyer/notations/{nid}/reask"), "").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        notation_state(&surreal, nid).await,
        "reask__client",
        "an incomplete resubmit must leave the matter parked"
    );
}
