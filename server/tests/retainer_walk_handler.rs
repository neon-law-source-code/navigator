#![allow(clippy::doc_markdown)]
//! Integration tests for the stepwise retainer walker.
//!
//! Covers the full lifecycle:
//!   1. `POST /app/lawyer/retainers/new` creates Person + Project +
//!      role + Notation and redirects to `/step`.
//!   2. `GET /app/lawyer/notations/:id/step` renders the current
//!      question (read from the runtime + spec).
//!   3. `POST /app/lawyer/notations/:id/step` writes the Answer row and
//!      signals the runtime (the runtime — InMemoryRuntime in tests,
//!      the workflows-service worker in production — owns
//!      `notation_events`).
//!   4. The final POST hits END, drives the workflow, and renders
//!      the result page with the rendered retainer.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime, StateName};

const TEMPLATE_CODE: &str = "onboarding__letter";

async fn build_app_and_notation() -> (
    axum::Router,
    store::surreal::SurrealDb,
    uuid::Uuid,
    Arc<InMemoryRuntime>,
) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-retainer-walk-repos-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    // Template bodies seed into blob storage; the app reads them back
    // from the same handle, so seed and AppState share one storage.
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-walker-test-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    // seed_canonical inserts the bundled `onboarding__letter`
    // template; reuse the current version instead of double-inserting.
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed pass inserts onboarding__letter");

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

    // Keep a fourth `Arc<InMemoryRuntime>` so the test body can
    // call `runtime.events(MachineKind::Questionnaire, …)` to assert
    // on the recorded transitions — the runtime, not the journal,
    // is the source of truth once the walker stopped writing
    // `notation_events` directly.
    let runtime = Arc::new(InMemoryRuntime::new());
    let runtime_for_assertions = runtime.clone();
    // The `generate_pdf__retainer_pdf` step is worker-dispatched, so
    // wrap the in-memory runtime in `DispatchingRuntime` (the same
    // in-process path the dev binary and feature suite use) — otherwise
    // the PDF is never rendered/persisted and the signature read-back
    // 404s.
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
        runtime_for_assertions,
    )
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn step_get_at_begin_renders_the_first_question() {
    let (app, _surreal, nid, _runtime) = build_app_and_notation().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/step"))
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
    let html = body_string(resp).await;
    // First question after BEGIN is the entity record.
    assert!(html.contains("entity"), "html: {html}");
    assert!(html.contains("step 1 of 8"));
    assert!(html.contains(format!("/app/lawyer/notations/{nid}/step").as_str()));
}

#[tokio::test]
async fn step_get_prefill_is_scoped_to_current_notation() {
    let (app, surreal, nid, _runtime) = build_app_and_notation().await;
    let notation = store::notations::find_by_id(&surreal, nid)
        .await
        .unwrap()
        .unwrap();
    let mut new_other = store::notations::NewNotation::new(
        notation.template_id,
        notation.person_id,
        notation.project_id,
        "BEGIN",
    );
    if let Some(entity_id) = notation.entity_id {
        new_other = new_other.with_entity(entity_id);
    }
    let other_notation = store::notations::create(&surreal, &new_other)
        .await
        .unwrap();
    let client_name = store::questions::find_by_code(&surreal, "custom_text")
        .await
        .unwrap()
        .unwrap();
    store::answers::record(
        &surreal,
        &store::answers::NewAnswer::new(
            client_name.id,
            notation.person_id,
            store::answers::primitive("Other matter client"),
        )
        .in_notation(other_notation.id, "entity")
        .authored_by(store::answers::SOURCE_LAWYER, None),
    )
    .await
    .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/step"))
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
    let html = body_string(resp).await;
    assert!(html.contains("entity"), "html: {html}");
    assert!(
        !html.contains("Other matter client"),
        "stale answer from another notation leaked into prefill: {html}"
    );
}

