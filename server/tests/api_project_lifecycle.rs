#![allow(clippy::doc_markdown)]
//! Integration tests for `GET /app/api/project-lifecycle` — the admin-only
//! deployment-wide lifecycle read.

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::projects::{transition_project, NewProject, Transition};
use store::test_support::{mem_surreal, seed_entity};
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-project-lifecycle-test-key";
const PATH: &str = "/app/api/project-lifecycle";

struct Fixture {
    app: axum::Router,
    admin: String,
    lawyer: String,
    client: String,
    codes: [String; 3],
}

fn bearer(role: Role) -> String {
    let session = SessionData::fresh("api-project-lifecycle-sub", role);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let entity_id = seed_entity(&surreal).await;
    let suffix = Uuid::now_v7().simple().to_string();
    let codes = [
        format!("open-{}", &suffix[..8]),
        format!("closed-{}", &suffix[..8]),
        format!("archived-{}", &suffix[..8]),
    ];

    let mut ids = Vec::new();
    for code in &codes {
        ids.push(
            store::projects::create(
                &surreal,
                &NewProject {
                    code: code.clone(),
                    name: "Matter".into(),
                    status: "open".into(),
                    entity_id,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .id,
        );
    }
    transition_project(&surreal, ids[1], Transition::Close)
        .await
        .unwrap();
    transition_project(&surreal, ids[2], Transition::Archive)
        .await
        .unwrap();

    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        admin: bearer(Role::Admin),
        lawyer: bearer(Role::Lawyer),
        client: bearer(Role::Client),
        codes,
    }
}

async fn get(fx: &Fixture, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut request = Request::builder().method("GET").uri(PATH);
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(response: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn admin_reads_every_project_lifecycle_without_matter_content() {
    let fx = build_fixture().await;
    let response = get(&fx, Some(&fx.admin)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let rows = json(response).await.as_array().unwrap().clone();
    assert_eq!(rows.len(), 3);

    let fields: BTreeSet<&str> = rows[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, BTreeSet::from(["code", "status", "closed_at"]));

    let by_code = rows
        .into_iter()
        .map(|row| (row["code"].as_str().unwrap().to_owned(), row))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(by_code[&fx.codes[0]]["status"], "open");
    assert!(by_code[&fx.codes[0]]["closed_at"].is_null());
    assert_eq!(by_code[&fx.codes[1]]["status"], "closed");
    assert!(by_code[&fx.codes[1]]["closed_at"].is_string());
    assert_eq!(by_code[&fx.codes[2]]["status"], "archived");
    assert!(by_code[&fx.codes[2]]["closed_at"].is_string());
}

#[tokio::test]
async fn only_the_admin_tier_reaches_the_project_lifecycle_read() {
    let fx = build_fixture().await;
    assert_eq!(get(&fx, Some(&fx.admin)).await.status(), StatusCode::OK);
    assert_eq!(
        get(&fx, Some(&fx.lawyer)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&fx, Some(&fx.client)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(&fx, None).await.status(), StatusCode::UNAUTHORIZED);
}
