#![allow(clippy::doc_markdown)]
//! End-to-end tests for the browser-flow OAuth2 routes.
//!
//! These drive the real `server::neon_router()` via
//! `tower::ServiceExt::oneshot` and substitute a `wiremock`-backed
//! IdP for the upstream provider. No socket on the app side, no
//! browser anywhere.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::{oauth, AppState, OAuthConfig, SessionStore};
use serde_json::json;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

async fn state_with_oauth(oauth_cfg: OAuthConfig, sessions_store: SessionStore) -> AppState {
    let surreal = mem_surreal().await;
    AppState {
        sessions: sessions_store,
        oauth: Some(oauth_cfg),
        ..portal::test_support::app_state(surreal.clone()).await
    }
}

/// Extract the value of `param=` from an authorize-redirect `Location`.
fn query_param(location: &str, param: &str) -> String {
    let needle = format!("{param}=");
    location
        .split('&')
        .find_map(|p| p.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("`{param}` missing from {location}"))
        .to_string()
}

async fn seed_person(surreal: &store::surreal::SurrealDb, email: &str, role: store::persons::Role) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email.to_string(), email.to_string(), role),
    )
    .await
    .expect("seed person");
}

#[tokio::test]
async fn login_sets_pre_auth_cookie_and_redirects_to_idp() {
    let mock = MockServer::start().await;
    let cfg = OAuthConfig::new(
        "client-id",
        "client-secret",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let state = state_with_oauth(cfg, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with(&format!("{}/authorize?", mock.uri())),
        "got: {location}"
    );
    assert!(location.contains("response_type=code"));
    assert!(location.contains("client_id=client-id"));
    assert!(location.contains("code_challenge="));
    assert!(location.contains("code_challenge_method=S256"));
    assert!(location.contains("state="));
    // Cookie set.
    let set_cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
    assert!(set_cookie.contains(oauth::PRE_AUTH_COOKIE_NAME));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    // http://app.test deployment → cookies must NOT be Secure, or the
    // KIND loop over plain HTTP could never read them back.
    assert!(!set_cookie.contains("Secure"), "got: {set_cookie}");
}

#[tokio::test]
async fn auth_endpoints_throttle_a_single_ip() {
    // With the limiter enabled at 2/min, the third /auth/login from one
    // IP in the window is shed with 429 before any auth work runs.
    let mock = MockServer::start().await;
    let cfg = OAuthConfig::new(
        "c",
        "s",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: sessions(),
        oauth: Some(cfg),
        rate_limit: portal::rate_limit::RateLimit::new(2, std::time::Duration::from_mins(1)),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let hit = |app: axum::Router| async move {
        app.oneshot(
            Request::builder()
                .uri("/auth/login")
                .header("x-forwarded-for", "9.9.9.9")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    };

    assert_eq!(hit(app.clone()).await, StatusCode::SEE_OTHER);
    assert_eq!(hit(app.clone()).await, StatusCode::SEE_OTHER);
    assert_eq!(
        hit(app.clone()).await,
        StatusCode::TOO_MANY_REQUESTS,
        "third request from the same IP must be throttled"
    );
}

#[tokio::test]
async fn https_deployment_marks_auth_cookies_secure() {
    // The Secure flag is derived from the OAuth redirect URI scheme, so
    // an https:// deployment must mark the pre-auth (and session) cookies
    // Secure. The http://app.test config in the other tests must not.
    let mock = MockServer::start().await;
    let cfg = OAuthConfig::new(
        "client-id",
        "client-secret",
        "https://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let state = state_with_oauth(cfg, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let set_cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
    assert!(set_cookie.contains(oauth::PRE_AUTH_COOKIE_NAME));
    assert!(
        set_cookie.contains("Secure"),
        "https deployment must set Secure; got: {set_cookie}"
    );
}

#[tokio::test]
async fn callback_rejects_request_without_pre_auth_cookie() {
    let mock = MockServer::start().await;
    let cfg = OAuthConfig::new(
        "c",
        "s",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let state = state_with_oauth(cfg, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_round_trip_sets_session_cookie_and_redirects_to_return_to() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();

    // Verification is real: pin a test verifier to client_id "c" so the
    // callback runs full signature + iss/aud/nonce checks on the token.
    let cfg = portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "c",
            "s",
            "http://app.test/auth/callback",
            format!("{}/authorize", mock.uri()),
            format!("{}/token", mock.uri()),
        ),
        "c",
    );
    let state = state_with_oauth(cfg.clone(), sessions_store.clone()).await;
    // The IdP-supplied email must already exist in the persons table
    // for sign-in to succeed — sign-up is operator-mediated.
    seed_person(
        &state.surreal,
        "nick@neonlaw.com",
        store::persons::Role::Admin,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Step 1 — /auth/login?return_to=/app/admin/entities sets the
    // pre-auth cookie + redirects to the IdP. We extract the state,
    // nonce, and cookie value to replay them in step 2.
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login?return_to=/app/admin/entities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let location = login
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state_param = query_param(&location, "state");
    let nonce = query_param(&location, "nonce");
    let set_cookie = login.headers().get("set-cookie").unwrap().to_str().unwrap();
    let cookie_value = set_cookie.split(';').next().unwrap().to_string(); // "navigator_pre_auth=...."

    // Now that we hold the login's nonce, program the token endpoint to
    // return a properly-signed id_token carrying it. Body is form-encoded
    // so we string-match grant_type rather than json-match.
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": portal::test_support::sign_id_token(
                "c",
                &nonce,
                "rauthy-nick-subject",
                "nick@neonlaw.com",
                "Nick",
            ),
            "token_type": "Bearer",
        })))
        .mount(&mock)
        .await;

    // Step 2 — /auth/callback?code=...&state=... with the cookie set.
    let cb = app
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", cookie_value)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cb.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        cb.headers().get("location").unwrap().to_str().unwrap(),
        "/app/admin/entities"
    );

    // Two Set-Cookie headers: pre-auth expiring + session being set.
    let cookies: Vec<&str> = cb
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert!(cookies.iter().any(|c| c.contains("navigator_session=")));
    assert!(cookies
        .iter()
        .any(|c| c.contains("navigator_pre_auth=") && c.contains("Max-Age=0")));
}

