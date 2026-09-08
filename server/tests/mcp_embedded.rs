#![allow(clippy::doc_markdown)]
//! Tests that the `mcp` library router is mounted at `POST /mcp` by
//! the main composed router and that the same `require_auth` /
//! `require_policy` route_layers gate it. Drives the router via
//! `tower::ServiceExt::oneshot` — no socket binding.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, EncodingKey, Header};
use portal::{AppState, AuthClaims, AuthConfig};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn state_with(auth: AuthConfig) -> AppState {
    AppState {
        auth,
        storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-mcp-test-storage"))
                .await
                .unwrap(),
        ),
        ..portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
    }
}

fn mcp_request(body: &Value, bearer: Option<&str>) -> Request<Body> {
    mcp_request_at("/mcp", body, bearer)
}

fn mcp_request_at(uri: &str, body: &Value, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn mcp_initialize_round_trips_when_auth_disabled() {
    // disabled=true, so the require_auth layer passes through.
    let state = state_with(AuthConfig::new(true, None)).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(mcp_request(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["result"]["serverInfo"]["name"], "navigator-mcp");
}

#[tokio::test]
async fn mcp_rejects_request_without_bearer_when_auth_enforced() {
    let state = state_with(AuthConfig::new(false, Some("test-secret"))).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(mcp_request(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// ENG-84: `/app/mcp` is the private alias mounted beside `/mcp`, carrying
/// the identical Bearer-only stack — no session cookie, so an anonymous
/// caller still gets a bare `401`, never a login redirect.
#[tokio::test]
async fn app_mcp_rejects_request_without_bearer_when_auth_enforced() {
    let state = state_with(AuthConfig::new(false, Some("test-secret"))).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(mcp_request_at(
            "/app/mcp",
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// The same valid bearer that authenticates `/mcp` authenticates its `/app`
/// alias — one auth stack, mounted twice.
#[tokio::test]
async fn app_mcp_accepts_valid_bearer_token_when_auth_enforced() {
    let state = state_with(AuthConfig::new(false, Some("test-secret"))).await;
    let claims = AuthClaims {
        sub: "lawyer@neonlaw.com".into(),
        exp: i64::try_from(jsonwebtoken::get_current_timestamp() + 3600).unwrap(),
        role: store::persons::Role::Lawyer,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"test-secret"),
    )
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(mcp_request_at(
            "/app/mcp",
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"aida_create_person"));
}

#[tokio::test]
async fn mcp_accepts_valid_bearer_token_when_auth_enforced() {
    let state = state_with(AuthConfig::new(false, Some("test-secret"))).await;
    let claims = AuthClaims {
        sub: "lawyer@neonlaw.com".into(),
        exp: i64::try_from(jsonwebtoken::get_current_timestamp() + 3600).unwrap(),
        role: store::persons::Role::Lawyer,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"test-secret"),
    )
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(mcp_request(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"aida_create_person"));
    assert!(names.contains(&"aida_show_person"));
    assert!(names.contains(&"aida_list_jurisdictions"));
}
