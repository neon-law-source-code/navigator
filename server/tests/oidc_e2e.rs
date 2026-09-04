#![allow(clippy::doc_markdown)]
//! End-to-end OIDC + embedded Rego + persons-upsert integration test.
//!
//! This is the test the user demanded — a single test that exercises
//! the *entire* authentication and authorization pipeline against
//! a mocked IdP and the compiled policy, then asserts on the database state
//! the flow produced:
//!
//! 1. Start a `wiremock` `MockServer` and program it to behave like
//!    Rauthy's `/token` endpoint, returning an id_token with `sub`,
//!    `email`, and `name` (the tier lives on the persons row, not the
//!    token).
//! 2. Compile the checked-in Rego policy.
//! 3. Build the real composed router, sharing the test sessions
//!    store, an in-memory SQLite (with migrations applied), and an
//!    OAuth config pointed at the IdP mock.
//! 4. Hit `/auth/login?return_to=/app/admin/entities`, follow the redirect
//!    back to `/auth/callback`, then hit `/app/admin/entities` with the
//!    resulting session cookie and an `Authorization: Bearer …`
//!    that satisfies the existing bearer-token middleware.
//! 5. Assert:
//!    - the callback created a `persons` row keyed on the OIDC
//!      subject, with the email and name from the id_token;
//!    - the admin route returned 200 (policy allowed);
//!    - a Client role is denied by the same policy and the same request
//!      return 403.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::{policy, AppState, AuthConfig, OAuthConfig, SessionStore};
use serde_json::json;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

/// Assemble the AppState for one test, sharing one store so
/// `/auth/callback` can write a row that the assertions below read back.
async fn state(
    oauth_cfg: OAuthConfig,
    sessions_store: SessionStore,
    policy_client: policy::PolicyClient,
) -> (AppState, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let state = AppState {
        // bearer-token middleware in passthrough mode (no JWT
        // verification) so the test can focus on the OIDC flow.
        auth: AuthConfig::new(false, None),
        sessions: sessions_store,
        oauth: Some(oauth_cfg),
        storage: Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-oidc-e2e"))
                .await
                .unwrap(),
        ),
        policy: policy_client,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (state, surreal)
}

/// OAuth `client_id` every test uses; the verifier is pinned to it.
const CLIENT_ID: &str = "navigator-web";

/// An IdP mock plus the identity it will assert. The signed-token mock
/// is mounted lazily by [`complete_oauth_flow`] / [`callback_response`]
/// once they know the login's per-request `nonce`, so the token can
/// carry it and pass full verification.
struct TestIdp {
    server: MockServer,
    sub: String,
    email: String,
    name: String,
}

impl TestIdp {
    fn uri(&self) -> String {
        self.server.uri()
    }
}

/// Start an IdP mock that will assert the given identity. The role is
/// *intentionally* never in the token — it lives in the DB.
async fn idp_returning(sub: &str, email: &str, name: &str) -> TestIdp {
    TestIdp {
        server: MockServer::start().await,
        sub: sub.into(),
        email: email.into(),
        name: name.into(),
    }
}

/// Mount `idp`'s `/token` endpoint to return a properly-signed id_token
/// that carries `nonce` (so it passes signature + iss/aud/nonce checks).
/// Resets first so a repeated login on the same mock can't return a
/// stale nonce.
async fn mount_token_endpoint(idp: &TestIdp, nonce: &str) {
    idp.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": portal::test_support::sign_id_token(
                CLIENT_ID, nonce, &idp.sub, &idp.email, &idp.name,
            ),
            "token_type": "Bearer",
        })))
        .mount(&idp.server)
        .await;
}

/// Wrap `OAuthConfig::new` with the test id_token verifier pinned to
/// [`CLIENT_ID`], pointed at `idp`.
fn oauth_cfg(idp: &TestIdp) -> OAuthConfig {
    portal::test_support::oauth_config_with_verifier(
        OAuthConfig::new(
            CLIENT_ID,
            "navigator-web-secret",
            "http://app.test/auth/callback",
            format!("{}/authorize", idp.uri()),
            format!("{}/token", idp.uri()),
        ),
        CLIENT_ID,
    )
}

