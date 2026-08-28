#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/project-surfaces/{id}` — the admin-only
//! retry that creates or adopts a Project's Drive ingest folder and source
//! repository.
//!
//! Two things are proved here that the store tests cannot:
//!
//! - **The tier is enforced in the handler**, not only in policy.
//!   `portal::test_support::app_state` builds a router with the policy layer
//!   disabled, so a 403 observed here is the extractor's own `is_admin_tier`
//!   check.
//! - **An unassigned admin still reaches a matter they do not participate
//!   in.** Every other matter write on this surface is participation-scoped;
//!   this door is the deployment retry, so it keys off the id alone.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-project-surfaces-test-key";

struct Fixture {
    app: axum::Router,
    unassigned_admin: String,
    lawyer: String,
    client: String,
    clerk: String,
    project_id: Uuid,
    code: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-project-surfaces-sub", role);
    session.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

fn path(id: Uuid) -> String {
    format!("/app/api/project-surfaces/{id}")
}

async fn person(surreal: &store::surreal::SurrealDb, name: &str, role: Role) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            name,
            format!("{}@example.com", name.to_lowercase()),
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
    let suffix = Uuid::now_v7().simple().to_string();
    let code = format!("acme-{}", &suffix[..8]);
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: "Matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let admin_id = person(&surreal, "Admin", Role::Admin).await;
    let lawyer_id = person(&surreal, "Lawyer", Role::Lawyer).await;
    let client_id = person(&surreal, "Client", Role::Client).await;
    let clerk_id = person(&surreal, "Clerk", Role::Clerk).await;

    store::projects::add_participation(&surreal, project.id, lawyer_id, "attorney")
        .await
        .unwrap();

    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        unassigned_admin: bearer(admin_id, Role::Admin),
        lawyer: bearer(lawyer_id, Role::Lawyer),
        client: bearer(client_id, Role::Client),
        clerk: bearer(clerk_id, Role::Clerk),
        project_id: project.id,
        code,
    }
}

async fn post(fx: &Fixture, uri: &str, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut req = Request::builder().method("POST").uri(uri);
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("the response is JSON")
}

/// The documents-bucket prefix is derived from the code even when Drive and
/// the forge are unconfigured, which is the shape the test suite and the
/// local loop have. Nothing is written to object storage.
#[tokio::test]
async fn an_unassigned_admin_reconciles_the_documents_prefix() {
    let fx = build_fixture().await;

    let resp = post(&fx, &path(fx.project_id), Some(&fx.unassigned_admin)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json(resp).await;
    assert_eq!(body["code"], fx.code);
    assert_eq!(body["documents_prefix"], format!("projects/{}", fx.code));
    assert!(body["drive_folder_id"].is_null(), "{body}");
    assert!(body["repository_url"].is_null(), "{body}");
}

#[tokio::test]
async fn only_the_admin_tier_reaches_the_door() {
    let fx = build_fixture().await;
    let uri = path(fx.project_id);

    assert_eq!(
        post(&fx, &uri, Some(&fx.unassigned_admin)).await.status(),
        StatusCode::OK
    );

    for (tier, auth) in [
        ("lawyer", &fx.lawyer),
        ("clerk", &fx.clerk),
        ("client", &fx.client),
    ] {
        assert_eq!(
            post(&fx, &uri, Some(auth)).await.status(),
            StatusCode::FORBIDDEN,
            "{tier} must not reach an admin reconcile"
        );
    }

    assert_eq!(
        post(&fx, &uri, None).await.status(),
        StatusCode::UNAUTHORIZED,
        "an anonymous caller is refused before the tier is considered"
    );
}

#[tokio::test]
async fn an_unknown_project_is_not_found() {
    let fx = build_fixture().await;
    let resp = post(&fx, &path(Uuid::now_v7()), Some(&fx.unassigned_admin)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
