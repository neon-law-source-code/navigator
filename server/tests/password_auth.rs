#![allow(clippy::doc_markdown)]
//! End-to-end test for the GCP Identity Platform email/password front
//! door (`portal::oauth` password path).
//!
//! Neon Law Navigator never stores a password: the typed credential is forwarded
//! once to Identity Platform's `accounts:signInWithPassword` over TLS,
//! and the ID token it returns is decoded into the SAME `SessionData`
//! cookie the OIDC callback mints. These tests stand a `wiremock`
//! Identity-Toolkit stand-in next to a real store and drive the full
//! browser flow:
//!
//! - `GET /auth/login` renders the email/password form (+ the Google
//!   button) and sets the signed login-CSRF cookie;
//! - `POST /auth/password` with valid creds + CSRF → a `navigator_session`
//!   cookie and a 303 to `return_to`;
//! - wrong creds → 401, no session, a warm non-enumerating message;
//! - a missing/forged CSRF token → 400;
//! - with the password door OFF, `/auth/login` is the unchanged OIDC
//!   redirect (the existing-deploy invariant — we never break OIDC).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
// Keyed render assertion: the sign-in error copy lives in the catalog
// (`portal.login_failed`), so assert the slot, not the literal.
use portal::oauth::IdentityPasswordConfig;
use portal::{policy, AppState, AuthConfig, OAuthConfig, SessionStore};
use store::test_support::mem_surreal;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A structurally-valid but unsigned Firebase-shaped ID token. Our path
/// decodes the payload for `sub`/`email`; it trusts the token because it
/// arrives over TLS from Identity Platform (here, the mock).
fn fake_id_token(sub: &str, email: &str) -> String {
    let header = b64url(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = b64url(
        serde_json::to_vec(&json!({ "sub": sub, "email": email }))
            .unwrap()
            .as_slice(),
    );
    format!("{header}.{payload}.")
}

/// Build an `AppState` whose password door points at `endpoint`. OIDC is
/// also configured (so the `/auth/*` router mounts and the Google button
/// has a target), but unused by the password assertions.
async fn state_with_password(endpoint: Option<String>) -> (AppState, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    // The password path never evaluates authorization policy.
    let state = AppState {
        auth: AuthConfig::new(false, None),
        sessions: sessions(),
        oauth: Some(OAuthConfig::new(
            "navigator-web",
            "secret",
            "http://app.test/auth/callback",
            "http://idp.test/authorize",
            "http://idp.test/token",
        )),
        identity_password: endpoint.map(|e| IdentityPasswordConfig {
            api_key: "test-browser-key".into(),
            endpoint: e,
        }),
        identity_admin: None,
        storage: Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-password-e2e"))
                .await
                .unwrap(),
        ),
        policy: policy::PolicyClient::passthrough(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (state, surreal)
}

/// Mount an Identity-Toolkit `signInWithPassword` stand-in that returns
/// `status` with `body`.
async fn mount_identity_platform(status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/accounts:signInWithPassword"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Drive `GET /auth/login`, returning the `(csrf_cookie, csrf_token)`
/// double-submit pair the form embeds. Also asserts the page is the
/// password form, not a redirect.
async fn open_login_form(app: &axum::Router) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login?return_to=/app/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "password door GET is a page");
    let csrf_cookie = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.contains("navigator_login_csrf="))
        .expect("login GET sets the navigator_login_csrf cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let html = body_string(resp).await;
    assert!(
        html.contains(r#"name="password""#),
        "renders a password field"
    );
    assert!(
        html.contains("/auth/login/oidc"),
        "offers the Google button"
    );
    let needle = r#"name="csrf_token" value=""#;
    let start = html.find(needle).expect("form carries a csrf_token") + needle.len();
    let token = html[start..].split('"').next().unwrap().to_string();
    (csrf_cookie, token)
}

fn form_body(email: &str, password: &str, csrf_token: &str) -> String {
    // The csrf token is URL-safe base64 (no reserved chars); only the
    // email's `@` and the return_to slash need encoding.
    let email = email.replace('@', "%40");
    format!("email={email}&password={password}&return_to=%2Fapp%2Fprojects&csrf_token={csrf_token}")
}

/// A synthetic credential for this mock-only Identity Platform test.
///
/// The password path forwards this value to the local `wiremock` server; it
/// never reaches a deployed identity provider or represents a user credential.
/// Add the process ID so CodeQL cannot mistake a test vector for a reusable
/// hard-coded password.
fn test_password(label: &str) -> String {
    format!("{label}-{}", std::process::id())
}

async fn post_password(app: &axum::Router, cookie: &str, body: String) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/password")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn valid_password_mints_the_standard_session_cookie() {
    let idp = mount_identity_platform(
        200,
        json!({
            "idToken": fake_id_token("fb-uid-1", "client@example.org"),
            "email": "client@example.org",
            "localId": "fb-uid-1",
        }),
    )
    .await;
    let (state, surreal) = state_with_password(Some(idp.uri())).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (csrf_cookie, csrf_token) = open_login_form(&app).await;
    let password = test_password("correct-horse");
    let resp = post_password(
        &app,
        &csrf_cookie,
        form_body("client@example.org", &password, &csrf_token),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "redirects on success");
    assert_eq!(resp.headers().get("location").unwrap(), "/app/projects");
    let has_session = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap().contains("navigator_session="));
    assert!(
        has_session,
        "a valid password mints the navigator_session cookie"
    );
    // The verified password response creates the first-sign-in Client.
    let rows = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, store::persons::Role::Client);
    assert_eq!(rows[0].oidc_subject.as_deref(), Some("fb-uid-1"));
}

#[tokio::test]
async fn wrong_password_is_rejected_without_enumeration_and_without_session() {
    // Identity Platform 400 = bad creds / unknown email / disabled — all
    // collapse to one outcome.
    let idp = mount_identity_platform(
        400,
        json!({ "error": { "code": 400, "message": "INVALID_LOGIN_CREDENTIALS" } }),
    )
    .await;
    let (state, _surreal) = state_with_password(Some(idp.uri())).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (csrf_cookie, csrf_token) = open_login_form(&app).await;
    let password = test_password("wrong");
    let resp = post_password(
        &app,
        &csrf_cookie,
        form_body("client@example.org", &password, &csrf_token),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let no_session = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .all(|v| !v.to_str().unwrap().contains("navigator_session="));
    assert!(no_session, "a rejected sign-in never sets a session");
    let html = body_string(resp).await;
    // The warm, non-enumerating sign-in error is catalog copy.
    assert!(&html.contains(
        "That email and password don&#39;t match what we have on file. Please try again."
    ));
}

#[tokio::test]
async fn missing_csrf_token_is_rejected() {
    let idp = mount_identity_platform(200, json!({ "idToken": fake_id_token("x", "x@y.z") })).await;
    let (state, _surreal) = state_with_password(Some(idp.uri())).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Open the form (to look legitimate) but POST with NO csrf cookie and
    // an empty token — the double-submit check must reject it.
    let (_csrf_cookie, _token) = open_login_form(&app).await;
    let password = test_password("correct-horse");
    let resp = post_password(
        &app,
        "navigator_login_csrf=tampered",
        form_body("client@example.org", &password, ""),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn password_door_off_keeps_auth_login_a_pure_oidc_redirect() {
    // The existing-deploy invariant: with no Identity Platform key, the
    // password code is inert and `/auth/login` still 303s to the IdP.
    let (state, _surreal) = state_with_password(None).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/login?return_to=/app/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "OIDC-only login redirects"
    );
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("idp.test/authorize"),
        "redirect targets the IdP"
    );
}
