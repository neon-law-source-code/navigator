#![allow(clippy::doc_markdown)]
//! End-to-end tests for the **second** browser sign-in provider, Microsoft
//! Entra ID, running alongside the primary OIDC slot rather than replacing it.
//!
//! These drive the real `server::neon_router()` through
//! `tower::ServiceExt::oneshot`, with `wiremock` standing in for both IdPs and
//! a locally-held RSA key signing the id_tokens. No socket on the app side, no
//! browser, and no network call to Microsoft.
//!
//! Three things are worth testing here and nothing else is:
//!
//! 1. **The templated issuer.** Multi-tenant Entra publishes the literal
//!    `https://login.microsoftonline.com/{tenantid}/v2.0` as its `issuer`, and
//!    the token carries the signing tenant's GUID in `tid`. Pinning the
//!    template verbatim — what `Validation::set_issuer` does for every other
//!    provider — rejects every real token, so the per-tenant path has to be
//!    exercised for real.
//! 2. **The tenant allowlist.** It is the control that makes multi-tenant
//!    sign-in safe, so a token from an unlisted tenant must be refused even
//!    when its signature, audience, nonce and issuer are all internally
//!    consistent.
//! 3. **Provider disambiguation.** Both providers share one redirect URI, so
//!    the callback picks the provider out of the signed pre-auth cookie. A
//!    Microsoft login must not be redeemable at the primary provider's token
//!    endpoint, and vice versa.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use portal::{oauth, AppState, OAuthConfig, SessionStore};
use serde_json::json;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The tenant an operator has allowlisted — stand-in for the client firm's own
/// Entra directory.
const CLIENT_TENANT: &str = "11111111-2222-3333-4444-555555555555";
/// A tenant nobody allowlisted — stand-in for any of the millions of Entra
/// directories a stranger can create for free.
const STRANGER_TENANT: &str = "99999999-8888-7777-6666-555555555555";

fn sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

/// A deployment with both doors open: the primary OIDC slot and Microsoft.
/// Both carry a real verifier, so every callback below runs full signature,
/// audience, issuer and nonce checks.
async fn state_with_both_providers(mock: &MockServer, sessions_store: SessionStore) -> AppState {
    let primary = portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "primary-client",
            "primary-secret",
            "http://app.test/auth/callback",
            format!("{}/authorize", mock.uri()),
            format!("{}/token", mock.uri()),
        ),
        "primary-client",
    );
    let microsoft = portal::test_support::microsoft_oauth_config(
        OAuthConfig::new(
            "ms-client",
            "ms-secret",
            "http://app.test/auth/callback",
            format!("{}/ms/authorize", mock.uri()),
            format!("{}/ms/token", mock.uri()),
        ),
        "ms-client",
        &[CLIENT_TENANT],
    );
    let surreal = mem_surreal().await;
    AppState {
        sessions: sessions_store,
        oauth: Some(primary),
        oauth_microsoft: Some(microsoft),
        ..portal::test_support::app_state(surreal.clone()).await
    }
}

async fn seed_person(surreal: &store::surreal::SurrealDb, email: &str, role: store::persons::Role) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email.to_string(), email.to_string(), role),
    )
    .await
    .expect("seed person");
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

/// Start a Microsoft login and hand back `(state, nonce, pre_auth_cookie)`.
async fn begin_microsoft_login(app: &axum::Router, return_to: &str) -> (String, String, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/login/microsoft?return_to={return_to}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "expected an IdP redirect"
    );
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let set_cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
    (
        query_param(&location, "state"),
        query_param(&location, "nonce"),
        set_cookie.split(';').next().unwrap().to_string(),
    )
}

