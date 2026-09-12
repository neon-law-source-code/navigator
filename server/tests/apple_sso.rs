#![allow(clippy::doc_markdown)]
//! Sign in with Apple against a fully mocked OIDC surface.
//!
//! The test keypair is generated in memory for each case. The mocked token
//! endpoint inspects the real form body, verifies the ES256 client-secret JWT
//! with the generated public key, and returns a normally signed test id_token.
//! No Apple endpoint or Apple credential is contacted.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::Generate;
use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use portal::{AppState, OAuthConfig, SessionStore};
use serde::{Deserialize, Serialize};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const APPLE_CLIENT_ID: &str = "test-services-id";
const APPLE_TEAM_ID: &str = "test-team-id";
const APPLE_KEY_ID: &str = "test-key-id";

#[derive(Debug, Deserialize)]
struct AppleClientSecretClaims {
    iss: String,
    sub: String,
    aud: String,
    iat: i64,
    exp: i64,
}

#[derive(Serialize)]
struct AppleIdTokenClaims<'a> {
    iss: &'a str,
    aud: &'a str,
    exp: i64,
    sub: &'a str,
    email: &'a str,
    email_verified: bool,
    name: &'a str,
    nonce: &'a str,
}

struct AppleFixture {
    private_key_pem: String,
    public_key_pem: String,
}

impl AppleFixture {
    fn generated() -> Self {
        let signing_key = SigningKey::generate();
        Self {
            private_key_pem: signing_key
                .to_pkcs8_pem(LineEnding::LF)
                .expect("test key serialises")
                .to_string(),
            public_key_pem: signing_key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .expect("test public key serialises"),
        }
    }

    fn id_token_verifier(&self) -> portal::oauth::IdTokenVerifier {
        portal::oauth::IdTokenVerifier::from_keys_with_algorithm(
            vec![(
                APPLE_KEY_ID.to_string(),
                DecodingKey::from_ec_pem(self.public_key_pem.as_bytes())
                    .expect("Apple test public key parses"),
            )],
            "https://apple.test",
            APPLE_CLIENT_ID,
            portal::oauth::IssuerPolicy::Exact,
            Algorithm::ES256,
        )
    }
}

fn sign_apple_id_token(fixture: &AppleFixture, nonce: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(APPLE_KEY_ID.to_string());
    encode(
        &header,
        &AppleIdTokenClaims {
            iss: "https://apple.test",
            aud: APPLE_CLIENT_ID,
            exp: portal::session::now_unix_secs() + 600,
            sub: "apple-test-subject",
            email: "apple-user@example.test",
            email_verified: true,
            name: "Apple User",
            nonce,
        },
        &EncodingKey::from_ec_pem(fixture.private_key_pem.as_bytes())
            .expect("Apple test private key parses"),
    )
    .expect("Apple test id_token signs")
}

fn sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

fn query_param(location: &str, name: &str) -> String {
    let url = url::Url::parse(location).expect("mock authorization URL parses");
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
        .unwrap_or_else(|| panic!("{name} missing from {location}"))
}

async fn app(
    mock: &MockServer,
    fixture: &AppleFixture,
    apple_logout_endpoint: Option<&str>,
) -> (axum::Router, SessionStore, store::surreal::SurrealDb) {
    let sessions_store = sessions();
    let primary = portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "primary-test",
            "primary-secret",
            "http://app.test/auth/callback",
            format!("{}/authorize", mock.uri()),
            format!("{}/token", mock.uri()),
        ),
        "primary-test",
    );
    let apple = OAuthConfig::new_apple(
        APPLE_CLIENT_ID,
        APPLE_TEAM_ID,
        APPLE_KEY_ID,
        fixture.private_key_pem.as_bytes(),
        "http://app.test/auth/callback",
        format!("{}/apple/authorize", mock.uri()),
        format!("{}/apple/token", mock.uri()),
    )
    .expect("test Apple config builds")
    .with_id_token_verifier(fixture.id_token_verifier());
    let apple = apple_logout_endpoint.map_or(apple.clone(), |endpoint| {
        apple.with_end_session_endpoint(endpoint)
    });
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: sessions_store.clone(),
        oauth: Some(primary),
        oauth_apple: Some(apple),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        sessions_store,
        surreal,
    )
}

async fn seed_person(surreal: &store::surreal::SurrealDb) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Apple User",
            "apple-user@example.test",
            Role::Client,
        ),
    )
    .await
    .expect("seed Apple test person");
}

async fn begin_apple_login(app: &axum::Router) -> (String, String, String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login/apple?return_to=/app/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (
        query_param(&location, "state"),
        query_param(&location, "nonce"),
        location,
        cookie,
    )
}

async fn finish_apple_login(
    app: &axum::Router,
    mock: &MockServer,
    fixture: &AppleFixture,
    state: &str,
    nonce: &str,
    cookie: &str,
) -> axum::http::Response<Body> {
    Mock::given(method("POST"))
        .and(path("/apple/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": sign_apple_id_token(fixture, nonce),
            "token_type": "Bearer",
        })))
        .mount(mock)
        .await;
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=test-code&state={state}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn apple_login_redirect_carries_client_id_and_pkce() {
    let mock = MockServer::start().await;
    let fixture = AppleFixture::generated();
    let (app, _, _) = app(&mock, &fixture, None).await;
    let (_, _, location, _) = begin_apple_login(&app).await;
    assert!(location.contains(&format!("client_id={APPLE_CLIENT_ID}")));
    assert!(location.contains("code_challenge="));
    assert!(location.contains("code_challenge_method=S256"));
}