#[tokio::test]
async fn step_post_writes_answer_signals_runtime_and_redirects_to_next_question() {
    let (app, surreal, nid, runtime) = build_app_and_notation().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/lawyer/notations/{nid}/step"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "value={}",
                    urlencoding("Libra Holdings LLC")
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    // Redirect to GET /step for the next question.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(location, format!("/app/lawyer/notations/{nid}/step"));

    // The runtime saw exactly one transition on the questionnaire
    // timeline: BEGIN → entity via `_`. The walker no longer
    // writes `notation_events` itself — in production the
    // workflows-service worker journals these via `ctx.run`; in this
    // test the in-memory runtime records them in `Vec<WorkflowEvent>`.
    let events =
        StateMachineRuntime::events(runtime.as_ref(), MachineKind::Questionnaire, nid).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].from, StateName::begin());
    assert_eq!(events[0].to.as_str(), "entity");
    assert_eq!(events[0].condition, "_");

    // Answer row landed: `answers` is application data, written by
    // the walker (the worker doesn't touch it). Answers are now
    // notation-scoped, so filter by the notation we just walked.
    let our_answers = store::answers::for_notation(&surreal, nid).await.unwrap();
    assert_eq!(our_answers.len(), 1);
    assert_eq!(
        store::answers::display_value(&our_answers[0].value),
        "Libra Holdings LLC"
    );
    assert_eq!(
        our_answers[0].state_name.as_deref(),
        Some("entity"),
        "the walked state name is recorded on the answer"
    );

    // Next GET asks for the entity's principal office address.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/step"))
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
    let html = body_string(resp).await;
    assert!(html.contains("address__principal_office"));
    assert!(html.contains("step 2 of 8"));
}

#[tokio::test]
async fn step_post_for_unknown_notation_returns_404() {
    let (app, _surreal, _nid, _runtime) = build_app_and_notation().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/app/lawyer/notations/{}/step",
                    uuid::Uuid::from_u128(9999)
                ))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("value=x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn walking_the_full_questionnaire_records_all_transitions_through_end() {
    let (app, _surreal, nid, runtime) = build_app_and_notation().await;

    // Walk all eight questions. The last POST drives the workflow; every
    // answer redirects (303) — the last onto the review screen.
    for value in [
        "Libra Holdings LLC",
        "500 Innovation Way Reno NV 89501",
        "Libra",
        "Firm Principal",
        "Estate plan",
        "2026-09-01",
        "Draft and file the matter documents.",
        "nevada",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/lawyer/notations/{nid}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", urlencoding(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "value={value}");
    }

    // Runtime: BEGIN → entity → principal office → client → firm DRI →
    // project → start date → scope → governing law → END = 9 events on the
    // questionnaire timeline. The walker no longer writes `notation_events`
    // — in production the workflows-service worker does, via
    // `ctx.run`; here, the InMemoryRuntime is the source of truth.
    let events =
        StateMachineRuntime::events(runtime.as_ref(), MachineKind::Questionnaire, nid).await;
    assert_eq!(
        events.len(),
        9,
        "expected 9 questionnaire transitions, got {events:?}"
    );
    assert_eq!(events.last().unwrap().to, StateName::end());

    // GET after END redirects to /app/lawyer (workflow already
    // finished synchronously in the previous POST).
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/step"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/app/lawyer")
    );
}

