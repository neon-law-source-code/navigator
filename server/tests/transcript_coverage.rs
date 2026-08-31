#![allow(clippy::doc_markdown)]
//! Integration tests for the questionnaire walk's **transcript input mode** —
//! PR 3 of #349. `POST /app/lawyer/notations/:id/transcript` runs `live_inquiry`
//! batch coverage over the notation's template and the uploaded transcript,
//! persists each covered inquiry as a proposed answer (`source = extracted`),
//! and returns a JSON coverage summary. It never advances the questionnaire:
//! the covered answers surface as the walk's prior-answer defaults for a lawyer to
//! confirm or edit, and uncovered questions still prompt.
//!
//! Asserts, against a real questionnaire:
//!   1. Coverage populates answers with `source = extracted`, and the JSON
//!      summary reports which questions were covered vs. left uncovered.
//!   2. The widened JSON step surfaces the extracted proposal as `prior_answer`
//!      (tagged `prior_source = extracted`) so the CLI can offer it as a
//!      default; an uncovered question carries no prior answer and still asks.
//!   3. Confirming a proposal through the normal `/step` POST writes a normal
//!      (`source = lawyer`) answer row that supersedes the extracted proposal.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::InMemoryRuntime;

/// A questionnaire the batch coverage engine can partially cover: the recording
/// consent (covered when the transcript mentions "consent"), the testator name
/// (covered via the `testator` label), and a free-text note the transcript
/// never touches (left uncovered so the walk still asks it).
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

const TRANSCRIPT: &str =
    "The client gave their consent to record this sitting. The testator is Jane Doe.";

/// Build the app with a project, a notation on a template carrying
/// [`QUESTIONNAIRE`], and return the router + db + the notation id.
async fn build() -> (axum::Router, store::surreal::SurrealDb, uuid::Uuid) {
    build_with(QUESTIONNAIRE).await
}

/// Build the app with a notation bound to a template carrying `body` — lets
/// the error-path tests bind a body with no questionnaire frontmatter.
async fn build_with(body: &[u8]) -> (axum::Router, store::surreal::SurrealDb, uuid::Uuid) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-transcript-coverage-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-transcript-coverage-store-{}",
            uuid::Uuid::now_v7()
        )))
        .await
        .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let blob = store::assets::ingest_content(&surreal, &storage, body, "text/markdown")
        .await
        .unwrap();
    let template = store::templates::save_version(
        &surreal,
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

    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("transcript-matter-{}", uuid::Uuid::now_v7()),
            name: "Transcript matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(template.id, client.id, project.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        notation_id,
    )
}

/// POST a transcript to the coverage endpoint; return the parsed JSON summary.
async fn post_transcript(
    app: &axum::Router,
    nid: uuid::Uuid,
    transcript: &str,
) -> serde_json::Value {
    let body = format!("transcript={}", urlencoding(transcript));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/notations/{nid}/transcript"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "transcript coverage responds 200"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Minimal form-body encoder for the transcript field (spaces and punctuation).
fn urlencoding(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn step_json(app: &axum::Router, nid: uuid::Uuid) -> serde_json::Value {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/step?format=json"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn step_post(app: &axum::Router, nid: uuid::Uuid, body: String) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/notations/{nid}/step"))
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
        .status()
}

async fn latest_answer(
    surreal: &store::surreal::SurrealDb,
    nid: uuid::Uuid,
    state_name: &str,
) -> Option<store::answers::Answer> {
    // Append-only: the last row for this state is the latest answer.
    store::answers::for_notation(surreal, nid)
        .await
        .unwrap()
        .into_iter()
        .rfind(|a| a.state_name.as_deref() == Some(state_name))
}