/// The shape Apple actually uses. Because the authorization request asks for
/// the `email` scope, Apple requires `response_mode=form_post` and answers by
/// POSTing a form to the redirect URI instead of redirecting to it — so the
/// callback has to accept a POST, and the pre-auth cookie has to survive a
/// cross-site request. A GET-only callback returns 405 here and a `SameSite=Lax`
/// cookie is never sent at all, which is why the sibling test above cannot
/// stand in for this one.
#[tokio::test]
async fn apple_completes_a_sign_in_through_the_form_post_callback() {
    let mock = MockServer::start().await;
    let fixture = AppleFixture::generated();
    let (app, sessions_store, surreal) = app(&mock, &fixture, None).await;
    seed_person(&surreal).await;

    let (state, nonce, location, cookie) = begin_apple_login(&app).await;
    assert!(
        location.contains("response_mode=form_post"),
        "Apple refuses a scoped authorization request without form_post: {location}"
    );

    Mock::given(method("POST"))
        .and(path("/apple/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": sign_apple_id_token(&fixture, &nonce),
            "token_type": "Bearer",
        })))
        .mount(&mock)
        .await;

    // Exactly what Apple sends: the code and state as a form body, plus the
    // first-login `user` field, which this flow ignores in favour of the
    // verified id_token.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", "test-code")
        .append_pair("state", &state)
        .append_pair("user", r#"{"name":{"firstName":"Test"}}"#)
        .finish();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/callback")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "the form_post callback must complete the sign-in"
    );
    let session_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.contains("navigator_session="))
        .expect("session cookie set");
    let raw = session_cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("navigator_session=");
    let session = sessions_store.decode(raw).expect("session decodes");
    assert_eq!(session.provider.as_deref(), Some("apple"));
}

#[tokio::test]
async fn apple_token_exchange_verifies_the_client_secret_and_records_provider() {
    let mock = MockServer::start().await;
    let fixture = AppleFixture::generated();
    let (app, sessions_store, surreal) = app(&mock, &fixture, None).await;
    seed_person(&surreal).await;

    let (state, nonce, _, cookie) = begin_apple_login(&app).await;
    let callback = finish_apple_login(&app, &mock, &fixture, &state, &nonce, &cookie).await;
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    let session_cookie = callback
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.contains("navigator_session="))
        .expect("session cookie set");
    let raw = session_cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("navigator_session=");
    let session = sessions_store.decode(raw).expect("session decodes");
    assert_eq!(session.provider.as_deref(), Some("apple"));

    let request = mock
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .find(|request| request.url.path() == "/apple/token")
        .expect("Apple token endpoint called");
    let body = String::from_utf8(request.body).expect("token form is UTF-8");
    let client_secret = url::form_urlencoded::parse(body.as_bytes())
        .find_map(|(key, value)| (key == "client_secret").then(|| value.into_owned()))
        .expect("client_secret form field");
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_issuer(&[APPLE_TEAM_ID]);
    validation.set_audience(&["https://appleid.apple.com"]);
    let claims = decode::<AppleClientSecretClaims>(
        &client_secret,
        &DecodingKey::from_ec_pem(fixture.public_key_pem.as_bytes()).expect("public key parses"),
        &validation,
    )
    .expect("Apple client-secret signature must verify");
    assert_eq!(claims.header.alg, Algorithm::ES256);
    assert_eq!(claims.header.kid.as_deref(), Some(APPLE_KEY_ID));
    assert_eq!(claims.claims.iss, APPLE_TEAM_ID);
    assert_eq!(claims.claims.sub, APPLE_CLIENT_ID);
    assert_eq!(claims.claims.aud, "https://appleid.apple.com");
    assert!(claims.claims.exp > claims.claims.iat);
}

#[tokio::test]
async fn apple_logout_uses_discovered_endpoint_and_falls_back_when_absent() {
    let mock = MockServer::start().await;
    let fixture = AppleFixture::generated();
    let (apple_app, sessions_store, _) =
        app(&mock, &fixture, Some("https://apple.test/logout")).await;
    let mut session = portal::SessionData::fresh("apple-test-subject", Role::Client);
    session.provider = Some("apple".to_string());
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        sessions_store.encode(&session)
    );
    let response = apple_app
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
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.starts_with("https://apple.test/logout?"));
    assert!(location.contains(&format!("client_id={APPLE_CLIENT_ID}")));

    let (fallback, fallback_sessions, _) = app(&mock, &fixture, None).await;
    let mut session = portal::SessionData::fresh("apple-test-subject", Role::Client);
    session.provider = Some("apple".to_string());
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        fallback_sessions.encode(&session)
    );
    let response = fallback
        .oneshot(
            Request::builder()
                .uri("/auth/logout")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers().get("location").unwrap(), "/");
}
