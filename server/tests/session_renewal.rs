//! End-to-end tests for sliding session renewal.
//!
//! Drives the real `server::neon_router()` via `tower::ServiceExt::oneshot`.
//! A browser session that is past the half-way point of its TTL must be
//! re-issued (fresh `Set-Cookie`) on any request; a still-fresh one must
//! not be, so we don't emit a `Set-Cookie` on every single request.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::{now_unix_secs, SessionData, SessionSource, DEFAULT_SESSION_TTL_SECS};
use portal::{AppState, SessionStore};
use store::test_support::mem_surreal;
use tower::ServiceExt;

const KEY: &str = "test-session-key-not-for-production-0000";

async fn app_with_sessions(store: SessionStore) -> axum::Router {
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: store,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR))
}

fn browser_session(exp: i64) -> SessionData {
    SessionData {
        sub: "sub-renewal".into(),
        email: None,
        person_id: None,
        exp,
        role: store::persons::Role::Client,
        csrf_token: "csrf-token".into(),
        source: SessionSource::Browser,
        provider: None,
        impersonation: None,
        scope: None,
    }
}

fn session_set_cookie(resp: &axum::response::Response) -> Option<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.contains("navigator_session="))
        .map(ToString::to_string)
}

#[tokio::test]
async fn aged_session_is_renewed_with_a_fresh_persistent_cookie() {
    let store = SessionStore::new(KEY);
    // Three-quarters elapsed → in the second half → must renew.
    let cookie = store.encode(&browser_session(
        now_unix_secs() + DEFAULT_SESSION_TTL_SECS / 4,
    ));
    let app = app_with_sessions(store).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("cookie", format!("navigator_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = session_set_cookie(&resp).expect("aged session must be re-issued");
    // Persistent: carries a Max-Age (survives a browser restart).
    assert!(
        set_cookie.contains("Max-Age="),
        "renewed cookie must be persistent: {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly"));
}

#[tokio::test]
async fn fresh_session_is_not_re_issued() {
    let store = SessionStore::new(KEY);
    // Just minted → first half → no Set-Cookie churn.
    let cookie = store.encode(&browser_session(now_unix_secs() + DEFAULT_SESSION_TTL_SECS));
    let app = app_with_sessions(store).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("cookie", format!("navigator_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        session_set_cookie(&resp).is_none(),
        "a fresh session must not be re-issued on every request"
    );
}
