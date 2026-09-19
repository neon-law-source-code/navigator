//! In-process testimonial route authorization: handler and store refuse
//! writes that Rego may still admit for Owner and Admin.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use portal::api::ApiState;
use serde_json::{json, Value};
use store::persons::{NewPerson, Role};
use store::test_support::mem_surreal;
use tower::ServiceExt;

struct Fixture {
    surreal: store::surreal::SurrealDb,
    app: Router,
    project: store::projects::Project,
    other_project: store::projects::Project,
    client: store::persons::Person,
    other_client: store::persons::Person,
    lawyer: store::persons::Person,
    clerk: store::persons::Person,
    admin: store::persons::Person,
    owner: store::persons::Person,
}

fn session(person: &store::persons::Person) -> portal::SessionData {
    portal::SessionData {
        person_id: Some(person.id),
        ..portal::SessionData::fresh(person.email.clone(), person.role)
    }
}

async fn person(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    role: Role,
) -> store::persons::Person {
    store::persons::create(
        surreal,
        &NewPerson {
            role,
            ..NewPerson::new(name, email)
        },
    )
    .await
    .unwrap()
}

async fn fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let client = person(
        &surreal,
        "Route Client",
        "route-testimonial-client@example.com",
        Role::Client,
    )
    .await;
    let other_client = person(
        &surreal,
        "Route Other Client",
        "route-testimonial-other@example.com",
        Role::Client,
    )
    .await;
    let lawyer = person(
        &surreal,
        "Route Lawyer",
        "route-testimonial-lawyer@example.com",
        Role::Lawyer,
    )
    .await;
    let clerk = person(
        &surreal,
        "Route Clerk",
        "route-testimonial-clerk@example.com",
        Role::Clerk,
    )
    .await;
    let admin = person(
        &surreal,
        "Route Admin",
        "route-testimonial-admin@example.com",
        Role::Admin,
    )
    .await;
    let owner = person(
        &surreal,
        "Route Owner",
        "route-testimonial-owner@example.com",
        Role::Owner,
    )
    .await;
    let project_id = store::test_support::seed_project_surreal(&surreal, "route-testimonial").await;
    let other_project_id =
        store::test_support::seed_project_surreal(&surreal, "route-testimonial-other").await;
    let project = store::projects::find_by_id(&surreal, project_id)
        .await
        .unwrap()
        .expect("seeded project");
    let other_project = store::projects::find_by_id(&surreal, other_project_id)
        .await
        .unwrap()
        .expect("seeded other project");
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        client.id,
        store::projects::DriSide::Client,
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        other_project.id,
        other_client.id,
        store::projects::DriSide::Client,
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        other_project.id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    for (person_id, participation) in [
        (clerk.id, "clerk"),
        (admin.id, "admin"),
        (owner.id, "owner"),
    ] {
        store::projects::add_participation(&surreal, project.id, person_id, participation)
            .await
            .unwrap();
    }

    let state = portal::test_support::app_state(surreal.clone()).await;
    let api_state = ApiState {
        surreal: state.surreal.clone(),
        email: state.email.clone(),
        bootstrap_owner_email: state.bootstrap_owner_email.clone(),
        bootstrap_company: "Synthetic Firm".into(),
        questionnaire_runtime: state.questionnaire_runtime.clone(),
        storage: state.storage.clone(),
        workflow_runtime: state.workflow_runtime.clone(),
        assets_storage: state.assets_storage.clone(),
        forms_registry: state.forms_registry.clone(),
        signature_provider: state.signature_provider.clone(),
        contract_reviewer: state.contract_reviewer.clone(),
        integration_providers: state.integration_providers.clone(),
    };
    Fixture {
        surreal,
        app: portal::api::routes().with_state(api_state),
        project,
        other_project,
        client,
        other_client,
        lawyer,
        clerk,
        admin,
        owner,
    }
}

async fn json_post(
    app: &Router,
    uri: String,
    person: &store::persons::Person,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .extension(session(person))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .expect("build request"),
        )
        .await
        .expect("run request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn save_body(quote: &str, request_public: bool) -> Value {
    json!({
        "quote": quote,
        "attribution": "Founder",
        "request_public": request_public
    })
}

