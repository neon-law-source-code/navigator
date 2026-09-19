//! Cucumber runner for `features/onboarding_welcome.feature`.
//!
//! Drives the OAuth callback end-to-end against a wiremock `IdP`, then
//! asserts on the welcome emails captured by the shared
//! [`CapturingEmail`]. The capture is the visible artifact of the
//! `email_send__welcome` dispatch — the workflow trigger fires the
//! `onboarding__welcome` spec via `state.workflow_runtime`, the
//! [`workflows::DispatchingRuntime`] wrapper (from
//! [`features::app_state_with_email`]) catches the
//! `email_send__welcome` transition, and routes the render through
//! the shared [`workflows::EmailService`].
//!
//! The trigger is `tokio::spawn`'d in the callback — assertions
//! `tokio::yield_now` a few times before checking the captured list
//! so the background task gets a turn.

// Cucumber's step-attribute macros want `async fn` everywhere.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::http::StatusCode;
use cucumber::{given, then, when, World};
use features::{app_state_with_email, drive_verified_oauth, fs_storage, verified_oauth_config};
use portal::email::CapturingEmail;
use portal::{policy::PolicyClient, SessionStore};
use wiremock::MockServer;
use workflows::{EmailService, InMemoryRuntime};

#[derive(Default, World)]
#[world(init = Self::default)]
struct WelcomeWorld {
    idp: Option<Arc<MockServer>>,
    app: Option<axum::Router>,
    captured: Option<Arc<CapturingEmail>>,
    issued_sub: Option<String>,
    issued_email: Option<String>,
    issued_name: Option<String>,
}

impl std::fmt::Debug for WelcomeWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WelcomeWorld").finish_non_exhaustive()
    }
}

impl WelcomeWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    fn captured(&self) -> Vec<portal::email::OutboundEmail> {
        self.captured
            .as_ref()
            .expect("capturing email not wired")
            .captured()
    }
}

/// The system's bootstrap Owner email. The callback creates this identity as
/// `owner`, rather than the ordinary first-sign-in `client`, and fires the
/// welcome once.
const BOOTSTRAP_OWNER_EMAIL: &str = "nick@neonlaw.com";

async fn build_app(world: &mut WelcomeWorld, idp_uri: &str) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("onboarding-welcome").await;
    let oauth = verified_oauth_config(idp_uri);
    let capturing = Arc::new(CapturingEmail::new());
    let capturing_as_service: Arc<dyn EmailService> = capturing.clone();
    let mut state = app_state_with_email(
        runtime,
        storage,
        PolicyClient::passthrough(),
        Some(oauth),
        SessionStore::new("test-session-key-not-for-production"),
        capturing_as_service,
    )
    .await;
    // The welcome only fires on the bootstrap-Owner JIT-create path, so
    // the test app must know which email owns the system.
    state.bootstrap_owner_email = Some(BOOTSTRAP_OWNER_EMAIL.to_string());
    world.captured = Some(capturing);
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[given("a CapturingEmail backend wired into the app")]
async fn capturing_backend(world: &mut WelcomeWorld) {
    // Every scenario here turns on the bootstrap Owner not existing yet:
    // the welcome fires on the JIT-create, once. The person directory
    // lives in `SurrealDB` (#1093; ENG-19) and the cucumber suites share
    // one engine, so that row now outlives the scenario that created it
    // and the next scenario's "first login" would be a return login.
    // Removing it restores the precondition the feature is written
    // against. `main` runs these scenarios one at a time so this reset
    // cannot land in the middle of another one.
    let surreal = features::shared_surreal().await;
    if let Some(existing) = store::persons::find_by_email_ci(&surreal, BOOTSTRAP_OWNER_EMAIL)
        .await
        .expect("bootstrap owner lookup")
    {
        store::persons::delete(&surreal, existing.id)
            .await
            .expect("clear the bootstrap owner");
    }

    // The IdP mock has to be running first because OAuthConfig
    // captures the URI by value at construction time. If
    // `seed_idp_token` hasn't run yet (the Lawyer scenario runs the
    // `seeded person` step first), spin one up here.
    if world.idp.is_none() {
        let server = MockServer::start().await;
        world.idp = Some(Arc::new(server));
    }
    if world.app.is_none() {
        let uri = world.idp.as_ref().unwrap().uri();
        build_app(world, &uri).await;
    }
}

