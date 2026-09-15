//! Browser sign-in must start on the origin that receives the callback.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::{OAuthConfig, SessionStore};
use tower::ServiceExt;

fn request(host: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .body(Body::empty())
        .unwrap()
}

async fn app() -> axum::Router {
    app_with_config("https://primary.example/auth/callback", false, None).await
}

async fn app_with_config(callback: &str, chooser: bool, canonical: Option<&str>) -> axum::Router {
    let surreal = store::test_support::mem_surreal().await;
    let mut state = portal::test_support::app_state(surreal).await;
    state.sessions = SessionStore::new("synthetic-host-test-signing-key");
    state.oauth = Some(OAuthConfig::new(
        "client",
        "secret",
        callback,
        "https://idp.example/authorize",
        "https://idp.example/token",
    ));
    if chooser {
        state.oauth_microsoft = state.oauth.clone();
        state.oauth_apple = state.oauth.clone();
    }
    // A secondary local listener resolves a brand without relying on a live
    // brand hostname. Cookies still belong to the hostname, not the port.
    state.canonical_host = portal::canonical_host::CanonicalHost::new(canonical.map(str::to_owned))
        .with_local_ports([(20430, views::brand::BrandKey::DeleteYourData)].into());
    server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR))
}

#[tokio::test]
async fn chooser_cookie_and_provider_selection_begin_only_on_primary() {
    use http_body_util::BodyExt;

    let app = app_with_config("https://primary.example/auth/callback", true, None).await;
    let path = "/auth/login?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed%26search%3Dsample";
    let secondary = app
        .clone()
        .oneshot(request("secondary.example:20430", path))
        .await
        .unwrap();
    assert_eq!(secondary.status(), StatusCode::SEE_OTHER);
    assert!(!secondary.headers().contains_key("set-cookie"));
    let primary = app
        .clone()
        .oneshot(request("primary.example", path))
        .await
        .unwrap();
    assert_eq!(primary.status(), StatusCode::OK);
    assert!(primary.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .contains(portal::oauth::LOGIN_CSRF_COOKIE_NAME));
    let body = primary.into_body().collect().await.unwrap().to_bytes();
    let html = std::str::from_utf8(&body).unwrap();
    for provider in ["oidc", "microsoft", "apple"] {
        assert!(
            html.contains(&format!(
            "/auth/login/{provider}?return_to=/app/projects%3Fstatus%3Dclosed%26search%3Dsample"
        )),
            "{html}"
        );
        let response = app
            .clone()
            .oneshot(request(
                "primary.example",
                &format!("/auth/login/{provider}"),
            ))
            .await
            .unwrap();
        let pre = pre_auth(&response);
        assert_eq!(pre.provider.slug(), provider);
        assert!(!pre.state.is_empty());
        assert!(!pre.verifier.is_empty());
    }
}

fn pre_auth(response: &axum::response::Response) -> portal::oauth::PreAuth {
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find_map(|value| value.strip_prefix("navigator_pre_auth="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let bytes = SessionStore::new("synthetic-host-test-signing-key")
        .decode_signed_bytes(cookie)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn unsafe_return_destinations_are_not_signed_into_pre_auth() {
    let app = app().await;
    for target in [
        "https://outside.example/",
        "//outside.example/",
        "/\\outside.example/",
        "/app\r\nLocation: https://outside.example/",
    ] {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("return_to", target)
            .finish();
        let response = app
            .clone()
            .oneshot(request("primary.example", &format!("/auth/login?{query}")))
            .await
            .unwrap();
        assert_eq!(pre_auth(&response).return_to, "", "{target}");
    }
    let response = app
        .oneshot(request(
            "primary.example",
            "/auth/login?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed%26search%3Dsample",
        ))
        .await
        .unwrap();
    assert_eq!(
        pre_auth(&response).return_to,
        "/app/projects?status=closed&search=sample"
    );
}

#[tokio::test]
async fn callback_origin_wins_over_canonical_host_without_looping() {
    let app = app_with_config(
        "http://localhost:20400/auth/callback",
        false,
        Some("canonical.example"),
    )
    .await;
    let secondary = app
        .clone()
        .oneshot(request("localhost:20430", "/auth/login"))
        .await
        .unwrap();
    assert_eq!(
        secondary.headers()["location"],
        "http://localhost:20400/auth/login"
    );
    let primary = app
        .clone()
        .oneshot(request("localhost:20400", "/auth/login"))
        .await
        .unwrap();
    assert!(primary.headers()["location"]
        .to_str()
        .unwrap()
        .starts_with("https://idp.example/authorize?"));
    let callback = app
        .oneshot(request("localhost:20400", "/auth/callback?code=x&state=x"))
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::BAD_REQUEST);
}

async fn round_trip_app() -> (axum::Router, wiremock::MockServer) {
    let idp = wiremock::MockServer::start().await;
    let surreal = store::test_support::mem_surreal().await;
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Synthetic Person",
            "person@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let mut state = portal::test_support::app_state(surreal).await;
    state.sessions = SessionStore::new("synthetic-host-test-signing-key");
    state.oauth = Some(portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            "client",
            "secret",
            "https://primary.example/auth/callback",
            format!("{}/authorize", idp.uri()),
            format!("{}/token", idp.uri()),
        ),
        "client",
    ));
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    (app, idp)
}

