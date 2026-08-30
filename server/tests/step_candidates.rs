#![allow(clippy::doc_markdown)]
//! Integration tests for the DB-backed record/reference pickers on the
//! questionnaire walk — the widened `/step` JSON contract PR 1 of #349
//! ships for the CLI text-adventure to consume.
//!
//! Asserts, against a real questionnaire:
//!   1. `GET …/step?format=json` carries `question.candidates` (`[{id,name}]`)
//!      for a record/reference question — global seed rows for `country`,
//!      the project-scoped matter entity for `entity`.
//!   2. `POST …/step` with a selected `id` resolves the row and stores the
//!      `{"value":name,"name":name,"id":uuid}` envelope; the read-back
//!      surfaces `<state>.id`.
//!   3. An out-of-scope / unknown `id` is rejected — the walk doesn't advance.
//!   4. The browser's `country` `<select>` path (POST the display name as
//!      `value`, no `id`) still works and now resolves + stores the id.
//!   5. A record type (`entity`) still free-types a new row (no id) — so the
//!      existing free-typed person/entity walks stay green.
//!   6. A `person` pick is project-scoped — the matter's DRI is offered, the
//!      off-matter client is not, and the picked id lands in the envelope.
//!   7. A `jurisdiction` reference offers candidates but still free-types —
//!      an off-list value is kept (no id), unlike an off-list `country`.
//!   8. A malformed (non-uuid) `id` is rejected before any candidate match.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::InMemoryRuntime;

/// A questionnaire exercising a global reference pick (`country`, twice),
/// a project-scoped record pick (`entity`), a project-scoped `person` pick,
/// a global `jurisdiction` reference (candidates offered, free-typing still
/// allowed), and a trailing free-text question so none of the picks is the
/// terminal step (which would drive the post-questionnaire workflow).
const QUESTIONNAIRE: &[u8] = br"---
questionnaire:
  BEGIN:
    _: country__of_birth
  country__of_birth:
    _: country__of_citizenship
  country__of_citizenship:
    _: entity__company
  entity__company:
    _: person__contact
  person__contact:
    _: jurisdiction__formation
  jurisdiction__formation:
    _: custom_text__note
  custom_text__note:
    _: END
  END: {}
---

# Candidate walk
";

/// Build the app with a project (bound to a seeded entity), a notation on
/// a template carrying [`QUESTIONNAIRE`], and return the router + db + the
/// notation id + the project's entity id.
async fn build() -> (
    axum::Router,
    store::surreal::SurrealDb,
    uuid::Uuid,
    uuid::Uuid,
) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-step-candidates-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-step-candidates-store-{}",
            uuid::Uuid::now_v7()
        )))
        .await
        .unwrap(),
    );
    // Seeds the canonical templates, the question registry rows, and the
    // seeded jurisdictions (countries) the picker lists.
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let blob = store::assets::ingest_content(&surreal, &storage, QUESTIONNAIRE, "text/markdown")
        .await
        .unwrap();
    let template = store::templates::save_version(
        &surreal,
        None,
        "test__candidate_walk",
        store::templates::Version {
            title: "Candidate walk".into(),
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
            code: format!("candidate-matter-{}", uuid::Uuid::now_v7()),
            name: "Candidate matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Put the DRI Fixture on the matter's participation ledger — the collapsed
    // form of the retired lawyer/client DRI columns. The person picker is
    // scoped to matter people, so this is what makes the DRI a candidate.
    let dri = store::test_support::dri_person(&surreal).await;
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        dri,
        store::projects::DriSide::Lawyer,
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
        entity_id,
    )
}

/// GET the current step as parsed JSON.
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