#[tokio::test]
async fn a_client_cannot_save_through_the_api_for_the_wrong_project() {
    let fixture = fixture().await;
    let (ok, _) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Kept on the right matter.", true),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    let before = store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.client.id,
        fixture.project.id,
    )
    .await
    .unwrap();

    let (status, _) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.other_project.id),
        &fixture.client,
        save_body("Wrong matter.", true),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        store::testimonials::for_person_project(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
        )
        .await
        .unwrap(),
        before
    );
    assert!(store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.client.id,
        fixture.other_project.id,
    )
    .await
    .unwrap()
    .is_none());
    assert!(store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.other_client.id,
        fixture.other_project.id,
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn non_client_tiers_cannot_create_consent_through_the_api_save_route() {
    let fixture = fixture().await;
    for person in [
        &fixture.lawyer,
        &fixture.clerk,
        &fixture.admin,
        &fixture.owner,
    ] {
        let (status, _) = json_post(
            &fixture.app,
            format!("/app/api/projects/{}/testimonial", fixture.project.id),
            person,
            save_body("Firm-written consent.", true),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{:?} is admitted past AuthedSession and still refused by the store",
            person.role
        );
        assert!(store::testimonials::for_person_project(
            &fixture.surreal,
            person.id,
            fixture.project.id,
        )
        .await
        .unwrap()
        .is_none());
    }
    assert!(store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.client.id,
        fixture.project.id,
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn admin_cannot_manufacture_consent_or_overwrite_it_through_the_api() {
    let fixture = fixture().await;
    let (ok, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Client consent.", true),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    let testimonial_id: uuid::Uuid = serde_json::from_value(body["id"].clone()).unwrap();
    store::testimonials::publish(
        &fixture.surreal,
        Some(fixture.lawyer.id),
        Role::Lawyer,
        testimonial_id,
    )
    .await
    .unwrap();
    let before = store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.client.id,
        fixture.project.id,
    )
    .await
    .unwrap();

    let (unknown_fields, _) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.admin,
        json!({
            "quote": "Admin overwrite.",
            "request_public": true,
            "consented_at": "2026-01-01T00:00:00Z",
            "published_at": "2026-01-02T00:00:00Z"
        }),
    )
    .await;
    assert_eq!(unknown_fields, StatusCode::BAD_REQUEST);

    let (status, _) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.admin,
        save_body("Admin overwrite.", true),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        store::testimonials::for_person_project(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
        )
        .await
        .unwrap(),
        before
    );
}

#[tokio::test]
async fn a_client_cannot_publish_through_the_api_door() {
    let fixture = fixture().await;
    let (ok, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Waiting for approval.", true),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    assert!(body["published_at"].is_null());
    let testimonial_id: uuid::Uuid = serde_json::from_value(body["id"].clone()).unwrap();
    let before = store::testimonials::for_person_project(
        &fixture.surreal,
        fixture.client.id,
        fixture.project.id,
    )
    .await
    .unwrap();

    let (status, _) = json_post(
        &fixture.app,
        format!("/app/api/testimonials/{testimonial_id}/publish"),
        &fixture.client,
        json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "LawyerSession refuses a client before the store write"
    );
    assert_eq!(
        store::testimonials::for_person_project(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
        )
        .await
        .unwrap(),
        before
    );
    assert!(
        store::testimonials::published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revoking_consent_through_the_api_removes_a_published_row_from_public_reads() {
    let fixture = fixture().await;
    let (ok, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Publish me.", true),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    let testimonial_id: uuid::Uuid = serde_json::from_value(body["id"].clone()).unwrap();
    let (published, _) = json_post(
        &fixture.app,
        format!("/app/api/testimonials/{testimonial_id}/publish"),
        &fixture.lawyer,
        json!({}),
    )
    .await;
    assert_eq!(published, StatusCode::OK);
    assert_eq!(
        store::testimonials::published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let (revoked, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Keep this private now.", false),
    )
    .await;
    assert_eq!(revoked, StatusCode::OK);
    assert!(body["consented_at"].is_null());
    assert!(body["published_at"].is_null());
    assert!(
        store::testimonials::published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn editing_through_the_api_clears_publication_and_follows_the_submitted_consent() {
    let fixture = fixture().await;
    let (ok, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("First public request.", true),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    let first_consent = body["consented_at"].as_str().unwrap().to_string();
    let testimonial_id: uuid::Uuid = serde_json::from_value(body["id"].clone()).unwrap();
    json_post(
        &fixture.app,
        format!("/app/api/testimonials/{testimonial_id}/publish"),
        &fixture.lawyer,
        json!({}),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let (renewed, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Edited public request.", true),
    )
    .await;
    assert_eq!(renewed, StatusCode::OK);
    assert_eq!(body["quote"], "Edited public request.");
    assert!(body["published_at"].is_null());
    assert_ne!(body["consented_at"].as_str().unwrap(), first_consent);
    assert!(
        store::testimonials::published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let (cleared, body) = json_post(
        &fixture.app,
        format!("/app/api/projects/{}/testimonial", fixture.project.id),
        &fixture.client,
        save_body("Edited private note.", false),
    )
    .await;
    assert_eq!(cleared, StatusCode::OK);
    assert!(body["consented_at"].is_null());
    assert!(body["published_at"].is_null());
}