/// Program the Microsoft token endpoint to return `id_token`, then run the
/// shared `/auth/callback` with the pre-auth cookie from the login.
async fn finish_microsoft_callback(
    app: &axum::Router,
    mock: &MockServer,
    state_param: &str,
    cookie: &str,
    id_token: String,
) -> axum::http::Response<Body> {
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": id_token,
            "token_type": "Bearer",
        })))
        .mount(mock)
        .await;
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// With two providers configured, `/auth/login` stops redirecting and renders
/// the chooser with one button per provider. A single-provider deployment must
/// keep its immediate redirect, which `oauth_flow.rs` already asserts.
#[tokio::test]
async fn login_renders_a_chooser_once_a_second_provider_is_configured() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
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
    assert_eq!(resp.status(), StatusCode::OK, "chooser, not a redirect");
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("/auth/login/oidc?return_to=/app/projects"),
        "primary button missing: {html}"
    );
    assert!(
        html.contains("/auth/login/microsoft?return_to=/app/projects"),
        "microsoft button missing: {html}"
    );
    assert!(html.contains("Sign in with Microsoft"), "{html}");
}

/// An unrecognised provider slug is a 404, never a silent fall-through to the
/// primary provider. Somebody who clicked a Microsoft button must not land on
/// a Google consent screen.
#[tokio::test]
async fn unknown_provider_slug_is_not_found() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/login/okta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A provider this deployment has not configured is also a 404 — the route
/// exists, the door does not.
#[tokio::test]
async fn microsoft_route_is_not_found_when_the_provider_is_unconfigured() {
    let mock = MockServer::start().await;
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
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: sessions(),
        oauth: Some(cfg),
        oauth_microsoft: None,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/login/microsoft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The headline case: a pre-seeded external client signs in through Entra.
///
/// The token is shaped like a real multi-tenant one — `iss` interpolated from
/// `tid`, the address in `preferred_username`, and **no `email` claim at all**,
/// which is what Entra emits for a managed user whose directory `mail`
/// attribute is empty. The session must still resolve to the seeded row and
/// carry the provider slug, because sign-out depends on it.
#[tokio::test]
async fn entra_login_resolves_a_seeded_client_from_preferred_username() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();
    let state = state_with_both_providers(&mock, sessions_store.clone()).await;
    seed_person(
        &state.surreal,
        "sam@clientfirm.test",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    let token = portal::test_support::sign_entra_id_token(
        "ms-client",
        &nonce,
        "entra-pairwise-subject",
        CLIENT_TENANT,
        Some("sam@clientfirm.test"),
        None,
        None,
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;

    assert_eq!(
        cb.status(),
        StatusCode::SEE_OTHER,
        "expected a signed-in redirect"
    );
    assert_eq!(
        cb.headers().get("location").unwrap().to_str().unwrap(),
        "/app/projects"
    );
    let session_cookie = cb
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.contains("navigator_session="))
        .expect("session cookie set");
    let raw = session_cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("navigator_session=");
    let decoded = sessions_store.decode(raw).expect("session decodes");
    assert_eq!(decoded.email.as_deref(), Some("sam@clientfirm.test"));
    assert_eq!(decoded.role, store::persons::Role::Client);
    assert_eq!(
        decoded.provider.as_deref(),
        Some("microsoft"),
        "the session must record which provider authenticated it",
    );
}

/// The security case. A token from a tenant nobody allowlisted is refused even
/// though it is signed by the same key, carries the right audience and nonce,
/// and its `iss` is internally consistent with its own `tid`.
///
/// This is what stops anyone who can create a free Entra tenant — which is
/// anyone — from asserting a seeded person's address and inheriting that
/// person's role and matters.
#[tokio::test]
async fn entra_login_from_an_unlisted_tenant_is_refused() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    // Deliberately seeded: the refusal must come from the tenant check, not
    // from the person lookup, or the test proves nothing.
    seed_person(
        &state.surreal,
        "owner@neonlaw.test",
        store::persons::Role::Owner,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    let token = portal::test_support::sign_entra_id_token(
        "ms-client",
        &nonce,
        "attacker-subject",
        STRANGER_TENANT,
        Some("owner@neonlaw.test"),
        None,
        None,
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;

    assert_eq!(cb.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !cb.headers()
            .get_all("set-cookie")
            .iter()
            .any(|c| c.to_str().unwrap().contains("navigator_session=")),
        "no session may be minted for an unlisted tenant",
    );
}

/// An allowlisted tenant whose token claims some *other* tenant's issuer is
/// refused. Without this check the interpolated-issuer scheme would degrade to
/// "any issuer at all", since `Validation` is not enforcing `iss` on this path.
#[tokio::test]
async fn entra_login_with_a_mismatched_issuer_is_refused() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    seed_person(
        &state.surreal,
        "sam@clientfirm.test",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    let token = portal::test_support::sign_entra_id_token(
        "ms-client",
        &nonce,
        "entra-pairwise-subject",
        CLIENT_TENANT,
        Some("sam@clientfirm.test"),
        None,
        // `tid` is allowlisted, but `iss` names a different tenant.
        Some("https://entra.test/00000000-0000-0000-0000-000000000000/v2.0"),
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;
    assert_eq!(cb.status(), StatusCode::UNAUTHORIZED);
}

/// A token with no `tid` cannot have its issuer validated at all, so it is
/// refused rather than passed through on the strength of its signature.
#[tokio::test]
async fn entra_login_without_a_tenant_claim_is_refused() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    // The primary provider's token shape: correctly signed for the Microsoft
    // audience, but with a fixed `iss` and no `tid` — exactly what a
    // single-issuer provider emits.
    let token = portal::test_support::sign_id_token(
        "ms-client",
        &nonce,
        "no-tenant-subject",
        "sam@clientfirm.test",
        "Sam",
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;
    assert_eq!(cb.status(), StatusCode::UNAUTHORIZED);
}

/// `preferred_username` wins over `email` when both are present.
///
/// This is the impersonation guard, and it is the whole reason the claim
/// choice is per-provider. Entra populates `email` from the directory's `mail`
/// attribute, which no one verifies — a tenant admin can set it to any string.
/// A token whose UPN is the real user but whose `email` names somebody else
/// must resolve to the UPN's row.
#[tokio::test]
async fn entra_login_prefers_the_upn_over_the_unverified_email_claim() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();
    let state = state_with_both_providers(&mock, sessions_store.clone()).await;
    seed_person(
        &state.surreal,
        "feather@clientfirm.test",
        store::persons::Role::Client,
    )
    .await;
    seed_person(
        &state.surreal,
        "owner@neonlaw.test",
        store::persons::Role::Owner,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    let token = portal::test_support::sign_entra_id_token(
        "ms-client",
        &nonce,
        "entra-pairwise-subject",
        CLIENT_TENANT,
        Some("feather@clientfirm.test"),
        // The claim an allowlisted tenant's admin could set to anything.
        Some("owner@neonlaw.test"),
        None,
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;

    assert_eq!(cb.status(), StatusCode::SEE_OTHER);
    let session_cookie = cb
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.contains("navigator_session="))
        .expect("session cookie set");
    let raw = session_cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("navigator_session=");
    let decoded = sessions_store.decode(raw).expect("session decodes");
    assert_eq!(
        decoded.email.as_deref(),
        Some("feather@clientfirm.test"),
        "the UPN, not the tenant-asserted email, selects the person row",
    );
    assert_eq!(
        decoded.role,
        store::persons::Role::Client,
        "the Owner row must not be reachable through the `email` claim",
    );
}

/// A Microsoft login's code must not be redeemable at the primary provider,
/// and the primary provider's token must not verify against Microsoft's
/// verifier. The pre-auth cookie is what binds them, so swapping the cookie
/// for one minted by the other door has to fail.
#[tokio::test]
async fn a_microsoft_code_cannot_be_redeemed_against_the_primary_provider() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    seed_person(
        &state.surreal,
        "sam@clientfirm.test",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Start a *primary* login, so the signed pre-auth cookie says `oidc`, and
    // point the primary token endpoint at an Entra-shaped token. The primary
    // verifier pins a fixed issuer, so the Entra token's tenant-derived `iss`
    // cannot satisfy it.
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login/oidc?return_to=/app/projects")
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
    assert!(
        location.contains("client_id=primary-client"),
        "the `oidc` slug must reach the primary provider: {location}"
    );
    let state_param = query_param(&location, "state");
    let nonce = query_param(&location, "nonce");
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

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": portal::test_support::sign_entra_id_token(
                "primary-client",
                &nonce,
                "entra-pairwise-subject",
                CLIENT_TENANT,
                Some("sam@clientfirm.test"),
                None,
                None,
            ),
            "token_type": "Bearer",
        })))
        .mount(&mock)
        .await;

    let cb = app
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cb.status(),
        StatusCode::UNAUTHORIZED,
        "an Entra token must not satisfy the primary provider's pinned issuer",
    );
}

