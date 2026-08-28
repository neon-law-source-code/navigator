#![allow(clippy::doc_markdown)]
//! Integration tests for the credential-keyed CSRF middleware.
//!
//! The pattern: encode a valid session cookie ourselves (no IdP
//! round-trip needed), then exercise the middleware paths — happy
//! POST with matching `_csrf`, missing `_csrf`, mismatched `_csrf`,
//! and missing session (passthrough). The form HTML itself
//! includes the hidden input when the session is attached.
//!
//! Beyond the classic form path, the middleware guards the mutating
//! `/app/api/*` routes on the same credential rule: a cookie-authenticated
//! JSON write must echo the session token in the `X-CSRF-Token` header,
//! a bearer-authenticated write stays exempt, and a cross-site
//! Origin/Referer on a cookie write is rejected as defense-in-depth.
//! Those cases live at the bottom of this file.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::{session::SESSION_COOKIE_NAME, AppState, SessionData, SessionStore};
use std::sync::OnceLock;
use store::test_support::mem_surreal;
use tower::ServiceExt;

fn sessions() -> SessionStore {
    SessionStore::new("csrf-test-session-key")
}

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn state(s: SessionStore) -> AppState {
    let surreal = mem_surreal().await;
    AppState {
        sessions: s,
        ..portal::test_support::app_state(surreal.clone()).await
    }
}

async fn body(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Build a `Cookie:` header value carrying a freshly-encoded
/// session, and return the encoded session's CSRF token so the
/// caller can include it in form bodies.
fn fresh_session_cookie(s: &SessionStore) -> (String, String) {
    let session = SessionData::fresh("nick@neonlaw.com", store::persons::Role::Admin);
    let token = session.csrf_token.clone();
    let cookie_value = s.encode(&session);
    (format!("{SESSION_COOKIE_NAME}={cookie_value}"), token)
}

/// A form-encoded `/app/admin/entities` create body that redirects on
/// success — the canonical classic-form (`CsrfMode::Form`) vector now
/// that People mutations moved to the JSON `/app/api/*` surface. Seeds the
/// entity-type FK the insert needs, plus a real jurisdiction row in the
/// engine that holds that table (ENG-20) — the create reads it back
/// before writing, so a synthetic id would exercise the refusal path
/// instead of the successful create this body exists for. `token`
/// embeds the `_csrf` field, or `None` omits it (the passthrough
/// cases).
async fn entity_form_body(surreal: &store::surreal::SurrealDb, token: Option<&str>) -> String {
    let seeded = store::test_support::seed_entity(surreal).await;
    let entity = store::entities::find_by_id(surreal, seeded)
        .await
        .unwrap()
        .unwrap();
    let jurisdiction = store::jurisdictions::find_or_create(
        surreal,
        &store::jurisdictions::NewJurisdiction::new("Test State", "TS", "state"),
    )
    .await
    .unwrap();
    let base = format!(
        "name=Csrf%20Co&entity_type_id={}&jurisdiction_id={}",
        entity.entity_type_id, jurisdiction.id
    );
    match token {
        Some(t) => format!("_csrf={t}&{base}"),
        None => base,
    }
}

#[tokio::test]
async fn admin_form_renders_csrf_hidden_input_when_session_present() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people/new")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("name=\"_csrf\""));
    assert!(html.contains(&format!("value=\"{token}\"")));
    assert!(html.contains("type=\"hidden\""));
}

#[tokio::test]
async fn admin_post_with_session_and_matching_csrf_redirects() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let st = state(store.clone()).await;
    let (cookie, token) = fresh_session_cookie(&store);
    let form = entity_form_body(&st.surreal, Some(&token)).await;
    let app = server::neon_router(st, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie)
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        resp.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
}

