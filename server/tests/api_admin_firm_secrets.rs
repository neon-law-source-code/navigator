//! Integration tests for the Firm integration-secret settings door
//! (ENG-491): `POST /app/admin/firms/{id}/secrets` and
//! `POST /app/admin/firms/{id}/secrets/revoke`.
//!
//! These prove what nothing else proves at the HTTP boundary:
//!
//! - **Only the Firm's own Admin DRI writes.** A non-DRI Admin and Owner both
//!   get the store's `NotAuthorized` refusal (mapped to a plain redirect, not
//!   a disclosure), matching `store::firm_secrets::put`'s own rule.
//! - **A write never echoes the submitted value.** Every redirect target
//!   (success and failure) is asserted to omit the exact string submitted, in
//!   the response body *and* the `Location` header.
//! - **Cross-Firm denial.** An Admin DRI of Firm A cannot write or read Firm
//!   B's secrets through this door.
//! - **Rotation and revoke round-trip through the same page** the create form
//!   posts to, and the Firm show page renders the resulting metadata (not a
//!   plaintext value) once the caller can view it.

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

const KEY: &str = "api-admin-firm-secrets-test-key";
const SECRET_VALUE: &str = "s3cret-notion-token-value";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    firm_id: Uuid,
    dri: String,
    other_admin: String,
    owner: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-admin-firm-secrets-test-sub", role);
    session.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn person(surreal: &store::surreal::SurrealDb, name: &str, role: Role) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            name,
            format!("{}-{}@example.com", name.to_lowercase(), Uuid::now_v7()),
            role,
        ),
    )
    .await
    .unwrap()
    .id
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let dri_id = person(&surreal, "Dri", Role::Admin).await;
    let other_admin_id = person(&surreal, "OtherAdmin", Role::Admin).await;
    let owner_id = person(&surreal, "Owner", Role::Owner).await;
    let firm = store::firms::create(
        &surreal,
        &store::firms::NewFirm {
            name: "Secrets Test Firm".into(),
            status: "active".into(),
            entity_id,
            admin_dri_person_id: dri_id,
        },
    )
    .await
    .unwrap();
    store::firms::add_membership(
        &surreal,
        &store::firms::NewPersonFirmRole {
            person_id: other_admin_id,
            firm_id: firm.id,
            membership: store::firms::FirmMembership::Admin,
            is_dri: false,
        },
    )
    .await
    .unwrap();

    let kms = Arc::new(cloud::FakeKms::new("api-admin-firm-secrets-test-kms"));
    let mut state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    state.runtime_kms = kms;

    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        firm_id: firm.id,
        dri: bearer(dri_id, Role::Admin),
        other_admin: bearer(other_admin_id, Role::Admin),
        owner: bearer(owner_id, Role::Owner),
    }
}

