//! Cucumber runner for `features/oidc_callback.feature`.
//!
//! Stands up wiremock as Rauthy, drives `/auth/login` →
//! `/auth/callback` end-to-end, and asserts on the resulting
//! `persons` table state. Mirrors the patterns in
//! `web/tests/oidc_e2e.rs`.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::http::StatusCode;
use cucumber::{given, then, when, World};
use features::{app_state, drive_verified_oauth, fs_storage, verified_oauth_config};
use portal::{policy::PolicyClient, SessionStore};
use wiremock::MockServer;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct OidcWorld {
    idp: Option<Arc<MockServer>>,
    app: Option<axum::Router>,
    issued_sub: Option<String>,
    issued_email: Option<String>,
    issued_name: Option<String>,
    callback_status: Option<StatusCode>,
    callback_landing: Option<String>,
}

impl std::fmt::Debug for OidcWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcWorld")
            .field("issued_sub", &self.issued_sub)
            .field("issued_email", &self.issued_email)
            .finish_non_exhaustive()
    }
}

impl OidcWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
}

/// Build the `AppState` + Router once we know the `IdP` URI. The
/// per-scenario identity is recorded by `seed_idp_token` and signed
/// into the `/token` response by [`drive_verified_oauth`] once the
/// login leg reveals the nonce; the `OAuthConfig` carries the test
/// `id_token` verifier so the callback runs full verification.
async fn build_app(world: &mut OidcWorld) {
    let idp = world.idp.as_ref().expect("idp mock not started");
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("oidc").await;
    let oauth = verified_oauth_config(&idp.uri());
    let state = app_state(
        runtime,
        storage,
        PolicyClient::passthrough(),
        Some(oauth),
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[given("a mock IdP returning an id_token")]
async fn start_idp(world: &mut OidcWorld) {
    let server = MockServer::start().await;
    world.idp = Some(Arc::new(server));
}

#[given(regex = r#"^the IdP issues sub="([^"]+)", email="([^"]+)", name="([^"]+)"$"#)]
async fn seed_idp_token(world: &mut OidcWorld, sub: String, email: String, name: String) {
    // Only record the identity — the `/token` mock is mounted per
    // login by `drive_verified_oauth`, which has to sign the
    // id_token with that login's `nonce` to satisfy the verifier.
    world.issued_sub = Some(sub);
    world.issued_email = Some(email);
    world.issued_name = Some(name);
    // The IdP mock has to be live before the AppState is built — the
    // OAuthConfig captures the URI by value at construction time.
    if world.app.is_none() {
        build_app(world).await;
    }
}

#[given(regex = r#"^a seeded person with email "([^"]+)" and role "([^"]+)"$"#)]
async fn seed_person(world: &mut OidcWorld, email: String, role: String) {
    // The seeded row has to land in the same DB the callback writes
    // to. Build the app first if the previous step hasn't.
    if world.app.is_none() {
        build_app(world).await;
    }
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

#[when(regex = r"^(?:Libra|Lawyer|Cancer) completes the OAuth login dance(?: again)?$")]
async fn complete_oauth(world: &mut OidcWorld) {
    let app = world.app();
    let idp = world.idp.as_ref().expect("idp not started").clone();
    let sub = world.issued_sub.clone().expect("identity seeded");
    let email = world.issued_email.clone().expect("identity seeded");
    let name = world.issued_name.clone().expect("identity seeded");
    let (status, landing) = drive_verified_oauth(&app, &idp, &sub, &email, &name).await;
    world.callback_status = Some(status);
    world.callback_landing = landing;
}

#[then("the callback redirects with 303")]
async fn callback_redirects(world: &mut OidcWorld) {
    assert_eq!(world.callback_status, Some(StatusCode::SEE_OTHER));
}

#[then(regex = r#"^the callback lands on "([^"]+)"$"#)]
async fn callback_lands_on(world: &mut OidcWorld, expected: String) {
    assert_eq!(
        world.callback_landing.as_deref(),
        Some(expected.as_str()),
        "a firm tier lands on the team home and a client on their matters",
    );
}

/// Every row this scenario's identity owns.
///
/// Scoped to the issued email rather than reading the whole table: the
/// person directory lives in `SurrealDB` (#1093; ENG-19) and the cucumber
/// suites share one engine across concurrently-running scenarios, so a
/// table-wide count would see a sibling scenario's rows. What each
/// assertion below means is "for *this* identity" — one row, or none.
async fn rows_for_this_identity(world: &OidcWorld) -> Vec<store::persons::Person> {
    let email = world.issued_email.as_deref().expect("an issued email");
    store::persons::find_by_email_ci(&features::shared_surreal().await, email)
        .await
        .expect("person lookup")
        .into_iter()
        .collect()
}

#[then(regex = r"^exactly (\d+) persons rows? exists?$")]
async fn count_persons(world: &mut OidcWorld, expected: usize) {
    let rows = rows_for_this_identity(world).await;
    assert_eq!(rows.len(), expected, "rows: {rows:?}");
}

#[then(regex = r#"^the persons row has oidc_subject "([^"]+)"$"#)]
async fn assert_subject(world: &mut OidcWorld, expected: String) {
    let rows = rows_for_this_identity(world).await;
    let row = rows.first().expect("at least one persons row");
    assert_eq!(row.oidc_subject.as_deref(), Some(expected.as_str()));
}

#[then(regex = r#"^the persons row has email "([^"]+)"$"#)]
async fn assert_email(world: &mut OidcWorld, expected: String) {
    let rows = rows_for_this_identity(world).await;
    let row = rows.first().expect("at least one persons row");
    assert_eq!(row.email, expected);
}

#[then(regex = r#"^the persons row has name "([^"]+)"$"#)]
async fn assert_name(world: &mut OidcWorld, expected: String) {
    let rows = rows_for_this_identity(world).await;
    let row = rows.first().expect("at least one persons row");
    assert_eq!(row.name, expected);
}

#[then(regex = r#"^the persons row keeps the "([^"]+)" role$"#)]
async fn assert_role_preserved(world: &mut OidcWorld, role: String) {
    let expected = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
        _ => store::persons::Role::Client,
    };
    let rows = rows_for_this_identity(world).await;
    let row = rows.first().expect("at least one persons row");
    assert_eq!(row.role, expected);
}

#[tokio::main]
async fn main() {
    OidcWorld::cucumber()
        .run_and_exit("tests/features/oidc_callback.feature")
        .await;
}