#[tokio::test]
async fn admin_post_with_session_and_missing_csrf_returns_403() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, _token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie)
                .body(Body::from("name=Csrf%20Co"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_post_with_session_and_wrong_csrf_returns_403() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, _token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", cookie)
                .body(Body::from("_csrf=NOT_THE_REAL_TOKEN&name=Csrf%20Co"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_post_without_session_passes_through_csrf_layer() {
    let _lock = test_lock().lock().await;
    // The previous suite already proves this works — we re-assert
    // here so a future regression of the "no session = passthrough"
    // behavior fails this file too.
    let store = sessions();
    let st = state(store).await;
    let form = entity_form_body(&st.surreal, None).await;
    let app = server::neon_router(st, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        resp.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
}

#[tokio::test]
async fn admin_post_with_tampered_session_cookie_passes_through() {
    let _lock = test_lock().lock().await;
    // A tampered/expired session cookie fails to decode → middleware
    // treats request as anonymous → CSRF layer no-ops → handler
    // succeeds in the dev/test path (no auth enforced).
    let store = sessions();
    let st = state(store).await;
    let form = entity_form_body(&st.surreal, None).await;
    let app = server::neon_router(st, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header("content-type", "application/x-www-form-urlencoded")
                .header(
                    "cookie",
                    format!("{SESSION_COOKIE_NAME}=this-is-not-a-valid-signed-cookie"),
                )
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        resp.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
}

// --- Credential-keyed CSRF on the mutating `/app/api/*` JSON routes ---
//
// `POST /app/api/people` resolves a session from the cookie layer *or* a
// bearer credential. These prove the CSRF middleware now keys on the
// credential, not the content type, so a cookie-authenticated JSON write
// can't skip the check the way a content-type gate let it.

/// A `Bearer` header carrying the same signed `SessionData` blob the CLI
/// presents — a lawyer-tier session so the `/app/api/people` handler's role
/// check passes once the request is (correctly) CSRF-exempt.
fn bearer_header(s: &SessionStore) -> String {
    let session = SessionData::fresh("cli@neonlaw.com", store::persons::Role::Admin);
    format!("Bearer {}", s.encode(&session))
}

const CREATE_PERSON_JSON: &str = r#"{"name":"Libra Example","email":"libra-api-csrf@example.com"}"#;

#[tokio::test]
async fn api_json_write_with_cookie_and_valid_csrf_header_succeeds() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .header("x-csrf-token", token)
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn api_patch_write_with_cookie_and_missing_csrf_is_403() {
    let _lock = test_lock().lock().await;
    // The re-key guards every mutating `/app/api/*` verb, not just POST: a
    // cookie-authenticated PATCH with no `X-CSRF-Token` is rejected
    // before it reaches the handler (so no person row is needed).
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, _token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", uuid::Uuid::from_u128(1)))
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(
                    r#"{"name":"Libra","email":"libra@example.com"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_json_write_with_cookie_and_missing_csrf_is_403() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, _token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_json_write_with_cookie_and_wrong_csrf_header_is_403() {
    let _lock = test_lock().lock().await;
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, _token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .header("x-csrf-token", "not-the-real-token")
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_json_write_with_bearer_and_no_csrf_stays_exempt() {
    let _lock = test_lock().lock().await;
    // Bearer is not browser-CSRF-vulnerable (no auto-attached cookie),
    // so it must succeed with no token — the CLI / MCP path.
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header("authorization", bearer_header(&store))
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn api_write_with_bearer_and_cookie_runs_as_the_bearer_principal() {
    let _lock = test_lock().lock().await;
    // Regression for the stale-principal bug: when a request carries BOTH
    // a bearer credential and a session cookie, `require_policy` (which
    // runs before `require_csrf`) authorizes the bearer principal that
    // `inject_bearer_session` resolved first — so the handler must run as
    // that SAME principal. `require_csrf` must not overwrite it with the
    // cookie's session. Here the bearer is a non-lawyer client and the
    // cookie is an admin; the create must be Forbidden (client tier),
    // proving the handler saw the bearer principal, not the cookie's admin.
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, token) = fresh_session_cookie(&store); // admin / lawyer tier
    let client_bearer = {
        let session = SessionData::fresh("client@example.com", store::persons::Role::Client);
        format!("Bearer {}", store.encode(&session))
    };

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header("authorization", client_bearer)
                .header("cookie", cookie)
                .header("x-csrf-token", token)
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_json_write_with_cross_site_origin_is_403_even_with_valid_token() {
    let _lock = test_lock().lock().await;
    // Defense-in-depth: a cross-site Origin on a cookie-authenticated
    // state change is rejected outright, independent of the token — so
    // even a leaked/guessed token can't be replayed from another origin.
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .header("x-csrf-token", token)
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_json_write_with_same_origin_and_valid_token_succeeds() {
    let _lock = test_lock().lock().await;
    // The origin check must not false-positive on a legitimate
    // same-origin browser write.
    let store = sessions();
    let app = server::neon_router(
        state(store.clone()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let (cookie, token) = fresh_session_cookie(&store);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("host", "app.example")
                .header("origin", "https://app.example")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .header("x-csrf-token", token)
                .body(Body::from(CREATE_PERSON_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}