#[tokio::test]
async fn callback_rejects_a_token_whose_nonce_does_not_match_the_login() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();
    let cfg = portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "c",
            "s",
            "http://app.test/auth/callback",
            format!("{}/authorize", mock.uri()),
            format!("{}/token", mock.uri()),
        ),
        "c",
    );
    let state = state_with_oauth(cfg, sessions_store).await;
    seed_person(
        &state.surreal,
        "nick@neonlaw.com",
        store::persons::Role::Admin,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = login
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state_param = query_param(&location, "state");
    let cookie_value = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Sign a token carrying a nonce the login never issued — an injected
    // / replayed id_token. The callback must refuse it (401).
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": portal::test_support::sign_id_token(
                "c",
                "attacker-chosen-nonce",
                "rauthy-nick-subject",
                "nick@neonlaw.com",
                "Nick",
            ),
            "token_type": "Bearer",
        })))
        .mount(&mock)
        .await;

    let cb = app
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", cookie_value)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cb.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn callback_rejects_state_mismatch_even_with_valid_cookie() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();
    let cfg = OAuthConfig::new(
        "c",
        "s",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let state = state_with_oauth(cfg, sessions_store).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/callback?code=any&state=WRONG_STATE_VALUE")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("state mismatch"));
}

#[tokio::test]
async fn logout_clears_session_cookie_and_redirects_home() {
    let mock = MockServer::start().await;
    let cfg = OAuthConfig::new(
        "c",
        "s",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    );
    let state = state_with_oauth(cfg, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        "/"
    );
    let cookies: Vec<&str> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    // Both cookies expired (Max-Age=0).
    assert!(cookies
        .iter()
        .any(|c| c.contains("navigator_session=") && c.contains("Max-Age=0")));
    assert!(cookies
        .iter()
        .any(|c| c.contains("navigator_pre_auth=") && c.contains("Max-Age=0")));
}

#[tokio::test]
async fn logout_redirects_to_provider_end_session_endpoint_and_clears_session() {
    let mock = MockServer::start().await;
    // A provider that publishes an `end_session_endpoint` (Rauthy does): the
    // logout must bounce through it so the provider drops its own SSO session,
    // otherwise the next `/auth/login` silently re-authenticates.
    let cfg = OAuthConfig::new(
        "c",
        "s",
        "http://app.test/auth/callback",
        format!("{}/authorize", mock.uri()),
        format!("{}/token", mock.uri()),
    )
    .with_end_session_endpoint(format!("{}/auth/v1/oidc/logout", mock.uri()));
    let state = state_with_oauth(cfg, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with(&format!("{}/auth/v1/oidc/logout?", mock.uri())),
        "expected a redirect to the provider end-session endpoint; got: {location}"
    );
    // Carries the app's own origin as the post-logout redirect (derived from
    // the OAuth redirect URI, so it matches the provider's allowlist), plus
    // `client_id` so the provider can validate it without an id_token_hint.
    assert!(
        location.contains("post_logout_redirect_uri=http%3A%2F%2Fapp.test"),
        "expected post_logout_redirect_uri back to the app origin; got: {location}"
    );
    assert!(
        location.contains("client_id=c"),
        "expected client_id on the end-session request; got: {location}"
    );
    // The app session is still cleared regardless of the provider bounce.
    let cookies: Vec<&str> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert!(cookies
        .iter()
        .any(|c| c.contains("navigator_session=") && c.contains("Max-Age=0")));
    assert!(cookies
        .iter()
        .any(|c| c.contains("navigator_pre_auth=") && c.contains("Max-Age=0")));
}