/// The seeded `Mexico` country candidate's id, read off the current step.
async fn mexico_id(app: &axum::Router, nid: uuid::Uuid) -> String {
    step_json(app, nid).await["question"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mexico")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Answer the two country picks (by id) and free-type the entity, leaving
/// the walk on `person__contact` — the first project-scoped `person` step.
async fn walk_to_person(app: &axum::Router, nid: uuid::Uuid) {
    let mx = mexico_id(app, nid).await;
    step_post(app, nid, format!("id={mx}")).await;
    step_post(app, nid, format!("id={mx}")).await;
    step_post(app, nid, "value=Bright Star Ventures".into()).await;
    assert_eq!(
        step_json(app, nid).await["question"]["code"],
        "person__contact",
        "walk lands on the person step"
    );
}

/// POST one answer body; return the response status.
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

/// The latest stored answer envelope for a walked state, if any.
async fn latest_answer(
    surreal: &store::surreal::SurrealDb,
    nid: uuid::Uuid,
    state_name: &str,
) -> Option<serde_json::Value> {
    // Append-only: the last row for this state is the latest answer.
    store::answers::for_notation(surreal, nid)
        .await
        .unwrap()
        .into_iter()
        .rfind(|a| a.state_name.as_deref() == Some(state_name))
        .map(|a| a.value)
}

#[tokio::test]
async fn country_step_carries_seeded_candidates() {
    let (app, _surreal, nid, _entity_id) = build().await;
    let json = step_json(&app, nid).await;
    let question = &json["question"];
    assert_eq!(question["code"], "country__of_birth");
    assert_eq!(question["answer_type"], "country");
    let candidates = question["candidates"].as_array().expect("candidates array");
    assert!(
        candidates.len() > 1,
        "the country picker lists every seeded country: {candidates:?}"
    );
    // Each candidate is a real {id, name} row — the id is a uuid.
    let mexico = candidates
        .iter()
        .find(|c| c["name"] == "Mexico")
        .expect("Mexico is a seeded country candidate");
    uuid::Uuid::parse_str(mexico["id"].as_str().unwrap()).expect("candidate id is a uuid");
}

#[tokio::test]
async fn posting_a_candidate_id_stores_the_row_id_in_the_envelope() {
    let (app, surreal, nid, _entity_id) = build().await;
    let json = step_json(&app, nid).await;
    let mexico_id = json["question"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mexico")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Post the picker selection by id — advances (303) to the next step.
    assert_eq!(
        step_post(&app, nid, format!("id={mexico_id}")).await,
        StatusCode::SEE_OTHER
    );

    // The stored envelope mirrors the row: value/name are the display
    // string (placeholders unchanged), id is the selected row.
    let value = latest_answer(&surreal, nid, "country__of_birth")
        .await
        .expect("country answer persisted");
    assert_eq!(value["value"], "Mexico");
    assert_eq!(value["name"], "Mexico");
    assert_eq!(
        value["id"], mexico_id,
        "the picked row id lands in the envelope"
    );
}

#[tokio::test]
async fn country_select_name_path_resolves_and_stores_the_id() {
    // The browser `<select>` posts the display name as `value` (no `id`).
    // A reference answer still resolves to the seeded row and now stores it.
    let (app, surreal, nid, _entity_id) = build().await;
    // Move past of_birth so we're on of_citizenship.
    let mexico_id = step_json(&app, nid).await["question"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mexico")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        step_post(&app, nid, format!("id={mexico_id}")).await,
        StatusCode::SEE_OTHER
    );

    // of_citizenship: post the NAME, as the select does.
    assert_eq!(
        step_json(&app, nid).await["question"]["code"],
        "country__of_citizenship"
    );
    assert_eq!(
        step_post(&app, nid, "value=Mexico".into()).await,
        StatusCode::SEE_OTHER
    );
    let value = latest_answer(&surreal, nid, "country__of_citizenship")
        .await
        .expect("citizenship answer persisted");
    assert_eq!(value["name"], "Mexico");
    assert_eq!(
        value["id"], mexico_id,
        "the name path resolves to the same seeded row id"
    );
}

#[tokio::test]
async fn an_unknown_country_name_does_not_advance() {
    let (app, surreal, nid, _entity_id) = build().await;
    // A hand-crafted value naming no seeded country is rejected — redirect
    // back to the same step, no answer row written.
    assert_eq!(
        step_post(&app, nid, "value=Atlantis".into()).await,
        StatusCode::SEE_OTHER
    );
    assert!(
        latest_answer(&surreal, nid, "country__of_birth")
            .await
            .is_none(),
        "an off-list reference value must not persist"
    );
    // Still on the first question.
    assert_eq!(
        step_json(&app, nid).await["question"]["code"],
        "country__of_birth"
    );
}

#[tokio::test]
async fn entity_candidates_are_scoped_to_the_matter_and_reject_an_out_of_scope_id() {
    let (app, surreal, nid, entity_id) = build().await;
    // Walk past both country questions to reach entity__company.
    let mexico_id = step_json(&app, nid).await["question"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mexico")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    step_post(&app, nid, format!("id={mexico_id}")).await;
    step_post(&app, nid, format!("id={mexico_id}")).await;

    // entity__company: the picker offers exactly the matter's entity.
    let json = step_json(&app, nid).await;
    assert_eq!(json["question"]["code"], "entity__company");
    let candidates = json["question"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "only the matter entity is in scope");
    assert_eq!(candidates[0]["id"], entity_id.to_string());

    // An id that isn't in scope (a random uuid) is rejected — no advance.
    let stray = uuid::Uuid::now_v7();
    assert_eq!(
        step_post(&app, nid, format!("id={stray}")).await,
        StatusCode::SEE_OTHER
    );
    assert!(
        latest_answer(&surreal, nid, "entity__company")
            .await
            .is_none(),
        "an out-of-scope id must not persist"
    );
    assert_eq!(
        step_json(&app, nid).await["question"]["code"],
        "entity__company",
        "walk stays on the entity question after a rejected id"
    );

    // The in-scope entity id resolves and stores the row.
    assert_eq!(
        step_post(&app, nid, format!("id={entity_id}")).await,
        StatusCode::SEE_OTHER
    );
    let value = latest_answer(&surreal, nid, "entity__company")
        .await
        .expect("entity answer persisted");
    assert_eq!(value["id"], entity_id.to_string());
}

#[tokio::test]
async fn a_record_type_still_free_types_a_new_row_without_an_id() {
    // A record type (`entity`) may name a NEW row the picker doesn't list —
    // this is how the LLC walk creates the entity being formed. Free text,
    // no id, still advances (keeps the existing free-typed walks green).
    let (app, surreal, nid, _entity_id) = build().await;
    let mexico_id = step_json(&app, nid).await["question"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mexico")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    step_post(&app, nid, format!("id={mexico_id}")).await;
    step_post(&app, nid, format!("id={mexico_id}")).await;

    assert_eq!(
        step_post(&app, nid, "value=Bright Star Ventures".into()).await,
        StatusCode::SEE_OTHER
    );
    let value = latest_answer(&surreal, nid, "entity__company")
        .await
        .expect("free-typed entity persisted");
    assert_eq!(value["value"], "Bright Star Ventures");
    assert_eq!(value["name"], "Bright Star Ventures");
    assert!(
        value.get("id").is_none(),
        "a free-typed record answer carries no resolved id: {value}"
    );
}

#[tokio::test]
async fn person_candidates_are_scoped_to_the_matter_and_store_the_picked_id() {
    // The `person` picker is project-scoped: it offers the matter's people
    // (here the DRI on the project), never every person in the DB. The
    // notation's own client (`Libra`) is not on the matter's participation
    // rows, so the picker must not surface her.
    let (app, surreal, nid, _entity_id) = build().await;
    walk_to_person(&app, nid).await;

    let json = step_json(&app, nid).await;
    assert_eq!(json["question"]["answer_type"], "person");
    let candidates = json["question"]["candidates"].as_array().unwrap();
    let names: Vec<&str> = candidates
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"DRI Fixture"),
        "the matter's DRI is a person candidate: {names:?}"
    );
    assert!(
        !names.contains(&"Libra"),
        "an off-matter person (the client) is not offered: {names:?}"
    );

    // Post the in-scope person's id — it resolves and lands in the envelope.
    let dri = candidates
        .iter()
        .find(|c| c["name"] == "DRI Fixture")
        .unwrap();
    let dri_id = dri["id"].as_str().unwrap().to_string();
    assert_eq!(
        step_post(&app, nid, format!("id={dri_id}")).await,
        StatusCode::SEE_OTHER
    );
    let value = latest_answer(&surreal, nid, "person__contact")
        .await
        .expect("person answer persisted");
    assert_eq!(value["name"], "DRI Fixture");
    assert_eq!(
        value["id"], dri_id,
        "the picked person id lands in the envelope"
    );
}

#[tokio::test]
async fn a_jurisdiction_offers_candidates_but_still_free_types() {
    // `jurisdiction` (unlike `country`) offers candidates for a picker but
    // does not force a pick — a free-typed value naming no seeded row is
    // kept as free text (no resolved id), so real specs that free-type a
    // formation jurisdiction keep working.
    let (app, surreal, nid, _entity_id) = build().await;
    walk_to_person(&app, nid).await;
    // Advance past person__contact (free-typed) to jurisdiction__formation.
    step_post(&app, nid, "value=Someone New".into()).await;

    let json = step_json(&app, nid).await;
    assert_eq!(json["question"]["code"], "jurisdiction__formation");
    assert_eq!(json["question"]["answer_type"], "jurisdiction");
    assert!(
        !json["question"]["candidates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the jurisdiction picker still lists seeded rows"
    );

    // A free-typed jurisdiction that matches no seeded row is accepted and
    // stored without an id — not rejected the way an off-list country is.
    assert_eq!(
        step_post(&app, nid, "value=State of Atlantis".into()).await,
        StatusCode::SEE_OTHER
    );
    let value = latest_answer(&surreal, nid, "jurisdiction__formation")
        .await
        .expect("free-typed jurisdiction persisted");
    assert_eq!(value["value"], "State of Atlantis");
    assert!(
        value.get("id").is_none(),
        "a free-typed jurisdiction carries no resolved id: {value}"
    );
}

#[tokio::test]
async fn a_malformed_id_is_rejected() {
    // A picker `id` that isn't a uuid is rejected before any candidate
    // lookup can match it — the walk stays put, nothing persists.
    let (app, surreal, nid, _entity_id) = build().await;
    assert_eq!(
        step_post(&app, nid, "id=not-a-uuid".into()).await,
        StatusCode::SEE_OTHER
    );
    assert!(
        latest_answer(&surreal, nid, "country__of_birth")
            .await
            .is_none(),
        "a malformed id must not persist"
    );
    assert_eq!(
        step_json(&app, nid).await["question"]["code"],
        "country__of_birth",
        "walk stays on the first question after a malformed id"
    );
}
