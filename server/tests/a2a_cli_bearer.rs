#![allow(clippy::doc_markdown)]
//! The `navigator` CLI's own bearer must reach `/app/api/mcp/rpc`.
//!
//! `navigator site mcp` bridges Claude to A2A using the credential
//! `site login` stores — the HMAC-signed `SessionData` blob `cli_auth`
//! mints. Before the first-party lane existed that credential could not
//! reach this endpoint at all: `require_auth` found no JWT to validate,
//! the policy layer evaluated an anonymous session against a rule
//! requiring `is_lawyer`, and the request was answered with a `303` to a
//! login page a JSON-RPC client cannot follow.
//!
//! Three layers had to agree, so this drives the composed router rather
//! than any one of them: `inject_bearer_session` resolves the blob,
//! `require_google_oauth` lets an already-resolved first-party session
//! by, and `inject_principal` reads the principal off it. A regression in
//! any one of the three puts the CLI back outside the door, so the test
//! asserts the end state — a dispatched tool call — instead of the
//! individual hand-offs.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::{AppState, SessionData, SessionStore};
use serde_json::{json, Value};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;

const KEY: &[u8] = b"a2a-cli-bearer-test-key-0123456789";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

/// Mint the credential `site login` writes to `~/.navigator.json`: a
/// signed `SessionData` carrying the caller's role and — the part the
/// principal is read from — their email.
async fn cli_bearer(surreal: &store::surreal::SurrealDb, email: &str, role: Role) -> String {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    let mut session = SessionData::fresh(email, role);
    session.email = Some(email.to_string());
    session.source = portal::SessionSource::Cli;
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn dispatch(
    app: &axum::Router,
    bearer: Option<&str>,
    skill: &str,
    arguments: Value,
) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "message/send",
        "params": {
            "message": {
                "messageId": "m-cli",
                "role": "user",
                "kind": "message",
                "parts": [],
                "metadata": { "skill": skill, "arguments": arguments }
            }
        }
    });
    let mut builder = Request::builder()
        .method("POST")
        .uri("/app/api/mcp/rpc")
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        builder = builder.header("authorization", b);
    }
    let resp = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a resolved CLI bearer must not be redirected or refused at the transport"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn a_cli_bearer_reaches_a2a_and_runs_a_read() {
    let (app, surreal) = build_app().await;
    let bearer = cli_bearer(&surreal, "lawyer@neonlaw.com", Role::Lawyer).await;

    let body = dispatch(&app, Some(&bearer), "list_jurisdictions", json!({})).await;
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    assert_eq!(
        body["result"]["status"]["state"], "completed",
        "the read should run: {body}"
    );
}

#[tokio::test]
async fn a_cli_bearer_supplies_the_principal_a_write_requires() {
    // The sharper claim. A side-effecting skill is refused outright
    // without a lawyer *principal*, so this passing proves the session's
    // email became one — not merely that the request got through the
    // transport.
    let (app, surreal) = build_app().await;
    let bearer = cli_bearer(&surreal, "lawyer@neonlaw.com", Role::Lawyer).await;

    let body = dispatch(
        &app,
        Some(&bearer),
        "create_person",
        json!({ "name": "Bridge Person", "email": "bridge-person@example.com" }),
    )
    .await;
    assert_eq!(
        body["result"]["status"]["state"], "completed",
        "a lawyer's CLI bearer should carry a principal: {body}"
    );

    let landed = store::persons::find_by_email_ci(&surreal, "bridge-person@example.com")
        .await
        .unwrap();
    assert!(landed.is_some(), "the row should have been written");
}

#[tokio::test]
async fn a_client_tier_cli_bearer_cannot_run_a_write() {
    // The lane resolves an identity; it does not confer authorization.
    //
    // Two independent things refuse a client-tier caller here, and this
    // exercises the inner one. The endpoint's Rego lawyer-gate is the
    // outer one — it answers `303` on a real deployment, verified by
    // curl, and its five-case matrix lives in `navigator_test.rego`. It
    // cannot be exercised through this fixture, because
    // `test_support::app_state` disables auth and policy enforcement so
    // handler tests can address handlers.
    //
    // What that leaves is worth pinning on its own: even with the gate
    // switched off, `dispatch_single`'s own lawyer-tier check refuses the
    // write. A misconfigured policy would not be enough to let a
    // client-tier credential change data.
    let (app, surreal) = build_app().await;
    let bearer = cli_bearer(&surreal, "client@neonlaw.com", Role::Client).await;

    let body = dispatch(
        &app,
        Some(&bearer),
        "create_person",
        json!({ "name": "Nope", "email": "nope@example.com" }),
    )
    .await;

    assert_eq!(
        body["result"]["status"]["state"], "failed",
        "a client-tier principal must not run a write: {body}"
    );
    let text = body["result"]["status"]["message"]["parts"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("lawyer"),
        "the refusal should name the tier it wanted, got: {text}"
    );
    assert!(
        store::persons::find_by_email_ci(&surreal, "nope@example.com")
            .await
            .unwrap()
            .is_none(),
        "nothing may be written for a refused caller"
    );
}

#[tokio::test]
async fn an_anonymous_caller_cannot_run_a_write() {
    // Same shape, no credential at all: with no bearer there is no
    // session, so no principal, and the tier check has nothing to admit.
    // On a real deployment this never reaches the handler — policy
    // answers `303` — but the write must be refused even if it does.
    let (app, surreal) = build_app().await;

    let body = dispatch(
        &app,
        None,
        "create_person",
        json!({ "name": "Anon", "email": "anon@example.com" }),
    )
    .await;

    assert_eq!(
        body["result"]["status"]["state"], "failed",
        "an anonymous caller must not run a write: {body}"
    );
    assert!(
        store::persons::find_by_email_ci(&surreal, "anon@example.com")
            .await
            .unwrap()
            .is_none(),
        "nothing may be written for an anonymous caller"
    );
}