/// Insert a `persons` row up-front so a downstream `/auth/callback`
/// can promote (link the `oidc_subject`) rather than 403. Sign-up is
/// operator-mediated — every test that drives the callback to a
/// successful session has to call this first.
async fn seed_person(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    name: &str,
    role: store::persons::Role,
) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, role),
    )
    .await
    .expect("seed person");
}

/// `/auth/login` → extract state/nonce → drive `/auth/callback`, mounting
/// the signed-token endpoint with the login's nonce. Returns the raw
/// callback response so callers can assert success *or* failure.
async fn drive_callback(
    app: &axum::Router,
    idp: &TestIdp,
    return_to: &str,
) -> axum::http::Response<Body> {
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/login?return_to={return_to}"))
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
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    mount_token_endpoint(idp, &nonce).await;

    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=any-code&state={state_param}"))
                .header("cookie", &pre_auth_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Drive the full flow and return the session cookie, asserting the
/// callback succeeded (SEE_OTHER).
async fn complete_oauth_flow(app: &axum::Router, idp: &TestIdp, return_to: &str) -> String {
    let cb = drive_callback(app, idp, return_to).await;
    assert_eq!(cb.status(), StatusCode::SEE_OTHER);
    cb.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.contains("navigator_session="))
        .unwrap_or_else(|| panic!("expected navigator_session cookie in callback response"))
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn full_oidc_flow_upserts_person_and_allows_lawyer() {
    // ----- IdP mock (Rauthy stand-in) -----
    let idp = idp_returning("rauthy-lawyer-subject", "lawyer@neonlaw.com", "Lawyer").await;

    let policy_client = policy::PolicyClient::embedded().expect("embedded policy compiles");

    let (state, surreal) = state(oauth_cfg(&idp), sessions(), policy_client).await;
    // Pre-seed Lawyer — sign-up is operator-mediated. The callback
    // promotes (links `oidc_subject`) instead of inserting.
    seed_person(
        &surreal,
        "lawyer@neonlaw.com",
        "Lawyer",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // 1. Drive the OAuth dance.
    let session_cookie = complete_oauth_flow(&app, &idp, "/app/admin/entities").await;

    // 2. The callback should have promoted the pre-seeded row by
    //    stamping the IdP subject. Email + name stay as seeded; the
    //    callback never overwrites them from the token.
    let persons = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(persons.len(), 1, "expected exactly one persons row");
    assert_eq!(
        persons[0].oidc_subject.as_deref(),
        Some("rauthy-lawyer-subject")
    );
    assert_eq!(persons[0].email, "lawyer@neonlaw.com");
    assert_eq!(persons[0].name, "Lawyer");

    // 3. The embedded policy allows Lawyer to view /app/admin/entities.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/entities")
                .header("cookie", &session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "admin request must succeed under allow; got {}",
        resp.status(),
    );
}

#[tokio::test]
async fn embedded_policy_denies_client_admin_route_with_403() {
    // Same IdP mock — successful login still happens.
    let idp = idp_returning("rauthy-taurus-subject", "taurus@example.com", "Taurus").await;

    let policy_client = policy::PolicyClient::embedded().expect("embedded policy compiles");

    let (state, surreal) = state(oauth_cfg(&idp), sessions(), policy_client).await;
    // Taurus is pre-seeded as a Client. They can sign in — sign-in only
    // checks that the persons row exists — but the policy
    // then blocks every protected route.
    seed_person(
        &surreal,
        "taurus@example.com",
        "Taurus",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let session_cookie = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    // Promotion happened — the row gained `oidc_subject` but kept
    // its Client tier.
    let persons = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(persons.len(), 1);
    assert_eq!(
        persons[0].oidc_subject.as_deref(),
        Some("rauthy-taurus-subject")
    );

    // ...but the policy denies, so the admin route returns 403.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("cookie", &session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "admin request must be 403 under deny; got {}",
        resp.status(),
    );
}

#[tokio::test]
async fn second_login_with_same_subject_does_not_create_duplicate_person() {
    let idp = idp_returning("rauthy-lawyer-subject", "lawyer@neonlaw.com", "Lawyer").await;
    let (state, surreal) = state(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
    )
    .await;
    seed_person(
        &surreal,
        "lawyer@neonlaw.com",
        "Lawyer",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let _ = complete_oauth_flow(&app, &idp, "/app/lawyer").await;
    let _ = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    let persons = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(
        persons.len(),
        1,
        "two logins with the same `sub` must produce one person row, got {}",
        persons.len(),
    );
}

// ---------- DB-sourced role gating across multiple admin routes ----------
//
// The next two tests prove the architectural claim documented in
// `docs/oidc.md` + `docs/access-model.md`: the IdP token *cannot*
// grant access on its own. The system-wide tier lives on
// `persons.role` and is read into the session at callback time. The policy
// evaluates `input.session.role`, which therefore reflects whatever
// the DB says regardless of what the IdP claimed.

const ADMIN_ROUTES: &[&str] = &[
    "/app/lawyer",
    // `/app/lawyer/people` is absent since ENG-304 deleted the browser mirror: the
    // one people surface is the admin console's `/app/admin/people`, which this
    // lawyer-tier walk is answered 403 at by design.
    "/app/admin/entities",
    "/app/admin/jurisdictions",
    "/app/admin/entity-types",
    "/app/admin/templates",
    "/app/admin/questions",
];

/// The matter surface is not an admin route. Every authenticated tier enters
/// `/app/projects`, including a client — the policy cannot make the
/// firm/client split because it cannot read the participation ledger, so the
/// handler makes it and the *rows* differ rather than the status.
#[tokio::test]
async fn a_client_reaches_the_matter_surface_the_admin_routes_deny_them() {
    let idp = idp_returning("rauthy-libra-subject", "libra@example.com", "Libra").await;
    let (state, surreal) = state(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
    )
    .await;
    seed_person(
        &surreal,
        "libra@example.com",
        "Libra",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let session_cookie = complete_oauth_flow(&app, &idp, "/app/projects").await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("cookie", &session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a client enters the one matter surface; scoping happens in the handler"
    );
}

#[tokio::test]
async fn user_with_db_lawyer_role_can_hit_every_admin_route() {
    let idp = idp_returning("rauthy-lawyer-subject", "lawyer@neonlaw.com", "Lawyer").await;
    let policy_client = policy::PolicyClient::embedded().expect("embedded policy compiles");

    let (state, surreal) = state(oauth_cfg(&idp), sessions(), policy_client).await;

    // Pre-seed the persons row with email + lawyer role. The OAuth
    // callback will *promote* this row when Lawyer logs in for the
    // first time (link by email, stamp the subject).
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let session_cookie = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    // The promoted row now has the OIDC subject linked.
    let lawyer = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.email == "lawyer@neonlaw.com")
        .unwrap();
    assert_eq!(
        lawyer.oidc_subject.as_deref(),
        Some("rauthy-lawyer-subject")
    );
    assert_eq!(
        lawyer.role,
        store::persons::Role::Lawyer,
        "the seeded role must survive the promotion",
    );

    // Hit every admin GET route — each should pass the DB-role gate
    // and render with HTTP 200.
    for route in ADMIN_ROUTES {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(*route)
                    .header("cookie", &session_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{route} must succeed under DB-lawyer role; got {}",
            resp.status(),
        );
    }
}

#[tokio::test]
async fn user_with_client_role_is_denied_from_admin_routes() {
    // Cancer is pre-seeded as a Client. The IdP says nothing about a
    // tier (the callback ignores any claim anyway). After the
    // callback, `persons.role = 'client'`. Every admin route must 403.
    let idp = idp_returning("rauthy-cancer-subject", "cancer@example.com", "Cancer").await;
    let (state, surreal) = state(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
    )
    .await;
    seed_person(
        &surreal,
        "cancer@example.com",
        "Cancer",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let session_cookie = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    let cancer = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.email == "cancer@example.com")
        .unwrap();
    assert_eq!(
        cancer.role,
        store::persons::Role::Client,
        "seeded Client tier must persist across login",
    );

    for route in ADMIN_ROUTES {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(*route)
                    .header("cookie", &session_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{route} must 403 for a Client-tier user; got {}",
            resp.status(),
        );
    }
}

#[tokio::test]
async fn db_role_revocation_takes_effect_on_next_login() {
    // Lawyer starts as Lawyer, logs in, hits admin (success). Then an
    // admin demotes them to Client. The *existing* session keeps
    // working (sessions are signed snapshots), but their next login
    // picks up the Client tier and admin starts returning 403.
    let idp = idp_returning("rauthy-lawyer-subject", "lawyer@neonlaw.com", "Lawyer").await;
    let (state, surreal) = state(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
    )
    .await;

    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let first_session = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("cookie", &first_session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Revoke the role.
    let lawyer = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.email == "lawyer@neonlaw.com")
        .unwrap();
    store::persons::set_role(&surreal, lawyer.id, store::persons::Role::Client)
        .await
        .unwrap();

    // Next login picks up the Client tier → 403.
    let second_session = complete_oauth_flow(&app, &idp, "/app/lawyer").await;
    let second = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("cookie", &second_session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::FORBIDDEN,
        "after DB role revocation, the next session must be denied; got {}",
        second.status(),
    );
}

// ---------- Pre-seed requirement ----------

/// Drive `/auth/login` → `/auth/callback` and return the raw callback
/// response. Used by tests that expect the callback to *fail* (no
/// pre-seeded persons row) — `complete_oauth_flow` asserts SEE_OTHER
/// on the callback, which is exactly what we don't want here.
async fn callback_response(
    app: &axum::Router,
    idp: &TestIdp,
    return_to: &str,
) -> axum::http::Response<Body> {
    drive_callback(app, idp, return_to).await
}

/// Build an `AppState` that already carries the supplied
/// `bootstrap_owner_email`, sharing `db` with the test so assertions can
/// read back what the callback wrote.
async fn state_with_bootstrap_owner(
    oauth_cfg: OAuthConfig,
    sessions_store: SessionStore,
    policy_client: policy::PolicyClient,
    bootstrap_owner: Option<String>,
) -> (AppState, store::surreal::SurrealDb) {
    let (mut s, surreal) = state(oauth_cfg, sessions_store, policy_client).await;
    s.bootstrap_owner_email = bootstrap_owner;
    (s, surreal)
}

#[tokio::test]
async fn callback_returns_403_html_when_email_is_not_pre_seeded() {
    // Scorpio logs in with a perfectly valid id_token but no operator
    // has ever inserted a `persons` row for `scorpio@example.com`. The
    // callback must refuse to mint a session and render the styled
    // 403 page instead — sign-up is operator-mediated by design.
    //
    // The page is the sign-in-specific one, not the generic Forbidden: Scorpio
    // is not a misconfigured account but someone who has never engaged the
    // firm, and "not authorized" would tell them nothing about what to do.
    let idp = idp_returning("rauthy-scorpio-subject", "scorpio@example.com", "Scorpio").await;
    let (s, surreal) = state_with_bootstrap_owner(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
        // Bootstrap Owner override deliberately points elsewhere so scorpio@
        // is NOT the carve-out.
        Some("nobody@unreachable.invalid".into()),
    )
    .await;
    let app = server::neon_router(s, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = callback_response(&app, &idp, "/app/lawyer").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&body);
    assert!(html.starts_with("<!DOCTYPE html>"), "got: {html}");
    assert!(
        html.contains("already engaged"),
        "the denial must name the precondition rather than say \"not authorized\": {html}",
    );
    // The CTA's own text: the site header's nav carries `href="/contact"` too,
    // so matching the bare link would pass with the button deleted.
    assert!(
        html.contains(">Contact us<"),
        "the denial must offer a way to get in touch: {html}",
    );

    // No persons row created — operator must seed first.
    let persons = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert!(
        persons.is_empty(),
        "callback must not create a row when sign-up is operator-mediated; got {persons:?}",
    );
}

#[tokio::test]
async fn a_retained_but_unadmitted_client_row_cannot_mint_a_session() {
    // This is the post-discard shape: the retainer walk leaves the client
    // Person row behind, but the admission decision is explicitly off. A
    // valid IdP token must not turn that retained row into a client session.
    let idp = idp_returning(
        "rauthy-refused-intake-subject",
        "refused-intake@example.com",
        "Refused Intake",
    )
    .await;
    let (s, surreal) = state_with_bootstrap_owner(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
        Some("nobody@unreachable.invalid".into()),
    )
    .await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Refused Intake",
            "refused-intake@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .expect("seed retained person");
    surreal
        .query("UPDATE $id SET is_admitted = false")
        .bind(("id", store::surreal::record_id("person", person.id)))
        .await
        .expect("mark retained person unadmitted");

    let app = server::neon_router(s, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = callback_response(&app, &idp, "/app/projects").await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        !response
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value.to_str().unwrap().contains("navigator_session=")),
        "an unadmitted row must not receive a session cookie",
    );
    let retained = store::persons::find_by_id(&surreal, person.id)
        .await
        .unwrap()
        .expect("retained person remains in the directory");
    assert_eq!(retained.role, store::persons::Role::Client);
    assert_eq!(retained.oidc_subject, None);
}

#[tokio::test]
async fn callback_jit_creates_bootstrap_owner_with_owner_role_when_absent() {
    // The bootstrap Owner email is configured via env. If that Owner
    // signs in to a fresh deployment where no `persons` row exists
    // yet, the callback JIT-creates the row WITH the `owner` role.
    // This is the sole carve-out from the pre-seed rule — it exists
    // so a brand-new cluster can never lock its operator out.
    let idp = idp_returning("rauthy-nick-subject", "nick@neonlaw.com", "Nick").await;
    let (s, surreal) = state_with_bootstrap_owner(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
        Some("nick@neonlaw.com".into()),
    )
    .await;
    let app = server::neon_router(s, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let _ = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    let persons = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(persons.len(), 1, "bootstrap Owner row was JIT-created");
    assert_eq!(persons[0].email, "nick@neonlaw.com");
    assert_eq!(persons[0].role, store::persons::Role::Owner);
}

#[tokio::test]
async fn bootstrap_owner_role_heals_back_after_being_cleared() {
    // The bootstrap Owner email is "always owner" — even if an
    // administrator demotes the row in the UI, the next sign-in
    // restores `owner`. Belt-and-suspenders: a fork's Owner
    // cannot accidentally lock themselves out.
    let idp = idp_returning("rauthy-nick-subject", "nick@neonlaw.com", "Nick").await;
    let (s, surreal) = state_with_bootstrap_owner(
        oauth_cfg(&idp),
        sessions(),
        policy::PolicyClient::embedded().expect("embedded policy compiles"),
        Some("nick@neonlaw.com".into()),
    )
    .await;
    // Pre-seed as Client — simulating a malicious or mistaken
    // demotion of the bootstrap Owner row.
    seed_person(
        &surreal,
        "nick@neonlaw.com",
        "Nick",
        store::persons::Role::Client,
    )
    .await;
    let app = server::neon_router(s, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let _ = complete_oauth_flow(&app, &idp, "/app/lawyer").await;

    let row = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.email == "nick@neonlaw.com")
        .unwrap();
    assert_eq!(
        row.role,
        store::persons::Role::Owner,
        "bootstrap Owner role must heal back after sign-in",
    );
}