/// Sign-out ends the SSO session at the provider that actually holds it.
///
/// Before the session recorded its provider there was one end-session
/// endpoint for every session, so a Microsoft-authenticated person was bounced
/// through the primary provider's logout with a `client_id` that provider does
/// not own — their Entra session survived.
#[tokio::test]
async fn logout_redirects_to_the_end_session_endpoint_of_the_signing_provider() {
    let mock = MockServer::start().await;
    let sessions_store = sessions();
    let primary = portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "primary-client",
            "primary-secret",
            "http://app.test/auth/callback",
            format!("{}/authorize", mock.uri()),
            format!("{}/token", mock.uri()),
        ),
        "primary-client",
    )
    .with_end_session_endpoint("https://primary.test/logout");
    let microsoft = portal::test_support::microsoft_oauth_config(
        OAuthConfig::new(
            "ms-client",
            "ms-secret",
            "http://app.test/auth/callback",
            format!("{}/ms/authorize", mock.uri()),
            format!("{}/ms/token", mock.uri()),
        ),
        "ms-client",
        &[CLIENT_TENANT],
    )
    .with_end_session_endpoint("https://entra.test/organizations/oauth2/v2.0/logout");
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: sessions_store.clone(),
        oauth: Some(primary),
        oauth_microsoft: Some(microsoft),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let mut session =
        portal::SessionData::fresh("entra-pairwise-subject", store::persons::Role::Client);
    session.provider = Some("microsoft".to_string());
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        sessions_store.encode(&session)
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/logout")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with("https://entra.test/organizations/oauth2/v2.0/logout?"),
        "got: {location}"
    );
    assert!(location.contains("client_id=ms-client"), "got: {location}");

    // A session with no recorded provider — one minted before the field
    // existed — still logs out, through the primary provider.
    let legacy = portal::SessionData::fresh("legacy-subject", store::persons::Role::Client);
    let legacy_cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        sessions_store.encode(&legacy)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/logout")
                .header("cookie", legacy_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("https://primary.test/logout?"),);
}

