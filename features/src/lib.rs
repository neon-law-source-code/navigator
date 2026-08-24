//! Shared scaffolding for the Neon Law Navigator BDD feature suite.
//!
//! Each `tests/<name>.rs` runner owns its own `cucumber::World` and
//! step set; this library only carries pieces that more than one
//! runner would otherwise duplicate — an in-memory `AppState`
//! constructor, a signed-id_token OAuth driver for the OIDC
//! scenarios, and a tiny body-reader.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use portal::{
    policy, AppState, AuthConfig, CanonicalHost, OAuthConfig, SessionStore, WorkshopIndex,
};
use workflows::{DispatchingRuntime, EmailService, InMemoryRuntime, StateMachineRuntime};

pub mod journey;
pub mod template_shapes;

#[cfg(feature = "webdriver")]
pub mod webdriver;

/// The router the `neon` binary serves: the Navigator application under the
/// public face.
///
/// The BDD runners drive the application, not the face, but they need the
/// site composed to have a router at all. Composed through `neon`'s own entry
/// points so a scenario cannot pass against a surface no binary serves.
///
/// One router, not two. The site's pages used to be separate crates behind
/// separate host routers, and a scenario had to pick the right one or walk a
/// `404`; they are one binary now, so every scenario composes this. Both
/// Catalog catalogs — the anonymous talks and the gated Navigator classes —
/// mount here along with everything else.
///
/// # Panics
///
/// Panics when the site's declared paths collide with a Navigator-owned
/// prefix.
pub fn neon_router(state: AppState, public_dir: &std::path::Path) -> axum::Router {
    let dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        public_dir,
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        dioxus,
    )
    .expect("the site must not claim Navigator-owned routes")
}

/// One embedded `SurrealDB` for the whole BDD process.
///
/// `store::surreal::test_support::mem()` hands out a private engine per
/// call, which is right for a `#[tokio::test]` that owns its handle to
/// the end. Cucumber is different: it drops each scenario's `World` —
/// and with it the last handle to that scenario's engine — from inside
/// its own async runner, and dropping an embedded engine's runtime in an
/// async context panics with "Cannot drop a runtime in a context where
/// blocking is not allowed".
///
/// Sharing one engine for the process removes the drop entirely.
/// Scenarios therefore share store state; per-scenario isolation, when a
/// scenario needs it, is the caller's to arrange.
static SURREAL: tokio::sync::OnceCell<store::surreal::SurrealDb> =
    tokio::sync::OnceCell::const_new();

pub async fn shared_surreal() -> store::surreal::SurrealDb {
    init_repo_root();
    SURREAL
        .get_or_init(|| async { store::surreal::test_support::mem().await })
        .await
        .clone()
}