#[tokio::test]
async fn coverage_persists_extracted_answers_and_reports_gaps() {
    let (app, surreal, nid) = build().await;

    let summary = post_transcript(&app, nid, TRANSCRIPT).await;

    // The summary reports the two covered inquiries and the one gap.
    let covered: Vec<String> = summary["covered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["code"].as_str().unwrap().to_string())
        .collect();
    assert!(
        covered.contains(&"custom_yes_no__recording_consent".to_string())
            && covered.contains(&"custom_text__testator_name".to_string()),
        "consent + testator are covered: {covered:?}"
    );
    let uncovered: Vec<String> = summary["uncovered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert!(
        uncovered.contains(&"custom_text__note".to_string()),
        "the untouched note question is a gap: {uncovered:?}"
    );

    // Each covered inquiry is persisted with source = extracted.
    let consent = latest_answer(&surreal, nid, "custom_yes_no__recording_consent")
        .await
        .expect("consent answer written");
    assert_eq!(consent.source, "extracted");
    assert_eq!(store::answers::display_value(&consent.value), "Yes");

    let testator = latest_answer(&surreal, nid, "custom_text__testator_name")
        .await
        .expect("testator answer written");
    assert_eq!(testator.source, "extracted");
    assert_eq!(store::answers::display_value(&testator.value), "Jane Doe");

    // The untouched question stays unanswered — the walk will still ask it.
    assert!(latest_answer(&surreal, nid, "custom_text__note")
        .await
        .is_none());
}

#[tokio::test]
async fn the_walk_offers_the_proposal_then_confirms_it_as_a_lawyer_answer() {
    let (app, surreal, nid) = build().await;
    post_transcript(&app, nid, TRANSCRIPT).await;

    // The machine is still at BEGIN → the first step is the covered consent
    // question, and it surfaces the extracted proposal as a default.
    let step = step_json(&app, nid).await;
    assert_eq!(step["question"]["code"], "custom_yes_no__recording_consent");
    assert_eq!(step["question"]["prior_answer"], "Yes");
    assert_eq!(step["question"]["prior_source"], "extracted");

    // Confirming the proposal is a normal lawyer answer that supersedes it.
    assert_eq!(
        step_post(&app, nid, "value=Yes".into()).await,
        StatusCode::SEE_OTHER
    );
    let confirmed = latest_answer(&surreal, nid, "custom_yes_no__recording_consent")
        .await
        .expect("confirmed answer written");
    assert_eq!(confirmed.source, "lawyer");
    assert_eq!(store::answers::display_value(&confirmed.value), "Yes");

    // The testator proposal now leads; confirm it too and land on the gap.
    let step = step_json(&app, nid).await;
    assert_eq!(step["question"]["code"], "custom_text__testator_name");
    assert_eq!(step["question"]["prior_answer"], "Jane Doe");
    step_post(&app, nid, "value=Jane Doe".into()).await;

    // The uncovered note question prompts with no prior answer.
    let step = step_json(&app, nid).await;
    assert_eq!(step["question"]["code"], "custom_text__note");
    assert!(step["question"]["prior_answer"].is_null());
    assert!(step["question"]["prior_source"].is_null());
}

/// POST a raw form body to the transcript endpoint; return just the status.
async fn post_status(app: &axum::Router, nid: uuid::Uuid, body: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/notations/{nid}/transcript"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_missing_transcript_field_is_rejected() {
    let (app, _surreal, nid) = build().await;
    assert_eq!(
        post_status(&app, nid, "transcript=").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_status(&app, nid, "other=x").await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_unknown_notation_is_not_found() {
    let (app, _surreal, _nid) = build().await;
    assert_eq!(
        post_status(&app, uuid::Uuid::now_v7(), "transcript=hello").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_template_without_questionnaire_frontmatter_is_unprocessable() {
    // A body with no frontmatter at all can't yield a questionnaire, so
    // coverage has nothing to run — the endpoint says so rather than 500ing.
    let (app, _surreal, nid) = build_with(b"# Just prose, no frontmatter\n").await;
    assert_eq!(
        post_status(&app, nid, "transcript=hello").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}