#[tokio::test]
async fn two_host_round_trip_keeps_state_pkce_and_safe_return_path() {
    use base64::Engine;
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, ResponseTemplate};

    let (app, idp) = round_trip_app().await;
    let login_path = "/auth/login?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed%26search%3Dsample";
    let secondary = app
        .clone()
        .oneshot(request("secondary.example", login_path))
        .await
        .unwrap();
    assert_eq!(
        secondary.headers()["location"],
        format!("https://primary.example{login_path}")
    );
    assert!(!secondary.headers().contains_key("set-cookie"));

    let login = app
        .clone()
        .oneshot(request("primary.example", login_path))
        .await
        .unwrap();
    let pre = pre_auth(&login);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let authorize = url::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    let params: std::collections::HashMap<_, _> = authorize.query_pairs().collect();
    assert_eq!(params["state"], pre.state);
    assert_eq!(params["nonce"], pre.nonce);
    assert_eq!(
        params["redirect_uri"],
        "https://primary.example/auth/callback"
    );
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(
        params["code_challenge"],
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(pre.verifier.as_bytes()))
    );

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!(
            "code_verifier={}",
            pre.verifier
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": portal::test_support::sign_id_token(
                "client", &pre.nonce, "synthetic-subject", "person@example.com", "Synthetic Person",
            ),
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&idp)
        .await;

    // A callback without the primary host's cookie still fails closed.
    let callback_path = format!("/auth/callback?code=code&state={}", pre.state);
    let missing = app
        .clone()
        .oneshot(request("primary.example", &callback_path))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    let body = missing.into_body().collect().await.unwrap().to_bytes();
    assert!(std::str::from_utf8(&body)
        .unwrap()
        .contains("https://primary.example/auth/login"));
    let mut wrong_state = request("primary.example", "/auth/callback?code=code&state=wrong");
    wrong_state
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let rejected = app.clone().oneshot(wrong_state).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert!(idp.received_requests().await.unwrap().is_empty());

    let mut callback = request("primary.example", &callback_path);
    callback
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let response = app.oneshot(callback).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "/app/projects?status=closed&search=sample"
    );
    assert!(response
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|value| value.to_str().unwrap().starts_with("navigator_session=")));
}