fn init_repo_root() {
    let root = std::env::temp_dir().join(format!("navigator-feature-repos-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create feature repo root");
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", root);
}

/// Assemble an [`AppState`] suitable for `oneshot` tests. Callers
/// pass a shared `InMemoryRuntime` so they can assert on its event
/// log; the runtime stands in for both the workflow and
/// questionnaire timelines (the production binary uses two distinct
/// trait objects, but a single runtime satisfies both).
///
/// Internally allocates a fresh [`portal::email::CapturingEmail`]; use
/// [`app_state_with_email`] when the scenario needs to share the
/// concrete `CapturingEmail` with assertions (e.g. counting welcome
/// emails dispatched through the workflow path).
pub async fn app_state(
    runtime: Arc<InMemoryRuntime>,
    storage: Arc<dyn cloud::StorageService>,
    policy_client: policy::PolicyClient,
    oauth: Option<OAuthConfig>,
    sessions: SessionStore,
) -> AppState {
    app_state_with_email(
        runtime,
        storage,
        policy_client,
        oauth,
        sessions,
        Arc::new(portal::email::CapturingEmail::new()),
    )
    .await
}

/// Variant of [`app_state`] that lets the caller inject a specific
/// `EmailService` (typically a shared [`portal::email::CapturingEmail`]
/// so scenarios can assert on what was dispatched). The workflow
/// timeline is wrapped in [`workflows::DispatchingRuntime`] backed
/// by the same service, mirroring the prod worker — a transition
/// into an `email_send__*` state dispatches the email inline.
pub async fn app_state_with_email(
    runtime: Arc<InMemoryRuntime>,
    storage: Arc<dyn cloud::StorageService>,
    policy_client: policy::PolicyClient,
    oauth: Option<OAuthConfig>,
    sessions: SessionStore,
    email: Arc<dyn EmailService>,
) -> AppState {
    // Back the dispatching runtime with the store so compliance-submission
    // and matter-closing (`firm_signature__*`) side effects run in-process,
    // mirroring the prod worker.
    let surreal = shared_surreal().await;
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        DispatchingRuntime::new(runtime.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    AppState {
        brand_bundle: None,
        // The BDD runner drives real handlers, so it needs the store the
        // same way a test AppState does — a private embedded engine with
        // the schema applied.
        surreal,
        assets_storage: storage.clone(),
        applications_storage: storage.clone(),
        forms_registry: Arc::new(forms::registry().expect("forms registry loads")),
        workshops: WorkshopIndex::empty(),
        docs: portal::DocsIndex::empty(),
        blog: portal::BlogIndex::empty(),
        auth: AuthConfig::new(true, None),
        google_oauth: portal::google_oauth::GoogleOauthConfig::passthrough(),
        // The BDD runner exercises one browser provider. The Microsoft door has
        // its own integration suite (`server/tests/microsoft_sso.rs`), and
        // leaving it off here keeps `/auth/login` an immediate redirect rather
        // than a chooser, which is what these scenarios drive.
        oauth_microsoft: None,
        rate_limit: portal::rate_limit::RateLimit::disabled(),
        canonical_host: CanonicalHost::new(None),
        portal_only: portal::PortalOnly::default(),
        sessions,
        oauth,
        storage,
        policy: policy_client,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: Arc::new(portal::signature::StubSignatureProvider::new()),
        billing_provider: Arc::new(portal::billing::StubBillingProvider::new()),
        contract_reviewer: Arc::new(portal::contract_review::StubContractReviewer),
        esignature_webhook_secret: None,
        esignature_hmac_key: None,
        email,
        attachment_scanner: Arc::new(portal::attachment_scanner::FakeAttachmentScanner::clean()),
        inbound_email_secret: None,
        email_events_secret: None,
        sendgrid_events_public_key: None,
        bootstrap_owner_email: None,
        self_signup_enabled: false,
        identity_password: None,
        identity_admin: None,
        a2a_router: None,
    }
}

/// Stand up a filesystem-backed `StorageService` rooted in a
/// per-suite temp directory. The path includes `suite` so parallel
/// integration tests don't trample each other.
pub async fn fs_storage(suite: &str) -> Arc<dyn cloud::StorageService> {
    Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-features-{suite}")))
            .await
            .expect("create FsStorage temp root"),
    )
}

/// Drain a response body into a `String`. The Neon Law Navigator handlers
/// always emit UTF-8.
pub async fn body_string(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).expect("response body is UTF-8")
}

/// Rendered HTML with its HTML character references decoded.
///
/// The questionnaire walkers render through Dioxus, whose SSR escapes an
/// apostrophe in a text node to a numeric reference (`client&#39;s`), so a
/// prompt carrying one is not a raw substring of the response body. Decoding
/// first keeps the assertion about the words the reader sees rather than how
/// the renderer spells them.
#[must_use]
pub fn decode_character_references(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').and_then(|end| {
            let reference = &rest[1..end];
            let ch = match reference {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => reference
                    .strip_prefix('#')
                    .and_then(|digits| match digits.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => digits.parse().ok(),
                    })
                    .and_then(char::from_u32),
            };
            ch.map(|ch| (ch, end))
        });
        if let Some((ch, end)) = decoded {
            out.push(ch);
            rest = &rest[end + 1..];
        } else {
            // A bare `&` — an inline script's `&&`, say — is not a reference:
            // emit it and carry on from the next byte.
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The OAuth `client_id` the OIDC BDD apps register; the test
/// `id_token` verifier is pinned to it as the expected `aud`.
pub const OAUTH_CLIENT_ID: &str = "navigator-web";

/// Build the [`OAuthConfig`] the OIDC BDD suites hand to
/// [`app_state`], pointed at a wiremock `IdP` and carrying the shared
/// test `id_token` verifier (`portal::test_support::oidc_verifier`) — so
/// `/auth/callback` runs the full production signature + `iss`/`aud`/
/// `nonce` verification instead of refusing with 500.
#[must_use]
pub fn verified_oauth_config(idp_uri: &str) -> OAuthConfig {
    portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            OAUTH_CLIENT_ID,
            "navigator-web-secret",
            "http://app.test/auth/callback",
            format!("{idp_uri}/authorize"),
            format!("{idp_uri}/token"),
        ),
        OAUTH_CLIENT_ID,
    )
}