/// The pre-seed gate is provider-agnostic: an Entra identity from an
/// allowlisted tenant with no `persons` row still gets the operator-mediated
/// 403, not a session. Authentication is not provisioning, and adding a
/// provider does not change who may sign in.
#[tokio::test]
async fn entra_login_for_an_unprovisioned_person_is_still_forbidden() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (state_param, nonce, cookie) = begin_microsoft_login(&app, "/app/projects").await;
    let token = portal::test_support::sign_entra_id_token(
        "ms-client",
        &nonce,
        "stranger-subject",
        CLIENT_TENANT,
        Some("nobody@clientfirm.test"),
        None,
        None,
    );
    let cb = finish_microsoft_callback(&app, &mock, &state_param, &cookie, token).await;
    assert_eq!(cb.status(), StatusCode::FORBIDDEN);
}

/// The pre-auth cookie is the disambiguator, so it has to be tamper-proof.
/// A cookie whose signature does not verify is rejected outright — nobody can
/// hand-craft one that says `provider: microsoft`.
#[tokio::test]
async fn a_forged_pre_auth_cookie_cannot_choose_a_provider() {
    let mock = MockServer::start().await;
    let state = state_with_both_providers(&mock, sessions()).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let forged = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "provider": "microsoft",
            "state": "attacker-state",
            "verifier": "attacker-verifier",
            "nonce": "attacker-nonce",
            "return_to": "/app/projects",
            "exp": i64::MAX,
        }))
        .unwrap(),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/auth/callback?code=abc&state=attacker-state")
                .header(
                    "cookie",
                    format!("{}={forged}.not-a-signature", oauth::PRE_AUTH_COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