#[given(regex = r#"^the IdP issues sub="([^"]+)", email="([^"]+)", name="([^"]+)"$"#)]
async fn seed_idp_token(world: &mut WelcomeWorld, sub: String, email: String, name: String) {
    // Only record the identity — the `/token` mock is mounted per
    // login by `drive_verified_oauth`, which has to sign the
    // id_token with that login's `nonce` to satisfy the verifier.
    world.issued_sub = Some(sub);
    world.issued_email = Some(email);
    world.issued_name = Some(name);
}

#[given(regex = r#"^a seeded person with email "([^"]+)" and role "([^"]+)"$"#)]
async fn seed_person(_world: &mut WelcomeWorld, email: String, role: String) {
    let role = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
        _ => store::persons::Role::Client,
    };
    store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson::with_role(String::new(), email, role),
    )
    .await;
}

async fn drive_oauth(world: &WelcomeWorld) {
    let app = world.app();
    let idp = world.idp.as_ref().expect("idp not started").clone();
    let sub = world.issued_sub.as_deref().expect("identity seeded");
    let email = world.issued_email.as_deref().expect("identity seeded");
    let name = world.issued_name.as_deref().expect("identity seeded");
    let (status, _landing) = drive_verified_oauth(&app, &idp, sub, email, name).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[when(regex = r"^(?:Lawyer|the bootstrap Owner) completes the OAuth login dance(?: again)?$")]
async fn complete_oauth(world: &mut WelcomeWorld) {
    // The welcome dispatch is `tokio::spawn`'d inside the callback so
    // the HTTP response doesn't block on broker latency. Poll (bounded)
    // for it to land rather than racing a fixed yield burst — the burst
    // flaked under full-suite CPU contention. Scenarios that expect no
    // welcome simply exhaust the small budget before asserting empty.
    let before = world.captured().len();
    drive_oauth(world).await;
    for _ in 0..200 {
        if world.captured().len() > before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

#[then(regex = r"^exactly (\d+) captured emails? exists?$")]
async fn assert_captured_count(world: &mut WelcomeWorld, expected: usize) {
    let captured = world.captured();
    assert_eq!(
        captured.len(),
        expected,
        "captured emails: {:?}",
        captured.iter().map(|e| &e.to).collect::<Vec<_>>()
    );
}

#[then("no captured emails exist")]
async fn assert_no_captured(world: &mut WelcomeWorld) {
    let captured = world.captured();
    assert!(
        captured.is_empty(),
        "expected no welcomes, got: {captured:?}"
    );
}

#[then(regex = r#"^the captured email is addressed to "([^"]+)"$"#)]
async fn assert_captured_to(world: &mut WelcomeWorld, expected: String) {
    let captured = world.captured();
    let first = captured.first().expect("at least one captured email");
    assert_eq!(first.to, expected);
}

#[then(regex = r#"^the captured email subject is "([^"]+)"$"#)]
async fn assert_captured_subject(world: &mut WelcomeWorld, expected: String) {
    let captured = world.captured();
    let first = captured.first().expect("at least one captured email");
    assert_eq!(first.subject, expected);
}

#[then(regex = r#"^the captured email body mentions "([^"]+)"$"#)]
async fn assert_captured_body_contains(world: &mut WelcomeWorld, needle: String) {
    let captured = world.captured();
    let first = captured.first().expect("at least one captured email");
    assert!(
        first.body.contains(&needle),
        "body did not mention {needle:?}: {}",
        first.body
    );
}

#[tokio::main]
async fn main() {
    WelcomeWorld::cucumber()
        // One at a time. Each scenario resets the bootstrap Owner row on
        // an engine every scenario shares, so concurrent runs would clear
        // each other's identity mid-login.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/onboarding_welcome.feature")
        .await;
}
