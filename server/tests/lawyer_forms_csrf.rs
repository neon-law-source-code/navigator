#![allow(clippy::doc_markdown)]
//! Regression guard: every lawyer form page renders the session CSRF token.
//!
//! `portal::csrf::require_csrf` runs in `CsrfMode::Form` on every mutating
//! `/app/lawyer/*` route, so a cookie-authenticated form POST that carries no
//! valid `_csrf` is a 403. That makes a form which renders *without* the
//! hidden `_csrf` input a latent bug: a real lawyer user submits it in a
//! browser and gets a 403, even though the page looked fine. That is
//! exactly what happened to the entity form once the middleware landed.
//!
//! This test boots the router with an admin session and requests each
//! lawyer form page, asserting the rendered HTML carries a `_csrf` input
//! echoing the session token. A new lawyer form that forgets to thread the
//! token fails here instead of in production.
//!
//! Coverage note: the pages below are the form pages reachable with only
//! the canonical seed and an admin session. Form pages that need a richer
//! fixture (a parked notation, an in-flight contract review) assert their
//! own `_csrf` in their focused suites — e.g. the entity forms in `routes`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SESSION_COOKIE_NAME;
use portal::test_support::TEST_SESSION_KEY;
use portal::{AppState, SessionData, SessionStore};
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// An admin session cookie (encoded with the same key `app_state` uses)
/// and the CSRF token embedded in it.
fn admin_cookie_and_csrf() -> (String, String) {
    let sessions = SessionStore::new(TEST_SESSION_KEY);
    let session = SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    let csrf = session.csrf_token.clone();
    (
        format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&session)),
        csrf,
    )
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn lawyer_form_pages_render_the_session_csrf_token() {
    let surreal = mem_surreal().await;
    let state: AppState = portal::test_support::app_state(surreal.clone()).await;
    let storage: Arc<dyn cloud::StorageService> = state.storage.clone();
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_cookie_and_csrf();

    // Lawyer form pages reachable with the canonical seed. Each must render
    // the hidden `_csrf` field carrying the session token.
    let form_pages = [
        "/app/admin/entities/new",
        // The people create form is the admin console's since ENG-304; the
        // session above is an admin one, so it renders here.
        "/app/admin/people/new",
        "/app/projects/new",
        "/app/admin/playbooks/new",
    ];

    for path in form_pages {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET {path} should render, got {}",
            resp.status()
        );
        let html = body_string(resp).await;
        assert!(
            html.contains("name=\"_csrf\""),
            "{path} renders no hidden _csrf input — a real submit would 403"
        );
        assert!(
            html.contains(&format!("value=\"{csrf}\"")),
            "{path} renders a _csrf input but not the session token"
        );
    }
}