/// Tiny URL-encoder for the test bodies — only escapes the
/// characters the retainer answers actually contain.
fn urlencoding(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

#[tokio::test]
async fn start_get_renders_the_minimal_create_form() {
    let (app, _surreal, _nid, _runtime) = build_app_and_notation().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer/retainers/new")
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
    let html = body_string(resp).await;
    assert!(html.contains("action=\"/app/lawyer/retainers/new\""));
    assert!(html.contains("name=\"client_email\""));
    // The template picker is a dropdown of the onboarding family, not a
    // free-text code field — so lawyers pick the product, not type a code.
    assert!(html.contains("<select"));
    assert!(html.contains("name=\"retainer_template_code\""));
    assert!(
        html.contains("onboarding__letter"),
        "the onboarding letter should be a selectable option",
    );
    // Only onboarding templates open a matter; an offboarding letter is not
    // an option here (it belongs to the close flow).
    assert!(
        !html.contains("offboarding__letter"),
        "the offboarding letter must not be a matter-open option",
    );
    // The walker collects these; they must NOT be on the create form.
    assert!(!html.contains("name=\"person__client\""));
    assert!(!html.contains("name=\"project_name\""));
}

#[tokio::test]
async fn start_post_creates_person_project_role_notation_and_redirects_to_step() {
    // Fresh app+db (no pre-seeded notation). The POST provisions the
    // project repo, so this test needs its own repo root — process
    // isolation (nextest) means no sibling's env var is visible here.
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-retainer-walk-start-repos-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    // seed_canonical inserts the bundled onboarding__letter
    // template — that's what this test will POST against.
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-walker-start-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/lawyer/retainers/new")
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "client_email={}&retainer_template_code={TEMPLATE_CODE}",
                    "libra%40example.com"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.starts_with("/app/lawyer/notations/"),
        "redirect was {loc:?}"
    );
    assert!(loc.ends_with("/step"));

    // The four rows the walker depends on landed.
    let libra = store::persons::find_by_email_ci(&surreal, "libra@example.com")
        .await
        .unwrap()
        .expect("person row inserted");
    let project = store::projects::find_by_name(&surreal, "(pending) libra@example.com")
        .await
        .unwrap()
        .expect("project row inserted");
    let role = store::projects::participations_for_project(&surreal, project.id)
        .await
        .unwrap()
        .into_iter()
        .find(|role| role.person_id == libra.id)
        .expect("role row inserted");
    assert_eq!(role.participation, "client");
    let notations = store::notations::list_by_person(&surreal, libra.id)
        .await
        .unwrap();
    assert_eq!(notations.len(), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn start_post_refuses_a_self_serve_intake_adverse_to_a_current_client() {
    // A self-serve intake whose email matches a person already adverse to a
    // current client of the firm is refused: the matter, the client role, and
    // the retainer notation all roll back, and the form re-renders with a
    // *generic* message that never discloses the conflict (revealing adversity
    // to a current client would breach that client's confidentiality). #355
    // self-serve conflict gate.
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-retainer-walk-conflict-repos-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-walker-conflict-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let seed_client = |email: &'static str| {
        // The handle is a cheap clone around one shared engine, so each
        // call gets its own without moving the caller's.
        let surreal = surreal.clone();
        async move {
            store::persons::create(
                &surreal,
                &store::persons::NewPerson::with_role(email, email, store::persons::Role::Client),
            )
            .await
            .unwrap()
            .id
        }
    };

    // The opponent is already a client of the firm (an open matter).
    let opponent = seed_client("opponent@example.com").await;
    let existing = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("existing-matter-{}", uuid::Uuid::now_v7()),
            name: "Existing matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        existing.id,
        opponent,
        store::projects::DriSide::Client,
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        existing.id,
        store::test_support::dri_person(&surreal).await,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();

    // The self-serve intake email belongs to a person directly adverse to that
    // current client.
    let proposed = seed_client("newcomer@example.com").await;
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

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/lawyer/retainers/new")
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "client_email={}&retainer_template_code={TEMPLATE_CODE}",
                    "newcomer%40example.com"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    // Refused: back to the form with the generic reason, not a redirect into
    // the walk.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.starts_with("/app/lawyer/retainers/new?"),
        "redirect was {loc}"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(&loc)
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
    let html = body_string(resp).await;
    assert!(
        html.contains("unable to start this intake online"),
        "a blocking conflict re-renders the generic refusal: {html}",
    );
    // …and it discloses nothing about the conflict.
    let lower = html.to_lowercase();
    assert!(
        !lower.contains("adverse") && !lower.contains("conflict"),
        "the refusal must never disclose the conflict: {html}",
    );
    // No matter opened — the whole intake rolled back.
    assert!(
        store::projects::find_by_name(&surreal, "(pending) newcomer@example.com")
            .await
            .unwrap()
            .is_none(),
        "no matter opens on a blocking conflict",
    );
    assert!(
        !store::persons::is_admitted(&surreal, proposed)
            .await
            .unwrap(),
        "a retained client row left without a matter must not remain sign-in admitted",
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn start_post_pins_current_template_version_and_freezes_its_questionnaire() {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-versioned-start-repos-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-versioned-start-storage-{}",
            uuid::Uuid::now_v7()
        )))
        .await
        .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let old_blob = store::assets::ingest_content(
        &surreal,
        &storage,
        br"---
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: END
  END: {}
---

# Old retainer
",
        "text/markdown",
    )
    .await
    .unwrap();
    let current_blob = store::assets::ingest_content(
        &surreal,
        &storage,
        br"---
questionnaire:
  BEGIN:
    _: project__engagement
  project__engagement:
    _: END
  END: {}
---

# Current retainer
",
        "text/markdown",
    )
    .await
    .unwrap();
    let old = store::templates::save_version(
        &surreal,
        None,
        "onboarding__versioned_retainer",
        store::templates::Version {
            title: "Versioned retainer".into(),
            respondent_type: "person".into(),
            asset_id: Some(old_blob),
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let current = store::templates::save_version(
        &surreal,
        None,
        "onboarding__versioned_retainer",
        store::templates::Version {
            title: "Versioned retainer current".into(),
            respondent_type: "person".into(),
            asset_id: Some(current_blob),
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    assert_ne!(old.id, current.id);
    let resolved = store::templates::resolve(&surreal, None, "onboarding__versioned_retainer")
        .await
        .unwrap()
        .expect("versioned template resolves");
    assert_eq!(resolved.id, current.id);
    let seeded_snapshot = workflows::notation_session::questionnaire_snapshot_for_template(
        &surreal,
        Some(&storage),
        &resolved,
    )
    .await
    .expect("current version has a questionnaire snapshot");
    assert!(seeded_snapshot.to_string().contains("project__engagement"));

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/lawyer/retainers/new")
                .header("authorization", portal::test_support::lawyer_bearer_header())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client_email=versioned%40example.com&retainer_template_code=onboarding__versioned_retainer",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    let notation_id: uuid::Uuid = loc
        .trim_start_matches("/app/lawyer/notations/")
        .trim_end_matches("/step")
        .parse()
        .expect("redirect carries the notation id");

    let notation = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        notation.template_id, current.id,
        "matter open must pin the current template version, not the retired row"
    );
    let snapshot = notation
        .questionnaire_snapshot
        .expect("matter open must freeze the current questionnaire at creation");
    let snapshot_text = snapshot.to_string();
    assert!(
        snapshot_text.contains("project__engagement"),
        "snapshot must use the current version's questionnaire: {snapshot_text}"
    );
    assert!(
        !snapshot_text.contains("person__client"),
        "snapshot must not use the retired version's questionnaire: {snapshot_text}"
    );
}

#[tokio::test]
async fn close_matter_post_starts_a_closing_walk_for_an_existing_matter() {
    // A matter that already exists with a client — the close acts on
    // it rather than creating it.
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-close-start-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra-close@example.com"),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-estate-close-{}", uuid::Uuid::now_v7()),
            name: "Libra estate (to close)".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scoped_closing_tmpl = store::templates::save_version(
        &surreal,
        Some(project.id),
        "offboarding__letter",
        store::templates::Version {
            title: "Project Closing Letter".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    store::projects::add_participation(&surreal, project.id, libra.id, "client")
        .await
        .unwrap();

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{}/close", project.code))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.starts_with("/app/lawyer/notations/") && loc.ends_with("/step"),
        "redirect was {loc:?}"
    );

    // An offboarding__letter notation now hangs off the matter, addressed
    // to its client, at BEGIN — ready to walk.
    let notations = store::notations::list_by_project(&surreal, project.id)
        .await
        .unwrap();
    assert_eq!(notations.len(), 1);
    assert_eq!(notations[0].template_id, scoped_closing_tmpl.id);
    assert_eq!(notations[0].person_id, libra.id);
    assert_eq!(notations[0].state, "BEGIN");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn close_walk_renders_firm_signed_letter_and_closes_the_matter() {
    // An open matter with a client. Walk the close end to end and
    // assert the matter flips to `closed` and the closing letter PDF
    // lands in storage.
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-close-walk-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra-closewalk@example.com"),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-estate-closing-{}", uuid::Uuid::now_v7()),
            name: "Libra estate (closing)".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, libra.id, "client")
        .await
        .unwrap();

    // Dispatching runtime with a db so `generate_pdf__closing_letter`
    // renders/persists the PDF and the firm-signature transition runs
    // the `close_matter` side effect (the same in-process path the dev
    // binary uses).
    let inner = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        workflows::DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    let state = AppState {
        storage: storage.clone(),
        workflow_runtime,
        questionnaire_runtime: inner,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Open the close walk.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{}/close", project.code))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    // /app/lawyer/notations/<uuid>/step
    let nid: uuid::Uuid = loc
        .trim_start_matches("/app/lawyer/notations/")
        .trim_end_matches("/step")
        .parse()
        .expect("redirect carries the notation id");

    // Walk the six closing questions; the final POST drives the closing
    // workflow to END and redirects to /app/lawyer.
    let answers = [
        "Libra",
        "Estate plan",
        "Wound up the LLC",
        "paid_in_full",
        "Kept seven years",
        "None",
    ];
    for (i, value) in answers.iter().enumerate() {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/lawyer/notations/{nid}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", urlencoding(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "answer {i}={value}");
        if i == answers.len() - 1 {
            assert_eq!(
                resp.headers().get("location").and_then(|v| v.to_str().ok()),
                Some("/app/lawyer"),
                "final answer should close the matter and return to the firm dashboard"
            );
        }
    }

    // The matter is closed.
    let row = store::projects::find_by_id(&surreal, project.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "closed");

    // The firm-signed closing letter PDF was rendered and persisted.
    let pdf = storage
        .get(&portal::retainer_walk::closing_letter_storage_key(nid))
        .await
        .expect("closing letter PDF persisted")
        .bytes;
    assert!(
        pdf.starts_with(b"%PDF"),
        "expected a PDF, got {} bytes",
        pdf.len()
    );
}

#[tokio::test]
async fn start_post_rejects_missing_at_in_client_email_with_validation_error() {
    // The form renders through Dioxus at `GET /app/lawyer/retainers/new`, so a
    // rejected `POST` no longer re-renders it inline. It redirects back
    // (post/redirect/get) carrying the reason and what was typed, and the form
    // reads all three back out of the query.
    let (app, _surreal, _nid, _runtime) = build_app_and_notation().await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/lawyer/retainers/new")
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client_email=not-an-email&retainer_template_code=onboarding__letter",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.starts_with("/app/lawyer/retainers/new?"),
        "redirect was {loc}"
    );
    assert!(loc.contains("client_email=not-an-email"), "{loc}");
    assert!(
        loc.contains("retainer_template_code=onboarding__letter"),
        "{loc}"
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri(&loc)
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
    let html = body_string(resp).await;
    assert!(&html.contains("client email must contain an @"));
    // Nothing has to be retyped: the email is echoed back into the field and
    // the chosen template stays chosen instead of resetting to the default.
    assert!(html.contains("value=\"not-an-email\""), "{html}");
    assert!(html.contains("onboarding__letter"), "{html}");
}

#[tokio::test]
async fn final_post_drives_workflow_and_renders_result_with_substituted_template() {
    let (app, surreal, nid, _runtime) = build_app_and_notation().await;

    // Walk all eight questions.
    for value in [
        "Libra Holdings LLC",
        "500 Innovation Way Reno NV 89501",
        "Libra",
        "Firm Principal",
        "Estate plan",
        "2026-09-01",
        "Draft and file the matter documents.",
        "nevada",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/lawyer/notations/{nid}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", urlencoding(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        // The last answer lands on the review screen rather than rendering it
        // inline; every earlier one goes to the next question.
        let expected = if value == "nevada" {
            format!("/app/lawyer/notations/{nid}/review")
        } else {
            format!("/app/lawyer/notations/{nid}/step")
        };
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some(expected.as_str()),
        );
    }

    // Following that redirect is where the assembled document appears, with the
    // answers interpolated into the template body.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/lawyer/notations/{nid}/review"))
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
    let html = body_string(resp).await;
    assert!(html.contains("Libra"), "html: {html}");
    assert!(html.contains("Estate plan"));
    // The client's final answer parks at the `lawyer_review` gate — no PDF is
    // rendered on this request. The screen offers the lawyer approve action, not
    // a signature envelope.
    assert!(html.contains("lawyer_review"), "html: {html}");
    assert!(
        html.contains(&format!("/app/lawyer/notations/{nid}/approve-send")),
        "parked review must offer the approve action: {html}"
    );
    assert!(
        !html.contains("sent_for_signature__pending"),
        "no signature envelope until a lawyer approves and sends: {html}"
    );

    // Notation.state parks at the human gate; nothing is rendered or sent
    // on the client's completion request.
    let row = store::notations::find_by_id(&surreal, nid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "lawyer_review");
}