/// Run against the generated worktree environment and the browser harness.
/// Distinct loopback names exercise actual host-only cookie isolation.
#[tokio::test]
async fn browser_login_from_secondary_loopback_host_returns_to_primary() {
    use features::webdriver::{base_url, new_client_or_skip, require_harness, wait_for_path};
    use std::time::Duration;

    let Ok(port) = std::env::var("NAVIGATOR_LOCAL_DELETE_YOUR_DATA_PORT") else {
        assert!(
            !require_harness(),
            "source the worktree environment for the secondary listener"
        );
        return;
    };
    let Some(browser) = new_client_or_skip().await else {
        return;
    };
    let primary = url::Url::parse(&base_url()).unwrap();
    assert_eq!(primary.host_str(), Some("localhost"));
    let secondary = format!("http://127.0.0.1:{port}");
    login_with_optional_capture(&browser, &secondary).await;
    assert_eq!(
        browser.current_url().await.unwrap().origin(),
        primary.origin()
    );
    let cookies = browser.get_all_cookies().await.unwrap();
    assert!(cookies
        .iter()
        .any(|cookie| cookie.name() == "navigator_session"));
    browser.goto(&secondary).await.unwrap();
    assert!(browser
        .get_all_cookies()
        .await
        .unwrap()
        .iter()
        .all(|cookie| !matches!(
            cookie.name(),
            "navigator_session" | "navigator_pre_auth" | "navigator_login_csrf"
        )));
    browser
        .goto(&format!("{secondary}/app/projects"))
        .await
        .unwrap();
    wait_for_path(&browser, "/app/projects", Duration::from_secs(20)).await;
    assert_eq!(
        browser.current_url().await.unwrap().origin(),
        primary.origin()
    );
    assert!(!browser
        .source()
        .await
        .unwrap()
        .contains("missing pre-auth cookie"));
    browser.close().await.unwrap();
}

async fn login_with_optional_capture(browser: &fantoccini::Client, secondary: &str) {
    let password = ["pass", "word"].concat();
    let login = features::webdriver::login_as_at(
        browser,
        secondary,
        "lawyer@neonlaw.com",
        &password,
        "/app/projects",
    );
    if std::env::var("NAV_OAUTH_SHOTS").as_deref() != Ok("1") {
        login.await;
        return;
    }
    let directory = std::path::Path::new("/tmp/navigator-screenshots/oauth-host");
    std::fs::create_dir_all(directory).unwrap();
    tokio::pin!(login);
    let mut frame = 0;
    loop {
        let complete = tokio::select! {
            () = &mut login => true,
            () = tokio::time::sleep(std::time::Duration::from_millis(400)) => false,
        };
        std::fs::write(
            directory.join(format!("frame-{frame:04}.png")),
            browser.screenshot().await.unwrap(),
        )
        .unwrap();
        frame += 1;
        if complete {
            break;
        }
    }
}

#[tokio::test]
async fn secondary_login_redirects_before_minting_host_only_pre_auth_cookie() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/auth/login?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed")
                .header("host", "secondary.example:20430")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "https://primary.example/auth/login?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed"
    );
    assert!(!response.headers().contains_key("set-cookie"));
}

#[tokio::test]
async fn secondary_app_and_direct_provider_entries_keep_path_and_query() {
    let app = app().await;
    for path in [
        "/app",
        "/app/projects?status=closed&search=sample%20matter",
        "/auth/login/oidc?return_to=%2Fapp%2Fprojects%3Fstatus%3Dclosed",
        "/auth/login/microsoft?return_to=%2Fapp%2Fteam",
        "/auth/login/apple?return_to=%2Fapp%2Fteam",
    ] {
        let response = app
            .clone()
            .oneshot(request("secondary.example:20430", path))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(
            response.headers()["location"],
            format!("https://primary.example{path}")
        );
        assert!(!response.headers().contains_key("set-cookie"), "{path}");
    }
}

#[tokio::test]
async fn primary_login_accepts_case_and_default_port_without_a_redirect_loop() {
    let app = app().await;
    for host in ["primary.example", "PRIMARY.EXAMPLE:443"] {
        let response = app
            .clone()
            .oneshot(request(host, "/auth/login/oidc"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers()["location"].to_str().unwrap();
        assert!(location.starts_with("https://idp.example/authorize?"));
        assert!(location.contains("code_challenge_method=S256"));
        assert!(location.contains("state="));
        assert!(response.headers().contains_key("set-cookie"));
    }
    for slug in ["unknown", "microsoft", "apple"] {
        let response = app
            .clone()
            .oneshot(request("primary.example", &format!("/auth/login/{slug}")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.headers().contains_key("set-cookie"));
    }
}

#[tokio::test]
async fn secondary_public_pages_and_health_probes_stay_on_their_host() {
    let app = app().await;
    for path in ["/", "/app/health", "/app/readyz"] {
        let response = app
            .clone()
            .oneshot(request("secondary.example:20430", path))
            .await
            .unwrap();
        assert!(!response.status().is_redirection(), "{path}");
    }
}