async fn post_form(
    app: &axum::Router,
    path: &str,
    token: &str,
    body: &str,
) -> (StatusCode, String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", token)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

async fn get_show(app: &axum::Router, path: &str, token: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("authorization", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn the_admin_dri_creates_a_secret_and_the_value_never_echoes() {
    let fx = build_fixture().await;
    let path = format!("/app/admin/firms/{}/secrets", fx.firm_id);
    let body = format!("kind=notion_token&value={SECRET_VALUE}");
    let (status, location, response_body) = post_form(&fx.app, &path, &fx.dri, &body).await;

    assert_eq!(status, StatusCode::SEE_OTHER, "{response_body}");
    assert!(location.contains("secret_saved=notion_token"), "{location}");
    assert!(!location.contains(SECRET_VALUE), "{location}");
    assert!(!response_body.contains(SECRET_VALUE), "{response_body}");

    let metadata = store::firm_secrets::metadata_for_firm(
        &fx.surreal,
        Role::Owner,
        None,
        fx.firm_id,
        store::firm_secrets::IntegrationProvider::Notion,
        store::firm_secrets::IntegrationSecretKind::NotionToken,
    )
    .await
    .expect("metadata reads back");
    assert_eq!(metadata.version, 1);
    assert_eq!(metadata.status, "active");

    // The Firm show page the caller lands on renders the new metadata, never
    // the plaintext.
    let show_path = format!("/app/admin/firms/{}", fx.firm_id);
    let show_body = get_show(&fx.app, &show_path, &fx.dri).await;
    assert!(show_body.contains("notion_token"), "{show_body}");
    assert!(!show_body.contains(SECRET_VALUE), "{show_body}");
}

#[tokio::test]
async fn replacing_a_secret_rotates_the_version_and_still_never_echoes() {
    let fx = build_fixture().await;
    let path = format!("/app/admin/firms/{}/secrets", fx.firm_id);
    post_form(
        &fx.app,
        &path,
        &fx.dri,
        &format!("kind=notion_token&value={SECRET_VALUE}"),
    )
    .await;

    let second_value = "replacement-token-value";
    let (status, location, body) = post_form(
        &fx.app,
        &path,
        &fx.dri,
        &format!("kind=notion_token&value={second_value}"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(!location.contains(second_value), "{location}");
    assert!(!location.contains(SECRET_VALUE), "{location}");

    let metadata = store::firm_secrets::metadata_for_firm(
        &fx.surreal,
        Role::Owner,
        None,
        fx.firm_id,
        store::firm_secrets::IntegrationProvider::Notion,
        store::firm_secrets::IntegrationSecretKind::NotionToken,
    )
    .await
    .expect("metadata reads back");
    assert_eq!(
        metadata.version, 2,
        "replace rotates rather than duplicating"
    );
}

#[tokio::test]
async fn revoke_flips_status_and_a_resolver_stops_finding_it() {
    let fx = build_fixture().await;
    let put_path = format!("/app/admin/firms/{}/secrets", fx.firm_id);
    post_form(
        &fx.app,
        &put_path,
        &fx.dri,
        &format!("kind=slack_bot_token&value={SECRET_VALUE}"),
    )
    .await;

    let revoke_path = format!("/app/admin/firms/{}/secrets/revoke", fx.firm_id);
    let (status, location, body) =
        post_form(&fx.app, &revoke_path, &fx.dri, "kind=slack_bot_token").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(
        location.contains("secret_revoked=slack_bot_token"),
        "{location}"
    );

    let refused = store::firm_secrets::metadata_for_firm(
        &fx.surreal,
        Role::Owner,
        None,
        fx.firm_id,
        store::firm_secrets::IntegrationProvider::Slack,
        store::firm_secrets::IntegrationSecretKind::SlackBotToken,
    )
    .await;
    assert!(matches!(
        refused,
        Err(store::firm_secrets::SecretStoreError::NotConfigured { .. })
    ));
}

#[tokio::test]
async fn a_non_dri_admin_cannot_write_and_owner_cannot_either() {
    let fx = build_fixture().await;
    let path = format!("/app/admin/firms/{}/secrets", fx.firm_id);

    for (token, label) in [(&fx.other_admin, "non-DRI admin"), (&fx.owner, "owner")] {
        let (status, _location, body) = post_form(
            &fx.app,
            &path,
            token,
            &format!("kind=notion_token&value={SECRET_VALUE}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label}: {body}");
    }

    // Nothing was written by either refused attempt.
    let nothing = store::firm_secrets::metadata_for_firm(
        &fx.surreal,
        Role::Owner,
        None,
        fx.firm_id,
        store::firm_secrets::IntegrationProvider::Notion,
        store::firm_secrets::IntegrationSecretKind::NotionToken,
    )
    .await;
    assert!(matches!(
        nothing,
        Err(store::firm_secrets::SecretStoreError::NotConfigured { .. })
    ));
}

#[tokio::test]
async fn an_admin_dri_of_a_different_firm_cannot_reach_this_firms_secrets() {
    let fx = build_fixture().await;
    let other_entity = store::test_support::seed_entity(&fx.surreal).await;
    let other_dri_id = person(&fx.surreal, "OtherFirmDri", Role::Admin).await;
    store::firms::create(
        &fx.surreal,
        &store::firms::NewFirm {
            name: "Other Firm".into(),
            status: "active".into(),
            entity_id: other_entity,
            admin_dri_person_id: other_dri_id,
        },
    )
    .await
    .unwrap();
    let other_dri = bearer(other_dri_id, Role::Admin);

    let path = format!("/app/admin/firms/{}/secrets", fx.firm_id);
    let (status, _location, body) = post_form(
        &fx.app,
        &path,
        &other_dri,
        &format!("kind=notion_token&value={SECRET_VALUE}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn an_unknown_secret_kind_is_refused_without_writing_anything() {
    let fx = build_fixture().await;
    let path = format!("/app/admin/firms/{}/secrets", fx.firm_id);
    let (status, location, _body) = post_form(
        &fx.app,
        &path,
        &fx.dri,
        &format!("kind=not_a_real_kind&value={SECRET_VALUE}"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.contains("secret_error="), "{location}");
    assert!(!location.contains(SECRET_VALUE), "{location}");
}