/// Drive `/auth/login` → `/auth/callback` end-to-end against `app`,
/// programming `idp`'s `/token` endpoint to return a properly-signed
/// `id_token` only once the login leg reveals the per-request `nonce`
/// (the verifier binds the token to the login via that claim, so the
/// mock cannot be mounted up-front). The `IdP` is reset first so a
/// repeat login never replays a stale-nonce token.
///
/// Returns the callback's status and its `Location` header — `303` with the
/// role's post-login landing on a successful link, `403` with no location when
/// the identity isn't pre-seeded (sign-up is operator-mediated). The login leg
/// carries no `return_to`, so the landing is the tier default that
/// `post_login_landing` resolves, not a deep link.
pub async fn drive_verified_oauth(
    app: &axum::Router,
    idp: &wiremock::MockServer,
    sub: &str,
    email: &str,
    name: &str,
) -> (StatusCode, Option<String>) {
    use tower::ServiceExt;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, ResponseTemplate};

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
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let location = login
        .headers()
        .get("location")
        .expect("login redirects to the IdP")
        .to_str()
        .unwrap()
        .to_string();
    let qp = |name: &str| {
        let needle = format!("{name}=");
        location
            .split('&')
            .find_map(|p| p.strip_prefix(&needle))
            .unwrap_or_else(|| panic!("`{name}` missing from {location}"))
            .to_string()
    };
    let state_param = qp("state");
    let nonce = qp("nonce");
    let pre_auth_cookie = login
        .headers()
        .get("set-cookie")
        .expect("login set-cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    idp.reset().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": portal::test_support::sign_id_token(OAUTH_CLIENT_ID, &nonce, sub, email, name),
            "token_type": "Bearer",
        })))
        .mount(idp)
        .await;

    let cb = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", pre_auth_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let landing = cb
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    (cb.status(), landing)
}

/// Tiny URL encoder for the four characters our feature payloads
/// actually contain (space and `@`). Enough for retainer answers
/// like `Libra` and `libra@example.com`; not a general
/// `application/x-www-form-urlencoded` implementation.
#[must_use]
pub fn form_encode(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

#[cfg(test)]
mod character_reference_tests {
    use super::decode_character_references;

    #[test]
    fn a_prompt_reads_the_same_however_the_renderer_spells_its_punctuation() {
        // Dioxus SSR may write the apostrophe as a numeric reference or leave
        // it literal; a feature step asserting on the words the reader sees
        // must accept both.
        assert_eq!(
            decode_character_references("What is the client&#39;s full legal name?"),
            "What is the client's full legal name?"
        );
        assert_eq!(
            decode_character_references("What is the client's full legal name?"),
            "What is the client's full legal name?"
        );
        assert_eq!(
            decode_character_references("&quot;Libra&quot; &amp; &#x27;Gemini&#x27;"),
            "\"Libra\" & 'Gemini'"
        );
    }

    #[test]
    fn a_bare_ampersand_survives_decoding() {
        // Hydration scripts carry `&&` and query strings carry `?a=1&b=2`;
        // neither is a character reference, so both pass through untouched.
        assert_eq!(
            decode_character_references("if (a && b) go('/x?p=1&q=2')"),
            "if (a && b) go('/x?p=1&q=2')"
        );
    }
}

#[cfg(test)]
mod runner_exit_tests {
    /// Every cucumber runner must use `run_and_exit`, not `run`: with
    /// `harness = false`, `run` prints step failures but the process
    /// still exits 0, so a red scenario scrolls through a green CI job
    /// (#276). Scans the real runner sources so a new feature binary
    /// can't reintroduce the silent-pass.
    #[test]
    fn every_cucumber_runner_propagates_failure_through_its_exit_code() {
        let tests_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut runners = 0usize;
        for entry in std::fs::read_dir(&tests_dir).expect("read features/tests") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read runner source");
            if !source.contains("cucumber()") {
                continue;
            }
            runners += 1;
            // `.run(` (no closing quote) also catches a variable-path
            // call like `.run(path)`, and is not a substring of
            // `.run_and_exit(`, so the two checks are exact.
            assert!(
                !source.contains(".run("),
                "{}: uses `.run(...)`, which exits 0 even when scenarios fail — \
                 use `.run_and_exit(...)` so CI sees the failure",
                path.display(),
            );
            assert!(
                source.contains(".run_and_exit("),
                "{}: cucumber runner without `.run_and_exit(...)`",
                path.display(),
            );
        }
        assert!(
            runners > 10,
            "expected to scan the cucumber runner corpus, found {runners} — wrong path?"
        );
    }
}
