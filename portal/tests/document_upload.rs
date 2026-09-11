//! REST document-upload size boundary tests.

use axum::body::Body;
use axum::http::{header, Request};
use axum::Router;
use base64::Engine as _;
use portal::api::ApiState;
use serde_json::{json, Value};
use store::persons::{NewPerson, Role};
use store::test_support::mem_surreal;
use tower::ServiceExt;

// These tests exercise the route in-process, so no external matter data is used.

struct Fixture {
    app: Router,
    project_id: uuid::Uuid,
    session: portal::SessionData,
}

async fn fixture(code: &str) -> Fixture {
    let db = mem_surreal().await;
    let project_id = store::test_support::seed_project_surreal(&db, code).await;
    let lawyer = store::test_support::ensure_person(
        &db,
        &NewPerson {
            name: "Synthetic Lawyer".into(),
            email: "synthetic-lawyer@example.com".into(),
            role: Role::Lawyer,
            ..Default::default()
        },
    )
    .await;
    store::projects::designate_dri_in_surreal(
        &db,
        project_id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .expect("seed the synthetic lawyer's participation row");

    let state = portal::test_support::app_state(db).await;
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
    let session = portal::SessionData {
        person_id: Some(lawyer.id),
        ..portal::SessionData::fresh("synthetic-lawyer-sub", Role::Lawyer)
    };

    Fixture {
        app: portal::api::routes().with_state(api_state),
        project_id,
        session,
    }
}

fn request_body(bytes: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "filename": "synthetic-document.bin",
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "content_type": "application/octet-stream",
        "kind": "unclassified",
        "visibility": "internal"
    }))
    .expect("serialize synthetic upload")
}

async fn upload(fixture: &Fixture, bytes: &[u8], content_length: Option<&str>) -> Value {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/app/api/projects/{}/documents",
            fixture.project_id
        ))
        .header(header::CONTENT_TYPE, "application/json")
        .extension(fixture.session.clone());
    if let Some(content_length) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, content_length);
    }
    let response = fixture
        .app
        .clone()
        .oneshot(
            builder
                .body(Body::from(request_body(bytes)))
                .expect("build upload"),
        )
        .await
        .expect("run upload");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read upload response");
    let json: Value = serde_json::from_slice(&body).expect("upload response is JSON");
    json!({ "status": status.as_u16(), "body": json })
}

#[tokio::test]
async fn accepts_a_document_well_over_the_framework_default() {
    let fixture = fixture("upload-limit-accepted").await;
    let bytes = vec![b'x'; 2 * 1024 * 1024];
    let result = upload(&fixture, &bytes, None).await;

    assert_eq!(result["status"], 201);
    assert_eq!(result["body"]["current_version"]["size_bytes"], bytes.len());
}

#[tokio::test]
async fn refuses_a_document_over_the_limit_with_both_sizes_for_lied_and_missing_lengths() {
    let fixture = fixture("upload-limit-refused").await;
    let actual = store::documents::MAX_DOCUMENT_UPLOAD_BYTES + 1;
    let bytes = vec![b'x'; actual];
    for result in [
        upload(&fixture, &bytes, Some("1")).await,
        upload(&fixture, &bytes, None).await,
    ] {
        let message = result["body"]["message"].as_str().expect("size message");

        assert_eq!(result["status"], 400);
        assert_eq!(result["body"]["error"], "document_too_large");
        assert!(message.contains(&store::documents::MAX_DOCUMENT_UPLOAD_BYTES.to_string()));
        assert!(message.contains(&actual.to_string()));
        assert!(!message.contains("compress"));
        assert!(!message.contains("re-render"));
    }
}
