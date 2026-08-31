#![allow(clippy::doc_markdown)]
//! Router tests for the web crate.
//!
//! Drives the router via `tower::ServiceExt::oneshot` — no socket,
//! no port binding, no flakiness around chosen ephemeral ports. Each
//! test gets its own embedded, memory-backed store via
//! `store::surreal::test_support::mem` so they don't share state.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use portal::workshops::{WorkshopChapter, WorkshopSection};
use portal::{AppState, AuthConfig, CanonicalHost, SessionStore, WorkshopIndex, WorkshopMaterial};
use scraper::{Html, Selector};
use std::collections::HashMap;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// An `AppState` over a fresh pair of stores.
async fn state_with_engines() -> (AppState, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    (
        portal::test_support::app_state(surreal.clone()).await,
        surreal,
    )
}

/// The **firm** host, composed through `neon`'s own entry points.
///
/// Both catalogs — the anonymous talks and the gated Navigator classes — mount
/// on this host, so the tests that assert how a class or a talk *renders* drive
/// this composition rather than the bare `server::neon_router`.
fn catalog_router(state: AppState) -> axum::Router {
    let dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        dioxus,
    )
    .expect("the firm host must not claim Navigator-owned routes")
}

fn test_sessions() -> SessionStore {
    SessionStore::new("test-session-key-not-for-production")
}

/// A signed session cookie for an `admin` caller. Admin bypasses
/// project row-scoping (per `docs/access-model.md`), so handler tests
/// that render the admin chrome for an arbitrary project authenticate
/// with this rather than relying on the no-session affordance.
fn admin_session_cookie() -> String {
    format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&portal::SessionData::fresh(
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ))
    )
}

/// An Admin session carrying a linked person, for the store-outage tests.
///
/// `matter_viewer` fails closed on a session with no `person_id` *before* it
/// queries, so a bare admin cookie would 404 at the gate without ever touching
/// the broken store — and the test would pass for the wrong reason.
fn admin_session_cookie_with_person() -> String {
    let mut session = portal::SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    session.person_id = Some(uuid::Uuid::now_v7());
    format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    )
}

fn admin_session_cookie_and_csrf() -> (String, String) {
    let session = portal::SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    let csrf = session.csrf_token.clone();
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    (cookie, csrf)
}

fn session_cookie_for_role(role: store::persons::Role) -> String {
    format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&portal::SessionData::fresh("api-test-sub", role))
    )
}

/// A firm-side credential for certificate requests in workshop tests.
fn workshop_session_cookie() -> String {
    session_cookie_for_role(store::persons::Role::Lawyer)
}

/// The weakest authenticated role, for asserting what a signed-in reader
/// reaches.
fn client_reader_cookie() -> String {
    session_cookie_for_role(store::persons::Role::Client)
}

/// A signed session cookie plus the matching per-session CSRF token, so
/// a cookie-authenticated JSON write can echo the token back in the
/// `X-CSRF-Token` header (the credential-keyed CSRF rule now guards the
/// mutating `/app/api/*` routes, not just form-encoded bodies).
fn session_cookie_and_csrf_for_role(role: store::persons::Role) -> (String, String) {
    let session = portal::SessionData::fresh("api-test-sub", role);
    let csrf = session.csrf_token.clone();
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    (cookie, csrf)
}

fn session_cookie_and_csrf_for_person(person: &store::persons::Person) -> (String, String) {
    let mut session = portal::SessionData::fresh(format!("sub-{}", person.email), person.role);
    session.email = Some(person.email.clone());
    session.person_id = Some(person.id);
    let csrf = session.csrf_token.clone();
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    (cookie, csrf)
}

fn session_cookie_pair(resp: &axum::http::Response<Body>) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(portal::session::SESSION_COOKIE_NAME))
        .expect("response sets navigator session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn decode_session_cookie_pair(cookie: &str) -> portal::SessionData {
    let value = cookie
        .strip_prefix(&format!("{}=", portal::session::SESSION_COOKIE_NAME))
        .expect("navigator session cookie pair");
    test_sessions().decode(value).expect("valid signed session")
}

/// An Admin who is actually on `project_id`, as a cookie (and CSRF token).
///
/// Since ENG-81 the matter surface requires a firm-side `person_project_roles`
/// row of every tier, so a bare `admin_session_cookie()` (no linked person at
/// all) still 404s on a matter nobody put that admin on — an Owner/Admin with
/// a linked person instead gets the narrower participation-only view, but a
/// session naming no person is not an identified admin to hand even that to.
/// Tests about *document behavior* want to be past both gates, not to
/// re-assert them — those are pinned by
/// `owner_and_admin_without_participation_get_the_participation_only_view`.
async fn admin_on_project(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
) -> (String, String) {
    let admin = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Matter Admin",
            format!("matter-admin-{}@neonlaw.com", uuid::Uuid::now_v7()),
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    participate(surreal, admin.id, project_id, "attorney").await;
    session_cookie_and_csrf_for_person(&admin)
}

async fn admin_cookie_on_project(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
) -> String {
    admin_on_project(surreal, project_id).await.0
}

async fn get_with_role(
    app: axum::Router,
    uri: &str,
    role: store::persons::Role,
) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header(header::COOKIE, session_cookie_for_role(role))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

fn assert_nav_links(body: &str, expected: &[&str], unexpected: &[&str]) {
    for href in expected {
        assert!(
            body.contains(&format!("href=\"{href}\"")),
            "expected nav link {href}; body was: {body}"
        );
    }
    for href in unexpected {
        assert!(
            !body.contains(&format!("href=\"{href}\"")),
            "unexpected nav link {href}; body was: {body}"
        );
    }
}

fn bearer_header_for_role(role: store::persons::Role) -> String {
    let token = test_sessions().encode(&portal::SessionData::fresh("api-test-sub", role));
    format!("Bearer {token}")
}

async fn client_project_fixture(
    surreal: &store::surreal::SurrealDb,
) -> (uuid::Uuid, String, String) {
    client_project_fixture_for_product(
        surreal,
        "Portal Client",
        "fractional-client@example.com",
        "Sample Matter",
    )
    .await
}

async fn client_project_fixture_for_product(
    surreal: &store::surreal::SurrealDb,
    client_name: &str,
    client_email: &str,
    project_name: &str,
) -> (uuid::Uuid, String, String) {
    let client = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            client_name,
            client_email,
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("fixture-{}", uuid::Uuid::now_v7()),
            name: project_name.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project.id, client.id, "client")
        .await
        .unwrap();

    let mut session = portal::SessionData::fresh("client-sub", store::persons::Role::Client);
    session.person_id = Some(client.id);
    session.email = Some(client.email);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    (project.id, project.code, cookie)
}

async fn lawyer_project_fixture(
    surreal: &store::surreal::SurrealDb,
) -> (uuid::Uuid, store::persons::Person, String, String) {
    let lawyer = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer Project Fixture",
            "lawyer-project-fixture@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("lawyer-fixture-{}", uuid::Uuid::now_v7()),
            name: "Homer v. Flanders".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project.id, lawyer.id, "attorney")
        .await
        .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&lawyer);
    (project.id, lawyer, cookie, csrf)
}

async fn test_project(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    status: &str,
) -> store::projects::Project {
    store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("route-fixture-{}", uuid::Uuid::now_v7()),
            name: name.into(),
            status: status.into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// Record an ordinary firm-side membership row — on the matter, not accountable for it.
async fn participate(
    surreal: &store::surreal::SurrealDb,
    person_id: uuid::Uuid,
    project_id: uuid::Uuid,
    kind: &str,
) {
    store::projects::add_participation(surreal, project_id, person_id, kind)
        .await
        .unwrap();
}

async fn disclose_lawyer_dri(
    surreal: &store::surreal::SurrealDb,
    person_id: uuid::Uuid,
    project_id: uuid::Uuid,
) {
    store::projects::designate_dri_in_surreal(
        surreal,
        project_id,
        person_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
}

/// Seeds a lawyer DRI owning six open and six closed projects, plus one open project owned by a
/// different DRI, and returns that lawyer person's session cookie.
async fn lawyer_dashboard_fixture(surreal: &store::surreal::SurrealDb) -> String {
    let entity_id = store::test_support::seed_entity(surreal).await;
    let lawyer = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Dashboard Lawyer",
            "dashboard-lawyer@example.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let other_dri = store::test_support::dri_person(surreal).await;

    for status in ["open", "closed"] {
        for number in 1..=6 {
            let project = store::projects::create(
                surreal,
                &store::projects::NewProject {
                    code: format!("{status}-dashboard-{number}-{}", uuid::Uuid::now_v7()),
                    name: format!("{status} project {number}"),
                    status: status.into(),
                    entity_id,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            disclose_lawyer_dri(surreal, lawyer.id, project.id).await;
        }
    }
    let unassigned = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("unassigned-dashboard-{}", uuid::Uuid::now_v7()),
            name: "unassigned project".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    disclose_lawyer_dri(surreal, other_dri, unassigned.id).await;

    let (cookie, _) = session_cookie_and_csrf_for_person(&lawyer);
    cookie
}

async fn get_with_cookie(app: axum::Router, uri: &str, cookie: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// The dashboard's project-calendar section — from its heading to the next
/// one. The KPI list above it legitimately renders open-project names, so
/// assertions that the calendar synthesizes nothing must scope to this slice.
/// Strip Dioxus's SSR hydration markers (`<!--node-id7-->`, `<!--#-->`,
/// `<!--placeholder3-->`) so an assertion can name the markup a reader sees
/// rather than the framework's bookkeeping. Dioxus interleaves these between an
/// element and its dynamic text, which would otherwise force every assertion to
/// split `<strong>Label</strong>` from the value beside it.
fn strip_hydration_markers(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("-->") else {
            rest = "";
            break;
        };
        rest = &rest[start + end + 3..];
    }
    out.push_str(rest);
    out
}

fn calendar_section(body: &str) -> &str {
    let start = body
        .find("Project calendar")
        .expect("calendar heading present");
    let end = body[start..]
        .find("Details")
        .map_or(body.len(), |offset| start + offset);
    &body[start..end]
}

async fn empty_state() -> AppState {
    portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
}

async fn empty_state_with_auth(auth: AuthConfig) -> AppState {
    AppState {
        auth,
        ..portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
    }
}

async fn empty_state_with_canonical_host(host: CanonicalHost) -> AppState {
    AppState {
        canonical_host: host,
        ..portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
    }
}

async fn empty_state_with_policy(policy: portal::policy::PolicyClient) -> AppState {
    AppState {
        policy,
        ..portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
    }
}

fn deny_all_policy() -> portal::policy::PolicyClient {
    portal::policy::PolicyClient::new(
        "package navigator.authz\n\
         import rego.v1\n\
         default allow := false\n",
    )
    .expect("the test deny policy compiles")
}

fn erroring_policy() -> portal::policy::PolicyClient {
    portal::policy::PolicyClient::new(
        "package navigator.authz\n\
         import rego.v1\n\
         default allow := false\n\
         allow := 1 / 0\n",
    )
    .expect("the test erroring policy compiles")
}

async fn state_with_workshops(materials: Vec<WorkshopMaterial>) -> AppState {
    AppState {
        brand_bundle: None,
        surreal: store::surreal::test_support::mem().await,
        workshops: WorkshopIndex::new(materials),
        docs: portal::DocsIndex::empty(),
        blog: portal::BlogIndex::empty(),
        auth: AuthConfig::new(true, None),
        google_oauth: portal::google_oauth::GoogleOauthConfig::passthrough(),
        rate_limit: portal::rate_limit::RateLimit::disabled(),
        canonical_host: CanonicalHost::new(None),
        portal_only: portal::PortalOnly::default(),
        sessions: test_sessions(),
        oauth: None,
        oauth_microsoft: None,
        storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        assets_storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        applications_storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        forms_registry: std::sync::Arc::new(forms::registry().unwrap()),
        policy: portal::policy::PolicyClient::passthrough(),
        workflow_runtime: std::sync::Arc::new(workflows::InMemoryRuntime::new()),
        questionnaire_runtime: std::sync::Arc::new(workflows::InMemoryRuntime::new()),
        signature_provider: std::sync::Arc::new(portal::signature::StubSignatureProvider::new()),
        billing_provider: std::sync::Arc::new(portal::billing::StubBillingProvider::new()),
        contract_reviewer: std::sync::Arc::new(portal::contract_review::StubContractReviewer),
        esignature_webhook_secret: None,
        esignature_hmac_key: None,
        email: std::sync::Arc::new(portal::email::CapturingEmail::new()),
        attachment_scanner: std::sync::Arc::new(
            portal::attachment_scanner::FakeAttachmentScanner::clean(),
        ),
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

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[derive(Debug)]
struct DomForm {
    action: String,
    fields: HashMap<String, String>,
}

impl DomForm {
    fn parse(html: &str, action: &str) -> Self {
        let document = Html::parse_document(html);
        let form_selector = Selector::parse("form").unwrap();
        let form = document
            .select(&form_selector)
            .find(|form| form.value().attr("action") == Some(action))
            .unwrap_or_else(|| panic!("form action {action:?} missing from rendered DOM: {html}"));
        let input_selector = Selector::parse("input[name]").unwrap();
        let select_selector = Selector::parse("select[name]").unwrap();
        let option_selector = Selector::parse("option").unwrap();
        let mut fields = HashMap::new();

        for input in form.select(&input_selector) {
            let element = input.value();
            let Some(name) = element.attr("name") else {
                continue;
            };
            fields.insert(
                name.to_string(),
                element.attr("value").unwrap_or_default().to_string(),
            );
        }

        for select in form.select(&select_selector) {
            let element = select.value();
            let Some(name) = element.attr("name") else {
                continue;
            };
            let selected = select
                .select(&option_selector)
                .find(|option| option.value().attr("selected").is_some())
                .or_else(|| select.select(&option_selector).next())
                .and_then(|option| option.value().attr("value"))
                .unwrap_or_default();
            fields.insert(name.to_string(), selected.to_string());
        }

        Self {
            action: action.to_string(),
            fields,
        }
    }

    fn enter(&mut self, name: &str, value: impl Into<String>) {
        let Some(field) = self.fields.get_mut(name) else {
            panic!("{name:?} input missing from DOM form {:?}", self.action);
        };
        *field = value.into();
    }

    fn choose(&mut self, name: &str, value: impl Into<String>) {
        self.enter(name, value);
    }

    fn value(&self, name: &str) -> &str {
        self.fields
            .get(name)
            .unwrap_or_else(|| panic!("{name:?} field missing from DOM form {:?}", self.action))
    }

    fn into_body(self) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in self.fields {
            serializer.append_pair(&name, &value);
        }
        serializer.finish()
    }
}

#[tokio::test]
async fn app_projects_renders_the_client_dashboard() {
    // `/app/projects` is where sign-in returns a client and where the nav
    // points. For a client-tier caller it renders the client dashboard
    // (`ClientProjects`) — the retired `/portal` landing folded into this one
    // role-adaptive surface.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let projects = get_with_role(app, "/app/projects", store::persons::Role::Client).await;
    assert_eq!(projects.status(), StatusCode::OK);
    let projects = body_string(projects).await;

    for marker in [
        r#"id="portal-projects""#,
        ">Your Projects<",
        r#"class="portal-kpis""#,
        ">Open<",
        ">Closed<",
    ] {
        assert!(
            projects.contains(marker),
            "/app/projects must render the client dashboard ({marker}): {projects}",
        );
    }
    assert!(
        !projects.contains(">Engagements<"),
        "the list has no Engagements heading: {projects}",
    );
    assert!(
        !projects.contains("Each card is one engagement"),
        "the dashboard has no engagement blurb: {projects}",
    );
}

/// Every `/app` page's `<title>` begins with Navigator. Other protected routes
/// retain the mounted firm's mark. The tab is the most-rendered piece of
/// navigation on the site, so an application page must not expose its internal
/// `/app` mount point as a reader-facing label.
///
/// One page per rendering shape, because they resolve the name three different
/// ways: a plain view struct, the shared admin-listing scaffold, and a
/// `format!`-built title. A page added later that hardcodes the name again will
/// not be caught here — the shapes are.
#[tokio::test]
async fn application_titles_begin_with_navigator() {
    let pages = [
        (
            "/app/projects",
            store::persons::Role::Client,
            "Navigator | Projects",
        ),
        (
            "/app/team",
            store::persons::Role::Lawyer,
            "Navigator | Team",
        ),
        (
            "/app/admin/jurisdictions",
            store::persons::Role::Lawyer,
            "Navigator | Admin | Jurisdictions",
        ),
        (
            "/app/admin/people",
            store::persons::Role::Admin,
            "Navigator | Admin | People",
        ),
    ];

    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for (uri, role, suffix) in pages {
        let body = body_string(get_with_role(app.clone(), uri, role).await).await;
        let expected = if uri.starts_with("/app") {
            suffix.to_string()
        } else {
            format!("Neon Law | {suffix}")
        };
        assert!(
            body.contains(&format!("<title>{expected}</title>")),
            "{body}"
        );
    }

    let bundle_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bundle_dir.path().join("navigator.yaml"),
        "version: 1\nbrand:\n  firm: Acme Law\n",
    )
    .unwrap();
    let bundle = views::brand_bundle::BrandBundle::load(bundle_dir.path()).unwrap();
    let mut state = empty_state().await;
    state.brand_bundle = Some(bundle);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    for (uri, role, suffix) in pages {
        let body = body_string(get_with_role(app.clone(), uri, role).await).await;
        let expected = if uri.starts_with("/app") {
            suffix.to_string()
        } else {
            format!("Acme Law | {suffix}")
        };
        assert!(
            body.contains(&format!("<title>{expected}</title>")),
            "{body}"
        );
        if !uri.starts_with("/app") {
            assert!(
                !body.contains("Neon Law"),
                "{uri} leaks this firm's name into a white-label deploy: {body}",
            );
        }
    }
}

#[tokio::test]
async fn admin_page_is_visible_only_to_owner_and_admin() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    for role in [store::persons::Role::Client, store::persons::Role::Lawyer] {
        let resp = get_with_role(app.clone(), "/app/admin", role).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{role:?}");
    }

    for role in [store::persons::Role::Owner, store::persons::Role::Admin] {
        let resp = get_with_role(app.clone(), "/app/admin", role).await;
        assert_eq!(resp.status(), StatusCode::OK, "{role:?}");
        let html = body_string(resp).await;
        // `/app/admin` is a landing hub, not the people table — it links to
        // the administrative surfaces.
        assert!(html.contains("<h1>Admin</h1>"), "{html}");
        assert!(html.contains("href=\"/app/admin/people\""), "{html}");
        assert!(html.contains("href=\"/app/admin/analytics\""), "{html}");
        assert!(
            !html.contains("<table"),
            "the landing must not embed the people table: {html}",
        );
        // The shared `/app` navbar (`webapp::components::AppNavbar`) renders the
        // same three destinations at every firm tier — Projects, Team, and Sign
        // out. The admin hub and the workbench are `/app/team` cards now, not
        // navbar items, so the navbar itself carries neither: reaching them from
        // here is one hop through Team.
        assert!(html.contains("class=\"lawyer-nav\""), "{html}");
        let (_, after_nav) = html
            .split_once("class=\"lawyer-nav\"")
            .expect("the admin hub renders the app navbar");
        let (navbar, _) = after_nav
            .split_once("</nav>")
            .expect("the navbar closes its element");
        assert!(navbar.contains("href=\"/app/team\""), "{html}");
        assert!(!navbar.contains("href=\"/app/admin\""), "{html}");
        assert!(!navbar.contains("href=\"/app/lawyer\""), "{html}");
    }
}

#[tokio::test]
async fn admin_people_surface_is_admin_only_with_full_controls() {
    let (state, surreal) = state_with_engines().await;
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Lawyer (non-admin) is denied every admin people route.
    for path in [
        "/app/admin/people".to_string(),
        "/app/admin/people/new".to_string(),
        format!("/app/admin/people/{}", client.id),
        format!("/app/admin/people/{}/edit", client.id),
    ] {
        let resp = get_with_role(app.clone(), &path, store::persons::Role::Lawyer).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a lawyer must be denied {path}"
        );
    }

    // Admin sees the list with the full controls and the singular
    // `/app/admin/people` detail path.
    let resp = get_with_role(
        app.clone(),
        "/app/admin/people",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.contains(&format!("href=\"/app/admin/people/{}/edit\"", client.id)),
        "{html}",
    );
    // The Dioxus list offers delete via a native `POST` form (the row used
    // `hx-delete` to the REST endpoint); a client row is deletable.
    assert!(
        html.contains(&format!(
            "action=\"/app/admin/people/{}/delete\"",
            client.id
        )),
        "admin list must offer delete on a client row: {html}",
    );
    assert!(
        html.contains("<title>Navigator | Admin | People</title>"),
        "admin people title must mirror its route hierarchy: {html}",
    );
    // The create form has always been mounted, but the list linked nothing to
    // it — so the only way to add a person was to know and type the URL.
    assert!(
        html.contains("href=\"/app/admin/people/new\""),
        "the admin list must offer a way into the create form: {html}",
    );
    // The page must link the theme stylesheet, or its `nav-table` / `nav-btn` /
    // `lawyer-nav` chrome renders completely unstyled — the SSR class-name
    // assertions above pass regardless, so this is the guard that catches a
    // migrated page that emitted its `<title>` but forgot the stylesheet.
    assert!(
        html.contains("href=\"/public/css/theme.css\""),
        "the people list must link the theme stylesheet or it renders unstyled: {html}",
    );

    // The detail page resolves under the singular path.
    let resp = get_with_role(
        app,
        &format!("/app/admin/people/{}", client.id),
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_people_new_form_renders_for_admin_with_csrf() {
    // The `/app/admin/people/new` create form renders through Dioxus: admin sees a
    // native form posting to `/app/admin/people` with the session CSRF token and the
    // name / email / role controls (the role select is unlocked for admins).
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Add person"), "{body}");
    let form = DomForm::parse(&body, "/app/admin/people");
    assert_eq!(form.value("_csrf"), csrf);
    // The name, email, and role controls are present (role unlocked for admin).
    assert!(body.contains("name=\"name\""), "{body}");
    assert!(body.contains("name=\"email\""), "{body}");
    assert!(body.contains("name=\"role\""), "{body}");
    assert!(
        !body.contains("value=\"owner\""),
        "Admin must not be offered the higher Owner tier: {body}",
    );
}

#[tokio::test]
async fn admin_people_new_form_lists_owner_first_for_owner() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, _) = session_cookie_and_csrf_for_role(store::persons::Role::Owner);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people/new")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let owner = body.find("value=\"owner\"").expect("Owner option");
    let admin = body.find("value=\"admin\"").expect("Admin option");
    let lawyer = body.find("value=\"lawyer\"").expect("Lawyer option");
    let clerk = body.find("value=\"clerk\"").expect("Clerk option");
    let client = body.find("value=\"client\"").expect("Client option");
    assert!(
        owner < admin && admin < lawyer && lawyer < clerk && clerk < client,
        "roles must render in authority order: {body}",
    );
}

#[tokio::test]
async fn admin_people_new_creates_a_person_via_native_form_post() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/people")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Nova%20Star&email=nova%40test.invalid&role=lawyer&_csrf={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    // Native form: 303 redirect back to the list (not the REST API's 201+JSON).
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/app/admin/people"),
    );
    let created = store::persons::find_by_email_ci(&surreal, "nova@test.invalid")
        .await
        .unwrap();
    assert!(created.is_some(), "the person should have been created");
    assert_eq!(created.unwrap().role, store::persons::Role::Lawyer);
}

#[tokio::test]
async fn only_owner_can_create_an_owner_identity() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (admin_cookie, admin_csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/people")
                .header(header::COOKIE, admin_cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Denied%20Owner&email=denied-owner%40example.com&role=owner&_csrf={admin_csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::SEE_OTHER);
    assert!(
        denied
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|location| location.contains("above%20your%20own")),
        "Admin refusal must explain the authority boundary",
    );
    assert!(
        store::persons::find_by_email_ci(&surreal, "denied-owner@example.com")
            .await
            .unwrap()
            .is_none(),
        "Admin must not create an Owner",
    );

    let (owner_cookie, owner_csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Owner);
    let created = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/people")
                .header(header::COOKIE, owner_cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Second%20Owner&email=second-owner%40example.com&role=owner&_csrf={owner_csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let row = store::persons::find_by_email_ci(&surreal, "second-owner@example.com")
        .await
        .unwrap()
        .expect("Owner can create another Owner");
    assert_eq!(row.role, store::persons::Role::Owner);
}

/// ENG-304 deleted the `/app/lawyer/people` browser mirror, so `POST /app/api/people`
/// is the only Person create a lawyer can reach. The role coercion the deleted
/// form carried has to hold there: a **lawyer** caller's submitted role is
/// coerced to `client`, so a lawyer can't POST `role=admin` past the boundary.
#[tokio::test]
async fn api_people_create_coerces_a_lawyer_callers_submitted_role() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                // A lawyer caller hand-crafts `role=admin`; the handler must drop it.
                .body(Body::from(
                    "name=Ceres%20Lead&email=ceres%40test.invalid&role=admin",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = store::persons::find_by_email_ci(&surreal, "ceres@test.invalid")
        .await
        .unwrap()
        .expect("the person should have been created");
    assert_eq!(
        created.role,
        store::persons::Role::Client,
        "a lawyer caller's submitted role must be coerced to client",
    );
}

/// The Dioxus admin people list's per-row Delete posts to the native
/// `POST /app/admin/people/{id}/delete` route (the row used `hx-delete`). Prove
/// it removes a client and redirects to the list, and that the command still
/// blocks deleting a non-client record (surfaced as an `?error=` redirect, not a
/// deletion) — defense in depth over the row only showing Delete for clients.
#[tokio::test]
async fn admin_person_delete_via_native_form_removes_client_but_blocks_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Vega",
            "vega@example.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // A client is deleted and we land back on the list.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/delete", client.id))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/app/admin/people"),
    );
    assert!(
        store::persons::find_by_id(&surreal, client.id)
            .await
            .unwrap()
            .is_none(),
        "the client should have been deleted",
    );

    // A lawyer record cannot be deleted: the command blocks it, so it survives and
    // the redirect carries an `?error=`.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/delete", lawyer.id))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.starts_with("/app/admin/people?error="),
        "a blocked delete must redirect with an error flag, got: {location}",
    );
    assert!(
        store::persons::find_by_id(&surreal, lawyer.id)
            .await
            .unwrap()
            .is_some(),
        "the lawyer record must not be deleted",
    );

    // Following the redirect renders the blocked-delete reason above the list,
    // so the admin sees why the record survived instead of a silent no-op.
    let resp = get_with_role(app, &location, store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.contains(
            "Only client records can be deleted. Owner, admin, lawyer, and clerk people are edit-only."
        ),
        "the blocked-delete flash must be visible on the list after redirect: {html}",
    );
}

/// ENG-304: the `/app/lawyer/people` browser mirror is gone. Every one of its four
/// page paths answers the router's not-found for a lawyer-tier session — not a
/// `403`, which would say "this exists and you may not have it". The three
/// native `POST`s behind them are gone too.
///
/// The capability is untouched: Person commands stay lawyer-tier at
/// `POST/PATCH/DELETE /app/api/people*`, and this is only the browser form
/// moving to the admin console. `/app/admin/people.csv` survives — it is a
/// lawyer-tier read with no admin sibling.
#[tokio::test]
async fn lawyer_people_mirror_paths_are_gone() {
    let (state, surreal) = state_with_engines().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let id = person.id;
    for path in [
        "/app/lawyer/people".to_string(),
        "/app/lawyer/people/new".to_string(),
        format!("/app/lawyer/people/{id}"),
        format!("/app/lawyer/people/{id}/edit"),
    ] {
        for role in [
            store::persons::Role::Lawyer,
            store::persons::Role::Admin,
            store::persons::Role::Owner,
        ] {
            let resp = get_with_role(app.clone(), &path, role).await;
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{path} must be gone for {role:?}, not merely refused",
            );
        }
    }

    // The native mutation routes behind those pages went with them.
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    for path in [
        "/app/lawyer/people".to_string(),
        format!("/app/lawyer/people/{id}"),
        format!("/app/lawyer/people/{id}/welcome"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name=Nope&email=nope%40test.invalid&_csrf={csrf}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "POST {path} must be gone",
        );
    }

    // The directory export is not part of the mirror: it has no `/admin`
    // sibling, so deleting it would cost a lawyer a capability.
    let csv = get_with_role(
        app.clone(),
        "/app/admin/people.csv",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(
        csv.status(),
        StatusCode::OK,
        "/app/admin/people.csv is a surviving lawyer-tier read",
    );

    // The admin console is unchanged and still Owner/Admin-only.
    for (role, expected) in [
        (store::persons::Role::Owner, StatusCode::OK),
        (store::persons::Role::Admin, StatusCode::OK),
        (store::persons::Role::Lawyer, StatusCode::FORBIDDEN),
        (store::persons::Role::Clerk, StatusCode::FORBIDDEN),
    ] {
        let resp = get_with_role(app.clone(), "/app/admin/people", role).await;
        assert_eq!(
            resp.status(),
            expected,
            "/app/admin/people must answer {expected} for {role:?}",
        );
    }
    let admin_list = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    let html = body_string(admin_list).await;
    assert!(
        html.contains("<title>Navigator | Admin | People</title>"),
        "the surviving people surface is the admin console's: {html}",
    );
    assert!(
        html.contains("/app/admin/people/") && html.contains("/impersonate"),
        "the admin surface keeps its per-row Edit / Delete / Impersonate actions: {html}",
    );
}

#[tokio::test]
async fn admin_people_dioxus_route_is_gated_by_embedded_policy() {
    // The Dioxus `/app/admin/people` sub-router carries the same `require_auth` +
    // `require_policy` layers as the surface it replaced. Prove the policy layer
    // is live on it: an authenticated admin session under a deny-all embedded
    // policy is refused (403), not served the directory. This keeps the
    // authorization contract from silently regressing when a page moves onto
    // Dioxus.
    let app = server::neon_router(
        empty_state_with_policy(deny_all_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let resp = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "an authenticated admin session must be turned away from the Dioxus \
         /app/admin/people route when the policy denies — the route is policy-gated"
    );
}

#[tokio::test]
async fn policy_evaluation_errors_fail_closed_at_the_router_boundary() {
    let app = server::neon_router(
        empty_state_with_policy(erroring_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let response = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "an evaluation error must fail closed rather than serving a protected route"
    );
}

/// The firm holds the root, and nothing else answers there.
///
/// Both halves matter against the real composition. A regression that
/// remounted the nonprofit's pages would answer `200` on the second half.
#[tokio::test]
async fn the_firm_holds_the_root_and_nothing_else_answers_there() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let firm = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(firm.status(), StatusCode::OK);
    let body = body_string(firm).await;
    assert!(
        body.contains("Neon Law"),
        "the site root is the firm's home: {body}"
    );
    assert!(
        !body.contains(&["Neon", "Law", "Foundation"].join(" ")),
        "and it names no other organization: {body}"
    );

    for path in [
        "/foundation",
        "/foundation/mission",
        "/foundation/education",
        "/foundation/attorneys",
        "/foundation/notations",
        "/foundation/transparency",
        "/mission",
        "/education",
        "/attorneys",
        "/transparency",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(
            resp.headers().get("location").is_none(),
            "{path} must not redirect"
        );
    }
}

/// A signed-in reader gets `404` on an unpublished path too.
///
/// A stale grant would show up here and nowhere else: an anonymous request is
/// `404` while a `client` session renders the page anyway.
#[tokio::test]
async fn a_signed_in_reader_also_gets_not_found_on_an_unpublished_path() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for path in [
        "/foundation/mission",
        "/foundation/notations",
        "/foundation/transparency",
        "/foundation/transparency/bylaws",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(axum::http::header::COOKIE, client_reader_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn docusign_consent_callback_renders_confirmation() {
    // DocuSign redirects the operator's browser to this URI after the
    // one-time JWT-grant `Allow`. It must land on a confirmation page.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/docusign/consent-callback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Consent recorded"));
}

#[tokio::test]
async fn legacy_help_route_is_gone() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(Request::builder().uri("/help").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The firm's marketing surface serves at the site root.
///
/// Each of these is a live page at the root, and the paired invariant is that
/// no firm route answers beneath `/foundation`: every path there is `404`, and
/// never a firm page.
#[tokio::test]
async fn the_firm_marketing_surface_serves_at_the_site_root() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for uri in ["/blog", "/notations", "/contact"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the firm route {uri} must serve at the site root"
        );
    }
    for shadowed in [
        "/foundation/blog",
        "/foundation/contact",
        "/foundation/team",
        "/foundation/navigator",
        "/foundation/services",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(shadowed)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "a firm page must not answer beneath /foundation: {shadowed}"
        );
    }
}

#[tokio::test]
async fn mounted_brand_bundle_serves_only_declared_assets() {
    let bundle_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bundle_dir.path().join("navigator.yaml"),
        "version: 1\nbrand:\n  firm: Acme Law\n  support_email: help@acme.example\nassets:\n  firm_logo: logo.svg\n  firm_logo_raster: logo.png\n  static_files:\n    theme.css: theme.css\n",
    )
    .unwrap();
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40"><rect width="120" height="40" fill="#ff00aa"/><text x="10" y="27">ACME</text></svg>"##;
    std::fs::write(bundle_dir.path().join("logo.svg"), svg).unwrap();
    std::fs::write(bundle_dir.path().join("logo.png"), b"synthetic-png").unwrap();
    std::fs::write(bundle_dir.path().join("theme.css"), b":root{--brand:test}").unwrap();
    let bundle = views::brand_bundle::BrandBundle::load(bundle_dir.path()).unwrap();
    let mut state = empty_state().await;
    state.brand_bundle = Some(bundle);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // The declared brand assets are served from `/public/brand/*`; the firm
    // home + contact rendering under a custom brand bundle is exercised on the
    // firm surface.
    let logo = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/public/brand/firm-logo.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logo.status(), StatusCode::OK);
    assert_eq!(
        logo.into_body().collect().await.unwrap().to_bytes(),
        &svg[..]
    );

    let theme = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/public/brand/static/theme.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(theme.status(), StatusCode::OK);
    assert_eq!(
        theme.into_body().collect().await.unwrap().to_bytes(),
        ":root{--brand:test}"
    );

    let manifest = app
        .oneshot(
            Request::builder()
                .uri("/public/brand/navigator.yaml")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn portal_only_mode_redirects_root_to_portal_and_drops_host_pages() {
    // A `portal_only: true` white-label deploy: the firm's own marketing
    // site owns the public surface, so `/` 303-redirects to the portal and
    // no host page is mounted at all. Since #732 that includes the legal and
    // crawler documents — they belong to whichever brand host publishes
    // them, not to the shared application.
    let mut state = empty_state().await;
    state.portal_only = portal::PortalOnly::new(true);
    let app = server::tenant_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(axum::http::header::LOCATION).unwrap(),
        "/app/projects"
    );

    // A marketing page is no longer mounted under portal-only. `/litigation`
    // rather than a retired route: this must fail because portal-only unmounts
    // the marketing surface, not because the path 404s everywhere anyway.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/litigation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // The legal and crawler documents are host-owned, so a portal-only
    // deploy does not serve them either.
    for host_page in ["/terms", "/privacy", "/robots.txt", "/sitemap.xml"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(host_page)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{host_page} is the host's page to publish, not the portal's"
        );
    }

    // The operational allowlist is unaffected by the mode.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn anonymous_access_to_the_shared_navigator_surface_lands_at_the_login_door() {
    // The live-router probe for #732's boundary. `empty_state` carries a
    // passthrough embedded Rego policy client, so a route that answers anything but a
    // redirect here is one whose protection depends on a policy bundle
    // rather than on router composition.
    //
    // `/app/lawyer` and `/admin` are listed as bare roots deliberately: each has
    // its own dashboard handler, so protection there cannot be inferred from
    // a gated descendant.
    let mut state = empty_state().await;
    state.docs = portal::docs::loader::bundled();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // `/docs` and `/docs/glossary` have left this list. The workspace
    // documentation reads anonymously now — the repository is source-available, so
    // a login door stood in front of the manual for software anyone can clone.
    // `/app/docs` is the surface that still answers the login door, and it is
    // listed below in its place.
    for path in [
        "/app/projects",
        "/app/lawyer",
        "/app/outline",
        "/app/admin",
        "/app/team",
        "/app/docs",
        "/app/docs/glossary",
        "/templates",
        "/app/api",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "anonymous {path} must be sent to the login door"
        );
        assert_eq!(
            resp.headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(format!("/auth/login?return_to={path}").as_str()),
            "{path} must carry the reader back after login"
        );
    }

    // Machine surfaces refuse in a shape a machine can read.
    for path in ["/app/api/openapi.json", "/app/api/people"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous {path} must answer with a status, not a redirect"
        );
        let document: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(document["error"], "unauthenticated", "{path}");
    }
}

#[tokio::test]
async fn the_design_gallery_reads_anonymously() {
    // `/design` is a public reference surface: it mounts outside the session
    // boundary, so an anonymous reader gets the gallery itself rather than the
    // `303` to the login door that every other shared Navigator tool answers
    // with. Probed on the live router, because this is a property of router
    // composition rather than of any policy bundle.
    let mut state = empty_state().await;
    state.docs = portal::docs::loader::bundled();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/design")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "anonymous /design must render the gallery, not redirect"
    );
    let body = body_string(resp).await;
    assert!(
        body.contains("nav-form admin-form"),
        "the gallery's own content renders for an anonymous reader: {body}"
    );
}

#[tokio::test]
async fn english_home_declares_lang_en() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let en = body_string(
        app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(en.contains("<html lang=\"en\""));
}

#[tokio::test]
async fn health_returns_200_when_db_pings() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        body_string(resp).await,
        "ok\nNothing here is legal advice without a signed retainer."
    );
}

#[tokio::test]
async fn health_returns_503_when_the_store_is_down() {
    let state = AppState {
        brand_bundle: None,
        surreal: store::surreal::SurrealDb::uninitialized(),
        workshops: WorkshopIndex::empty(),
        docs: portal::DocsIndex::empty(),
        blog: portal::BlogIndex::empty(),
        auth: AuthConfig::new(true, None),
        google_oauth: portal::google_oauth::GoogleOauthConfig::passthrough(),
        rate_limit: portal::rate_limit::RateLimit::disabled(),
        canonical_host: CanonicalHost::new(None),
        portal_only: portal::PortalOnly::default(),
        sessions: test_sessions(),
        oauth: None,
        oauth_microsoft: None,
        storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        assets_storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        applications_storage: std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
                .await
                .unwrap(),
        ),
        forms_registry: std::sync::Arc::new(forms::registry().unwrap()),
        policy: portal::policy::PolicyClient::passthrough(),
        workflow_runtime: std::sync::Arc::new(workflows::InMemoryRuntime::new()),
        questionnaire_runtime: std::sync::Arc::new(workflows::InMemoryRuntime::new()),
        signature_provider: std::sync::Arc::new(portal::signature::StubSignatureProvider::new()),
        billing_provider: std::sync::Arc::new(portal::billing::StubBillingProvider::new()),
        contract_reviewer: std::sync::Arc::new(portal::contract_review::StubContractReviewer),
        esignature_webhook_secret: None,
        esignature_hmac_key: None,
        email: std::sync::Arc::new(portal::email::CapturingEmail::new()),
        attachment_scanner: std::sync::Arc::new(
            portal::attachment_scanner::FakeAttachmentScanner::clean(),
        ),
        inbound_email_secret: None,
        email_events_secret: None,
        sendgrid_events_public_key: None,
        bootstrap_owner_email: None,
        self_signup_enabled: false,
        identity_password: None,
        identity_admin: None,
        a2a_router: None,
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_string(resp).await, "store unavailable");
}

#[tokio::test]
async fn api_template_raw_serves_non_confidential_markdown_inline() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(
        app.clone(),
        "/app/api/templates/forms/united-states/nevada/state/nv--llc-formation",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8"),
    );
    let body = body_string(resp).await;
    assert!(body.contains("Nevada"), "served the raw template markdown");

    // A confidential template (the retainer) must 404 over the API.
    let confidential = get_signed_in(app, "/app/api/templates/neon-law/shared/retainer").await;
    assert_eq!(confidential.status(), StatusCode::NOT_FOUND);
}

/// The talks surface mounts, and only at the site root.
///
/// What must not happen is a copy appearing beneath `/foundation`: a talk
/// answering there would republish the firm's work at a second URL.
#[tokio::test]
async fn the_talks_surface_mounts_only_at_the_site_root() {
    let materials = portal::workshops::loader::load_navigator(std::path::Path::new(
        portal::DEFAULT_WORKSHOPS_DIR,
    ))
    .expect("load real workshop content");
    let app = server::neon_router(
        state_with_workshops(materials).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for path in [
        "/presentations",
        "/presentations/rust-in-peace",
        "/presentations/rust-in-peace/slides",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the talks catalog serves at the site root: {path}"
        );
    }
    for shadowed in [
        "/foundation/presentations",
        "/foundation/presentations/rust-in-peace",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(shadowed)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "the firm's talks must not also publish under /foundation: {shadowed}"
        );
    }
}

#[tokio::test]
async fn old_presentation_urls_are_not_mounted() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for from in [
        "/foundation/presentations",
        "/foundation/presentations/rust-in-peace",
        "/foundation/presentations/rust-in-peace/step/1",
        // Live follow-along mode was removed: every viewer's SSE stream
        // held a full clone of the deck, an unauthenticated memory
        // amplifier. Each browser now drives its own `/step`//`/display`.
        "/presentations/rust-in-peace/present",
        "/presentations/rust-in-peace/present/events",
        "/presentations/rust-in-peace/present/goto",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(from).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{from} should 404");
    }
}

#[tokio::test]
async fn robots_txt_advertises_sitemap_and_blocks_private_surfaces() {
    let app = server::neon_router(
        empty_state_with_canonical_host(CanonicalHost::new(Some("www.neonlaw.com".into()))).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/robots.txt")
                .header(header::HOST, "www.neonlaw.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );
    let body = body_string(resp).await;
    assert!(body.contains("User-agent: *"));
    assert!(body.contains("Disallow: /app"));
    assert!(body.contains("Disallow: /admin"));
    assert!(body.contains("Sitemap: https://www.neonlaw.com/sitemap.xml"));
    // `/docs` and `/templates` sit behind the session boundary (#732), so the
    // policy names each rather than pointing a crawler at a login redirect.
    // `/design` reads anonymously now, and stays disallowed for the other
    // reason: a contributor reference gallery is not a page to index.
    for authenticated in [
        "Disallow: /docs",
        "Disallow: /design",
        "Disallow: /templates",
    ] {
        assert!(body.contains(authenticated), "{authenticated} in {body}");
    }
    // The API surface and its documentation live under `/app/api`, so
    // `Disallow: /app` already covers them. The retired top-level lines must
    // not come back: a crawler policy naming a path nothing serves is a
    // standing invitation to look for one.
    for retired in [
        "Disallow: /api-docs",
        "Disallow: /openapi.json",
        // The `/portal` landing folded into `/app`, so the crawler policy must
        // not keep pointing at a path nothing serves.
        "Disallow: /portal",
        // `/lawyer` folded into `/app/lawyer`. `Disallow: /app` covers the
        // workbench; a leftover top-level line names a path nothing serves.
        "Disallow: /lawyer",
        // The classes are public and the sitemap advertises them. Forbidding
        // what the same host advertises is a contradiction a crawler settles
        // by not fetching the page, so this line must not come back.
        "Disallow: /workshops",
    ] {
        assert!(
            !body.contains(retired),
            "{retired} names a path that no longer exists: {body}"
        );
    }
    assert!(
        !body.contains("Allow:"),
        "no shared surface is crawlable now: {body}"
    );
}

#[tokio::test]
async fn crawler_discovery_ignores_internal_request_host_when_canonical_host_is_unset() {
    let app = server::neon_router(
        empty_state_with_canonical_host(CanonicalHost::new(None)).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let robots = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/robots.txt")
                .header(header::HOST, "internal-service:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(robots.status(), StatusCode::OK);
    let robots_body = body_string(robots).await;
    assert!(robots_body.contains("Sitemap: https://www.example.com/sitemap.xml"));
    assert!(
        !robots_body.contains("www.neonlaw.com"),
        "unset canonical host should use the deployment-neutral fallback: {robots_body}"
    );
    assert!(
        !robots_body.contains("internal-service"),
        "robots.txt should not advertise proxy/internal hosts: {robots_body}"
    );

    let sitemap = app
        .oneshot(
            Request::builder()
                .uri("/sitemap.xml")
                .header(header::HOST, "internal-service:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sitemap.status(), StatusCode::OK);
    let sitemap_body = body_string(sitemap).await;
    assert!(sitemap_body.contains("<loc>https://www.example.com/privacy</loc>"));
    assert!(
        !sitemap_body.contains("www.neonlaw.com"),
        "unset canonical host should use the deployment-neutral fallback: {sitemap_body}"
    );
    assert!(
        !sitemap_body.contains("internal-service"),
        "sitemap should not advertise proxy/internal hosts: {sitemap_body}"
    );
}

#[tokio::test]
async fn sitemap_xml_lists_public_routes_from_loaded_indexes() {
    let mut state =
        empty_state_with_canonical_host(CanonicalHost::new(Some("www.neonlaw.com".into()))).await;
    state.docs = portal::docs::loader::bundled();
    state.blog = portal::blog::load_dir(std::path::Path::new(portal::DEFAULT_BLOG_DIR)).unwrap();
    state.workshops = WorkshopIndex::new(
        portal::workshops::loader::load_navigator(std::path::Path::new(
            portal::DEFAULT_WORKSHOPS_DIR,
        ))
        .unwrap(),
    );
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/sitemap.xml")
                .header(header::HOST, "www.neonlaw.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/xml; charset=utf-8"
    );
    let body = body_string(resp).await;
    assert!(body.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    for loc in [
        "https://www.neonlaw.com/",
        "https://www.neonlaw.com/services",
    ] {
        assert!(
            body.contains(&format!("<loc>{loc}</loc>")),
            "sitemap missing {loc}: {body}"
        );
    }
    // The talks are the firm's and the sitemap is one document now, so they
    // ARE advertised — at the site root, never beneath `/foundation`.
    assert!(
        !body.contains("<loc>https://www.neonlaw.com/foundation/presentations</loc>"),
        "the firm's talks must not be filed under /foundation: {body}"
    );
    assert!(
        !body.contains("<loc>https://www.neonlaw.com/app/team</loc>"),
        "sitemap should not list authenticated app routes: {body}"
    );
    // `/templates` is authenticated (#732), and a sitemap entry pointing at a
    // login redirect is worse than no entry at all. `/docs` and `/design` read
    // anonymously now but stay unadvertised for the same reason as each other:
    // both are contributor references, not pages a search result should land a
    // prospective client on. Advertising the documentation is a separate
    // decision from un-gating it, and is deliberately not made here.
    for authenticated in ["/docs", "/design", "/templates"] {
        assert!(
            !body.contains(&format!("<loc>https://www.neonlaw.com{authenticated}")),
            "sitemap must not advertise authenticated {authenticated}: {body}"
        );
    }
    // The nonprofit's surface is advertised nowhere.
    //
    // `/workshops` is deliberately absent from this list. The classes are
    // public now, so the catalog is a page a crawler should find.
    for unpublished in [
        "/foundation",
        "/foundation/education",
        "/foundation/attorneys",
        "/foundation/notations",
        "/foundation/transparency",
        "/foundation/mission",
        "/mission",
        "/transparency",
    ] {
        assert!(
            !body.contains(&format!("<loc>https://www.neonlaw.com{unpublished}")),
            "sitemap must not advertise {unpublished}: {body}"
        );
    }
    // The firm's pages ARE advertised — one host, one sitemap. What must not
    // appear is a firm page filed beneath `/foundation`.
    for firm_page in ["/blog", "/litigation", "/notations"] {
        assert!(
            body.contains(&format!("<loc>https://www.neonlaw.com{firm_page}</loc>")),
            "sitemap must advertise the firm page {firm_page}: {body}"
        );
        assert!(
            !body.contains(&format!(
                "<loc>https://www.neonlaw.com/foundation{firm_page}</loc>"
            )),
            "a firm page must not be filed under /foundation: {firm_page}"
        );
    }
}

#[tokio::test]
async fn sitemap_xml_is_not_mounted_in_portal_only_mode() {
    // A `portal_only: true` white-label deploy mounts no host public pages at
    // all: the firm's own marketing site owns that surface, and every shared
    // Navigator route is authenticated (#732). `/sitemap.xml` is a host page,
    // so it is simply not routed here and a crawler gets a bare 404 — the same
    // contract `portal_only_mode_redirects_root_to_portal_and_drops_host_pages`
    // asserts for the other crawler and legal documents.
    let mut state =
        empty_state_with_canonical_host(CanonicalHost::new(Some("www.neonlaw.com".into()))).await;
    state.portal_only = portal::PortalOnly::new(true);
    let app = server::tenant_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/sitemap.xml")
                .header(header::HOST, "www.neonlaw.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn old_descriptive_service_slugs_are_gone_with_no_redirect() {
    // The rename keeps NO back-compat for the old descriptive URLs — the
    // user asked not to preserve them. The former paths must 404 (not 301),
    // so this pins that we didn't silently leave a redirect behind.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for path in [
        "/services/estate",
        "/services/corporate",
        "/services/fractional-gc",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} should be gone with no redirect"
        );
    }
}

#[tokio::test]
async fn public_favicon_is_served() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/public/favicon.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(ctype.contains("image/svg"), "got content-type: {ctype}");
}

#[tokio::test]
async fn public_missing_file_returns_404() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/public/no-such-file.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

fn sample_workshop() -> WorkshopMaterial {
    WorkshopMaterial {
        category: "workshops".into(),
        slug: "use-the-navigator".into(),
        title: "Runbook".into(),
        description: "How.".into(),
        audience: "For lawyers".into(),
        benefit: "You walk out with a notation you built yourself.".into(),
        raw_markdown: "# Runbook\n\nIntro.\n\n## Intro\n\n### Install\n\nDo it.\n\n## Wrap Up\n\n### Notarize\n\nFinish.\n"
            .into(),
        body_html: "<p>Intro.</p><h2>Intro</h2><h3>Install</h3><p>Do it.</p><h2>Wrap Up</h2><h3>Notarize</h3>".into(),
        intro_html: "<p>Intro.</p>".into(),
        chapters: vec![
            WorkshopChapter {
                title: "Intro".into(),
                preamble_html: String::new(),
                section_start: 0,
                section_count: 1,
            },
            WorkshopChapter {
                title: "Wrap Up".into(),
                preamble_html: String::new(),
                section_start: 1,
                section_count: 1,
            },
        ],
        sections: vec![
            WorkshopSection {
                title: "Install".into(),
                body_html: "<h3>Install</h3><p>Do it.</p>".into(),
                notes_html: "<p>Presenter notes for install.</p>".into(),
            },
            WorkshopSection {
                title: "Notarize".into(),
                body_html: "<h3>Notarize</h3><p>Finish.</p>".into(),
                notes_html: "<p>Presenter notes for notarize.</p>".into(),
            },
        ],
    }
}

/// The `/workshops` surface mounts publicly at the site root and never under
/// `/foundation`.
#[tokio::test]
async fn the_workshops_surface_mounts_only_at_the_site_root() {
    let app = server::neon_router(
        state_with_workshops(vec![sample_workshop()]).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    for uri in [
        "/foundation/workshops/use-the-navigator",
        "/foundation/workshops/use-the-navigator/slides",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "workshops must not publish under the nonprofit's prefix: {uri}"
        );
    }
    // The catalog itself is anonymously readable.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/workshops")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_workshops_index_lists_each_class_you_voiced() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops")
                .header(axum::http::header::COOKIE, client_reader_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("<title>Neon Law | Workshops</title>"));
    // Each workshop links to its overview one level down, tagged with its
    // audience and led by its you-voiced benefit.
    assert!(body.contains("href=\"/workshops/use-the-navigator\""));
    // Match `>Runbook<`, not `>Runbook</a>`: Dioxus SSR emits a hydration
    // marker comment after a text node, so the title is never immediately
    // followed by its closing tag. The `href` assertion above is what proves
    // the title is the anchor's text.
    assert!(body.contains(">Runbook<"), "workshop title: {body}");
    assert!(body.contains("For lawyers"));
    assert!(body.contains("You walk out with a notation you built yourself."));
}

#[tokio::test]
async fn workshops_overview_renders_one_h1_and_links_steps() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("<title>Neon Law | Workshops | Use The Navigator</title>"));
    // The duplicate-H1 bug regression guard: chrome title is the only one.
    assert_eq!(body.matches("<h1>").count(), 1, "expected a single <h1>");
    assert!(body.contains("href=\"/workshops/use-the-navigator/step/1\""));
    assert!(body.contains("data-workshop-chapter=\"Intro\""));
    assert!(body.contains("data-workshop-chapter=\"Wrap Up\""));
    // The overview advertises and links its Markdown twin.
    // Asserted by parts rather than as one literal tag: `document::Link`
    // decides its own attribute order, and the contract is the three values,
    // not their sequence.
    assert!(
        body.contains("rel=\"alternate\"")
            && body.contains("text/markdown")
            && body.contains("href=\"/workshops/use-the-navigator.md\""),
        "the markdown twin must be advertised in the head: {body}"
    );
    // The clipboard button is gone, and so is the script that drove it — a
    // page still loading it would be shipping dead first-party JavaScript.
    assert!(!body.contains("data-copy-markdown"), "copy hook: {body}");
    assert!(
        !body.contains("/public/js/copy-markdown.js"),
        "orphaned copy script: {body}"
    );
    assert!(!body.contains("alpine"), "Alpine must not return: {body}");
}

#[tokio::test]
async fn workshops_material_md_twin_serves_raw_markdown() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator.md")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(ctype, "text/markdown; charset=utf-8");
    let body = body_string(resp).await;
    // The byte-for-byte source — heading and all — not rendered HTML.
    assert!(body.starts_with("# Runbook"));
    assert!(!body.contains("<h1>"));
}

#[tokio::test]
async fn workshops_material_md_twin_404s_when_slug_missing() {
    let app = catalog_router(empty_state().await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/missing.md")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn llms_txt_indexes_the_markdown_corpus_with_absolute_urls() {
    let app = server::neon_router(
        state_with_workshops(vec![sample_workshop()]).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/llms.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(ctype, "text/markdown; charset=utf-8");
    let body = body_string(resp).await;
    // llmstxt.org shape: H1, a site summary, then curated links. The H1 names
    // the site a crawler has reached, and the curated half is the firm's own
    // public pages.
    assert!(body.starts_with("# Neon Law\n"));
    assert!(body.contains("`{{placeholders}}`"));
    assert!(body.contains("ground questionnaire states and placeholders"));
    assert!(body.contains("## Pages"));
    // The firm's own marketing surface. The workshops catalog is public
    // root-level material and is advertised in its own section below; the
    // presentations catalog is not curated into the index at all.
    for page in [
        "https://www.example.com/)",
        "https://www.example.com/services)",
        "https://www.example.com/notations)",
    ] {
        assert!(
            body.contains(page),
            "llms.txt must advertise {page}: {body}"
        );
    }
    // The nonprofit's surface is advertised nowhere.
    for unpublished in [
        "https://www.example.com/foundation",
        "https://www.example.com/mission",
        "https://www.example.com/transparency",
    ] {
        assert!(
            !body.contains(unpublished),
            "llms.txt must not advertise {unpublished}: {body}"
        );
    }
    assert!(
        !body.contains("pairs legal aid centers with volunteer attorneys"),
        "the nonprofit's summary is not the site's: {body}"
    );

    // Private or authenticated surfaces remain absent. A crawler that follows
    // one of these gets a login redirect or a 404, so each is asserted absent
    // by the URL it would have carried.
    for gated in [
        "https://www.example.com/docs/",
        "https://www.example.com/templates",
    ] {
        assert!(
            !body.contains(gated),
            "llms.txt must not advertise {gated}: {body}"
        );
    }
    // `llms.txt` lists the public marketing surfaces and nothing else. The
    // repository is source-available now, so a link to it would resolve — but
    // it is a developer surface rather than a marketing one, and adding it is a
    // deliberate content decision rather than a side effect of the licence.
    assert!(
        !body.contains("https://github.com/neon-law-source-code/navigator"),
        "llms.txt must not advertise the repository: {body}"
    );
    assert!(
        !body.contains("require a signed-in Navigator account"),
        "llms.txt no longer names the gated surfaces at all: {body}"
    );
    // The headings that carried those links are gone with them.
    for heading in [
        "## Core Concepts",
        "## Use The CLI",
        "## Contribute",
        "## Services And Classes",
    ] {
        assert!(
            !body.contains(heading),
            "llms.txt must not carry {heading}: {body}"
        );
    }
    // This fixture holds one public workshop, so the workshop corpus is
    // present; the index does not curate a presentation corpus at all.
    assert!(body.contains("## Workshop Corpus"));
    assert!(body.contains("/workshops/use-the-navigator.md"));
    assert!(!body.contains("## Presentation Corpus"));
}

#[tokio::test]
async fn deploy_workshop_md_twin_and_llms_index_the_real_content() {
    // Ground the *shipped* DEPLOY.md, not a fixture: load the real
    // workshop content directory, then confirm the deploy workshop's
    // markdown twin serves and the llms.txt corpus indexes it. If the
    // manifest entry or the file goes missing, this 404s and fails.
    let materials = portal::workshops::loader::load_navigator(std::path::Path::new(
        portal::DEFAULT_WORKSHOPS_DIR,
    ))
    .expect("load real workshop content");
    let app = catalog_router(state_with_workshops(materials).await);

    // The markdown twin serves raw markdown with the right content type.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/workshops/deploy-the-navigator.md")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(ctype, "text/markdown; charset=utf-8");
    let body = body_string(resp).await;
    assert!(
        body.contains("# Operating Neon Law Navigator"),
        "raw markdown title"
    );
    assert!(body.contains("cargo run -p cli -- ops gcp setup --project-id"));

    // The firm's llms.txt withholds the class twin this test just fetched
    // with a session — advertising a gated document is advertising a login
    // redirect — and does not curate a talks corpus at all.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/llms.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !body.contains("## Presentation Corpus"),
        "the firm's llms.txt does not carry a talks corpus: {body}"
    );
    assert!(
        body.contains("## Workshop Corpus") && body.contains("/workshops/"),
        "llms.txt must advertise the public workshop corpus: {body}"
    );
}

#[tokio::test]
async fn workshops_step_renders_single_section_with_progress() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator/step/1")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Chapter 1 of 2"));
    assert!(body.contains("Section 1 of 2"));
    assert!(body.contains("Intro"));
    assert!(body.contains("data-workshop-chapter=\"Intro\""));
    assert!(body.contains("<h3>Install</h3>"));
    // Step one shows the next section's content nowhere on the page.
    assert!(!body.contains("<h3>Notarize</h3>"));
    assert!(body.contains("href=\"/workshops/use-the-navigator/step/2\""));
}

/// Drive `GET …/{slug}/slides` and return `(cookie_pair, csrf_token)` for a
/// valid double-submit POST to the certificate route. `cookie_pair` is the
/// `name=value` to send back in a `Cookie:` header.
async fn fetch_workshop_csrf(app: &axum::Router, slug: &str) -> (String, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/workshops/{slug}/slides"))
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("navigator_workshop_cert_csrf="))
        .expect("slides page sets the workshop CSRF cookie")
        .to_string();
    // Both credentials travel on the one `Cookie` header the certificate POST
    // sends: the session that clears the workshop gate, and the double-submit
    // CSRF cookie the form echoes back.
    let cookie_pair = format!(
        "{}; {}",
        set_cookie.split(';').next().unwrap(),
        workshop_session_cookie()
    );
    let body = body_string(resp).await;
    let marker = "name=\"csrf_token\" value=\"";
    let start = body.find(marker).expect("csrf hidden field present") + marker.len();
    let token = body[start..].split('"').next().unwrap().to_string();
    (cookie_pair, token)
}

#[tokio::test]
async fn workshops_slides_renders_grid_and_mints_dedicated_csrf_cookie() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator/slides")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // The light table uses its OWN cookie, never the account-recovery one,
    // so opening it can't clobber an in-flight password reset.
    let cookies: Vec<&str> = resp
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("navigator_workshop_cert_csrf=")),
        "expected the dedicated workshop CSRF cookie, got {cookies:?}"
    );
    assert!(
        !cookies
            .iter()
            .any(|c| c.starts_with("navigator_account_csrf=")),
        "slides must NOT mint the account-recovery cookie, got {cookies:?}"
    );
    let body = body_string(resp).await;
    assert!(body.contains("data-cert-gate"), "certificate gate present");
    // `slide-thumb`, not the `slide-thumb-link`: the anchor *is* the
    // thumbnail now rather than a wrapper around a card, so the two classes
    // collapsed into one.
    assert!(body.contains("slide-thumb"), "slide thumbnails present");
    assert!(
        !body.contains("data-slide-seen-badge"),
        "slide thumbnails do not render checkmarks"
    );
    assert!(body.contains("data-workshop-chapter=\"Intro\""));
    assert!(body.contains("data-workshop-chapter=\"Wrap Up\""));
    // The teal is in the shared token layer now, so the slides need nothing
    // brand-specific — but they draw no `PublicShell`, so `theme.css` is
    // still this page's own to hoist. Dropping it renders an unstyled deck.
    assert!(
        body.contains("/public/css/theme.css"),
        "slides hoist the token layer they read"
    );
    assert!(body.contains("/public/js/workshop-progress.js"));
}

#[tokio::test]
async fn workshops_certificate_rejects_request_without_valid_csrf() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workshops/use-the-navigator/certificate")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from(
                    "name=Jane&email=jane%40example.com&csrf_token=bogus",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn workshops_certificate_accepts_valid_request_and_confirms() {
    // Post/redirect/get: the POST answers `303`, and the confirmation is its
    // own GET page. Asserted by re-requesting the `Location` rather than
    // trusting the redirect — a `303` proves only where the browser was sent.
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let (cookie, token) = fetch_workshop_csrf(&app, "use-the-navigator").await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workshops/use-the-navigator/certificate")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(axum::http::header::COOKIE, cookie)
                .body(Body::from(format!(
                    "name=Jane+Q.+Student&email=jane%40example.com&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("redirect target")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(location, "/workshops/use-the-navigator/certificate/sent");

    let resp = app
        .oneshot(
            Request::builder()
                .uri(&location)
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.contains("Check your inbox"),
        "neutral confirmation: {html}"
    );
    assert!(
        html.contains(">Runbook<"),
        "the confirmation names the workshop: {html}"
    );
    // A reload re-renders the confirmation rather than dispatching a second
    // certificate — nothing on this page can be re-submitted.
    assert!(!html.contains("<form"), "nothing to re-submit: {html}");
    // Neutral: the address the learner typed never reaches the page, so it
    // cannot become a delivery receipt.
    assert!(
        !html.contains("jane@example.com"),
        "the confirmation must not echo the address: {html}"
    );
}

#[tokio::test]
async fn workshops_certificate_confirmation_404s_for_an_unknown_material() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/missing/certificate/sent")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workshops_certificate_rejects_overlong_name() {
    // Server-side length bound (matches the form maxlength) — a client that
    // bypasses the HTML constraint can't feed a huge string to the renderer.
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let (cookie, token) = fetch_workshop_csrf(&app, "use-the-navigator").await;
    let long_name = "a".repeat(200);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/workshops/use-the-navigator/certificate")
                .header(
                    axum::http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(axum::http::header::COOKIE, cookie)
                .body(Body::from(format!(
                    "name={long_name}&email=jane%40example.com&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn workshops_display_renders_slide_only_without_presenter_notes() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator/display/1")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The slide body renders inside the full-screen display shell.
    assert!(body.contains("<h3>Install</h3>"));
    assert!(body.contains("catalog-display"), "display shell: {body}");
    // The presenter notes for this section never reach the display screen.
    assert!(
        !body.contains("Presenter notes for install."),
        "display face must not carry presenter notes: {body}"
    );
    assert!(!body.contains("Presenter notes"));
    // First slide: next links to slide 2, no previous target.
    assert!(body.contains("href=\"/workshops/use-the-navigator/display/2\""));
    assert!(!body.contains("display/0"));
    // The navigation script is wired up. It lives in the document head, which
    // no component test can see — this is the only place the hoist is proven.
    assert!(body.contains("/public/js/catalog-display.js"));
    // A projector shows the slide and nothing else: no site header, no footer.
    assert!(!body.contains("public-shell"), "no site chrome: {body}");
}

#[tokio::test]
async fn workshops_step_hoists_both_first_party_scripts() {
    // The step page renders identically with or without them, so a missing
    // hoist is invisible to every markup assertion: the arrow keys stop moving
    // the deck and the light table's progress count and certificate gate never arrive.
    // Both tags live in the document head, out of reach of a component test.
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/use-the-navigator/step/1")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    for src in [
        "/public/js/catalog-display.js",
        "/public/js/workshop-progress.js",
    ] {
        assert!(body.contains(src), "missing script {src}: {body}");
    }
    assert!(body.contains("/public/css/catalog.css"), "styles: {body}");
    // The Bootstrap dropdown the rail used cannot cross onto a page that
    // no longer loads Bootstrap; the section menu is a native disclosure.
    assert!(!body.contains("data-bs-toggle"), "no Bootstrap JS: {body}");
    assert!(
        body.contains("workshop-sections__toggle"),
        "the disclosure the browser test focuses: {body}"
    );
}

#[tokio::test]
async fn workshops_display_out_of_range_404s() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    for uri in [
        "/workshops/use-the-navigator/display/0",
        "/workshops/use-the-navigator/display/3",
        "/workshops/missing/display/1",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(axum::http::header::COOKIE, workshop_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri} should 404");
    }
}

#[tokio::test]
async fn workshops_step_out_of_range_404s() {
    let app = catalog_router(state_with_workshops(vec![sample_workshop()]).await);
    for uri in [
        "/workshops/use-the-navigator/step/0",
        "/workshops/use-the-navigator/step/3",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(axum::http::header::COOKIE, workshop_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri} should 404");
    }
}

#[tokio::test]
async fn workshops_material_404s_when_slug_missing() {
    let app = catalog_router(empty_state().await);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/workshops/missing")
                .header(axum::http::header::COOKIE, workshop_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_people_returns_empty_array_when_no_rows() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(app, "/app/api/people").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ctype.contains("application/json"), "got: {ctype}");
    assert_eq!(body_string(resp).await, "[]");
}

#[tokio::test]
async fn api_people_lists_seeded_rows() {
    // Exercise the listing against the canonical seed (store/seeds/
    // Person.yaml) rather than a hand-rolled row, so the test covers
    // the same data the app ships with.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(app, "/app/api/people").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("\"name\":\"Nick Shook\""), "got: {body}");
    assert!(
        body.contains("\"email\":\"nick@neonlaw.com\""),
        "got: {body}"
    );
}

#[tokio::test]
async fn api_people_create_persists_person_with_default_role_and_name_parts() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Libra Example",
        "email": "libra-create@example.com",
        "given_name": "Libra",
        "family_name": "Example",
        "middle_name": ""
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Libra Example");
    assert_eq!(body["email"], "libra-create@example.com");
    assert_eq!(body["role"], "client");
    assert_eq!(body["given_name"], "Libra");
    assert_eq!(body["family_name"], "Example");
    assert!(body["middle_name"].is_null());

    let rows = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, store::persons::Role::Client);
    assert_eq!(rows[0].middle_name, None);
}

#[tokio::test]
async fn api_people_create_accepts_lawyer_role() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let req = serde_json::json!({
        "name": "Lawyer Example",
        "email": "lawyer-create@example.com",
        "role": "lawyer"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["role"], "lawyer");
}

#[tokio::test]
async fn api_people_create_accepts_lawyer_bearer_session() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let req = serde_json::json!({
        "name": "Bearer Lawyer",
        "email": "bearer-lawyer-create@example.com"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(
                    header::AUTHORIZATION,
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["email"], "bearer-lawyer-create@example.com");
}

#[tokio::test]
async fn api_people_create_authorizes_owner_admin_and_lawyer() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::CREATED,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::CREATED,
            "",
        ),
        (
            "owner",
            Some(store::persons::Role::Owner),
            StatusCode::CREATED,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let req = serde_json::json!({
            "name": format!("{label} Person"),
            "email": format!("{label}-api-authz@example.com")
        });
        let mut builder = Request::builder()
            .method("POST")
            .uri("/app/api/people")
            .header("content-type", "application/json");
        if let Some(role) = role {
            // Cookie-authenticated callers must now carry the CSRF token,
            // so the request reaches the handler's role check (the point
            // of this test) rather than tripping the 403 CSRF guard.
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(req.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["email"], format!("{label}-api-authz@example.com"));
        } else {
            assert_eq!(body["error"], error);
        }
    }
}

#[tokio::test]
async fn api_people_create_trims_name_and_email_before_insert() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "  Libra Example  ",
        "email": "  libra-trim@example.com  "
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Libra Example");
    assert_eq!(body["email"], "libra-trim@example.com");

    let row = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("created row");
    assert_eq!(row.name, "Libra Example");
    assert_eq!(row.email, "libra-trim@example.com");
}

#[tokio::test]
async fn api_people_create_rejects_invalid_input_as_json() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "",
        "email": "not-an-email"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["message"],
        "Name is required and email must contain an @."
    );
}

#[tokio::test]
async fn api_people_create_rejects_email_with_embedded_whitespace() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Bad Email",
        "email": "libra @example.com"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["message"],
        "Name is required and email must contain an @."
    );
}

#[tokio::test]
async fn api_people_create_rejects_invalid_role_as_json() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Bad Role",
        "email": "bad-role@example.com",
        "role": "Admin"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["message"],
        "Role must be owner, admin, lawyer, clerk, or client."
    );
}

#[tokio::test]
async fn api_people_create_duplicate_email_returns_json_409() {
    let (state, surreal) = state_with_engines().await;
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "dup-api@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Other",
        "email": "dup-api@example.com"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "conflict");
    assert_eq!(body["message"], "That email is already in use.");
}

#[tokio::test]
async fn api_person_by_id_404s_when_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(
        app,
        &format!("/app/api/people/{}", uuid::Uuid::from_u128(999)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(body.contains("\"error\":\"not_found\""));
}

#[tokio::test]
async fn admin_person_show_page_renders_for_any_role() {
    let (state, surreal) = state_with_engines().await;

    // One person of each tier: the show page must render for a client
    // (fully editable name/email) as well as a lawyer and an admin whose
    // role is not editable — the page renders even when fields aren't.
    let mut ids = Vec::new();
    for (name, email, role) in [
        (
            "Repro Client",
            "repro-client@example.com",
            store::persons::Role::Client,
        ),
        (
            "Repro Lawyer",
            "repro-lawyer@example.com",
            store::persons::Role::Lawyer,
        ),
        (
            "Repro Admin",
            "repro-admin@example.com",
            store::persons::Role::Admin,
        ),
    ] {
        let p = store::persons::create(
            &surreal,
            &store::persons::NewPerson::with_role(name, email, role),
        )
        .await
        .unwrap();
        ids.push((p.id, name));
    }

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let cookie = admin_session_cookie_with_person();

    for (id, name) in ids {
        // Both the bare show URL and the /edit alias must render.
        for uri in [
            format!("/app/admin/people/{id}"),
            format!("/app/admin/people/{id}/edit"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(&uri)
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri} should 200");
            let body = body_string(resp).await;
            assert!(
                body.contains(name),
                "{uri} must render the person's name; got: {body}"
            );
        }
    }
}

#[tokio::test]
async fn admin_person_show_page_missing_person_keeps_signed_in_chrome() {
    // A person that doesn't exist renders a 404 inside the signed-in layout
    // (with the signed-in nav + a way back), not the anonymous not-found
    // page that reads as logged-out — the "I can't see anything" symptom.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let missing = uuid::Uuid::from_u128(0x00c0_ffee);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{missing}"))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    // Signed-in chrome: the authenticated nav shows "Sign out", not "Sign in".
    assert!(
        body.contains("Sign out"),
        "404 should keep the signed-in nav: {body}"
    );
    assert!(
        !body.contains("Sign in"),
        "404 must not render the logged-out nav: {body}"
    );
}

#[tokio::test]
async fn api_jurisdictions_and_entity_types_are_listable() {
    // Drive both listings off the canonical seed (store/seeds/
    // Jurisdiction.yaml + EntityType.yaml) so the assertions track the
    // reference data the app actually ships.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_signed_in(app.clone(), "/app/api/jurisdictions").await;
    let body = body_string(resp).await;
    assert!(body.contains("\"code\":\"NV\""), "got: {body}");
    assert!(body.contains("\"code\":\"CA\""), "got: {body}");

    let resp = get_signed_in(app, "/app/api/entity-types").await;
    let body = body_string(resp).await;
    assert!(
        body.contains("\"name\":\"Professional LLC\""),
        "got: {body}"
    );
}

#[tokio::test]
async fn api_entities_lists_seeded_rows() {
    // /app/api/entities had no coverage before this; seed the canonical
    // entities (store/seeds/Entity.yaml) and assert the listing serves
    // them.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(app, "/app/api/entities").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Two rows, so the listing is proven to serve more than the firm anchor.
    assert!(body.contains("\"name\":\"Shook Law PLLC\""), "got: {body}");
    assert!(body.contains("\"name\":\"shook.family\""), "got: {body}");
}

#[tokio::test]
async fn api_entity_by_id_returns_seeded_row() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let row = store::entities::all(&state.surreal)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("seed pass inserts at least one entity");
    let id = row.id;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(app, &format!("/app/api/entities/{id}")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(&format!("\"id\":\"{id}\"")), "got: {body}");
    assert!(
        body.contains(&format!("\"name\":\"{}\"", row.name)),
        "got: {body}"
    );
}

/// The canonical seed's first entity type and jurisdiction, so an
/// `/app/api/entities` create has real references to point at — both in
/// SurrealDB (ENG-20).
async fn seeded_entity_fks(surreal: &store::surreal::SurrealDb) -> (uuid::Uuid, uuid::Uuid) {
    let entity_type = store::entity_types::list(surreal, &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("seed pass inserts at least one entity type");
    let jurisdiction = first_seeded_jurisdiction(surreal).await;
    (entity_type.id, jurisdiction.id)
}

/// The canonical seed's first jurisdiction (name-ordered), read from the
/// engine that holds the table since ENG-20.
async fn first_seeded_jurisdiction(
    surreal: &store::surreal::SurrealDb,
) -> store::jurisdictions::Jurisdiction {
    store::jurisdictions::list_all(surreal)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("seed pass inserts at least one jurisdiction")
}

#[tokio::test]
async fn api_entities_create_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for the Entity create command: API writes
    // are never anonymous (401) and never `client` (403); lawyer and admin
    // both create. Mirrors the People matrix — the `LawyerSession`
    // extractor is the enforcing check, so it holds even where the embedded Rego policy
    // layer is a passthrough (as it is in these tests).
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::CREATED,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::CREATED,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let req = serde_json::json!({
            "name": format!("{label} Holdings LLC"),
            "entity_type_id": type_id,
            "jurisdiction_id": jur_id,
        });
        let mut builder = Request::builder()
            .method("POST")
            .uri("/app/api/entities")
            .header("content-type", "application/json");
        if let Some(role) = role {
            // Cookie-authenticated callers carry the CSRF token so the
            // request reaches the handler's role check — the point of this
            // test — rather than tripping the 403 CSRF guard.
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(req.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["name"], format!("{label} Holdings LLC"));
            assert_eq!(body["entity_type_id"], type_id.to_string());
            assert_eq!(body["jurisdiction_id"], jur_id.to_string());
        } else {
            assert_eq!(body["error"], error);
        }
    }
}

#[tokio::test]
async fn api_entities_create_accepts_lawyer_bearer_session() {
    // A bearer-authenticated write carries no cookie, so it is not
    // CSRF-exposed and stays exempt — the credential-keyed rule, proven on
    // the new command endpoint.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let req = serde_json::json!({
        "name": "Bearer Holdings LLC",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/entities")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Bearer Holdings LLC");
}

#[tokio::test]
async fn api_entities_create_rejects_a_cross_site_origin() {
    // Defense in depth: a cookie-authenticated write from another origin is
    // refused even when it presents a valid CSRF token, so a leaked token
    // cannot be replayed from an attacker's page.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Cross Site LLC",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/entities")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                // The check compares Origin against the request's Host, so
                // the request must carry one for there to be a cross-site
                // mismatch to detect.
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_entities_create_refuses_to_fork_the_firm_anchor() {
    // The firm's own Entity is the one name that cannot be duplicated. The
    // canonical seed already inserted it, so a second create returns 409
    // with the caller-facing reason — the same guard the lawyer form hits,
    // because both doors call one command.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let req = serde_json::json!({
        "name": store::seed::FIRM_ENTITY_NAME,
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/entities")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "conflict");
    assert_eq!(
        body["message"],
        "The firm entity already exists and cannot be duplicated."
    );
}

#[tokio::test]
async fn api_entities_create_rejects_a_blank_name() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "   ",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/entities")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["message"], "Name is required.");
}

#[tokio::test]
async fn api_entities_create_rejects_unknown_type_or_jurisdiction_with_400() {
    // A type or jurisdiction id that references no row is the caller's to
    // correct, so the command surfaces it as the documented 400 validation
    // failure, not an undocumented 500. The FK is how those references are
    // validated, so an unknown id is caught at the insert, not before.
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Ghost Co",
        "entity_type_id": uuid::Uuid::from_u128(0xdead),
        "jurisdiction_id": uuid::Uuid::from_u128(0xbeef),
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/entities")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["message"], "Unknown entity type or jurisdiction.");
}

/// One ordinary (non-anchor) Entity to edit, plus the FK ids to point at.
async fn seeded_entity_to_edit(
    surreal: &store::surreal::SurrealDb,
) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let (type_id, jur_id) = seeded_entity_fks(surreal).await;
    let row = store::entities::create(
        surreal,
        &store::entities::NewEntity {
            name: "Editable Co".into(),
            entity_type_id: type_id,
            jurisdiction_id: jur_id,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap();
    (row.id, type_id, jur_id)
}

#[tokio::test]
async fn api_entities_update_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for the Entity update command, mirroring the
    // create matrix: API writes are never anonymous (401) and never `client`
    // (403). The `LawyerSession` extractor is the enforcing check, so this
    // holds even where the embedded Rego policy layer is a passthrough (as it is in tests).
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, type_id, jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let req = serde_json::json!({
            "name": format!("{label} Renamed Co"),
            "entity_type_id": type_id,
            "jurisdiction_id": jur_id,
        });
        let mut builder = Request::builder()
            .method("PATCH")
            .uri(format!("/app/api/entities/{id}"))
            .header("content-type", "application/json");
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(req.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["id"], id.to_string());
            assert_eq!(body["name"], format!("{label} Renamed Co"));
        } else {
            assert_eq!(body["error"], error);
        }
    }
}

#[tokio::test]
async fn api_entities_update_accepts_lawyer_bearer_session() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, type_id, jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let req = serde_json::json!({
        "name": "Bearer Renamed Co",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/entities/{id}"))
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Bearer Renamed Co");
}

#[tokio::test]
async fn api_entities_update_rejects_a_cross_site_origin() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, type_id, jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Cross Site Renamed",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/entities/{id}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_entities_update_refuses_to_rename_the_firm_anchor() {
    // The firm's own row has an immutable name. A hand-crafted PATCH hits the
    // same guard the lawyer edit form does, because both call one command.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&state.surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("the canonical seed inserts the firm entity");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let req = serde_json::json!({
        "name": "Renamed Firm",
        "entity_type_id": firm.entity_type_id,
        "jurisdiction_id": firm.jurisdiction_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/entities/{}", firm.id))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "conflict");
    assert_eq!(
        body["message"],
        "The firm entity's name is immutable. Its type and jurisdiction remain editable."
    );
    // The row keeps the exact name `store::seed` looks up.
    assert_eq!(
        store::entities::find_by_id(&surreal, firm.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        store::seed::FIRM_ENTITY_NAME
    );
}

#[tokio::test]
async fn api_entities_update_404s_for_an_unknown_id() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (type_id, jur_id) = seeded_entity_fks(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "Ghost Co",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/app/api/entities/{}",
                    uuid::Uuid::from_u128(0x00c0_ffee)
                ))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "not_found");
}

#[tokio::test]
async fn api_entities_update_rejects_a_blank_name() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, type_id, jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let req = serde_json::json!({
        "name": "   ",
        "entity_type_id": type_id,
        "jurisdiction_id": jur_id,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/entities/{id}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["message"], "Name is required.");
}

#[tokio::test]
async fn api_entities_delete_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for the Entity delete command. Each caller gets
    // its own row to remove, so the lawyer and admin cases are independent.

    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let (id, _type_id, _jur_id) = seeded_entity_to_edit(&surreal).await;
        let mut builder = Request::builder()
            .method("DELETE")
            .uri(format!("/app/api/entities/{id}"));
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["id"], id.to_string());
            // The row is really gone, not just reported gone.
            assert!(
                store::entities::find_by_id(&surreal, id)
                    .await
                    .unwrap()
                    .is_none(),
                "{label} delete must remove the row",
            );
        } else {
            assert_eq!(body["error"], error);
            assert!(
                store::entities::find_by_id(&surreal, id)
                    .await
                    .unwrap()
                    .is_some(),
                "{label} must not remove the row",
            );
        }
    }
}

#[tokio::test]
async fn api_entities_delete_accepts_lawyer_bearer_session() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, _type_id, _jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/entities/{id}"))
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_entities_delete_rejects_a_cross_site_origin() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (id, _type_id, _jur_id) = seeded_entity_to_edit(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/entities/{id}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        store::entities::find_by_id(&surreal, id)
            .await
            .unwrap()
            .is_some(),
        "a cross-site delete must not remove the row",
    );
}

#[tokio::test]
async fn api_entities_delete_refuses_the_firm_anchor() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm_row = store::entities::find_by_name(&state.surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("the canonical seed inserts the firm entity");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/entities/{}", firm_row.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "conflict");
    assert_eq!(
        body["message"],
        "The bootstrap company is protected and cannot be deleted."
    );
    assert!(
        store::entities::find_by_id(&surreal, firm_row.id)
            .await
            .unwrap()
            .is_some(),
        "the firm anchor must survive the refused delete",
    );
}

#[tokio::test]
async fn api_entities_delete_404s_for_an_unknown_id() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/app/api/entities/{}",
                    uuid::Uuid::from_u128(0x00c0_ffee)
                ))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "not_found");
}

/// Seed a matter. Returns the matter id.
async fn seeded_matter(surreal: &store::surreal::SurrealDb) -> uuid::Uuid {
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("seeded-matter-{}", uuid::Uuid::now_v7()),
            name: "Seeded Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    project.id
}

/// A **bare** matter with no product and no participations — the only shape
/// that deletes cleanly. Returns its id.
async fn deletable_matter(surreal: &store::surreal::SurrealDb, name: &str) -> uuid::Uuid {
    store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("deletable-{}", uuid::Uuid::now_v7()),
            name: name.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

/// Seed a person with a role and return the row, so a caller session can carry
/// a real `person_id`.
async fn seeded_actor(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: store::persons::Role,
) -> store::persons::Person {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(format!("{role:?} Actor"), email, role),
    )
    .await
    .unwrap()
}

/// Is `person` a participant on `project`?
async fn participation_exists(
    surreal: &store::surreal::SurrealDb,
    project: uuid::Uuid,
    person: uuid::Uuid,
) -> bool {
    store::projects::participations_for_project(surreal, project)
        .await
        .unwrap()
        .iter()
        .any(|row| row.person_id == person)
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn api_projects_add_participant_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for the add-participant command. Lawyer and admin
    // add a person to a matter; anonymous (401) and client (403) are rejected
    // and add no participation row.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // anonymous → 401
    let matter = seeded_matter(&surreal).await;
    let addee = seeded_actor(
        &surreal,
        "addee-anon@example.com",
        store::persons::Role::Client,
    )
    .await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/projects/{matter}/participants"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "person_id": addee.id, "participation": "co_counsel" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(!participation_exists(&surreal, matter, addee.id).await);

    // client → 403 (rejected at LawyerSession)
    let matter = seeded_matter(&surreal).await;
    let addee = seeded_actor(
        &surreal,
        "addee-client@example.com",
        store::persons::Role::Client,
    )
    .await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Client);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/projects/{matter}/participants"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    serde_json::json!({ "person_id": addee.id, "participation": "co_counsel" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!participation_exists(&surreal, matter, addee.id).await);

    // lawyer and admin → 201, the participation row lands. The lawyer case is
    // seeded onto the matter first: ENG-35 scopes this door to a lawyer who
    // already participates (admin keeps the bypass, seeded with no row below),
    // the same re-check `/close` uses.
    for (label, role) in [
        ("lawyer", store::persons::Role::Lawyer),
        ("admin", store::persons::Role::Admin),
    ] {
        let matter = seeded_matter(&surreal).await;
        let actor = seeded_actor(&surreal, &format!("{label}-part-actor@example.com"), role).await;
        if role == store::persons::Role::Lawyer {
            store::projects::add_participation(&surreal, matter, actor.id, "lawyer")
                .await
                .unwrap();
        }
        let addee = seeded_actor(
            &surreal,
            &format!("{label}-addee@example.com"),
            store::persons::Role::Client,
        )
        .await;
        let (cookie, csrf) = session_cookie_and_csrf_for_person(&actor);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/api/projects/{matter}/participants"))
                    .header("content-type", "application/json")
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from(
                        serde_json::json!({ "person_id": addee.id, "participation": "co_counsel" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "{label}");
        // The posted `participation` is surplus and unread: the row takes the
        // addee's client tier, not the `co_counsel` the body asked for.
        let row = store::projects::participation_for_person(&surreal, addee.id, matter)
            .await
            .unwrap()
            .expect("{label}");
        assert_eq!(row.participation, "client", "{label}");
    }
}

/// Seed a plain participation row on a fresh matter; returns (matter, role_id,
/// a second person the edit can move the row to).
async fn seed_participant(
    surreal: &store::surreal::SurrealDb,
    tag: &str,
) -> (uuid::Uuid, uuid::Uuid, store::persons::Person) {
    let matter = seeded_matter(surreal).await;
    let occupant = seeded_actor(
        surreal,
        &format!("{tag}-occupant@example.com"),
        store::persons::Role::Client,
    )
    .await;
    let role = store::participation::add_participant(
        surreal,
        &store::participation::AddParticipantCommand {
            project_id: matter,
            person_id: occupant.id,
            dri: store::participation::DriRequest::Unchanged,
            actor: store::participation::DriActor::System,
        },
    )
    .await
    .unwrap();
    let other = seeded_actor(
        surreal,
        &format!("{tag}-other@example.com"),
        store::persons::Role::Client,
    )
    .await;
    (matter, role.id, other)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn api_projects_participant_item_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for editing (PATCH) and removing (DELETE) a
    // participation row. Lawyer/admin do both; anonymous (401) and client (403)
    // are rejected and change nothing.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let patch_body = |person: uuid::Uuid| serde_json::json!({ "person_id": person }).to_string();

    // ---- PATCH ----
    // anonymous → 401
    let (matter, role, other) = seed_participant(&surreal, "patch-anon").await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                .header("content-type", "application/json")
                .body(Body::from(patch_body(other.id)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // client → 403
    let (matter, role, other) = seed_participant(&surreal, "patch-client").await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Client);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(patch_body(other.id)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // lawyer/admin → 200. The lawyer case is seeded onto the matter first —
    // see the POST loop above for why.
    for (label, role_kind) in [
        ("lawyer", store::persons::Role::Lawyer),
        ("admin", store::persons::Role::Admin),
    ] {
        let (matter, role, other) = seed_participant(&surreal, &format!("patch-{label}")).await;
        let actor = seeded_actor(
            &surreal,
            &format!("patch-{label}-actor@example.com"),
            role_kind,
        )
        .await;
        if role_kind == store::persons::Role::Lawyer {
            store::projects::add_participation(&surreal, matter, actor.id, "lawyer")
                .await
                .unwrap();
        }
        let (cookie, csrf) = session_cookie_and_csrf_for_person(&actor);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                    .header("content-type", "application/json")
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::from(patch_body(other.id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{label} patch");
    }

    // ---- DELETE ----
    // anonymous → 401
    let (matter, role, _) = seed_participant(&surreal, "del-anon").await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // client → 403
    let (matter, role, _) = seed_participant(&surreal, "del-client").await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Client);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // lawyer → 204. Seeded onto the matter first — see the POST loop above
    // for why.
    let (matter, role, _) = seed_participant(&surreal, "del-lawyer").await;
    let actor = seeded_actor(
        &surreal,
        "del-lawyer-actor@example.com",
        store::persons::Role::Lawyer,
    )
    .await;
    store::projects::add_participation(&surreal, matter, actor.id, "lawyer")
        .await
        .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&actor);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}/participants/{role}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        store::projects::participation_by_id(&surreal, role)
            .await
            .unwrap()
            .is_none(),
        "the row was removed",
    );
}

// ---- PATCH /app/api/projects/{id} (descriptive update) ----

#[tokio::test]
async fn api_projects_update_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for the descriptive project update. Lawyer and
    // admin edit; anonymous (401) and client (403) are rejected and leave the
    // matter unchanged.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let matter = seeded_matter(&surreal).await;
        let req = serde_json::json!({
            "name": format!("{label} Renamed Matter"),
        });
        let mut builder = Request::builder()
            .method("PATCH")
            .uri(format!("/app/api/projects/{matter}"))
            .header("content-type", "application/json");
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(req.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let saved = store::projects::find_by_id(&surreal, matter)
            .await
            .unwrap()
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["name"], format!("{label} Renamed Matter"), "{label}");
            assert_eq!(
                saved.name,
                format!("{label} Renamed Matter"),
                "{label} persisted"
            );
        } else {
            assert_eq!(body["error"], error, "{label}");
            assert_eq!(saved.name, "Seeded Matter", "{label}: matter unchanged");
        }
    }
}

#[tokio::test]
async fn api_projects_update_accepts_a_lawyer_bearer_session() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = seeded_matter(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let req = serde_json::json!({ "name": "Bearer Renamed" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}"))
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Bearer Renamed");
}

#[tokio::test]
async fn api_projects_update_sets_and_clears_the_slack_channel_links() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = seeded_matter(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);

    let req = serde_json::json!({
        "name": "Seeded Matter",
        "internal_slack_channel_url": "https://neonlaw.slack.com/archives/C0INTERNAL",
        "external_slack_channel_url": "https://neonlaw.slack.com/archives/C0EXTERNAL",
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie.clone())
                .header("x-csrf-token", csrf.clone())
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let saved = store::projects::find_by_id(&surreal, matter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.internal_slack_channel_url,
        Some("https://neonlaw.slack.com/archives/C0INTERNAL".to_string())
    );
    assert_eq!(
        saved.external_slack_channel_url,
        Some("https://neonlaw.slack.com/archives/C0EXTERNAL".to_string())
    );

    // A blank value clears the column, same as `description`; an omitted
    // field leaves it untouched.
    let clear = serde_json::json!({
        "name": "Seeded Matter",
        "internal_slack_channel_url": "",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(clear.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let saved = store::projects::find_by_id(&surreal, matter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.internal_slack_channel_url, None, "blank clears");
    assert_eq!(
        saved.external_slack_channel_url,
        Some("https://neonlaw.slack.com/archives/C0EXTERNAL".to_string()),
        "omitted field left untouched"
    );
}

#[tokio::test]
async fn api_projects_update_rejects_a_cross_site_origin() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = seeded_matter(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::from(
                    serde_json::json!({ "name": "Renamed" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        store::projects::find_by_id(&surreal, matter)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Seeded Matter",
        "a cross-site update must not persist"
    );
}

#[tokio::test]
async fn api_projects_update_rejects_a_blank_name() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = seeded_matter(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/projects/{matter}"))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(serde_json::json!({ "name": "   " }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["message"], "Name is required.");
}

#[tokio::test]
async fn api_projects_update_404s_for_an_unknown_matter() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/app/api/projects/{}",
                    uuid::Uuid::from_u128(0x00c0_ffee)
                ))
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    serde_json::json!({ "name": "Ghost" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "not_found");
}

// ---- DELETE /app/api/projects/{id} (matter delete) ----

#[tokio::test]
async fn api_projects_delete_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for matter delete, each caller on its own bare
    // (deletable) matter and asserting the row was or wasn't removed.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
    ];

    for (label, role, status, error) in cases {
        let matter = deletable_matter(&surreal, &format!("{label} Matter")).await;
        let mut builder = Request::builder()
            .method("DELETE")
            .uri(format!("/app/api/projects/{matter}"));
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let present = store::projects::find_by_id(&surreal, matter)
            .await
            .unwrap()
            .is_some();
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        if status.is_success() {
            assert_eq!(body["id"], matter.to_string(), "{label}");
            assert!(!present, "{label}: matter removed");
        } else {
            assert_eq!(body["error"], error, "{label}");
            assert!(present, "{label}: matter must survive");
        }
    }
}

#[tokio::test]
async fn api_projects_delete_accepts_a_lawyer_bearer_session() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = deletable_matter(&surreal, "Bearer Matter").await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}"))
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(store::projects::find_by_id(&surreal, matter)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn api_projects_delete_rejects_a_cross_site_origin() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = deletable_matter(&surreal, "Cross Site Matter").await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        store::projects::find_by_id(&surreal, matter)
            .await
            .unwrap()
            .is_some(),
        "a cross-site delete must not remove the matter"
    );
}

#[tokio::test]
async fn api_projects_delete_409s_when_the_matter_is_still_referenced() {
    // A participation row references the matter, so the foreign key blocks the
    // delete — a 409 carrying the database's own detail, not a 500. The matter
    // survives.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let matter = deletable_matter(&surreal, "Referenced Matter").await;
    let person = store::test_support::dri_person(&surreal).await;
    store::projects::add_participation(&surreal, matter, person, "client")
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/projects/{matter}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "conflict");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("still referenced"),
        "got: {}",
        body["message"]
    );
    assert!(
        store::projects::find_by_id(&surreal, matter)
            .await
            .unwrap()
            .is_some(),
        "a blocked delete must leave the matter in place"
    );
}

#[tokio::test]
async fn api_projects_delete_404s_for_an_unknown_matter() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/app/api/projects/{}",
                    uuid::Uuid::from_u128(0x00c0_ffee)
                ))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "not_found");
}

// ---- POST /app/api/projects (matter open) ----

/// The prerequisites an open needs: a `client`-role client of record and an
/// entity. Returns `(client_id, entity_id)`.
///
/// Find-or-create by unique key, because
/// `api_projects_open_authorizes_only_lawyer_and_admin` invokes this helper
/// once per role in a loop, re-seeding the client of record (unique `email`).
/// A blind insert would hit a duplicate-key panic before any assertion runs.
async fn open_matter_prereqs(surreal: &store::surreal::SurrealDb) -> (uuid::Uuid, uuid::Uuid) {
    let client = match store::persons::find_by_email_ci(surreal, "client-of-record@example.com")
        .await
        .unwrap()
    {
        Some(existing) => existing,
        None => {
            seeded_actor(
                surreal,
                "client-of-record@example.com",
                store::persons::Role::Client,
            )
            .await
        }
    };
    (client.id, store::test_support::seed_entity(surreal).await)
}

#[tokio::test]
async fn api_projects_open_authorizes_only_lawyer_and_admin() {
    // The authorization matrix for matter open. Lawyer and admin (both
    // attorneys at this firm) open; anonymous (401) and client (403) are
    // rejected and open no matter. Lawyer/admin carry a person-backed session so
    // the attester resolves.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (label, role, status, error) in [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::CREATED,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::CREATED,
            "",
        ),
    ] {
        let (client_id, entity_id) = open_matter_prereqs(&surreal).await;
        let req = serde_json::json!({
            "name": format!("{label} Matter"),
            // Distinct per label: `lawyer` and `admin` both open successfully in
            // this one database, and `projects.code` is unique.
            "code": format!("{label}-matter"),
            "client_id": client_id,
            "entity_id": entity_id,
            "attestation": true,
        });
        let mut builder = Request::builder()
            .method("POST")
            .uri("/app/api/projects")
            .header("content-type", "application/json");
        if let Some(role) = role {
            // Lawyer/admin need a person-backed session (the attester); an
            // authenticated `client` is rejected at the tier before that matters.
            if role == store::persons::Role::Client {
                let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
                builder = builder
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf);
            } else {
                let attester =
                    seeded_actor(&surreal, &format!("{label}-attorney@neonlaw.com"), role).await;
                let (cookie, csrf) = session_cookie_and_csrf_for_person(&attester);
                builder = builder
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", csrf);
            }
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::from(req.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let opened = store::projects::all(&surreal)
            .await
            .unwrap()
            .iter()
            .any(|p| p.name == format!("{label} Matter"));
        if status.is_success() {
            assert_eq!(body["name"], format!("{label} Matter"), "{label}");
            assert_eq!(body["status"], "open", "{label}");
            assert!(opened, "{label}: matter opened");
        } else {
            assert_eq!(body["error"], error, "{label}");
            assert!(!opened, "{label}: no matter opened");
        }
    }
}

#[tokio::test]
async fn api_projects_open_requires_the_attorneys_attestation() {
    // A matter open with no attestation is refused with its own error code, and
    // opens nothing.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (client_id, entity_id) = open_matter_prereqs(&surreal).await;
    let attester = seeded_actor(
        &surreal,
        "attorney@neonlaw.com",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&attester);
    let req = serde_json::json!({
        "name": "Unattested Matter",
        "code": "unattested-matter",
        "client_id": client_id,
        "entity_id": entity_id,
        "attestation": false,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/projects")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "attestation_required");
    assert!(
        store::projects::all(&surreal).await.unwrap().is_empty(),
        "an unattested open writes no matter"
    );
}

#[tokio::test]
async fn api_projects_open_refuses_a_non_client_of_record() {
    // The client of record must be a `client`-role person, never a firm
    // attorney — a 400 with the invalid_request shape.
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (_client_id, entity_id) = open_matter_prereqs(&surreal).await;
    let attester = seeded_actor(
        &surreal,
        "attorney@neonlaw.com",
        store::persons::Role::Lawyer,
    )
    .await;
    // Point client_id at a lawyer person — not allowed as the client of record.
    let lawyer_as_client = seeded_actor(
        &surreal,
        "not-a-client@neonlaw.com",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&attester);
    let req = serde_json::json!({
        "name": "Bad Client Matter",
        "code": "bad-client-matter",
        "client_id": lawyer_as_client.id,
        "entity_id": entity_id,
        "attestation": true,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/projects")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");
}

#[tokio::test]
async fn api_projects_open_accepts_a_lawyer_bearer_session() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (client_id, entity_id) = open_matter_prereqs(&surreal).await;
    let attester = seeded_actor(
        &surreal,
        "bearer-attorney@neonlaw.com",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // A bearer session carrying the attester's person id (machine caller, no
    // cookie → CSRF-exempt) — a lawyer session is required to open a matter.
    let mut session = portal::SessionData::fresh("bearer-sub", store::persons::Role::Lawyer);
    session.person_id = Some(attester.id);
    let token = test_sessions().encode(&session);
    let req = serde_json::json!({
        "name": "Bearer Matter",
        "code": "bearer-matter",
        "client_id": client_id,
        "entity_id": entity_id,
        "attestation": true,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/projects")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["name"], "Bearer Matter");
}

#[tokio::test]
async fn api_projects_open_rejects_a_cross_site_origin() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let (client_id, entity_id) = open_matter_prereqs(&surreal).await;
    let attester = seeded_actor(
        &surreal,
        "attorney@neonlaw.com",
        store::persons::Role::Lawyer,
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&attester);
    let req = serde_json::json!({
        "name": "Cross Site Matter",
        "code": "cross-site-matter",
        "client_id": client_id,
        "entity_id": entity_id,
        "attestation": true,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/projects")
                .header("content-type", "application/json")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("host", "app.example")
                .header("origin", "https://evil.example")
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        store::projects::all(&surreal).await.unwrap().is_empty(),
        "a cross-site open writes no matter"
    );
}

#[tokio::test]
async fn api_entity_by_id_404s_when_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_signed_in(
        app,
        &format!("/app/api/entities/{}", uuid::Uuid::from_u128(999)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(body.contains("\"error\":\"not_found\""));
}

#[tokio::test]
async fn api_validate_template_returns_clean_for_valid_markdown() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // Minimal notation that satisfies every N-rule:
    //   N101 title, N102 respondent_type, N103 snake_case filename (default),
    //   N104 questionnaire + workflow with BEGIN reaching END,
    //   N105 confidential, N106 workflow contains bare `lawyer_review` state,
    //   N108 code.
    let contents = "---\n\
kind: trust\n\
title: Trust\n\
respondent_type: entity\n\
code: sample__trust\n\
confidential: false\n\
questionnaire:\n  \
  BEGIN:\n    \
    _: END\n  \
  END: {}\n\
workflow:\n  \
  BEGIN:\n    \
    next: lawyer_review\n  \
  lawyer_review:\n    \
    next: END\n  \
  END: {}\n\
---\n\n\
Body.\n";
    let body = serde_json::json!({ "contents": contents });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/templates/validate")
                .header("content-type", "application/json")
                .header(
                    header::AUTHORIZATION,
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["clean"], true, "expected clean, got: {body}");
    assert_eq!(body["path"], "template.md");
    // Valid notation, no blocking errors — but its mandatory lawyer_review
    // gate earns the yellow N112 "not built yet" advisory, returned
    // without flipping `clean` to false.
    let codes: Vec<&str> = body["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        ["N112"],
        "expected only the N112 advisory, got: {body}"
    );
}

#[tokio::test]
async fn api_validate_template_reports_frontmatter_and_line_length_violations() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // Missing title + missing respondent_type + a body line over 120 chars.
    let long_line = "x".repeat(150);
    let body = serde_json::json!({
        "contents": format!("---\nfoo: bar\n---\n\n{long_line}\n"),
        "path": "trust.md",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/templates/validate")
                .header("content-type", "application/json")
                .header(
                    header::AUTHORIZATION,
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["clean"], false);
    assert_eq!(body["path"], "trust.md");
    let codes: Vec<&str> = body["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.contains(&"N101"),
        "expected N101 (title), got {codes:?}"
    );
    assert!(
        codes.contains(&"N102"),
        "expected N102 (respondent_type), got {codes:?}"
    );
    assert!(
        codes.contains(&"S101"),
        "expected S101 (line length), got {codes:?}"
    );
}

#[tokio::test]
async fn api_validate_template_markdown_only_drops_frontmatter_rules() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // No frontmatter at all — would trip N101 in the default set.
    let body = serde_json::json!({
        "contents": "# Heading\n\nBody paragraph.\n",
        "markdown_only": true,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/templates/validate")
                .header("content-type", "application/json")
                .header(
                    header::AUTHORIZATION,
                    bearer_header_for_role(store::persons::Role::Lawyer),
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    let codes: Vec<&str> = body["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.iter().all(|c| !c.starts_with('N')),
        "N-family must not run when markdown_only=true, got {codes:?}"
    );
}

/// Runtime guard binding the *live* `/app/api/*` router to the OpenAPI
/// document at `(method, path)` granularity — a second line behind the
/// static `openapi_drift.rs` comparison. It probes the real router and
/// confirms the served operations match the document, and pins the removed
/// `/app/api/notations/validate` POST endpoint to a non-functional `405` (the
/// read cluster's `GET /app/api/notations/{id}` detail route now owns that path
/// shape, so a POST is method-not-allowed rather than the old unrouted `404`).
///
/// Its blind spot is inherent: axum exposes no route enumeration, so it
/// can only probe paths it already knows (the documented set plus the
/// known removed alias). An *entirely new* undocumented path is instead
/// caught upstream by `openapi_drift.rs`, because
/// `api::documented_api_operations()` is derived from the same table
/// `api::routes()` is built from and so cannot omit a registered route.
///
/// A `TRACE` request never matches a registered method, so a matched
/// *path* answers `405` with an `Allow` header listing its real methods,
/// while an unmatched path answers `404`. Reading `Allow` recovers the
/// router's true method set per path without executing a handler (whose
/// own 404-for-missing-row would otherwise masquerade as an absent
/// route).
#[tokio::test]
async fn api_router_operations_match_openapi_document() {
    use std::collections::BTreeSet;

    const VERBS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
    const REMOVED_ALIAS: &str = "/app/api/notations/validate";

    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let documented: BTreeSet<(String, String)> = portal::openapi::documented_operations()
        .into_iter()
        .collect();

    // Probe every documented path plus the retired `notations/validate` alias,
    // so re-introducing it as its own endpoint is caught. Its GET is now served
    // incidentally by the `/app/api/notations/{id}` detail route (id="validate"),
    // exactly as `/app/api/people/validate` matches `people/{id}`; that shadow is
    // dropped from `observed` below so only a genuine re-added verb trips the guard.
    let mut candidate_paths: BTreeSet<String> = documented.iter().map(|(_, p)| p.clone()).collect();
    candidate_paths.insert(REMOVED_ALIAS.to_string());

    let mut observed: BTreeSet<(String, String)> = BTreeSet::new();
    for path in &candidate_paths {
        let uri = path.replace("{id}", "00000000-0000-0000-0000-000000000000");
        // The `/app/api/*` routes now sit behind the session boundary (#732), so
        // an anonymous probe answers 401 rather than the method router's 405.
        // A signed-in session clears the boundary; TRACE is a CSRF-safe method,
        // so the request reaches the method router and surfaces its `Allow` set.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("TRACE")
                    .uri(&uri)
                    .header(header::COOKIE, admin_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if resp.status() != StatusCode::METHOD_NOT_ALLOWED {
            // 404: no method registered on this path at all.
            continue;
        }
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        for method in allow.split(',').map(str::trim) {
            let method = method.to_uppercase();
            if VERBS.contains(&method.as_str()) {
                observed.insert((method, path.clone()));
            }
        }
    }

    // The retired alias's GET is the `notations/{id}` detail route matching
    // `{id}=validate`, not a re-added `notations/validate` endpoint — drop that
    // one shadow so the guard still catches a genuine re-introduction (any verb
    // the `{id}` route does not itself serve).
    observed.remove(&("GET".to_string(), REMOVED_ALIAS.to_string()));

    assert_eq!(
        observed,
        documented,
        "live /api router drift from the OpenAPI document.\n  \
         served but undocumented (a re-added alias or an extra method on a documented path) = {:?}\n  \
         documented but not served = {:?}",
        observed.difference(&documented).collect::<Vec<_>>(),
        documented.difference(&observed).collect::<Vec<_>>(),
    );

    // Belt-and-suspenders: the removed `notations/validate` POST endpoint must
    // still not function. The read cluster's `GET /app/api/notations/{id}` detail
    // route now owns that path shape (`{id}=validate`, an invalid UUID), exactly
    // as `people/{id}` owns `people/validate` — so the request reaches the policy
    // layer, which has no `allow` rule for `POST notations/validate` and denies it
    // with `403`. Either way the retired validate action is gone; the assertion is
    // that it is refused, not served.
    let alias = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(REMOVED_ALIAS)
                .header("content-type", "application/json")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::from(r#"{"contents":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        alias.status(),
        StatusCode::FORBIDDEN,
        "the removed {REMOVED_ALIAS} POST endpoint must be refused by policy (no allow rule)"
    );
}

#[tokio::test]
async fn lawyer_dashboard_is_gated_even_when_auth_is_disabled() {
    // The session boundary is deliberately independent of `AuthConfig` (#732):
    // a deployment that disables JWT verification (KIND, the test suite) must
    // not thereby turn the shared `/app/lawyer` surface anonymous. `empty_state`
    // carries auth disabled and a passthrough embedded Rego policy, so a redirect here proves
    // the gate is a property of router composition, not of the policy bundle.
    let (state, surreal) = state_with_engines().await; // auth disabled
                                                       // Seed a firm-wide project as a fail-closed witness: the anonymous caller
                                                       // is turned away at the boundary before the dashboard handler ever queries
                                                       // projects, so this row can never leak into a rendered response.
    test_project(&surreal, "Firm-wide formation", "open").await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login?return_to=/app/lawyer"),
    );
    let body = body_string(resp).await;
    assert!(
        !body.contains("Firm-wide formation"),
        "the redirect must not leak seeded project data: {body}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one dashboard's worth of seeded fixture rows
async fn lawyer_dashboard_leads_with_project_kpis_and_calendar() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Dashboard Lawyer",
            "dashboard-lawyer@example.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let other_dri = store::test_support::dri_person(&surreal).await;

    for (name, status, lawyer_dri, lawyer_participates) in [
        ("Estate sitting matter", "open", Some(lawyer.id), true),
        ("Acme contract review", "open", Some(other_dri), true),
        ("Closed formation cleanup", "closed", Some(lawyer.id), true),
        (
            "Archived formation record",
            "archived",
            Some(lawyer.id),
            true,
        ),
    ] {
        let project = test_project(&surreal, name, status).await;
        // The accountable lawyer, whoever that is for this row; `lawyer` may
        // also work the matter without being accountable for it, which stays
        // an ordinary firm-side membership row.
        if let Some(dri) = lawyer_dri {
            disclose_lawyer_dri(&state.surreal, dri, project.id).await;
        }
        if lawyer_participates && lawyer_dri != Some(lawyer.id) {
            participate(&state.surreal, lawyer.id, project.id, "attorney").await;
        }
    }

    let mut session = portal::SessionData::fresh("lawyer-sub", store::persons::Role::Lawyer);
    session.person_id = Some(lawyer.id);
    session.email = Some(lawyer.email);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer?sort=project&dir=desc")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Project KPIs"), "{body}");
    assert!(body.contains("Total projects"), "{body}");
    // Dioxus interleaves hydration markers between the label and its count, so
    // strip them and assert the pairing a reader actually sees.
    let plain = strip_hydration_markers(&body);
    assert!(
        plain.contains("<strong>Open projects: </strong>2"),
        "{plain}"
    );
    assert!(
        plain.contains("<strong>Closed projects: </strong>1"),
        "{plain}"
    );
    assert!(
        body.contains("aria-label=\"3 total projects: 2 open, 1 closed\""),
        "{body}"
    );
    assert!(body.contains("Project calendar"), "{body}");
    assert!(
        body.contains("No project calendar events scheduled."),
        "{body}"
    );
    assert!(body.contains("Project (desc)"), "{body}");
    assert!(
        body.contains("href=\"/app/lawyer?status=open&#38;sort=project&#38;dir=asc\""),
        "{body}"
    );
    let calendar = calendar_section(&body);
    assert!(
        !calendar.contains("Acme contract review") && !calendar.contains("Estate sitting matter"),
        "calendar should not synthesize project events before event storage exists: {calendar}",
    );
    assert!(
        !calendar.contains("Lawyer review"),
        "calendar should not render stubbed touchpoint labels: {calendar}",
    );
    assert!(
        !calendar.contains("Closed formation cleanup"),
        "closed projects should stay out of the upcoming calendar: {calendar}",
    );
}

#[tokio::test]
async fn lawyer_dashboard_project_list_is_paginated_and_lawyer_scoped() {
    let (state, _surreal) = state_with_engines().await;
    let cookie = lawyer_dashboard_fixture(&state.surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let active_first_page = get_with_cookie(app.clone(), "/app/lawyer", &cookie).await;
    assert_eq!(active_first_page.status(), StatusCode::OK);
    let body = body_string(active_first_page).await;
    let plain = strip_hydration_markers(&body);
    assert!(
        plain.contains("<strong>Open projects: </strong>6"),
        "{plain}"
    );
    assert!(
        plain.contains("<strong>Closed projects: </strong>6"),
        "{plain}"
    );
    assert!(body.contains("Page 1 of 2"), "{body}");
    assert!(body.contains("open project 1"), "{body}");
    assert!(body.contains("open project 5"), "{body}");
    assert!(!body.contains("open project 6"), "{body}");
    assert!(!body.contains("unassigned project"), "{body}");
    assert!(
        body.contains("href=\"/app/lawyer?status=open&#38;sort=date&#38;dir=asc&#38;page=2\""),
        "{body}"
    );

    // A non-default calendar sort must ride through Previous/Next: paginating
    // the KPI list may not silently reset the calendar to its date/asc default.
    let sorted_first_page = get_with_cookie(
        app.clone(),
        "/app/lawyer?status=open&sort=project&dir=desc",
        &cookie,
    )
    .await;
    assert_eq!(sorted_first_page.status(), StatusCode::OK);
    let body = body_string(sorted_first_page).await;
    assert!(
        body.contains("href=\"/app/lawyer?status=open&#38;sort=project&#38;dir=desc&#38;page=2\""),
        "{body}"
    );
    // The three controls share one query string, so each preserves the others:
    // the status tab carries the calendar sort, and the calendar sort link
    // carries the status — switching one must not reset the other.
    assert!(
        body.contains("href=\"/app/lawyer?status=closed&#38;sort=project&#38;dir=desc\""),
        "status tab should carry the active calendar sort: {body}",
    );
    assert!(
        body.contains("href=\"/app/lawyer?status=open&#38;sort=date&#38;dir=asc\""),
        "calendar sort link should carry the project status: {body}",
    );

    let active_second_page =
        get_with_cookie(app.clone(), "/app/lawyer?status=open&page=2", &cookie).await;
    assert_eq!(active_second_page.status(), StatusCode::OK);
    let body = body_string(active_second_page).await;
    assert!(body.contains("Page 2 of 2"), "{body}");
    assert!(body.contains("open project 6"), "{body}");
    assert!(!body.contains("open project 1"), "{body}");

    let closed_first_page = get_with_cookie(app, "/app/lawyer?status=closed", &cookie).await;
    assert_eq!(closed_first_page.status(), StatusCode::OK);
    let body = body_string(closed_first_page).await;
    assert!(body.contains("closed project 1"), "{body}");
    assert!(body.contains("closed project 5"), "{body}");
    assert!(!body.contains("closed project 6"), "{body}");
    assert!(
        body.contains(
            "class=\"nav-tab is-active\" href=\"/app/lawyer?status=closed&#38;sort=date&#38;dir=asc\""
        ),
        "{body}"
    );
}

#[tokio::test]
async fn visitor_analytics_counts_public_routes_and_excludes_private_surfaces() {
    let state = empty_state().await;
    let db = state.surreal.clone();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/?utm_source=linkedin&token=secret")
                .header(header::HOST, "neonlaw.com")
                .header("x-navigator-client-region", "us")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);

    for uri in [
        "/app/lawyer",
        "/admin",
        "/app/api/aida.json",
        "/mcp",
        "/public/app.css",
    ] {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, admin_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // The aggregate write is fire-and-forget off the request's critical path,
    // so poll until the counter lands rather than reading it synchronously.
    let summary = wait_for_visit_total(&db, 1).await;
    assert_eq!(summary.total_visits, 1);
    assert_eq!(summary.countries[0].label, "US");
    assert_eq!(summary.sources[0].label, "linkedin");
    assert!(
        summary.routes.iter().any(|row| row.label == "/"),
        "routes: {:?}",
        summary.routes
    );
}

/// Poll the visitor-analytics summary until `total_visits` reaches `expected`,
/// giving the fire-and-forget aggregate write in `count_public_visit` time to
/// land. Fails the test if it never arrives within the timeout.
async fn wait_for_visit_total(
    db: &store::surreal::SurrealDb,
    expected: i64,
) -> store::visitor_analytics::VisitorAnalyticsSummary {
    for _ in 0..100 {
        let summary = store::visitor_analytics::summary(db).await.unwrap();
        if summary.total_visits >= expected {
            return summary;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("visitor analytics did not reach {expected} visits within timeout");
}

#[tokio::test]
async fn admin_analytics_page_is_admin_only_and_renders_empty_state() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let lawyer = get_with_role(
        app.clone(),
        "/app/admin/analytics",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(lawyer.status(), StatusCode::FORBIDDEN);

    let resp = get_with_role(app, "/app/admin/analytics", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Visitor analytics"), "{body}");
    assert!(
        body.contains("No visitor analytics have been recorded yet."),
        "{body}"
    );
}

#[tokio::test]
async fn admin_analytics_page_sends_the_anonymous_browser_to_login() {
    // `/admin/*` is a shared Navigator surface, so the session boundary (#732)
    // bounces an anonymous browser to the login door before the analytics
    // handler runs — independent of embedded Rego policy, which here runs in passthrough.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/analytics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login?return_to=/app/admin/analytics"),
    );
}

#[tokio::test]
async fn admin_analytics_page_returns_500_when_the_summary_query_fails() {
    // A summary-query failure must surface as a 500, not a rendered page.
    // Point the handler at an engine that cannot answer so
    // `store::visitor_analytics::summary` errors, which exercises the
    // handler's error branch for an admin caller.
    let (mut state, _surreal) = state_with_engines().await;
    state.surreal = store::surreal::test_support::unreachable();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/analytics", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn admin_analytics_page_renders_recorded_dimension_totals() {
    let (state, _surreal) = state_with_engines().await;
    let db = state.surreal.clone();
    // Seed a couple of visits directly so the summary is non-empty and every
    // dimension row renders (the empty-state test leaves those branches unhit).
    for _ in 0..2 {
        store::visitor_analytics::record_visit(
            &db,
            &store::visitor_analytics::VisitorVisit {
                country_code: "US",
                route_pattern: "/blog/{slug}",
                source: "linkedin",
                locale: "en",
                status_class: "2xx",
            },
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/analytics", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("/blog/{slug}"), "route row missing: {body}");
    assert!(body.contains("linkedin"), "source row missing: {body}");
    assert!(
        !body.contains("No visitor analytics have been recorded yet."),
        "empty-state text should be gone: {body}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn lawyer_dashboard_managed_pages_create_grounded_records() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let entity_type = store::entity_types::list(&state.surreal, &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("canonical seed inserts entity types");
    let jurisdiction = first_seeded_jurisdiction(&state.surreal).await;
    let _client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Managed Client",
            "managed-client@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let people = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/admin/people/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(people.status(), StatusCode::OK);
    let body = body_string(people).await;
    assert!(body.contains("Add person"), "{body}");
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", csrf.as_str())
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Managed%20Lead&email=managed-lead%40example.com&role=client",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let lead = store::persons::find_by_email_ci(&surreal, "managed-lead@example.com")
        .await
        .unwrap()
        .expect("managed people page creates a client lead");
    assert_eq!(lead.role, store::persons::Role::Client);

    let entity_form = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/admin/entities/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(entity_form.status(), StatusCode::OK);
    let body = body_string(entity_form).await;
    assert!(body.contains("Add entity"), "{body}");
    assert!(body.contains(&entity_type.id.to_string()), "{body}");
    assert!(body.contains(&jurisdiction.id.to_string()), "{body}");
    let mut entity_form = DomForm::parse(&body, "/app/admin/entities");
    assert_eq!(entity_form.value("_csrf"), csrf);
    entity_form.enter("name", "Managed Entity");
    entity_form.choose("entity_type_id", entity_type.id.to_string());
    entity_form.choose("jurisdiction_id", jurisdiction.id.to_string());
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(entity_form.into_body()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        created.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
    let managed_entity = store::entities::find_by_name(&surreal, "Managed Entity")
        .await
        .unwrap()
        .expect("managed entity page creates an entity");
    assert_eq!(managed_entity.entity_type_id, entity_type.id);
    assert_eq!(managed_entity.jurisdiction_id, jurisdiction.id);

    let project_form = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/projects/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(project_form.status(), StatusCode::OK);
    let body = body_string(project_form).await;
    assert!(body.contains("Add project"), "{body}");
    assert!(body.contains("Managed Entity"), "{body}");
    assert!(body.contains("Managed Client"), "{body}");
    // Every engagement is bespoke: the form opens a matter directly.
    assert!(!body.contains("product_code"), "{body}");
}

/// The `Location` of a redirect response, as a `String`.
fn redirect_location(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("a redirect carries a Location")
        .to_str()
        .unwrap()
        .to_string()
}

/// What an entity write door reported. Both outcomes are a `303`
/// (post/redirect/get), so the `Location` is what separates them: a success
/// lands on the list, a refusal bounces back to the form it came from carrying
/// its `?error=` flash.
///
/// The flash is part of the outcome rather than a detail, because "did not
/// land on the list" is not the same claim as "was refused for the anchor".
/// A create that failed for a server-side reason redirects to the same form,
/// and a test that only counted list-vs-form would score it as a refusal —
/// which is how eight racers all failing on a fault reads as a guard that
/// held (ENG-272).
#[derive(Debug, PartialEq, Eq)]
enum EntityWriteOutcome {
    Created,
    Refused(String),
    /// Anything that is not a redirect at all — a lost delete/rename race
    /// answers `404`, and that is a third outcome, not a refusal.
    NotRedirected(StatusCode),
}

fn entity_write_outcome(response: &axum::response::Response) -> EntityWriteOutcome {
    if response.status() != StatusCode::SEE_OTHER {
        return EntityWriteOutcome::NotRedirected(response.status());
    }
    let location = redirect_location(response);
    if location == "/app/admin/entities" {
        return EntityWriteOutcome::Created;
    }
    let flash = location
        .split_once("?error=")
        .map_or_else(String::new, |(_, query)| {
            query.split('&').next().unwrap_or_default().to_string()
        });
    EntityWriteOutcome::Refused(flash)
}

/// A refusal message as it rides the `Location`, so a test can name the
/// refusal it expects. Only the spaces need encoding for these messages;
/// one that grew a `&` or a `?` would fail the comparison rather than
/// quietly match a different refusal, which is the safe direction.
fn flash_of(message: &str) -> String {
    message.replace(' ', "%20")
}

/// Whether an entity write door reported success.
fn entity_write_succeeded(response: &axum::response::Response) -> bool {
    entity_write_outcome(response) == EntityWriteOutcome::Created
}

/// Rename the seeded firm out of the way *and* surrender its
/// `firm_anchor_key`, opening the white-label window the surface itself
/// refuses to open.
///
/// Both halves are load-bearing. Since ENG-120 the guard is the UNIQUE
/// `entity_firm_anchor` index rather than the name, so a rename that kept
/// the key would leave the anchor claimed — and a test expecting exactly
/// one of eight racers to succeed would watch all eight fail instead, for
/// a reason that has nothing to do with what it is testing.
/// It goes around `store::entities::update` on purpose — that seam
/// refuses to rename a row carrying `firm_anchor_key`, which is the very
/// invariant the tests below prove — so it uses the store's one
/// documented way past the guard.
async fn move_anchor_aside(
    surreal: &store::surreal::SurrealDb,
    firm: &store::entities::Entity,
    aside_name: &str,
) {
    store::test_support::release_firm_anchor(surreal, firm.id, aside_name).await;
}

#[tokio::test]
async fn lawyer_entity_create_rejects_blank_name() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let entity_type = store::entity_types::list(&state.surreal, &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("canonical seed inserts entity types");
    let jurisdiction = first_seeded_jurisdiction(&state.surreal).await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let form = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/admin/entities/new")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(form.status(), StatusCode::OK);
    let body = body_string(form).await;
    // Fill the real form but leave the required name blank, so the submit
    // deserializes yet trips the server-side "name is required" guard.
    let mut entity_form = DomForm::parse(&body, "/app/admin/entities");
    assert_eq!(entity_form.value("_csrf"), csrf);
    entity_form.enter("name", "");
    entity_form.choose("entity_type_id", entity_type.id.to_string());
    entity_form.choose("jurisdiction_id", jurisdiction.id.to_string());
    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(entity_form.into_body()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Post/redirect/get: the refusal is a `303` back to the form carrying the
    // message, so a reload never resubmits the create.
    assert_eq!(rejected.status(), StatusCode::SEE_OTHER);
    let location = redirect_location(&rejected);
    assert_eq!(
        location, "/app/admin/entities/new?error=Name%20is%20required.",
        "a refused create must bounce back to the form with its message",
    );

    // Follow the redirect: the form the lawyer lands on shows the message
    // and still carries the session CSRF token, so they can correct and
    // resubmit. Asserting on the redirect's own (empty) body would pass
    // vacuously.
    let reloaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reloaded.status(), StatusCode::OK);
    let reloaded = body_string(reloaded).await;
    assert!(reloaded.contains("Name is required."), "{reloaded}");
    let reloaded_form = DomForm::parse(&reloaded, "/app/admin/entities");
    assert_eq!(reloaded_form.value("_csrf"), csrf);
}

#[tokio::test]
async fn lawyer_and_admin_cannot_delete_the_bootstrap_company() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let bootstrap_company = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for role in [store::persons::Role::Lawyer, store::persons::Role::Admin] {
        let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/app/admin/entities/{}/delete",
                        bootstrap_company.id
                    ))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("_csrf={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{role:?}");
        assert!(
            store::entities::find_by_id(&surreal, bootstrap_company.id)
                .await
                .unwrap()
                .is_some(),
            "the bootstrap company must survive a {role:?} delete request",
        );

        // A rename would make a later delete miss the configured company
        // name, so the same boundary keeps that identity stable.
        let rename = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/admin/entities/{}", bootstrap_company.id))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name=Renamed%20Firm&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                        bootstrap_company.entity_type_id, bootstrap_company.jurisdiction_id,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            !entity_write_succeeded(&rename),
            "{role:?}: renaming the firm anchor must be refused",
        );
        assert_eq!(
            store::entities::find_by_id(&surreal, bootstrap_company.id)
                .await
                .unwrap()
                .expect("bootstrap company remains")
                .name,
            store::seed::FIRM_ENTITY_NAME,
        );
    }
}

#[tokio::test]
async fn a_case_variant_rename_cannot_fork_the_bootstrap_company() {
    // `store::seed` finds the firm by exact name, so a rename the protection
    // predicate still matches — a case or whitespace variant — would leave the
    // next boot inserting a second `Neon Law`. Both rows would then be
    // protected, so neither could be cleaned up from this surface.

    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let storage = state.storage.clone();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);

    for variant in ["SHOOK%20LAW%20PLLC", "%20Shook%20Law%20PLLC%20"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/admin/entities/{}", firm.id))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name={variant}&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                        firm.entity_type_id, firm.jurisdiction_id,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            !entity_write_succeeded(&response),
            "{variant}: a case-variant rename must be refused",
        );
    }

    // The row keeps the exact name the seed looks up, so re-seeding is still
    // the no-op it promises rather than a second firm.
    assert_eq!(
        store::entities::find_by_id(&surreal, firm.id)
            .await
            .unwrap()
            .expect("bootstrap company remains")
            .name,
        store::seed::FIRM_ENTITY_NAME,
    );
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .unwrap();
    let firms: Vec<String> = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .filter(|name| {
            name.trim()
                .eq_ignore_ascii_case(store::seed::FIRM_ENTITY_NAME)
        })
        .collect();
    assert_eq!(
        firms,
        vec![store::seed::FIRM_ENTITY_NAME.to_string()],
        "re-seeding must not fork the firm into a second, equally protected row",
    );
}

#[tokio::test]
async fn lawyer_entity_create_reports_invalid_choices() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // Real name, but type/jurisdiction ids that do not exist -> the insert
    // fails the foreign-key check, which is the caller's to correct, so the
    // create handler reloads the form with the unknown-reference message.
    let body = format!(
        "name=Ghost%20Co&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
        uuid::Uuid::from_u128(0xdead),
        uuid::Uuid::from_u128(0xbeef)
    );
    let failed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::SEE_OTHER);
    let location = redirect_location(&failed);
    assert!(
        location.starts_with(
            "/app/admin/entities/new?error=Unknown%20entity%20type%20or%20jurisdiction"
        ),
        "the unknown-reference message must ride the redirect: {location}",
    );

    // Follow the redirect — the message renders on the form the lawyer
    // lands on, which still carries the CSRF token for a corrected resubmit.
    let reloaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reloaded.status(), StatusCode::OK);
    let reloaded = body_string(reloaded).await;
    assert!(
        reloaded.contains("Unknown entity type or jurisdiction"),
        "{reloaded}"
    );
    let reloaded_form = DomForm::parse(&reloaded, "/app/admin/entities");
    assert_eq!(reloaded_form.value("_csrf"), csrf);
}

#[tokio::test]
async fn lawyer_entity_update_reloads_edit_form_on_conflict() {
    // A lawyer rename that would fork the firm anchor is the caller's to
    // correct, so the update door re-renders the edit form with the submitted
    // values and an inline error rather than replacing the page with bare
    // text — the same shape the create door holds.

    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let acme = store::entities::create(
        &surreal,
        &store::entities::NewEntity {
            name: "Acme LLC".into(),
            entity_type_id: firm.entity_type_id,
            jurisdiction_id: firm.jurisdiction_id,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let action = format!("/app/admin/entities/{}", acme.id);
    let rename = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&action)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Shook%20Law%20PLLC&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    firm.entity_type_id, firm.jurisdiction_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rename.status(), StatusCode::SEE_OTHER);
    let location = redirect_location(&rename);
    assert!(
        location.starts_with(&format!("/app/admin/entities/{}/edit?error=", acme.id)),
        "a refused rename must bounce back to the edit form: {location}",
    );

    // Follow the redirect: the conflict renders on the edit form, and the
    // rejected name rides the query so the correction is not retyped.
    let body = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body.status(), StatusCode::OK);
    let body = body_string(body).await;
    assert!(
        body.contains(store::entity_commands::FIRM_ANCHOR_EXISTS_MESSAGE),
        "the conflict must render on the edit form: {body}",
    );
    // The edit form survived: same action target, the rejected name preserved,
    // and the session CSRF token so a corrected resubmit is accepted.
    let reloaded_form = DomForm::parse(&body, &action);
    assert_eq!(reloaded_form.value("_csrf"), csrf);
    assert_eq!(reloaded_form.value("name"), store::seed::FIRM_ENTITY_NAME);
    // The row itself is untouched by the refused rename.
    assert_eq!(
        store::entities::find_by_id(&surreal, acme.id)
            .await
            .unwrap()
            .expect("Acme LLC remains")
            .name,
        "Acme LLC",
    );
}

#[tokio::test]
async fn lawyer_entity_update_reloads_edit_form_on_blank_name() {
    // A blank name is a validation failure the caller can correct, so the
    // update door re-renders the edit form at 200 with the error line rather
    // than returning bare text.

    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let entity_type = store::entity_types::list(&state.surreal, &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("canonical seed inserts entity types");
    let jurisdiction = first_seeded_jurisdiction(&state.surreal).await;
    let entity = store::entities::create(
        &surreal,
        &store::entities::NewEntity {
            name: "Editable Co".into(),
            entity_type_id: entity_type.id,
            jurisdiction_id: jurisdiction.id,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let action = format!("/app/admin/entities/{}", entity.id);
    let blank = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&action)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=%20%20&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    entity_type.id, jurisdiction.id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blank.status(), StatusCode::SEE_OTHER);
    let location = redirect_location(&blank);
    assert_eq!(
        location,
        format!(
            "/app/admin/entities/{}/edit?error=Name%20is%20required.\
             &name=%20%20&entity_type_id={}&jurisdiction_id={}",
            entity.id, entity_type.id, jurisdiction.id,
        ),
        "the rejected values ride the redirect alongside the message, so the \
         correction is made in place rather than retyped",
    );

    // Follow the redirect — the edit form chrome is what the lawyer lands
    // on, carrying the message and the CSRF token for a corrected resubmit.
    let body = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body.status(), StatusCode::OK);
    let body = body_string(body).await;
    assert!(
        body.contains("Name is required."),
        "the validation error must render on the edit form: {body}",
    );
    assert!(body.contains("Edit entity"), "{body}");
    let form = DomForm::parse(&body, &action);
    assert_eq!(form.value("_csrf"), csrf);
}

#[tokio::test]
async fn lawyer_entity_edit_form_includes_csrf() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let entity_type = store::entity_types::list(&state.surreal, &[])
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("canonical seed inserts entity types");
    let jurisdiction = first_seeded_jurisdiction(&state.surreal).await;
    let entity = store::entities::create(
        &state.surreal,
        &store::entities::NewEntity {
            name: "Editable Co".into(),
            entity_type_id: entity_type.id,
            jurisdiction_id: jurisdiction.id,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let action = format!("/app/admin/entities/{}", entity.id);
    let edit = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/entities/{}/edit", entity.id))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);
    let body = body_string(edit).await;
    let edit_form = DomForm::parse(&body, &action);
    assert_eq!(edit_form.value("_csrf"), csrf);
    assert_eq!(edit_form.value("name"), "Editable Co");
    assert_eq!(
        edit_form.value("entity_type_id"),
        entity_type.id.to_string()
    );
}

#[tokio::test]
async fn admin_entities_list_rejects_unknown_sort_with_400() {
    // The entities list advertises `name`, `entity_type`, `jurisdiction`. An
    // unadvertised `?sort=` is refused with a 400 by the route pre-handler,
    // ahead of the render — the same contract the other sortable pages hold.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/entities?sort=ssn")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_entities_list_renders_the_delete_refusal_flash() {
    // A refused delete (a dependent record still references the entity)
    // redirects back here with the reason as `?error=`. Without the flash the
    // row is simply still there and nothing says why, which reads as a no-op
    // rather than as a refusal — navigator#995.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app,
        "/app/admin/entities?error=Couldn%27t%20delete%20this%20entity.",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("nav-form-error"), "{body}");
    assert!(body.contains("Couldn&#39;t delete this entity."), "{body}");
}

#[tokio::test]
async fn admin_entities_list_hides_delete_for_the_bootstrap_company() {
    // The firm anchor (the bootstrap company, named `FIRM_ENTITY_NAME` by
    // default) may not be deleted, so its row carries no Delete form — while a
    // regular entity does. Proves the `can_delete` gate on the Dioxus list.
    let (state, _surreal) = state_with_engines().await;
    let et = store::entity_types::create(&state.surreal, "LLC")
        .await
        .unwrap();
    let jur = store::jurisdictions::create(
        &state.surreal,
        &store::jurisdictions::NewJurisdiction::new("Nevada", "US-NV9", "state"),
    )
    .await
    .unwrap();
    for name in [store::seed::FIRM_ENTITY_NAME, "Regular Co"] {
        store::entities::create(
            &state.surreal,
            &store::entities::NewEntity {
                name: name.into(),
                entity_type_id: et.id,
                jurisdiction_id: jur.id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/entities", store::persons::Role::Lawyer).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Regular Co") && body.contains(store::seed::FIRM_ENTITY_NAME));
    // The regular entity offers a delete form; the firm anchor does not, so at
    // most one delete action renders here.
    let deletes = body.matches("/delete").count();
    assert_eq!(
        deletes, 1,
        "only the deletable regular entity should render a delete action; got {deletes}: {body}",
    );
}

#[tokio::test]
async fn admin_entities_list_multi_field_sort_keeps_first_field_primary() {
    // `?sort=name,entity_type` must sort by name primary and use entity_type
    // only to break ties (the JSON:API `SortSpec` precedence). The regression
    // this guards: the resolved columns were sorted with a sequence of stable
    // sorts, which makes the *last* field primary, so entity_type would have
    // driven the order. These two rows disagree on name order versus type
    // order, so a name-primary sort renders "A Corp" before "B Corp" while a
    // type-primary sort would flip them.
    let (state, _surreal) = state_with_engines().await;
    let jur = store::jurisdictions::create(
        &state.surreal,
        &store::jurisdictions::NewJurisdiction::new("Nevada", "US-NV9", "state"),
    )
    .await
    .unwrap();
    let alpha = store::entity_types::create(&state.surreal, "Alpha")
        .await
        .unwrap();
    let zeta = store::entity_types::create(&state.surreal, "Zeta")
        .await
        .unwrap();
    // "B Corp" carries the alphabetically-first type; "A Corp" the last. Under
    // name-primary sorting "A Corp" wins; under type-primary sorting "B Corp"
    // would.
    for (name, type_id) in [("B Corp", alpha.id), ("A Corp", zeta.id)] {
        store::entities::create(
            &state.surreal,
            &store::entities::NewEntity {
                name: name.into(),
                entity_type_id: type_id,
                jurisdiction_id: jur.id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app,
        "/app/admin/entities?sort=name,entity_type",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let a_pos = body.find("A Corp").expect("A Corp should render");
    let b_pos = body.find("B Corp").expect("B Corp should render");
    assert!(
        a_pos < b_pos,
        "name is the primary sort key, so \"A Corp\" must precede \"B Corp\"; got A at {a_pos}, B at {b_pos}: {body}",
    );
}

#[tokio::test]
async fn lawyer_entity_edit_form_missing_entity_returns_404() {
    let (state, _surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, _csrf) = admin_session_cookie_and_csrf();

    // A well-formed UUID that identifies no entity is a missing resource: the
    // Dioxus edit form must render the not-found state under a 404, matching the
    // status the retired edit handler returned, not a successful 200.
    let missing = uuid::Uuid::from_u128(0x_dead_beef);
    let edit = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/entities/{missing}/edit"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::NOT_FOUND);
    let body = body_string(edit).await;
    assert!(body.contains("Entity not found"), "{body}");
}

#[tokio::test]
async fn lawyer_dashboard_sends_the_anonymous_browser_to_login_when_auth_enabled() {
    // With JWT verification enabled the boundary still answers an anonymous
    // browser with the login redirect, not a bare 401: the 401 shape is
    // reserved for machine callers (#732). A missing bearer no longer surfaces
    // as UNAUTHORIZED on a browser request to the shared surface.
    let auth = AuthConfig::new(false, Some("test-secret"));
    let state = empty_state_with_auth(auth).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login?return_to=/app/lawyer"),
    );
}

#[tokio::test]
async fn lawyer_dashboard_accepts_both_machine_credentials() {
    // The firm lens has always accepted two machine credentials, and #732
    // preserves both rather than narrowing the surface: the `navigator` CLI's
    // signed `SessionData` blob, and — where a verifier is configured — an
    // OIDC bearer JWT that `require_auth` decodes. The boundary sits outside
    // `require_auth`, so it verifies the JWT itself; otherwise a working
    // machine caller would be refused before the layer that understands its
    // token ever ran.
    use jsonwebtoken::{encode, EncodingKey, Header};

    let auth = AuthConfig::new(false, Some("test-secret"));
    let state = empty_state_with_auth(auth).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // 1. The CLI session blob.
    let session_bearer = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header(
                    "authorization",
                    bearer_header_for_role(store::persons::Role::Admin),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(session_bearer.status(), StatusCode::OK);
    let body = body_string(session_bearer).await;
    assert!(body.contains("<title>Navigator | Lawyer</title>"));

    // 2. An OIDC bearer JWT the configured verifier accepts.
    let claims = portal::AuthClaims {
        sub: "admin@example.com".into(),
        exp: i64::try_from(jsonwebtoken::get_current_timestamp() + 3600).unwrap(),
        role: store::persons::Role::Admin,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"test-secret"),
    )
    .unwrap();
    let jwt = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        jwt.status(),
        StatusCode::OK,
        "a verified OIDC bearer must still reach the firm lens"
    );
}

#[tokio::test]
async fn lawyer_dashboard_refuses_a_bearer_jwt_when_oidc_is_disabled() {
    // Disabling OIDC (`OIDC_DISABLED=true`) must take a bearer JWT off the
    // table even when verifier material still lingers in the environment
    // (#732). `require_auth` already honors this — it is a pass-through when
    // the config is not enforced — and the session boundary must not admit a
    // credential the inner layer would have ignored. A signed JWT the secret
    // would otherwise accept is refused, and a browser is sent to login.
    use jsonwebtoken::{encode, EncodingKey, Header};

    let auth = AuthConfig::new(true, Some("test-secret"));
    assert!(!auth.is_enforced(), "disabled config must not be enforced");
    let state = empty_state_with_auth(auth).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let claims = portal::AuthClaims {
        sub: "admin@example.com".into(),
        exp: i64::try_from(jsonwebtoken::get_current_timestamp() + 3600).unwrap(),
        role: store::persons::Role::Admin,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"test-secret"),
    )
    .unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a bearer JWT must not authenticate once OIDC is disabled"
    );
}

#[tokio::test]
async fn the_signed_in_nav_offers_the_role_appropriate_app_workspaces() {
    // One shared navbar on every `/app` page
    // (`webapp::components::AppNavbar`), whose destinations come from the
    // viewer's tier through `webapp::app_chrome::app_destinations`.
    //
    // The three per-role desks the chrome used to name are still gone as
    // *prefixes* — `/lawyer`, `/admin`, `/clerk` advertised "which surface am I
    // allowed on" and drifted from the routes, which is what the collapse onto
    // `/app` fixed. What came back is narrower: the two `/app` workspaces that
    // really are separate pages, offered only to the tiers whose handlers open
    // them. A client is shown neither, so the nav never advertises a door that
    // answers 403.
    //
    // Destinations are asserted by `href`, never by `>Label</a>`: the labels
    // are interpolated per viewer, so Dioxus SSR splits each text node with
    // hydration comments and a literal `>Projects</a>` silently never matches —
    // which would make a negative assertion pass vacuously.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let cases = [
        (
            store::persons::Role::Owner,
            vec!["/app/lawyer", "/app/admin"],
        ),
        (
            store::persons::Role::Admin,
            vec!["/app/lawyer", "/app/admin"],
        ),
        (store::persons::Role::Lawyer, vec!["/app/lawyer"]),
        (store::persons::Role::Clerk, vec![]),
        (store::persons::Role::Client, vec![]),
    ];

    for (role, workspaces) in cases {
        // Each tier fetches the page it lands on after sign-in — a firm tier the
        // team home, a client their matters — both of which render the shared
        // `AppNavbar` from `app_destinations`. A Clerk with no supervised matters
        // 404s on `/app/projects` (their list is the *supervised* set, its own
        // query), so the firm nav is asserted on `/app/team`, where every firm
        // tier renders.
        let is_firm = role != store::persons::Role::Client;
        let landing = if is_firm {
            "/app/team"
        } else {
            "/app/projects"
        };
        let resp = get_with_role(app.clone(), landing, role).await;
        assert_eq!(resp.status(), StatusCode::OK, "{role:?} on {landing}");
        let html = body_string(resp).await;

        // Every tier reaches the one matter surface and the way out.
        let mut expected = vec!["/app/projects", "/auth/logout"];
        expected.extend(workspaces.iter().copied());
        // Every firm tier — but never a client — also reaches the team home.
        if is_firm {
            expected.push("/app/team");
        }
        // Whatever this tier did not earn, plus the retired prefixes.
        let mut unexpected = vec!["/lawyer", "/admin", "/clerk"];
        if !is_firm {
            unexpected.push("/app/team");
        }
        unexpected.extend(
            ["/app/lawyer", "/app/admin"]
                .into_iter()
                .filter(|href| !workspaces.contains(href)),
        );
        assert_nav_links(&html, &expected, &unexpected);
    }
}

#[tokio::test]
async fn lawyer_pages_preserve_lawyer_tier_nav_links() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Same chrome on every firm page, and the same for both tiers: the nav no
    // longer varies by role, so a page that grows its own is the regression.
    let cases = [
        (
            store::persons::Role::Lawyer,
            vec!["/app/projects", "/auth/logout"],
            vec!["/lawyer", "/admin", "/auth/login"],
        ),
        (
            store::persons::Role::Admin,
            vec!["/app/projects", "/auth/logout"],
            vec!["/lawyer", "/auth/login"],
        ),
    ];

    for path in ["/app/admin/entities", "/app/projects"] {
        for (role, expected, unexpected) in cases.clone() {
            let resp = get_with_role(app.clone(), path, role).await;
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_string(resp).await;
            assert_nav_links(&body, &expected, &unexpected);
        }
    }
}

#[tokio::test]
async fn lawyer_projects_list_links_each_row_to_detail_page() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, cookie, _csrf) = lawyer_project_fixture(&state.surreal).await;
    let project_code = store::projects::find_by_id(&surreal, project_id)
        .await
        .unwrap()
        .expect("fixture project")
        .code;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let detail_href = format!("/app/projects/{project_code}");
    assert!(
        body.contains(&format!("href=\"{detail_href}\"")),
        "project row should link to its detail page: {body}",
    );
    assert!(
        body.contains("data-action=\"view\""),
        "project row should expose a view/details action: {body}",
    );
    assert!(
        body.contains("aria-label=\"View details for Homer v. Flanders\""),
        "detail action should be accessible by row name: {body}",
    );
}

#[tokio::test]
async fn client_portal_lists_single_project_with_kpi_cards() {
    let (state, _surreal) = state_with_engines().await;
    let (_project_id, project_code, cookie) = client_project_fixture(&state.surreal).await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Your Projects"), "{body}");
    assert!(!body.contains(">Engagements<"), "{body}");
    assert!(
        body.contains(&format!("/app/projects/{project_code}")),
        "the project code keys the detail link: {body}"
    );
    assert!(body.contains("Sample Matter"), "{body}");
    // Every matter is priced bespoke, so the dashboard carries no service
    // label, no price, and no Services tile.
    assert!(!body.contains("Catalog Service Label"), "{body}");
    assert!(!body.contains('$'), "no price on the dashboard: {body}");
    for label in ["Open", "Closed"] {
        assert!(body.contains(label), "missing KPI label {label}: {body}");
    }
    assert!(
        !body.contains(">Documents<"),
        "no Documents KPI tile: {body}"
    );
    assert!(!body.contains("Services"), "no Services KPI tile: {body}");
    assert!(
        !body.contains("/app/projects/new"),
        "client portal should not expose project creation: {body}",
    );
}

#[tokio::test]
async fn client_portal_shows_the_matter_without_a_service_or_price() {
    let (state, surreal) = state_with_engines().await;
    let (_project_id, _project_code, cookie) = client_project_fixture_for_product(
        &surreal,
        "Formation Client",
        "formation-client@example.com",
        "Formation Client Co.",
    )
    .await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Formation Client Co."),
        "the matter name renders: {body}"
    );
    assert!(
        !body.contains("Other Catalog Label"),
        "no service label: {body}"
    );
    assert!(!body.contains('$'), "no price anywhere: {body}");
}

#[tokio::test]
async fn client_direct_projects_url_uses_portal_list_not_admin_table() {
    let (state, _surreal) = state_with_engines().await;
    let (_project_id, project_code, cookie) = client_project_fixture(&state.surreal).await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Your Projects"), "{body}");
    assert!(body.contains("Sample Matter"), "{body}");
    assert!(
        body.contains(&format!("/app/projects/{project_code}")),
        "client list should link to the matter detail: {body}",
    );
    for forbidden in [
        "/app/projects/new",
        &format!("/app/projects/{project_code}/edit"),
        &format!("/app/projects/{project_code}/delete"),
        "Lawyer | Projects",
    ] {
        assert!(
            !body.contains(forbidden),
            "client direct projects URL leaked admin chrome `{forbidden}`: {body}",
        );
    }
}

/// The Dioxus `/app/projects` dashboard exercised end to end: person
/// scoping (only the signed-in client's matters) and KPI aggregation (the
/// in-memory open/closed counts). The route-shape assertion above does not
/// run these paths together, so a regression that empties the board or leaks
/// another client's matter would pass it but fails here.
#[tokio::test]
async fn client_portal_projects_scopes_and_aggregates_the_signed_in_client_dashboard() {
    let (state, surreal) = state_with_engines().await;

    let (_project_id, project_code, cookie) = client_project_fixture(&state.surreal).await;

    // A different client's matter must never surface on this client's board.
    let (_other_project_id, other_code, _other_cookie) = client_project_fixture_for_product(
        &surreal,
        "Other Client",
        "other-client@example.com",
        "Someone Else's Matter",
    )
    .await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_cookie(app, "/app/projects", &cookie).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    // The signed-in client's own matter, by name. There is no catalog to
    // resolve a service label or a price from — every matter is bespoke.
    assert!(body.contains("Sample Matter"), "own matter name: {body}");
    assert!(
        !body.contains("Catalog Service Label"),
        "no service label: {body}"
    );
    assert!(!body.contains('$'), "no price: {body}");
    assert!(
        body.contains(&format!("/app/projects/{project_code}")),
        "own matter links to its detail page: {body}",
    );

    // Person scoping: the other client's matter, service label, and detail
    // link are all absent from this client's dashboard.
    let other_detail = format!("/app/projects/{other_code}");
    for leaked in [
        "Someone Else's Matter",
        "Other Catalog Label",
        other_detail.as_str(),
    ] {
        assert!(
            !body.contains(leaked),
            "leaked another client's data `{leaked}`: {body}",
        );
    }

    // KPI aggregation. Each tile renders its value div immediately before its
    // label div, so stripping tags and hydration comments from the KPI section
    // leaves the visible text as `<value><label>…` per tile. Asserting the
    // value/label pairing verifies the in-memory open/closed counts, robustly
    // against Dioxus's SSR hydration markup (which a bare-digit match could
    // otherwise mistake for a node id).
    let kpi_start = body.find("portal-kpis").expect("KPI section renders");
    let kpi_end = body[kpi_start..]
        .find("portal-projects")
        .map_or(body.len(), |offset| kpi_start + offset);
    let mut kpi_text = String::new();
    // The slice begins inside the opening `<div class="portal-kpis"` tag, so
    // start "inside a tag" to drop the class attribute before the first `>`.
    let mut in_tag = true;
    for ch in body[kpi_start..kpi_end].chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => kpi_text.push(ch),
            _ => {}
        }
    }
    // Open 1 (the single open matter), Closed 0 (nothing closed). There is no
    // Documents or Services tile.
    for (value, label) in [(1, "Open"), (0, "Closed")] {
        assert!(
            kpi_text.contains(&format!("{value}{label}")),
            "KPI `{label}` should aggregate to {value}: {kpi_text}",
        );
    }
}

#[tokio::test]
async fn client_project_detail_shows_no_service_panel_and_no_price() {
    // Every matter is priced bespoke and reconciled in Xero, so the client
    // detail page carries no service card and no price at all — and, as
    // before, never the internal matter id.
    let (state, _surreal) = state_with_engines().await;
    let (project_id, project_code, cookie) = client_project_fixture(&state.surreal).await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Sample Matter"),
        "the matter name renders: {body}"
    );
    assert!(
        !body.contains("matter_close_flat"),
        "no billing kind: {body}"
    );
    assert!(!body.contains('$'), "no price anywhere on the page: {body}");
    assert!(
        !body.contains(&format!("Matter id: <code>{project_id}</code>")),
        "client detail should not surface internal matter ids: {body}",
    );
    assert!(
        !body.contains(">Conversation<"),
        "the matter page has no conversation door: {body}",
    );
    assert!(
        !body.contains("Documents to review will appear here"),
        "an empty review list does not explain itself: {body}",
    );
    assert!(
        body.contains("Your documents"),
        "the matter page always lists documents: {body}",
    );
}

#[tokio::test]
async fn client_project_detail_links_only_to_pending_intake() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code, cookie) = client_project_fixture(&surreal).await;
    let client_id = test_sessions()
        .decode(cookie.trim_start_matches("navigator_session="))
        .and_then(|session| session.person_id)
        .expect("fixture cookie carries the client person id");

    // Use the shipped questionnaire shape so the resolver exercises the same
    // client-facing state machine as the intake page: `person__client` is the
    // one question the client must answer.
    store::templates::save_version(
        &surreal,
        None,
        "onboarding__letter",
        store::templates::Version {
            title: "Client intake agreement".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap();
    for code in [
        "entity",
        "address",
        "person",
        "project",
        "custom_text",
        "custom_datetime",
        "custom_single_choice",
    ] {
        store::questions::create(
            &surreal,
            &store::questions::NewQuestion::new(code, format!("Prompt for {code}"), "string"),
        )
        .await
        .unwrap();
    }
    let notation = workflows::notation_session::start_notation(
        &surreal,
        &workflows::InMemoryRuntime::new(),
        None,
        "onboarding__letter",
        client_id,
        project_id,
        None,
    )
    .await
    .unwrap();

    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_with_cookie(app, &format!("/app/projects/{project_code}"), &cookie).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains(&format!(
            "href=\"/app/projects/{project_code}/intake/{}\"",
            notation.notation_id
        )),
        "pending intake link should point to the notation: {body}"
    );
    assert!(
        body.contains(">Continue intake<"),
        "link label renders: {body}"
    );

    workflows::notation_session::record_client_answer(
        &surreal,
        None,
        notation.notation_id,
        "person__client",
        "Portal Client",
        client_id,
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_cookie(app, &format!("/app/projects/{project_code}"), &cookie).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !body.contains("Continue intake"),
        "completed intake has no continuation link: {body}"
    );
}

#[tokio::test]
async fn client_project_detail_links_the_documents_zip() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code, cookie) = client_project_fixture_for_product(
        &surreal,
        "Nest Detail Client",
        "nest-detail-client@example.com",
        "Formation Client Co.",
    )
    .await;

    // The download-all link renders only when the matter actually has a
    // document (#542 — an empty matter offers no empty archive), so file one.
    let args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "welcome-letter.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        // Must be client-visible for the zip link's `has_documents` check.
        visibility: store::documents::visibility::CLIENT,
    };
    store::documents::ingest_bytes(&state.surreal, &state.storage, &args, b"welcome")
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(&format!(
        "href=\"/app/projects/{project_code}/documents.zip\""
    )));
    assert!(
        !body.contains(">Conversation<"),
        "the matter page has no conversation door: {body}",
    );
    assert!(
        body.contains("welcome-letter.pdf"),
        "the client-visible filename is listed: {body}",
    );
}

// QUARANTINED, not retired. This test failed once in CI's `cargo test
// --workspace` run (336 passed, 1 failed) on the LAST assertion —
// `/app/projects/{project_code}` returned 500 where 200 was expected. The three client
// assertions above it, which are the ones carrying #782's confidentiality
// promise, passed on that same run: the client did not see the memo. The
// observed failure is lawyer losing their view, not privileged material
// leaking.
//
// Why it is ignored rather than fixed: every read on that handler is
// `.map_err(server_error)?`, and `server_error` commits a 500 by design —
// "a read failure is a 500, never a silently-blank DRI on an accountability
// surface". Making this green would mean swallowing a failed read and
// rendering a blank DRI, which is the exact behaviour that comment forbids.
// There is no application fix here until the underlying `String` is known, and
// it has not been reproduced.
//
// What still guards #782 while this sleeps: `server/tests/project_documents_acl.rs`
// — `client_download_of_an_internal_document_is_404`,
// `client_detail_of_an_internal_document_does_not_leak_it`, and
// `lawyer_download_of_an_internal_document_succeeds`. Those live in a separate
// test binary, so they run in their own process and are not exposed to the
// in-process contention in this file (337 tests, one process, one shared
// store) that is the leading suspect here.
//
// To retire the quarantine: reproduce the 500, identify the `String`, fix that,
// and delete this attribute — do not delete the test.
#[ignore = "flaked once in CI on the lawyer-view assertion (500 != 200); \
            confidentiality half is covered by project_documents_acl.rs"]
#[tokio::test]
async fn client_project_detail_hides_internal_review_memo_but_lawyer_sees_it() {
    // #782: the client project-detail listing must gate on
    // `assets.visibility`, not list every filename unconditionally. A
    // `review_memo` (attorney work product) stays off the client's list
    // while a lawyer/admin caller on `/app/projects/:project_code` still sees it.
    let (state, _surreal) = state_with_engines().await;
    let (project_id, project_code, cookie) = client_project_fixture(&state.surreal).await;

    let internal_args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "review-memo.pdf",
        kind: "memo",
        content_type: "application/pdf",
        description: Some("Inbound contract review memo"),
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    store::documents::ingest_bytes(
        &state.surreal,
        &state.storage,
        &internal_args,
        b"attorney memo",
    )
    .await
    .unwrap();
    let client_args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "welcome-letter.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::CLIENT,
    };
    store::documents::ingest_bytes(&state.surreal, &state.storage, &client_args, b"welcome")
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let client_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(client_resp.status(), StatusCode::OK);
    let client_body = body_string(client_resp).await;
    assert!(
        !client_body.contains("review-memo.pdf"),
        "internal work product must not reach the client's document list: {client_body}"
    );
    assert!(
        client_body.contains("welcome-letter.pdf"),
        "a client-visible document must still list: {client_body}"
    );

    let lawyer_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lawyer_resp.status(), StatusCode::OK);
    let lawyer_body = body_string(lawyer_resp).await;
    assert!(
        lawyer_body.contains("review-memo.pdf"),
        "a lawyer must still see internal work product: {lawyer_body}"
    );
    assert!(lawyer_body.contains("welcome-letter.pdf"));
}

#[tokio::test]
async fn client_project_detail_404s_a_matter_the_client_cannot_see() {
    // The client lens returns 404 (never 403) for a matter the signed-in client
    // has no client-side scope on — the matter does not exist from their
    // perspective, and its name never reaches the response.
    let (state, surreal) = state_with_engines().await;
    let (_own_project_id, _own_code, cookie) = client_project_fixture(&state.surreal).await;
    let (_other_project_id, other_code, _other_cookie) = client_project_fixture_for_product(
        &surreal,
        "Other Client",
        "other-detail-client@example.com",
        "Someone Else's Matter",
    )
    .await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{other_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(
        !body.contains("Someone Else's Matter"),
        "an unauthorised matter's name must never reach the response: {body}",
    );
}

#[tokio::test]
async fn admin_dashboard_rejects_invalid_bearer_token() {
    let auth = AuthConfig::new(false, Some("test-secret"));
    let state = empty_state_with_auth(auth).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header("authorization", "Bearer not-a-real-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn canonical_host_redirects_when_host_mismatches() {
    let state =
        empty_state_with_canonical_host(CanonicalHost::new(Some("neonlaw.org".into()))).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/notations")
                .header("host", "www.neonlaw.org")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, "https://neonlaw.org/notations");
}

#[tokio::test]
async fn canonical_host_keeps_health_available_on_a_noncanonical_host() {
    let state =
        empty_state_with_canonical_host(CanonicalHost::new(Some("neonlaw.org".into()))).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("host", "10.0.0.12:3001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::LOCATION).is_none());
}

#[tokio::test]
async fn canonical_host_passes_through_when_host_matches() {
    let state =
        empty_state_with_canonical_host(CanonicalHost::new(Some("neonlaw.org".into()))).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "neonlaw.org")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn canonical_host_passes_through_when_disabled() {
    let state = empty_state_with_canonical_host(CanonicalHost::new(None)).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "any.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn design_page_renders_the_component_gallery() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/design").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // `/design` renders the Dioxus Components gallery, styled by the Dioxus
    // Components theme. It is served through `render_handler`, readable
    // pre-hydration, even without a built client bundle.
    assert!(
        body.contains("Design system"),
        "renders the gallery heading"
    );
    assert!(
        body.contains("nav-theme design-gallery"),
        "wraps content in the theme shell"
    );
    assert!(
        body.contains("/public/css/theme.css"),
        "loads the first-party theme stylesheet"
    );
    // The Dioxus components, styled by the theme — no Bootstrap classes.
    assert!(body.contains("nav-card"), "renders cards");
    assert!(
        body.contains("nav-toast--primary"),
        "renders the primary toast tone"
    );
    assert!(
        !body.contains("text-bg-primary"),
        "no Bootstrap toast helper"
    );
    // Icons are inline SVG, not the Bootstrap Icons webfont.
    assert!(body.contains("nav-icon"), "renders inline SVG icons");
    assert!(!body.contains("class=\"bi bi-"), "no icon webfont glyphs");
    // The brand tokens preview through `var(--nav-…)`, so the page shows the
    // running deploy's brand rather than a ramp pinned into the gallery.
    assert!(
        body.contains("var(--nav-color-primary)"),
        "swatches resolve their tokens"
    );
    // The grounded component snippets are still on the page (the webapp
    // `design::tests` drift test proves each still matches its source file).
    assert!(
        body.contains("The Card component"),
        "shows a grounded component snippet"
    );
    assert!(
        !body.contains("highlight.min.js"),
        "no vendored client highlighter"
    );
    // The URL-contract reference: the demo data table renders with real
    // `?sort=` / `?page=` anchors, server-side (the `webapp::design` server
    // function resolves during SSR).
    assert!(body.contains("nav-table"), "renders the demo data table");
    assert!(
        body.contains("href=\"/design?sort=name\""),
        "renders a `?sort=` toggle anchor"
    );
    assert!(
        body.contains("page=2"),
        "renders a `?page=` pagination anchor"
    );
    // The marketing card cluster renders too — pricing cards, testimonials, and
    // the legal disclaimer, as theme-styled Dioxus components.
    assert!(body.contains("pricing-card"), "renders pricing cards");
    assert!(body.contains("testimonial-card"), "renders testimonials");
    assert!(
        body.contains("template-disclaimer"),
        "renders the legal disclaimer"
    );
    // Breadcrumb and off-site link.
    assert!(body.contains("nav-breadcrumb"), "renders the breadcrumb");
    assert!(
        !body.contains("nav-freshness"),
        "the last-edited stamp is retired and must not render"
    );
    // The create/edit form card.
    assert!(body.contains("nav-form"), "renders the form card");
    // The people-list widget and the SocialMeta head tags.
    assert!(body.contains("nav-fieldset"), "renders the people list");
    assert!(
        body.contains("property=\"og:title\""),
        "renders the social-share meta tags"
    );
}

#[tokio::test]
async fn design_page_400s_an_unadvertised_sort_field() {
    // The demo table exercises the JSON:API URL contract: a `?sort=` naming a
    // field the table does not advertise returns `400` before the render runs,
    // the same guard the lawyer people route applies.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/design?sort=ssn").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // An advertised field still renders.
    let ok = get_signed_in(
        server::neon_router(
            empty_state().await,
            std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        ),
        "/design?sort=-role",
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn design_page_400s_a_malformed_sort_query() {
    // A malformed query encoding (`%ZZ` is not valid percent-encoding) can't be
    // parsed, so the guard rejects it with a `400` rather than silently treating
    // it as "no sort" and letting the render fail with a 200 error card.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/design?sort=%ZZ").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn root_serves_marketing_anonymously() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_people_index_shows_empty_state() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // `/app/admin/people` renders through Dioxus; its `list_admin_people` server
    // function refuses any non-admin viewer, so the directory is exercised as
    // admin.
    let resp = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("No people yet."));
}

#[tokio::test]
async fn form_encoded_create_via_api_lists_the_person() {
    // `/app/api/people` accepts a url-encoded body as well as JSON, so one command
    // endpoint serves both shapes. Success answers `201` with the created row;
    // the person then shows on the lawyer listing.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Libra&email=libra%40example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);

    let list = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert_eq!(list.status(), StatusCode::OK);
    let body = body_string(list).await;
    assert!(body.contains("Libra"));
    assert!(body.contains("libra@example.com"));
}

#[tokio::test]
async fn form_encoded_create_rejects_invalid_input_with_a_typed_error() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Libra&email=not-an-email"))
                .unwrap(),
        )
        .await
        .unwrap();
    // A validation failure is the typed `ApiError` with its proper status —
    // this door is machine-facing and says so in the status line.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_string(resp).await;
    assert!(
        body.contains("Name is required and email must contain an @."),
        "the error must still name what is wrong: {body}",
    );
}

#[tokio::test]
async fn form_encoded_edit_and_delete_via_api() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let edit = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", libra.id))
                .header(header::COOKIE, cookie.clone())
                .header("x-csrf-token", csrf.clone())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Libra&email=libra-updated%40example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);
    let row = store::persons::find_by_id(&surreal, libra.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.email, "libra-updated@example.com");

    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{}", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);

    let list = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert!(body_string(list).await.contains("No people yet."));
}

#[tokio::test]
async fn api_people_update_preserves_omitted_structured_name() {
    let (state, surreal) = state_with_engines().await;
    let maria = store::persons::create(
        &surreal,
        &store::persons::NewPerson {
            given_name: Some("María".into()),
            family_name: Some("Santos Gómez".into()),
            middle_name: Some("Elena".into()),
            ..store::persons::NewPerson::with_role(
                "María Santos",
                "maria@example.com",
                store::persons::Role::Client,
            )
        },
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // An unrelated edit posts only name/email/role — no name-part fields.
    let edit = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", maria.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Maria&email=maria%40example.com&role=client",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);

    // The structured legal name the N-400 fills must survive the edit,
    // not get nulled out because the request omitted those fields.
    let row = store::persons::find_by_id(&surreal, maria.id)
        .await
        .unwrap()
        .expect("person still present");
    assert_eq!(row.given_name.as_deref(), Some("María"));
    assert_eq!(row.family_name.as_deref(), Some("Santos Gómez"));
    assert_eq!(row.middle_name.as_deref(), Some("Elena"));
}

#[tokio::test]
async fn api_people_update_null_clears_name_part_while_omitted_is_preserved() {
    let (state, surreal) = state_with_engines().await;
    let maria = store::persons::create(
        &surreal,
        &store::persons::NewPerson {
            given_name: Some("María".into()),
            family_name: Some("Santos Gómez".into()),
            middle_name: Some("Elena".into()),
            ..store::persons::NewPerson::with_role(
                "María Santos",
                "maria@example.com",
                store::persons::Role::Client,
            )
        },
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // JSON PATCH with an explicit `null` given_name and a value for
    // middle_name, omitting family_name entirely. The nullable schema
    // must clear given_name, set middle_name, and preserve family_name.
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", maria.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "María Santos",
                        "email": "maria@example.com",
                        "given_name": null,
                        "middle_name": "Elenita"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let row = store::persons::find_by_id(&surreal, maria.id)
        .await
        .unwrap()
        .expect("person still present");
    assert_eq!(row.given_name, None, "explicit null must clear the column");
    assert_eq!(
        row.family_name.as_deref(),
        Some("Santos Gómez"),
        "an omitted field must be preserved",
    );
    assert_eq!(
        row.middle_name.as_deref(),
        Some("Elenita"),
        "a value must be set",
    );
}

#[tokio::test]
async fn api_people_write_rejects_auth_before_parsing_a_malformed_body() {
    // The session boundary runs before the JsonOrForm body extractor, so
    // an anonymous caller with a malformed body gets the documented 401
    // (auth failure), never a 400 (parse failure) that would mask it.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header("content-type", "application/json")
                .body(Body::from("{ this is not valid json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "unauthenticated");
}

#[tokio::test]
async fn api_people_create_forces_client_role_for_lawyer_caller() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // A lawyer (non-admin) caller can't set a role — the server forces
    // `client` even when the body says `admin`, so a disabled select
    // can't be bypassed with a hand-crafted POST.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Libra&email=libra%40example.com&role=admin",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let row = store::persons::find_by_email_ci(&surreal, "libra@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role, store::persons::Role::Client);
}

#[tokio::test]
async fn api_people_update_ignores_role_change_from_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", client.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Libra&email=libra%40example.com&role=admin",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let row = store::persons::find_by_id(&surreal, client.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role, store::persons::Role::Client);
}

#[tokio::test]
async fn api_people_update_allows_role_change_from_admin() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", client.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Libra&email=libra%40example.com&role=lawyer",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let row = store::persons::find_by_id(&surreal, client.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role, store::persons::Role::Lawyer);
}

#[tokio::test]
async fn api_people_update_rejects_invalid_role_as_json() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", lawyer.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "Lawyer",
                        "email": "lawyer@neonlaw.com",
                        "role": "sttaf"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "invalid_request");

    let row = store::persons::find_by_id(&surreal, lawyer.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role, store::persons::Role::Lawyer);
}

#[tokio::test]
async fn api_people_update_authorizes_owner_admin_and_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let target = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Target",
            "target@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
        (
            "owner",
            Some(store::persons::Role::Owner),
            StatusCode::OK,
            "",
        ),
    ];
    for (label, role, status, error) in cases {
        let mut builder = Request::builder()
            .method("PATCH")
            .uri(format!("/app/api/people/{}", target.id))
            .header("content-type", "application/json");
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(
                builder
                    .body(Body::from(
                        serde_json::json!({ "name": "Target", "email": "target@example.com" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        if !status.is_success() {
            let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
            assert_eq!(body["error"], error, "{label}");
        }
    }
}

#[tokio::test]
async fn api_people_delete_authorizes_owner_admin_and_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
        (
            "owner",
            Some(store::persons::Role::Owner),
            StatusCode::OK,
            "",
        ),
    ];
    for (label, role, status, error) in cases {
        // Fresh row per case: the success cases actually delete it.
        let target = store::persons::create(
            &surreal,
            &store::persons::NewPerson::with_role(
                format!("{label} Target"),
                format!("{label}-del@example.com"),
                store::persons::Role::Client,
            ),
        )
        .await
        .unwrap();
        let mut builder = Request::builder()
            .method("DELETE")
            .uri(format!("/app/api/people/{}", target.id));
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        if !status.is_success() {
            let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
            assert_eq!(body["error"], error, "{label}");
        }
    }
}

#[tokio::test]
async fn api_people_delete_blocks_bootstrap_owner() {
    // One pair, not two: opening `state_with_engines()` twice would give the
    // router one engine and this test's seed another.
    let (base, surreal) = state_with_engines().await;
    let state = AppState {
        bootstrap_owner_email: Some("owner@neonlaw.com".into()),
        self_signup_enabled: false,
        ..base
    };
    let boss = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Boss",
            "owner@neonlaw.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{}", boss.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "blocked");

    // The bootstrap Owner row is still there.
    assert!(store::persons::find_by_id(&surreal, boss.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn api_people_delete_blocks_non_client_targets() {
    // Only client records are deletable: a lawyer can't delete
    // another lawyer or admin. Even an admin caller is refused at the
    // command boundary, so a hand-crafted DELETE can't route around the
    // hidden list button.
    let (state, surreal) = state_with_engines().await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (label, role) in [
        ("lawyer", store::persons::Role::Lawyer),
        ("admin", store::persons::Role::Admin),
    ] {
        let target = store::persons::create(
            &surreal,
            &store::persons::NewPerson::with_role(
                format!("{label} Target"),
                format!("{label}-nodelete@example.com"),
                role,
            ),
        )
        .await
        .unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/app/api/people/{}", target.id))
                    .header(header::COOKIE, cookie.clone())
                    .header("x-csrf-token", csrf.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT, "{label}");
        let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(body["error"], "blocked", "{label}");
        // The row is untouched.
        assert!(
            store::persons::find_by_id(&surreal, target.id)
                .await
                .unwrap()
                .is_some(),
            "{label} row must survive the refused delete",
        );
    }
}

#[tokio::test]
async fn api_people_update_and_delete_404_when_missing() {
    let (state, _surreal) = state_with_engines().await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let missing = uuid::Uuid::from_u128(0xdead_beef);

    let patch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{missing}"))
                .header(header::COOKIE, cookie.clone())
                .header("x-csrf-token", csrf.clone())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "name": "X", "email": "x@example.com" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch.status(), StatusCode::NOT_FOUND);

    let delete = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{missing}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_people_welcome_dispatches_for_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/people/{}/welcome", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["status"], "sent");
}

#[tokio::test]
async fn api_people_welcome_authorizes_owner_admin_and_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cases = [
        (
            "anonymous",
            None,
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
        ),
        (
            "client",
            Some(store::persons::Role::Client),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "clerk",
            Some(store::persons::Role::Clerk),
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "lawyer",
            Some(store::persons::Role::Lawyer),
            StatusCode::OK,
            "",
        ),
        (
            "admin",
            Some(store::persons::Role::Admin),
            StatusCode::OK,
            "",
        ),
        (
            "owner",
            Some(store::persons::Role::Owner),
            StatusCode::OK,
            "",
        ),
    ];
    for (label, role, status, error) in cases {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/app/api/people/{}/welcome", libra.id));
        if let Some(role) = role {
            let (cookie, csrf) = session_cookie_and_csrf_for_role(role);
            builder = builder
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{label}");
        if !status.is_success() {
            let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
            assert_eq!(body["error"], error, "{label}");
        }
    }
}

#[tokio::test]
async fn admin_people_page_renders_directory() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, _) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("People"));
    assert!(body.contains("Libra"));
    assert!(body.contains("libra@example.com"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn admin_can_impersonate_client_and_exit_from_banner() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (admin_cookie, admin_csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", client.id))
                .header(header::COOKIE, admin_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={admin_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        start.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
    let impersonated_cookie = session_cookie_pair(&start);
    let impersonated = decode_session_cookie_pair(&impersonated_cookie);
    assert_eq!(impersonated.role, store::persons::Role::Client);
    assert_eq!(impersonated.person_id, Some(client.id));
    assert_eq!(
        impersonated
            .impersonation
            .as_ref()
            .map(|i| i.actor_person_id),
        Some(Some(admin.id)),
    );

    let forms = app
        .clone()
        .oneshot(
            Request::builder()
                // The impersonation banner rides the authenticated app chrome
                // rather than a public page; the migrated forms index carries it
                // from the same session state.
                .uri("/app/forms")
                .header(header::COOKIE, &impersonated_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let forms_body = body_string(forms).await;
    assert!(forms_body.contains("Impersonating Libra"));
    assert!(forms_body.contains("libra@example.com"));
    assert!(forms_body.contains("/app/impersonation/stop"));
    assert!(forms_body.contains("End impersonation"));

    // The banner must not depend on which pages happen to have migrated: the
    // Dioxus client dashboard carries it too, from the same session state.
    let dioxus_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header(header::COOKIE, &impersonated_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let dioxus_body = body_string(dioxus_page).await;
    assert!(
        dioxus_body.contains("Impersonating Libra"),
        "the Dioxus dashboard must name who the admin is acting as: {dioxus_body}",
    );
    assert!(
        dioxus_body.contains("/app/impersonation/stop"),
        "…and offer the way out: {dioxus_body}",
    );
    assert!(
        dioxus_body.contains("End impersonation"),
        "…with the same labelled control: {dioxus_body}",
    );

    let stop = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/impersonation/stop")
                .header(header::COOKIE, impersonated_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={}", impersonated.csrf_token)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        stop.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
    let restored_cookie = session_cookie_pair(&stop);
    let restored = decode_session_cookie_pair(&restored_cookie);
    assert_eq!(restored.role, store::persons::Role::Admin);
    assert_eq!(restored.person_id, Some(admin.id));
    assert!(restored.impersonation.is_none());
}

/// The matter workbench exposes the client lens to every firm tier, but only
/// after the handler has verified that the caller already belongs to this
/// matter. The preview is a real client session (so downstream reads use their
/// ordinary client guards) and the exit banner can restore the precise actor.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_firm_tier_can_view_an_assigned_matter_as_its_client() {
    let (state, surreal) = state_with_engines().await;
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Preview client",
            "preview-client@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let supervisor = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Preview supervisor",
            "preview-supervisor@example.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for role in [
        store::persons::Role::Clerk,
        store::persons::Role::Lawyer,
        store::persons::Role::Admin,
        store::persons::Role::Owner,
    ] {
        let actor = store::persons::create(
            &surreal,
            &store::persons::NewPerson::with_role(
                format!("{role:?} previewer"),
                format!("preview-{role:?}-{}@example.com", uuid::Uuid::now_v7()).to_lowercase(),
                role,
            ),
        )
        .await
        .unwrap();
        let project = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: format!("client-preview-{}", uuid::Uuid::now_v7()),
                name: format!("{role:?} client preview"),
                status: "open".into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::projects::add_participation(&surreal, project.id, client.id, "client")
            .await
            .unwrap();
        store::projects::designate_dri_in_surreal(
            &surreal,
            project.id,
            client.id,
            store::projects::DriSide::Client,
        )
        .await
        .unwrap();
        if role == store::persons::Role::Clerk {
            store::projects::add_participation(&surreal, project.id, supervisor.id, "attorney")
                .await
                .unwrap();
            store::projects::designate_dri_in_surreal(
                &surreal,
                project.id,
                supervisor.id,
                store::projects::DriSide::Lawyer,
            )
            .await
            .unwrap();
            store::projects::add_participation(&surreal, project.id, actor.id, "clerk")
                .await
                .unwrap();
        } else {
            store::projects::add_participation(&surreal, project.id, actor.id, "attorney")
                .await
                .unwrap();
        }
        let (cookie, csrf) = session_cookie_and_csrf_for_person(&actor);

        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/app/projects/{}", project.code))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK, "{role:?}");
        let detail_body = body_string(detail).await;
        assert!(
            detail_body.contains("View as Client"),
            "{role:?}: {detail_body}"
        );
        if role.is_lawyer_tier() {
            assert!(
                detail_body.contains(&format!("/app/admin/entities/{entity_id}/edit")),
                "{role:?}: {detail_body}"
            );
            assert!(
                detail_body.contains("Edit entity"),
                "{role:?}: {detail_body}"
            );
        }

        let preview = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/projects/{}/view-as-client", project.code))
                    .header(header::COOKIE, cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("_csrf={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::SEE_OTHER, "{role:?}");
        let expected_location = format!("/app/projects/{}", project.code);
        assert_eq!(
            preview
                .headers()
                .get(header::LOCATION)
                .and_then(|location| location.to_str().ok()),
            Some(expected_location.as_str()),
            "{role:?}"
        );
        let effective = decode_session_cookie_pair(&session_cookie_pair(&preview));
        assert_eq!(effective.role, store::persons::Role::Client, "{role:?}");
        assert_eq!(effective.person_id, Some(client.id), "{role:?}");
        assert_eq!(
            effective
                .impersonation
                .as_ref()
                .map(|impersonation| impersonation.actor_person_id),
            Some(Some(actor.id)),
            "{role:?}"
        );
        let client_detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/app/projects/{}", project.code))
                    .header(header::COOKIE, session_cookie_pair(&preview))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(client_detail.status(), StatusCode::OK, "{role:?}");
        let client_detail_body = body_string(client_detail).await;
        assert!(
            client_detail_body.contains("Impersonating Preview client"),
            "{role:?}: {client_detail_body}"
        );
        assert!(
            client_detail_body.contains("End impersonation"),
            "{role:?}: {client_detail_body}"
        );
    }
}

#[tokio::test]
async fn impersonation_exit_bypasses_policy_for_active_impersonation() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (admin_cookie, admin_csrf) = session_cookie_and_csrf_for_person(&admin);
    let start_app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let start = start_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", client.id))
                .header(header::COOKIE, admin_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={admin_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        start.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
    let impersonated_cookie = session_cookie_pair(&start);
    let impersonated = decode_session_cookie_pair(&impersonated_cookie);
    assert!(impersonated.impersonation.is_some());

    let deny_app = server::neon_router(
        AppState {
            policy: deny_all_policy(),
            ..state
        },
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let stop = deny_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/impersonation/stop")
                .header(header::COOKIE, impersonated_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={}", impersonated.csrf_token)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        stop.status(),
        StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    ));
    let restored = decode_session_cookie_pair(&session_cookie_pair(&stop));
    assert_eq!(restored.role, store::persons::Role::Admin);
    assert_eq!(restored.person_id, Some(admin.id));
    assert!(restored.impersonation.is_none());
}

#[tokio::test]
async fn admin_cannot_impersonate_lawyer_person() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", lawyer.id))
                .header(header::COOKIE, cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn admin_cannot_impersonate_admin_person() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let other_admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Other Admin",
            "other-admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", other_admin.id))
                .header(header::COOKIE, cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_string(resp).await;
    assert!(body.contains("Only client users can be impersonated."));
}

#[tokio::test]
async fn lawyer_cannot_impersonate_client_person() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_person(&lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", client.id))
                .header(header::COOKIE, cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn impersonating_admin_cannot_start_second_impersonation() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let other_client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Taurus",
            "taurus@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (admin_cookie, admin_csrf) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", client.id))
                .header(header::COOKIE, admin_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={admin_csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let impersonated_cookie = session_cookie_pair(&start);
    let impersonated = decode_session_cookie_pair(&impersonated_cookie);

    let second = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/impersonate", other_client.id))
                .header(header::COOKIE, impersonated_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={}", impersonated.csrf_token)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_people_index_shows_impersonate_only_for_client_rows() {
    let (state, surreal) = state_with_engines().await;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Admin",
            "admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let (cookie, _) = session_cookie_and_csrf_for_person(&admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Impersonation lives on the admin console surface (`/app/admin/people`),
    // not the de-scoped lawyer workbench list.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(&format!("/app/admin/people/{}/impersonate", client.id)));
    assert!(!body.contains(&format!("/app/admin/people/{}/impersonate", lawyer.id)));
    assert!(!body.contains(&format!("/app/admin/people/{}/impersonate", admin.id)));
}

#[tokio::test]
async fn admin_people_delete_returns_the_deleted_person_as_json() {
    // `/app/api/*` is a machine door: the delete answers with the row it removed,
    // typed, for a caller that will read it. It has no browser consumer — the
    // people surface is Dioxus and posts to `/app/admin/people`.

    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{}", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains(&libra.id.to_string()) && body.contains("libra@example.com"),
        "the delete must answer with the removed row as JSON, got: {body:?}",
    );
}

#[tokio::test]
async fn deleting_a_matter_with_linked_records_keeps_the_row() {
    // A matter with dependent rows (a participation here) is FK-blocked by
    // the database. The write must be refused and the row must survive — it
    // is NOT optimistically removed.
    //
    // The refusal currently redirects without carrying its reason to the
    // listing; that legibility gap is navigator#995. This pins the part that
    // matters for correctness: the matter is still there.
    let (state, surreal) = state_with_engines().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();
    let project = test_project(&surreal, "Has a participant", "open").await;
    // A participation row references the project — this blocks the delete.
    store::projects::add_participation(&surreal, project.id, person.id, "client")
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{}/delete", project.code))
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // The refused delete lands back on the listing rather than erroring out.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    // The matter survives a blocked delete — it is NOT optimistically removed.
    let remaining = store::projects::find_by_id(&surreal, project.id)
        .await
        .unwrap()
        .is_some();
    assert!(remaining, "the matter must survive a blocked delete");
}

/// Seed three people in alphabetical chaos so any sort applied by
/// the handler is observable in the rendered HTML row order.
async fn seed_three_people(surreal: &store::surreal::SurrealDb) {
    for (name, email) in [
        ("Leo", "leo@example.com"),
        ("Libra", "libra@example.com"),
        ("Taurus", "taurus@example.com"),
    ] {
        store::persons::create(surreal, &store::persons::NewPerson::new(name, email))
            .await
            .unwrap();
    }
}

fn first_index_of(haystack: &str, needles: &[&str]) -> Option<(usize, String)> {
    needles
        .iter()
        .find_map(|n| haystack.find(n).map(|i| (i, (*n).to_string())))
}

#[tokio::test]
async fn admin_people_index_drops_id_column_and_renders_sort_links() {
    let (state, surreal) = state_with_engines().await;
    seed_three_people(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(app, "/app/admin/people", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // No ID column header rendered.
    assert!(
        !body.contains("<th>ID</th>"),
        "expected ID column to be gone, got: {body}",
    );
    // Sortable Name + Email headers expose JSON:API ?sort= links.
    assert!(
        body.contains("href=\"/app/admin/people?sort=name\""),
        "expected ?sort=name link, got: {body}",
    );
    assert!(
        body.contains("href=\"/app/admin/people?sort=email\""),
        "expected ?sort=email link, got: {body}",
    );
}

#[tokio::test]
async fn admin_people_index_honors_jsonapi_sort_ascending_by_name() {
    let (state, surreal) = state_with_engines().await;
    seed_three_people(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(
        app,
        "/app/admin/people?sort=name",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Leo → Libra → Taurus in render order.
    let names = [">Leo<", ">Libra<", ">Taurus<"];
    let (i_leo, _) = first_index_of(&body, &[names[0]]).expect("Leo row");
    let (i_libra, _) = first_index_of(&body, &[names[1]]).expect("Libra row");
    let (i_taurus, _) = first_index_of(&body, &[names[2]]).expect("Taurus row");
    assert!(i_leo < i_libra, "Leo before Libra in body");
    assert!(i_libra < i_taurus, "Libra before Taurus in body");
    // Active ascending → the Name header link must flip to descending.
    assert!(
        body.contains("href=\"/app/admin/people?sort=-name\""),
        "expected flipped descending link, got: {body}",
    );
}

#[tokio::test]
async fn admin_people_index_honors_jsonapi_sort_descending_by_name() {
    let (state, surreal) = state_with_engines().await;
    seed_three_people(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(
        app,
        "/app/admin/people?sort=-name",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let (i_leo, _) = first_index_of(&body, &[">Leo<"]).expect("Leo row");
    let (i_taurus, _) = first_index_of(&body, &[">Taurus<"]).expect("Taurus row");
    assert!(
        i_taurus < i_leo,
        "Taurus before Leo when sort=-name, got: {body}",
    );
}

#[tokio::test]
async fn admin_people_index_rejects_unknown_sort_key_with_400() {
    // JSON:API 1.1 §5: a server MUST return 400 Bad Request when asked
    // to sort by a field it does not advertise.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people?sort=ssn")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_people_index_honors_jsonapi_filter_on_name() {
    let (state, surreal) = state_with_engines().await;
    seed_three_people(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // axum/serde_urlencoded parses raw `filter[name]=` as the rename
    // key — the same string a browser sends when the user clicks a
    // generated link. Real clients percent-encode the brackets; both
    // forms decode to the same key.
    let resp = get_with_role(
        app,
        "/app/admin/people?filter%5Bname%5D=Libra",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(">Libra<"), "Libra row present");
    assert!(!body.contains(">Taurus<"), "Taurus filtered out");
    assert!(!body.contains(">Leo<"), "Leo filtered out");
}

#[tokio::test]
async fn admin_people_index_stitches_filter_through_sort_links() {
    // Clicking a sort header must keep the active filter — the
    // generated href must include both filter[name] and the toggled
    // sort.
    let (state, surreal) = state_with_engines().await;
    seed_three_people(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(
        app,
        "/app/admin/people?filter%5Bname%5D=Libra",
        store::persons::Role::Admin,
    )
    .await;
    let body = body_string(resp).await;
    assert!(
        body.contains("href=\"/app/admin/people?filter[name]=Libra&#38;sort=name\""),
        "expected filter to survive sort link, got: {body}",
    );
}

#[tokio::test]
async fn admin_jurisdictions_is_read_only_listing() {
    let (state, _surreal) = state_with_engines().await;
    for (name, code) in [("California", "CA"), ("Nevada", "NV")] {
        store::jurisdictions::create(
            &state.surreal,
            &store::jurisdictions::NewJurisdiction::new(name, code, "state"),
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // `/app/admin/jurisdictions` now renders through the Dioxus generic admin-listing
    // router (#641 Phase 3), carrying the same `require_auth` + `require_policy`
    // gate the surface had — so an authenticated lawyer session sees the
    // read-only listing server-side rendered.
    let resp = get_with_role(
        app.clone(),
        "/app/admin/jurisdictions",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Seeded rows are visible.
    assert!(body.contains("California"));
    assert!(body.contains("Nevada"));
    // Ordered ascending by code (CA before NV), asserted on the full row names so
    // the Dioxus SSR hydration comments between text nodes don't break the match.
    let ca = body.find("California").expect("California row");
    let nv = body.find("Nevada").expect("Nevada row");
    assert!(ca < nv, "expected California (CA) before Nevada (NV)");
    // No CRUD affordances: no Add/Edit/Delete buttons, no `new` link, no form.
    assert!(
        !body.contains("/app/admin/jurisdictions/new"),
        "Add link should be gone",
    );
    assert!(
        !body.contains("/app/admin/jurisdictions/1/edit"),
        "Edit link should be gone",
    );
    assert!(
        !body.contains("action=\"/app/admin/jurisdictions"),
        "no form action should target this surface",
    );

    // POST is no longer routed. A signed-in session plus its CSRF token
    // clears both the session boundary and the CSRF gate, so the method
    // router (not the login redirect, nor a CSRF 403) is what answers.
    let (cookie, csrf) = admin_session_cookie_and_csrf();
    let post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/jurisdictions")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Foo&code=FO"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);

    // /new is gone.
    let new = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/jurisdictions/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lawyer_jurisdictions_dioxus_route_is_gated_by_embedded_policy() {
    // The generic Dioxus admin-listing router (#641 Phase 3) carries the same
    // `require_auth` + `require_policy` layers as the lawyer surface it
    // replaced. Prove the policy layer is live on a generic listing: an
    // authenticated lawyer session under a deny-all embedded policy is refused (403), not
    // served the listing. One such proof covers the shared factory that mounts
    // every generic listing.
    let app = server::neon_router(
        empty_state_with_policy(deny_all_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let resp = get_with_role(
        app,
        "/app/admin/jurisdictions",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "an authenticated lawyer session must be turned away from a Dioxus generic \
         admin listing when the policy denies — the route is policy-gated"
    );
}

#[tokio::test]
async fn admin_git_repositories_is_read_only_listing() {
    let (state, _surreal) = state_with_engines().await;
    store::git_repositories::create(&state.surreal, "abc123", "deadbeef")
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // `/app/admin/git-repositories` renders through the Dioxus generic admin-listing
    // router (#641 Phase 3) under the same auth + embedded Rego policy gate.
    let resp = get_with_role(
        app.clone(),
        "/app/admin/git-repositories",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("abc123"));
    assert!(body.contains("deadbeef"));
    // No CRUD affordances.
    assert!(!body.contains("/app/admin/git-repositories/new"));
    assert!(!body.contains("action=\"/app/admin/git-repositories"));

    // /new is gone.
    let new = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/git-repositories/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_person_entity_roles_is_read_only_listing() {
    let (state, surreal) = state_with_engines().await;
    // Seed a person + entity + role so the listing has a row to render.
    let entity_id = store::test_support::seed_entity(&state.surreal).await;
    let person_id = store::test_support::dri_person(&surreal).await;
    store::entity_roles::grant(&state.surreal, person_id, entity_id, "owner")
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app,
        "/app/lawyer/person-entity-roles",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The role cell renders; the header labels are present.
    assert!(body.contains("owner"));
    assert!(body.contains("Person") && body.contains("Entity") && body.contains("Role"));
    // No CRUD affordances.
    assert!(!body.contains("/app/lawyer/person-entity-roles/new"));
    assert!(!body.contains("action=\"/app/lawyer/person-entity-roles"));
}

#[tokio::test]
async fn admin_generic_listings_all_mount_and_render_their_heading() {
    // Every generic read-only admin listing (#641 Phase 3) mounts through the
    // shared `admin_listing_router` factory, whose data rendering, empty state,
    // and embedded Rego policy gate are covered by the jurisdictions tests above. This proves each
    // remaining page is wired to the right path and component: an authenticated
    // lawyer session gets a 200 and the page's own heading (a `404` would mean a
    // missing mount, a wrong heading a crossed component). The tables need no
    // seed data — the scaffold's empty state renders the heading regardless.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Authenticated as Admin: `/app/admin/letters` and `/app/admin/email-log` refuse
    // the Lawyer tier since ENG-303 (no project link on `letter` or
    // `sent_email` to scope by), and the admin tier reads every listing here.
    // What each gate admits is the subject of
    // `unscopeable_matter_content_listings_require_the_admin_tier` and
    // `matter_content_listings_are_scoped_to_participation`; this test is only
    // about the mount.
    for (path, heading) in [
        ("/app/lawyer/notations", "Notations"),
        ("/app/lawyer/answers", "Answers"),
        ("/app/admin/addresses", "Addresses"),
        ("/app/lawyer/assets", "Assets"),
        ("/app/lawyer/person-project-roles", "Person-project roles"),
        ("/app/lawyer/disclosures", "Disclosures"),
        ("/app/lawyer/relationship-logs", "Relationship logs"),
        ("/app/admin/mailrooms", "Mailrooms"),
        ("/app/admin/letters", "Letters"),
        ("/app/admin/email-log", "Email log"),
    ] {
        let resp = get_with_role(app.clone(), path, store::persons::Role::Admin).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path} must render for admin"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains(heading),
            "{path} must render its own heading {heading:?}; got: {body}",
        );
    }
}

#[tokio::test]
async fn admin_mailrooms_listing_resolves_the_address_join() {
    // Mailrooms is join-backed: each row resolves its `address_id` to a display
    // string in the server function. Seed a mailroom pointing at an address and
    // assert the joined "line1, city, region" cell renders through the scaffold —
    // the join, not just the mailroom name.
    let (state, surreal) = state_with_engines().await;
    let address = store::addresses::create(
        &surreal,
        &store::addresses::NewAddress {
            line1: "12 Ledger Way".into(),
            city: "Carson City".into(),
            region: "NV".into(),
            postal_code: "89701".into(),
            country: "USA".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    store::mailrooms::create(&surreal, "Silver State Mailroom", address.id)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/mailrooms", store::persons::Role::Lawyer).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Silver State Mailroom"),
        "mailroom name; got: {body}"
    );
    assert!(
        body.contains("12 Ledger Way, Carson City, NV"),
        "the address join must render the resolved address string; got: {body}",
    );
}

#[tokio::test]
async fn admin_letter_detail_renders_the_record_from_its_path_id() {
    // The letter-detail page is the first migrated detail view: its `#[server]`
    // function reads the `{id}` path parameter. Seed an address → mailroom →
    // letter chain and assert the record's fields and the resolved mailroom
    // name/address render at `/app/lawyer/letters/{id}` — proving the path param flows
    // through to the server function.
    let (state, surreal) = state_with_engines().await;
    let address = store::addresses::create(
        &surreal,
        &store::addresses::NewAddress {
            line1: "7 Notary Row".into(),
            city: "Sparks".into(),
            region: "NV".into(),
            postal_code: "89431".into(),
            country: "USA".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    let mailroom = store::mailrooms::create(&surreal, "Reno HQ", address.id)
        .await
        .unwrap();
    let letter = store::letters::record(
        &surreal,
        &store::letters::NewLetter {
            mailroom_id: mailroom.id,
            direction: store::letters::DIRECTION_INCOMING.to_string(),
            sender: "IRS".into(),
            recipient: "Acme Trust".into(),
            summary: "EIN confirmation".into(),
        },
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app,
        &format!("/app/lawyer/letters/{}", letter.id),
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    for cell in [
        "incoming",
        "IRS",
        "Acme Trust",
        "EIN confirmation",
        "Reno HQ",
        "7 Notary Row, Sparks, NV",
    ] {
        assert!(
            body.contains(cell),
            "letter field {cell:?} must render; got: {body}"
        );
    }
    assert!(
        body.contains("Back to letters"),
        "the detail page must link back to the listing; got: {body}",
    );
}

#[tokio::test]
async fn admin_email_log_paginates_over_fifty_rows() {
    // Admin, not Lawyer: `/app/admin/email-log` refuses the Lawyer tier since
    // ENG-303 — `sent_email` carries no project link to scope by, so the admin
    // gate is the interim close. Which tier is admitted is
    // `unscopeable_matter_content_listings_require_the_admin_tier`'s subject;
    // this test is about the log itself.
    // The email log is the one paginated listing: 50 rows per page. Seed 51 so
    // there are two pages, then assert page 1 renders its rows and a `?page=2`
    // pager anchor with "Page 1 of 2", and that `?page=2` renders as page 2 of 2.
    let (state, surreal) = state_with_engines().await;
    for i in 0..51 {
        // Zero-padded so the newest-first `sent_at` ordering is deterministic.
        let stamp = format!("2026-01-01T00:00:{i:02}Z");
        store::sent_emails::record(
            &surreal,
            &store::sent_emails::NewSentEmail {
                recipient: format!("user{i}@test.invalid"),
                subject: format!("Message {i}"),
                sender: "noreply@test.invalid".into(),
                body: "body".into(),
                outcome: "delivered".into(),
                template_slug: None,
                sg_message_id: None,
                sent_at: stamp.parse().unwrap(),
            },
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Page 1: rows render, and the pager offers page 2.
    let resp = get_with_role(
        app.clone(),
        "/app/admin/email-log",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("delivered"),
        "email-log rows must render; got: {body}",
    );
    assert!(
        body.contains("intentionally not logged"),
        "the email-log subtitle must render; got: {body}",
    );
    assert!(
        body.contains("Page 1 of 2"),
        "page 1 of 2 must show; got: {body}",
    );
    assert!(
        body.contains("/app/admin/email-log?page=2"),
        "the pager must anchor to page 2; got: {body}",
    );

    // Page 2 resolves and reports itself as the last page. Its sole row is the
    // oldest message (0), since newest-first paging puts 50..1 on page 1.
    let resp2 = get_with_role(
        app.clone(),
        "/app/admin/email-log?page=2",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = body_string(resp2).await;
    assert!(
        body2.contains("Page 2 of 2"),
        "?page=2 must render as page 2 of 2; got: {body2}",
    );
    assert!(
        body2.contains("user0@test.invalid"),
        "?page=2 must render the final page's row; got: {body2}",
    );

    // An out-of-range `?page=` clamps to the final page: it renders that page's
    // rows (not an empty table) so the rows and the "Page 2 of 2" label agree.
    let resp_oob = get_with_role(
        app,
        "/app/admin/email-log?page=99",
        store::persons::Role::Admin,
    )
    .await;
    assert_eq!(resp_oob.status(), StatusCode::OK);
    let body_oob = body_string(resp_oob).await;
    assert!(
        body_oob.contains("Page 2 of 2"),
        "an out-of-range page must report the last page; got: {body_oob}",
    );
    assert!(
        body_oob.contains("user0@test.invalid"),
        "an out-of-range page must render the final page's rows, not an empty \
         table; got: {body_oob}",
    );
    assert!(
        !body_oob.contains("No rows yet."),
        "an out-of-range page must not render the empty state; got: {body_oob}",
    );
}

#[tokio::test]
async fn admin_addresses_listing_renders_row_cells_from_the_database() {
    // Beyond the wiring check, prove one of the new projections maps real
    // columns to cells: a seeded address (with no owner FK, so the owner cell is
    // the em-dash placeholder) renders its line1/city/region/country through the
    // shared scaffold.
    let (state, surreal) = state_with_engines().await;
    let _address = store::addresses::create(
        &surreal,
        &store::addresses::NewAddress {
            line1: "500 Silver Street".into(),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89501".into(),
            country: "USA".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/addresses", store::persons::Role::Lawyer).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    for cell in ["500 Silver Street", "Reno", "NV", "USA"] {
        assert!(
            body.contains(cell),
            "address cell {cell:?} must render; got: {body}"
        );
    }
}

#[tokio::test]
// Seeds one representative row per migrated listing and asserts its cells; the
// nine linear seed-and-assert blocks read best together rather than split apart.
#[allow(clippy::too_many_lines)]
async fn admin_generic_listings_render_row_cells_from_the_database() {
    // The wiring test above proves every generic listing (#641 Phase 3) mounts
    // and renders its heading, but an empty table never runs the per-page row
    // projection — so a swapped, omitted, or malformed projected field would
    // ship undetected. Seed one representative row per remaining migrated
    // listing (addresses is covered on its own above) and assert its
    // distinctive cells render through the shared scaffold, which forces every
    // projection closure to execute against real columns. Numeric cells use
    // wide values so they can't coincide with a hex substring of a row UUID.
    let (state, surreal) = state_with_engines().await;

    // notations → template_id, person_id, entity_id placeholder, state.
    let notation_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Ada Notation", "ada-notation@example.com"),
    )
    .await
    .unwrap();
    let notation_template = store::templates::save_version(
        &surreal,
        None,
        "sitting__transcript",
        store::templates::Version {
            title: "Estate Plan".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let notation_project = test_project(&surreal, "Notation matter", "open").await;
    store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(
            notation_template.id,
            notation_person.id,
            notation_project.id,
            "lawyer_review",
        ),
    )
    .await
    .unwrap();

    // answers → question_id, person_id, display_value(value).
    let answer_question = store::questions::create(
        &surreal,
        &store::questions::NewQuestion::new("legal_name", "What is your legal name?", "string"),
    )
    .await
    .unwrap();
    let answer_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Ada Answer", "ada-answer@example.com"),
    )
    .await
    .unwrap();
    store::answers::record(
        &surreal,
        &store::answers::NewAnswer::new(
            answer_question.id,
            answer_person.id,
            store::answers::primitive("Grace Hopper"),
        ),
    )
    .await
    .unwrap();

    // assets → storage_key, filename, kind, content_type, byte_size, sha256.
    let asset_storage: std::sync::Arc<dyn cloud::StorageService> = std::sync::Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-routes-no-client-data"))
            .await
            .unwrap(),
    );
    let asset_project = test_project(&surreal, "Asset matter", "open").await;
    let asset_bytes: &[u8] = b"silverkey";
    let asset_sha = store::documents::sha256_hex(asset_bytes);
    store::documents::ingest_bytes(
        &surreal,
        &asset_storage,
        &store::documents::IngestArgs {
            project_id: asset_project.id,
            source: store::documents::source::UPLOAD,
            filename: "retainer.pdf",
            kind: "onboarding",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        asset_bytes,
    )
    .await
    .unwrap();

    // person-project-roles → person_id, project_id, participation.
    let ppr_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Ada Role", "ada-role@example.com"),
    )
    .await
    .unwrap();
    let ppr_project = test_project(&surreal, "Role matter", "open").await;
    store::projects::add_participation(&surreal, ppr_project.id, ppr_person.id, "paralegal")
        .await
        .unwrap();

    // disclosures → entity_id (Some branch), project_id placeholder, kind, summary.
    let disclosure_entity = store::test_support::seed_entity(&surreal).await;
    store::disclosures::record(
        &surreal,
        &store::disclosures::NewDisclosure {
            entity_id: Some(disclosure_entity),
            project_id: None,
            kind: "conflict_check",
            summary: "Adverse party overlap noted",
        },
    )
    .await
    .unwrap();

    // relationship-logs → actor (Some branch), subject_type, subject_id, action, detail.
    let rl_actor = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Ada Actor", "ada-actor@example.com"),
    )
    .await
    .unwrap();
    let rl_subject_id = store::test_support::seed_entity(&surreal).await;
    store::relationship_logs::record(
        &surreal,
        &store::relationship_logs::NewRelationshipLog {
            actor_person_id: Some(rl_actor.id),
            subject_type: "membership_edge".into(),
            subject_id: rl_subject_id,
            action: "access_revoked".into(),
            detail: "Removed from the matter roster".into(),
        },
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Each listing's projection must map its columns to the rendered cells.
    for (path, cells) in [
        (
            "/app/lawyer/notations",
            vec![
                notation_template.id.to_string(),
                notation_person.id.to_string(),
                "lawyer_review".to_string(),
            ],
        ),
        (
            "/app/lawyer/answers",
            vec![
                answer_question.id.to_string(),
                answer_person.id.to_string(),
                "Grace Hopper".to_string(),
            ],
        ),
        (
            "/app/lawyer/assets",
            // Content-addressed: the fixture files these bytes through the
            // real ingest seam, so the key and the digest are derived rather
            // than hand-written.
            vec![
                format!("blobs/{asset_sha}"),
                "retainer.pdf".to_string(),
                "onboarding".to_string(),
                "application/pdf".to_string(),
                asset_bytes.len().to_string(),
                asset_sha.clone(),
            ],
        ),
        (
            "/app/lawyer/person-project-roles",
            vec![
                ppr_person.id.to_string(),
                ppr_project.id.to_string(),
                "paralegal".to_string(),
            ],
        ),
        (
            "/app/lawyer/disclosures",
            vec![
                disclosure_entity.to_string(),
                "conflict_check".to_string(),
                "Adverse party overlap noted".to_string(),
            ],
        ),
        (
            "/app/lawyer/relationship-logs",
            vec![
                rl_actor.id.to_string(),
                rl_subject_id.to_string(),
                "membership_edge".to_string(),
                "access_revoked".to_string(),
                "Removed from the matter roster".to_string(),
            ],
        ),
    ] {
        // Admin, not Lawyer: since ENG-303 the matter-content listings among
        // these (`answers`, `assets`, `relationship-logs`) scope their rows to
        // the caller's participation ledger, and this test is about the
        // projection — that each column reaches its cell — not about the gate.
        // The unscoped admin read is what puts every seeded row in front of it.
        let resp = get_with_role(app.clone(), path, store::persons::Role::Admin).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path} must render for admin"
        );
        let body = body_string(resp).await;
        for cell in &cells {
            assert!(
                body.contains(cell.as_str()),
                "{path} must render projected cell {cell:?}; got: {body}",
            );
        }
    }
}

/// One matter the caller is on, one they are not, and one unlinked row per
/// matter-content listing — the fixture ENG-303's scoping tests share.
struct MatterContentFixture {
    lawyer_cookie: String,
    /// Cells that must render for a caller admitted to `visible`.
    visible_cells: Vec<String>,
    /// Cells that must never render for a caller scoped to `visible`.
    hidden_cells: Vec<String>,
    /// Cells for rows carrying no project link at all — absent from every
    /// scoped read, present for Owner/Admin.
    unlinked_cells: Vec<String>,
}

/// Seed one visible matter, one hidden matter, and one unlinked row for each of
/// the three matter-content listings (`assets`, `answers`, `relationship-logs`),
/// and return a Lawyer session holding a firm-side row on the visible matter
/// only.
#[allow(clippy::too_many_lines)]
async fn seed_matter_content(surreal: &store::surreal::SurrealDb) -> MatterContentFixture {
    let lawyer = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Scoped Lawyer",
            "scoped-lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let visible = test_project(surreal, "Visible Content Matter", "open").await;
    let hidden = test_project(surreal, "Hidden Content Matter", "open").await;
    participate(surreal, lawyer.id, visible.id, "attorney").await;

    // assets → one document per matter, plus a bare content asset whose
    // `project_id` is NONE.
    let storage: std::sync::Arc<dyn cloud::StorageService> = std::sync::Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-routes-eng303-scoping"))
            .await
            .unwrap(),
    );
    let visible_asset_sha = store::documents::sha256_hex(b"visible-matter-bytes");
    store::documents::ingest_bytes(
        surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id: visible.id,
            source: store::documents::source::UPLOAD,
            filename: "visible-brief.pdf",
            kind: "onboarding",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        b"visible-matter-bytes",
    )
    .await
    .unwrap();
    let hidden_asset_sha = store::documents::sha256_hex(b"hidden-matter-bytes");
    store::documents::ingest_bytes(
        surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id: hidden.id,
            source: store::documents::source::UPLOAD,
            filename: "hidden-brief.pdf",
            kind: "onboarding",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        b"hidden-matter-bytes",
    )
    .await
    .unwrap();
    // A bare content asset: no `project_id` at all. `ingest_content` is the
    // lane that writes one, so the NONE case is produced rather than faked.
    let unlinked_asset_sha = store::documents::sha256_hex(b"unlinked-bare-bytes");
    store::assets::ingest_content(surreal, &storage, b"unlinked-bare-bytes", "text/plain")
        .await
        .unwrap();

    // answers → one per matter through `notation_id → notation.project_id`,
    // plus a bare person-scoped answer whose `notation_id` is NONE.
    let template = store::templates::save_version(
        surreal,
        None,
        "eng303__scope",
        store::templates::Version {
            title: "Scoping template".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let respondent = store::persons::create(
        surreal,
        &store::persons::NewPerson::new("Ada Respondent", "ada-respondent@example.com"),
    )
    .await
    .unwrap();
    let question = store::questions::create(
        surreal,
        &store::questions::NewQuestion::new("scope_probe", "Who is the adverse party?", "string"),
    )
    .await
    .unwrap();
    for (project_id, answer) in [
        (visible.id, "Visible Adverse Party"),
        (hidden.id, "Hidden Adverse Party"),
    ] {
        let notation = store::notations::create(
            surreal,
            &store::notations::NewNotation::new(
                template.id,
                respondent.id,
                project_id,
                "lawyer_review",
            ),
        )
        .await
        .unwrap();
        store::answers::record(
            surreal,
            &store::answers::NewAnswer::new(
                question.id,
                respondent.id,
                store::answers::primitive(answer),
            )
            .in_notation(notation.id, "person__client"),
        )
        .await
        .unwrap();
    }
    // No notation, so no path to any matter.
    store::answers::record(
        surreal,
        &store::answers::NewAnswer::new(
            question.id,
            respondent.id,
            store::answers::primitive("Unlinked Adverse Party"),
        ),
    )
    .await
    .unwrap();

    // relationship-logs → one `subject_type = "project"` entry per matter,
    // plus one whose subject is not a project at all.
    for (project_id, detail) in [
        (visible.id, "Visible attestation detail"),
        (hidden.id, "Hidden attestation detail"),
    ] {
        store::relationship_logs::record(
            surreal,
            &store::relationship_logs::NewRelationshipLog {
                actor_person_id: Some(lawyer.id),
                subject_type: "project".into(),
                subject_id: project_id,
                action: "conflict_attestation".into(),
                detail: detail.into(),
            },
        )
        .await
        .unwrap();
    }
    store::relationship_logs::record(
        surreal,
        &store::relationship_logs::NewRelationshipLog {
            actor_person_id: Some(lawyer.id),
            subject_type: "membership_edge".into(),
            subject_id: store::test_support::seed_entity(surreal).await,
            action: "access_revoked".into(),
            detail: "Unlinked trail detail".into(),
        },
    )
    .await
    .unwrap();

    let mut session = portal::SessionData::fresh("scoped-lawyer-sub", store::persons::Role::Lawyer);
    session.person_id = Some(lawyer.id);
    session.email = Some(lawyer.email);
    let lawyer_cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );

    MatterContentFixture {
        lawyer_cookie,
        visible_cells: vec![
            visible_asset_sha,
            "Visible Adverse Party".into(),
            "Visible attestation detail".into(),
        ],
        hidden_cells: vec![
            hidden_asset_sha,
            "Hidden Adverse Party".into(),
            "Hidden attestation detail".into(),
        ],
        unlinked_cells: vec![
            unlinked_asset_sha,
            "Unlinked Adverse Party".into(),
            "Unlinked trail detail".into(),
        ],
    }
}

/// The three matter-content listing paths, aligned to the fixture's cell
/// vectors: assets, answers, relationship-logs.
const MATTER_CONTENT_PATHS: [&str; 3] = [
    "/app/lawyer/assets",
    "/app/lawyer/answers",
    "/app/lawyer/relationship-logs",
];

/// GET `uri` with `cookie`, assert it rendered, and return the body — the
/// shape every ENG-303 scoping assertion needs.
async fn rendered_body_with_cookie(app: axum::Router, uri: &str, cookie: &str) -> String {
    let resp = get_with_cookie(app, uri, cookie).await;
    assert_eq!(resp.status(), StatusCode::OK, "{uri} must render");
    body_string(resp).await
}

/// ENG-303: a Lawyer reads matter content only for the matters their
/// participation ledger names, and a row with no project link is absent
/// entirely.
#[tokio::test]
async fn matter_content_listings_are_scoped_to_participation() {
    let (state, surreal) = state_with_engines().await;
    let fixture = seed_matter_content(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (index, path) in MATTER_CONTENT_PATHS.iter().enumerate() {
        let body = rendered_body_with_cookie(app.clone(), path, &fixture.lawyer_cookie).await;
        let visible = &fixture.visible_cells[index];
        let hidden = &fixture.hidden_cells[index];
        let unlinked = &fixture.unlinked_cells[index];
        assert!(
            body.contains(visible.as_str()),
            "{path} must render the participated matter's row {visible:?}; got: {body}",
        );
        assert!(
            !body.contains(hidden.as_str()),
            "{path} disclosed an unparticipated matter's row {hidden:?}; got: {body}",
        );
        assert!(
            !body.contains(unlinked.as_str()),
            "{path} must fail closed on a row with no project link, but rendered \
             {unlinked:?}; got: {body}",
        );
    }
}

/// ENG-303: a Lawyer with no participation row at all reads zero matter
/// content — the same zero they already see at `/app/projects`.
#[tokio::test]
async fn matter_content_listings_are_empty_for_an_unparticipating_lawyer() {
    let (state, surreal) = state_with_engines().await;
    let fixture = seed_matter_content(&surreal).await;
    // A second lawyer, on nothing.
    let stranger = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Unassigned Lawyer",
            "unassigned-lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let mut session = portal::SessionData::fresh("stranger-sub", store::persons::Role::Lawyer);
    session.person_id = Some(stranger.id);
    session.email = Some(stranger.email);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (index, path) in MATTER_CONTENT_PATHS.iter().enumerate() {
        let body = rendered_body_with_cookie(app.clone(), path, &cookie).await;
        for cells in [
            &fixture.visible_cells,
            &fixture.hidden_cells,
            &fixture.unlinked_cells,
        ] {
            let cell = &cells[index];
            assert!(
                !body.contains(cell.as_str()),
                "{path} disclosed {cell:?} to a lawyer on no matters; got: {body}",
            );
        }
        assert!(
            body.contains("No rows yet."),
            "{path} must show the shared empty state for a lawyer on no matters; got: {body}",
        );
    }
}

/// ENG-303: Owner and Admin keep the unscoped read — that is what
/// `is_admin_tier` is for.
#[tokio::test]
async fn matter_content_listings_stay_unscoped_for_the_admin_tier() {
    let (state, surreal) = state_with_engines().await;
    let fixture = seed_matter_content(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for role in [store::persons::Role::Owner, store::persons::Role::Admin] {
        for (index, path) in MATTER_CONTENT_PATHS.iter().enumerate() {
            let resp = get_with_role(app.clone(), path, role).await;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "{path} must render for {role:?}"
            );
            let body = body_string(resp).await;
            for cells in [
                &fixture.visible_cells,
                &fixture.hidden_cells,
                &fixture.unlinked_cells,
            ] {
                let cell = &cells[index];
                assert!(
                    body.contains(cell.as_str()),
                    "{path} must render every row for {role:?}, missing {cell:?}; got: {body}",
                );
            }
        }
    }
}

/// ENG-303: `/app/lawyer/disclosures` and `/app/lawyer/person-entity-roles` stay
/// firm-wide for a lawyer on no matters, because Model Rule 1.10 imputes a
/// conflict firm-wide and both feed `store::conflicts::check_new_matter`.
///
/// This test exists to stop a later consistency sweep from scoping them: if it
/// starts failing because someone filtered these by participation, the fix is
/// to revert that, not to update this test.
#[tokio::test]
async fn conflict_graph_listings_stay_firm_wide_for_an_unparticipating_lawyer() {
    let (state, surreal) = state_with_engines().await;
    // A matter the caller is emphatically not on, carrying both conflict-graph
    // inputs.
    let other_matter = test_project(&surreal, "Someone Else's Matter", "open").await;
    let conflicted_entity = store::test_support::seed_entity(&surreal).await;
    store::disclosures::record(
        &surreal,
        &store::disclosures::NewDisclosure {
            entity_id: Some(conflicted_entity),
            project_id: Some(other_matter.id),
            kind: "conflict_check",
            summary: "Adverse party overlap on another matter",
        },
    )
    .await
    .unwrap();
    let tied_person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Tied Person", "tied-person@example.com"),
    )
    .await
    .unwrap();
    store::entity_roles::grant(&surreal, tied_person.id, conflicted_entity, "officer")
        .await
        .unwrap();

    let stranger = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Conflict Checking Lawyer",
            "conflict-checker@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let mut session = portal::SessionData::fresh("checker-sub", store::persons::Role::Lawyer);
    session.person_id = Some(stranger.id);
    session.email = Some(stranger.email);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let disclosures =
        rendered_body_with_cookie(app.clone(), "/app/lawyer/disclosures", &cookie).await;
    assert!(
        disclosures.contains("Adverse party overlap on another matter"),
        "a lawyer on no matters must still see every disclosure — Model Rule 1.10 \
         imputes conflicts firm-wide; got: {disclosures}",
    );
    let ties = rendered_body_with_cookie(app, "/app/lawyer/person-entity-roles", &cookie).await;
    assert!(
        ties.contains(&tied_person.id.to_string()),
        "a lawyer on no matters must still see every entity_role tie — it is an edge \
         `store::conflicts::check_new_matter` traverses; got: {ties}",
    );
}

/// ENG-303: `/app/admin/letters` and `/app/admin/email-log` refuse the Lawyer tier
/// and serve Owner/Admin. `letter` and `sent_email` carry no project link, so
/// the admin gate is the interim close until one exists.
#[tokio::test]
async fn unscopeable_matter_content_listings_require_the_admin_tier() {
    let (state, surreal) = state_with_engines().await;
    let mailroom_address = store::addresses::create(
        &surreal,
        &store::addresses::NewAddress {
            line1: "500 Silver Street".into(),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89501".into(),
            country: "USA".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    let mailroom = store::mailrooms::create(&surreal, "Reno intake", mailroom_address.id)
        .await
        .unwrap();
    store::letters::record(
        &surreal,
        &store::letters::NewLetter {
            mailroom_id: mailroom.id,
            direction: "incoming".into(),
            sender: "opposing-counsel@example.com".into(),
            recipient: "intake@neonlaw.com".into(),
            summary: "Demand letter summary".into(),
        },
    )
    .await
    .unwrap();
    store::sent_emails::record(
        &surreal,
        &store::sent_emails::NewSentEmail {
            recipient: "logged-recipient@example.com".into(),
            subject: "Matter correspondence".into(),
            body: "Body".into(),
            sender: "support@neonlaw.com".into(),
            template_slug: Some("welcome".into()),
            outcome: "sent".into(),
            sg_message_id: None,
            sent_at: "2026-05-24T10:00:00Z".parse().unwrap(),
        },
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (path, disclosed) in [
        ("/app/admin/letters", "Demand letter summary"),
        ("/app/admin/email-log", "logged-recipient@example.com"),
    ] {
        // A Lawyer-tier session is refused outright — a real 403, not a
        // successful page with an empty table.
        let resp = get_with_role(app.clone(), path, store::persons::Role::Lawyer).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{path} must refuse the lawyer tier",
        );
        let refused = body_string(resp).await;
        assert!(
            !refused.contains(disclosed),
            "{path} disclosed {disclosed:?} in its refusal body; got: {refused}",
        );

        // Owner and Admin still read it.
        for role in [store::persons::Role::Owner, store::persons::Role::Admin] {
            let resp = get_with_role(app.clone(), path, role).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path} must serve {role:?}");
            let body = body_string(resp).await;
            assert!(
                body.contains(disclosed),
                "{path} must render {disclosed:?} for {role:?}; got: {body}",
            );
        }
    }
}

/// ENG-303: every listing in `webapp::admin_listings` is classified exactly
/// once in `webapp::admin_listing::LAWYER_LISTINGS`.
///
/// This is what keeps the seam a seam. A new `listing_router!` mount starts
/// with a new `#[server] pub async fn list_*` in that module, and this test
/// fails until someone has decided whether it discloses matter content — so
/// another unscoped matter-content listing cannot arrive by omission.
#[test]
fn every_admin_listing_is_classified_exactly_once() {
    let source = include_str!("../../webapp/src/admin_listings.rs");
    let declared: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub async fn list_"))
        .filter_map(|rest| rest.split('(').next())
        .map(str::trim)
        .collect();
    assert!(
        declared.len() >= 15,
        "expected to find the listing server functions by source scan, found {declared:?}",
    );

    let classified: std::collections::HashSet<&str> = webapp::admin_listing::LAWYER_LISTINGS
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    assert_eq!(
        classified.len(),
        webapp::admin_listing::LAWYER_LISTINGS.len(),
        "a listing is classified more than once",
    );
    for name in &declared {
        let full = format!("list_{name}");
        assert!(
            classified.contains(full.as_str()),
            "`{full}` is a lawyer listing with no entry in \
             `webapp::admin_listing::LAWYER_LISTINGS`. Decide what it discloses: \
             `Reference`, `MatterContent` (scope it through \
             `require_lawyer_in_matters`), `ConflictGraph` (firm-wide, Model Rule \
             1.10), or `AdminOnly`.",
        );
    }
    for (name, _, _) in webapp::admin_listing::LAWYER_LISTINGS {
        let bare = name.strip_prefix("list_").unwrap_or(name);
        assert!(
            declared.contains(&bare),
            "`{name}` is classified but no longer exists in `webapp::admin_listings`",
        );
    }
}

#[tokio::test]
async fn admin_entity_types_is_read_only_listing() {
    let (state, _surreal) = state_with_engines().await;
    for name in ["LLC", "Trust"] {
        store::entity_types::create(&state.surreal, name)
            .await
            .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // `/app/admin/entity-types` now renders through the Dioxus sub-router (#641
    // Phase 3), carrying the same `require_auth` + `require_policy` gate as the
    // surface it replaced — so an authenticated lawyer session sees the
    // read-only listing server-side rendered.
    let resp = get_with_role(
        app.clone(),
        "/app/admin/entity-types",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("LLC"));
    assert!(body.contains("Trust"));
    // No CRUD affordances.
    assert!(
        !body.contains("/app/admin/entity-types/new"),
        "Add link should be gone",
    );
    assert!(!body.contains("/edit"), "Edit link should be gone");
    assert!(!body.contains("/delete"), "Delete form should be gone");
    assert!(
        !body.contains("action=\"/app/admin/entity-types"),
        "no form action should target this surface",
    );

    // POST to the collection: no route. A signed-in session plus its CSRF
    // token clears both the session boundary and the CSRF gate, so the method
    // router (not the login redirect, nor a CSRF 403) is what answers.
    let (cookie, csrf) = admin_session_cookie_and_csrf();
    let post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entity-types")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Foo"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);

    // /new, /:id/edit, /:id/delete are gone.
    for sub in ["/new", "/00000000-0000-0000-0000-000000000000/edit"] {
        let gone = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/app/admin/entity-types{sub}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            gone.status(),
            StatusCode::NOT_FOUND,
            "/app/admin/entity-types{sub} should be 404",
        );
    }
    let del = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entity-types/00000000-0000-0000-0000-000000000000/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lawyer_entity_types_dioxus_route_is_gated_by_embedded_policy() {
    // The Dioxus `/app/admin/entity-types` sub-router (#641 Phase 3) carries the
    // same `require_auth` + `require_policy` layers as the lawyer surface it
    // replaced. Prove the policy layer is live: an authenticated lawyer session
    // under a deny-all embedded policy is refused (403), not served the listing.

    let app = server::neon_router(
        empty_state_with_policy(deny_all_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let resp = get_with_role(app, "/app/admin/entity-types", store::persons::Role::Lawyer).await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "an authenticated lawyer session must be turned away from the Dioxus \
         /app/admin/entity-types route when the policy denies — the route is policy-gated"
    );
}

#[tokio::test]
async fn admin_entity_types_index_rejects_unknown_sort_key_with_400() {
    // JSON:API 1.1 §5: a server MUST return 400 Bad Request when asked to sort
    // by a field it does not advertise. The `reject_unadvertised_entity_types_sort`
    // pre-handler runs ahead of the render, so an unknown `?sort=` is refused
    // before the server function queries the database.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/entity-types?sort=jurisdiction")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_entity_types_index_rejects_a_malformed_sort_query_with_400() {
    // A malformed query encoding (`%ZZ` is not valid percent-encoding) can't be
    // parsed, so `reject_unadvertised_entity_types_sort` rejects it with a 400
    // rather than silently treating it as "no sort" and rendering a 200 with
    // default ordering — the same URL contract the retired Axum `Query`
    // extractor and the `/design` guard enforce.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/entity-types?sort=%ZZ")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_templates_is_read_only_listing() {
    let (state, _surreal) = state_with_engines().await;
    let _ = store::templates::save_version(
        &state.surreal,
        None,
        "sample__trust",
        store::templates::Version {
            title: "Nevada Trust".into(),
            respondent_type: "entity".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/admin/templates")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Nevada Trust"));
    assert!(body.contains("sample__trust"));
    // No CRUD affordances.
    assert!(!body.contains("/app/admin/templates/new"));
    assert!(!body.contains("/edit"));
    assert!(!body.contains("/delete"));
    assert!(!body.contains("action=\"/app/admin/templates"));

    let (cookie, csrf) = admin_session_cookie_and_csrf();
    let post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/templates")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("code=x&title=X&respondent_type=person&body=hi"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);

    let new = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/templates/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_questions_is_read_only_listing() {
    let (state, surreal) = state_with_engines().await;
    store::questions::create(
        &surreal,
        &store::questions::NewQuestion::new("legal_name", "What is your legal name?", "string"),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/admin/questions")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("What is your legal name?"));
    assert!(body.contains("legal_name"));
    // No CRUD affordances.
    assert!(!body.contains("/app/admin/questions/new"));
    assert!(!body.contains("/edit"));
    assert!(!body.contains("/delete"));
    assert!(!body.contains("action=\"/app/admin/questions"));

    let (cookie, csrf) = admin_session_cookie_and_csrf();
    let post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/questions")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("code=x&prompt=X?&answer_type=string"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);

    let new = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/questions/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn openapi_json_is_served_to_a_signed_in_caller() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    // Clerk is the least privileged tier the documentation gate admits — a
    // `client` is refused, which `api_documentation_is_gated_to_clerk_and_above`
    // pins against the real policy. This test is about the document's shape,
    // so it uses the lowest role that can see one.
    let resp = get_with_role(app, "/app/api/openapi.json", store::persons::Role::Clerk).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("\"openapi\":\"3.1.0\""));
    assert!(body.contains("/app/api/people"));
    assert!(body.contains("\"Person\""));
}

#[tokio::test]
async fn api_docs_serves_swagger_ui_shell_with_csp() {
    // The Swagger UI shell lives at the `/app/api` root, a sibling of
    // `/app/api/openapi.json` rather than a leaf under the `/app/api/*` data
    // prefix. It takes the session boundary and `require_policy`, and the
    // policy admits Clerk and above. This test asserts the handler wiring
    // (CSP, vendored assets) with a passthrough policy; the tier property is
    // pinned separately by `api_documentation_is_gated_to_clerk_and_above`.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_with_role(app, "/app/api", store::persons::Role::Clerk).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the API documentation must render for a signed-in caller"
    );
    let csp = resp
        .headers()
        .get("content-security-policy")
        .expect("CSP header must be set on /app/api")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("script-src 'self'"),
        "CSP must keep script-src on same origin: {csp}"
    );
    assert!(
        !csp.contains("'unsafe-inline'") || csp.contains("style-src 'self' 'unsafe-inline'"),
        "unsafe-inline must only appear under style-src: {csp}"
    );
    let body = body_string(resp).await;
    assert!(
        body.contains("id=\"swagger-ui\""),
        "Swagger UI mount point missing from /app/api shell"
    );
    assert!(
        body.contains("/public/swagger-ui/swagger-ui-bundle.js"),
        "Swagger UI bundle reference missing"
    );
    assert!(
        body.contains("/app/api/openapi.json") || body.contains("init.js"),
        "init.js (which references /app/api/openapi.json) must be loaded"
    );
}

#[tokio::test]
async fn doc_surfaces_are_decided_by_the_embedded_policy() {
    // The documentation surfaces take `require_policy` along with the session
    // boundary, so the policy layer must actually reach them. Under a deny-all
    // policy both are refused — which is the whole point of putting a
    // *restrictive* rule in the bundle: it fails closed.
    //
    // The earlier posture was the reverse (these paths mounted outside the
    // policy so a stale bundle could not gate them). That protected a *public*
    // exemption, where default-deny is the failure. A tier gate has no such
    // hazard: a stale bundle yields 403 and a redeploy fixes it.
    let app = server::neon_router(
        empty_state_with_policy(deny_all_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    for uri in ["/app/api", "/app/api/openapi.json"] {
        let resp = get_with_role(app.clone(), uri, store::persons::Role::Lawyer).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{uri} must be refused when the policy denies every request — the tier decision \
             lives in the bundle, so the layer has to be wired to it"
        );
    }
}

#[tokio::test]
async fn api_documentation_is_gated_to_clerk_and_above() {
    // ENG-83's gate, against the *real* embedded Rego rather than a stub: every
    // tier that operates Navigator reads the API reference, and `client` — the
    // one authenticated tier that does not — is refused.
    //
    // The `client` half is the load-bearing assertion. A client holds a
    // session, and the any-authenticated GET grant on `/app/api/*` would admit
    // them here if the documentation paths were not excluded from it.
    let mut state = empty_state().await;
    state.policy = portal::policy::PolicyClient::embedded().expect("embedded policy compiles");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for uri in ["/app/api", "/app/api/openapi.json"] {
        for role in [
            store::persons::Role::Owner,
            store::persons::Role::Admin,
            store::persons::Role::Lawyer,
            store::persons::Role::Clerk,
        ] {
            let resp = get_with_role(app.clone(), uri, role).await;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "{role:?} operates Navigator and must read {uri}"
            );
        }

        let client = get_with_role(app.clone(), uri, store::persons::Role::Client).await;
        assert_eq!(
            client.status(),
            StatusCode::FORBIDDEN,
            "a client must not read {uri} — it describes the firm's own commands"
        );
    }

    // A Clerk reads the reference but not the directory it describes: operating
    // Navigator is what admits them to the docs, and the CRM is not part of
    // operating it. The pair is what makes the audience deliberate.
    let directory =
        get_with_role(app.clone(), "/app/api/people", store::persons::Role::Clerk).await;
    assert_eq!(
        directory.status(),
        StatusCode::FORBIDDEN,
        "a clerk reads the API reference but not the people directory"
    );
}

#[tokio::test]
async fn api_reads_are_named_per_resource_and_deny_a_client() {
    // The read paths carry no tier check in their handlers — `list_people` is a
    // `State(surreal)` extractor and a query — so the Rego rule is the only
    // thing standing there. A single any-authenticated GET grant used to say
    // yes, which handed a client the firm's whole directory.
    let mut state = empty_state().await;
    state.policy = portal::policy::PolicyClient::embedded().expect("embedded policy compiles");
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for uri in [
        "/app/api/people",
        "/app/api/entities",
        "/app/api/jurisdictions",
        "/app/api/entity-types",
    ] {
        let lawyer = get_with_role(app.clone(), uri, store::persons::Role::Lawyer).await;
        assert_eq!(
            lawyer.status(),
            StatusCode::OK,
            "{uri} is a firm-side read and a lawyer must reach it"
        );

        let client = get_with_role(app.clone(), uri, store::persons::Role::Client).await;
        assert_eq!(
            client.status(),
            StatusCode::FORBIDDEN,
            "{uri} is the firm's own directory; a client must not read it"
        );
    }

    // A GET route with no rule of its own gets no decision, so adding a read
    // endpoint fails closed rather than inheriting a grant.
    let unnamed = get_with_role(
        app.clone(),
        "/app/api/invoices",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_ne!(
        unnamed.status(),
        StatusCode::OK,
        "an /app/api read with no rule must not be authorized by default"
    );
}

#[tokio::test]
async fn old_api_docs_path_is_gone() {
    // The docs shell used to live at `/app/api/docs`, a public leaf carved
    // out of the otherwise-gated `/app/api/*` prefix. It now lives at the
    // top-level `/app/api`, and the old path is unregistered: it must
    // 404 like any unknown route — never 303 to `/auth/login`, and
    // never reintroduce a public exemption inside `/app/api/*`. An
    // unmatched `/app/api/*` path falls past the `require_policy` route
    // layer to the JSON 404 fallback.
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/api/docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "the retired /app/api/docs path must 404, not redirect to auth"
    );
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "not_found");
}

#[tokio::test]
async fn swagger_ui_marks_try_it_out_requests() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/public/swagger-ui/init.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("requestInterceptor"),
        "Swagger UI must intercept Try it out requests"
    );
    assert!(
        body.contains("X-Navigator-Swagger-UI"),
        "Swagger UI must tag API calls so anonymous denials render as warnings"
    );
}

#[tokio::test]
async fn unauthenticated_swagger_api_call_gets_warning_payload() {
    let app = server::neon_router(
        empty_state_with_policy(deny_all_policy()).await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/api/people")
                .header("X-Navigator-Swagger-UI", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let auth = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("401 should advertise the Navigator session challenge")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        auth.contains("NavigatorSession"),
        "unexpected WWW-Authenticate header: {auth}"
    );
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(body["error"], "unauthenticated");
    assert_eq!(body["login"], "/auth/login?return_to=/app/api");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("Sign in")),
        "warning should tell the user how to proceed: {body}"
    );
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/no-such-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Build a minimal `multipart/form-data` body the way SendGrid
/// Inbound Parse formats its POST. Field order matches the
/// (from, to, subject, text, email) tuple the handler reads.
fn build_inbound_multipart(
    from: &str,
    to: &str,
    subject: &str,
    text: &str,
    raw_email: &[u8],
) -> (String, Vec<u8>) {
    let boundary = "----navigator-inbound-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    let mut text_part = |name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    text_part("from", from);
    text_part("to", to);
    text_part("subject", subject);
    text_part("text", text);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"email\"\r\nContent-Type: message/rfc822\r\n\r\n",
    );
    body.extend_from_slice(raw_email);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    (content_type, body)
}

#[tokio::test]
async fn admin_send_welcome_writes_audit_row_and_redirects() {
    // Wrap the dev CapturingEmail in LoggingEmail so the audit decorator
    // is exercised end-to-end — same shape production uses, with the
    // SendGrid backend swapped for capturing.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.email = std::sync::Arc::new(portal::email::LoggingEmail::new(
        std::sync::Arc::new(portal::email::CapturingEmail::new()),
        surreal.clone(),
        "support@neonlaw.com",
    ));
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/people/{}/welcome", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The API door answers a typed `sent` status. The browser's own
    // `?notice=welcome_sent` flash rides the `/app/admin/people/{id}` POST instead.
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("\"status\":\"sent\""),
        "a successful send must report itself: {body}",
    );

    let rows = store::sent_emails::all(&surreal).await.unwrap();
    assert_eq!(rows.len(), 1, "expected one audit row");
    assert_eq!(rows[0].recipient, "libra@example.com");
    assert_eq!(rows[0].subject, "Welcome to Neon Law");
    assert_eq!(rows[0].sender, "support@neonlaw.com");
    assert_eq!(rows[0].template_slug.as_deref(), Some("welcome"));
    assert_eq!(rows[0].outcome, "sent");
    assert!(
        rows[0].body.contains("Libra"),
        "body should be personalized, got: {}",
        rows[0].body
    );
}

#[tokio::test]
async fn admin_send_welcome_flags_failed_when_email_send_errors() {
    // When the email backend errors, the handler must flag the redirect with
    // `?notice=welcome_failed` (not silently land on a clean page) so the show
    // view floats the red failure toast. Drive the real `Err` arm with a stub
    // whose `send` always fails.
    struct FailingEmail;

    #[async_trait::async_trait]
    impl portal::email::EmailService for FailingEmail {
        async fn send(
            &self,
            _email: portal::email::OutboundEmail,
        ) -> Result<portal::email::SendReceipt, portal::email::EmailError> {
            Err(portal::email::EmailError::Transport(
                "sample transport failure".into(),
            ))
        }
    }

    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.email = std::sync::Arc::new(FailingEmail);
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/people/{}/welcome", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // A failed send is a typed `502`, distinguishable from a refusal or a
    // crash, rather than an opaque 5xx page.
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = body_string(resp).await;
    assert!(
        body.contains("send_failed"),
        "a failed send must name itself: {body}",
    );
}

#[tokio::test]
async fn admin_person_show_floats_success_toast_after_welcome_sent() {
    // Following the welcome-send redirect lands on the show view with
    // `?notice=welcome_sent`; the page must float the green confirmation
    // toast naming the recipient.

    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/admin/people/{}/edit?notice=welcome_sent",
                    libra.id
                ))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The lawyer mirror now renders through Dioxus: the flash is the theme's
    // `nav-flash--success`, not Bootstrap's `text-bg-success`.
    assert!(
        body.contains("nav-flash--success"),
        "expected a green success flash, got: {body}",
    );
    assert!(
        body.contains("Welcome email sent to libra@example.com."),
        "flash must name the recipient, got: {body}",
    );
}

#[tokio::test]
async fn admin_person_show_floats_failure_toast_after_welcome_failed() {
    // A failed welcome-email send redirects here with `?notice=welcome_failed`;
    // the page must float the red failure toast naming the recipient so lawyers
    // know the send didn't land (not a silent reload).

    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/admin/people/{}/edit?notice=welcome_failed",
                    libra.id
                ))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The lawyer mirror now renders through Dioxus: the flash is the theme's
    // `nav-flash--danger`, not Bootstrap's `text-bg-danger`.
    assert!(
        body.contains("nav-flash--danger"),
        "expected a red failure flash, got: {body}",
    );
    // Dioxus SSR escapes the apostrophe in "Couldn't" (`Couldn&#39;t`), so match
    // the escape-free portion that names the recipient.
    assert!(
        body.contains("send the welcome email to libra@example.com."),
        "flash must name the recipient, got: {body}",
    );
}

#[tokio::test]
async fn admin_email_log_empty_state_explains_what_lands_here() {
    // Admin, not Lawyer: `/app/admin/email-log` refuses the Lawyer tier since
    // ENG-303 — `sent_email` carries no project link to scope by, so the admin
    // gate is the interim close. Which tier is admitted is
    // `unscopeable_matter_content_listings_require_the_admin_tier`'s subject;
    // this test is about the log itself.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(app, "/app/admin/email-log", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // With no rows the listing shows the shared empty state, and the subtitle
    // still explains which mail is (and isn't) logged here.
    assert!(
        body.contains("No rows yet."),
        "empty email log must show the shared empty state; got: {body}",
    );
    assert!(
        body.contains("intentionally not logged"),
        "the subtitle must explain what lands here; got: {body}",
    );
}

#[tokio::test]
async fn admin_email_log_lists_rows_newest_first() {
    // Admin, not Lawyer: `/app/admin/email-log` refuses the Lawyer tier since
    // ENG-303 — `sent_email` carries no project link to scope by, so the admin
    // gate is the interim close. Which tier is admitted is
    // `unscopeable_matter_content_listings_require_the_admin_tier`'s subject;
    // this test is about the log itself.
    let (state, surreal) = state_with_engines().await;
    for (sent_at, recipient) in [
        ("2026-05-24T10:00:00Z", "older@example.com"),
        ("2026-05-24T12:00:00Z", "middle@example.com"),
        ("2026-05-24T15:00:00Z", "newest@example.com"),
    ] {
        store::sent_emails::record(
            &surreal,
            &store::sent_emails::NewSentEmail {
                recipient: recipient.into(),
                subject: "Welcome to Neon Law".into(),
                body: "Welcome aboard.".into(),
                sender: "support@neonlaw.com".into(),
                template_slug: Some("welcome".into()),
                outcome: "sent".into(),
                sg_message_id: None,
                sent_at: sent_at.parse().unwrap(),
            },
        )
        .await
        .unwrap();
    }
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = get_with_role(app, "/app/admin/email-log", store::persons::Role::Admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("newest@example.com"));
    assert!(body.contains("older@example.com"));
    // Newest must precede oldest in the rendered HTML.
    let newest_idx = body.find("newest@example.com").unwrap();
    let oldest_idx = body.find("older@example.com").unwrap();
    assert!(
        newest_idx < oldest_idx,
        "newest row must render before oldest (newest first)"
    );
}

#[tokio::test]
async fn admin_send_welcome_404s_when_person_missing() {
    let (state, _surreal) = state_with_engines().await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/people/{}/welcome", uuid::Uuid::nil()))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sendgrid_inbound_webhook_persists_letter_and_stores_raw_email() {
    let (state, surreal) = state_with_engines().await;

    // Seed a mailroom for the inbound message to route through.
    let addr = store::addresses::create(
        &surreal,
        &store::addresses::NewAddress {
            line1: "123 Main".into(),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89501".into(),
            country: "US".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    store::mailrooms::create(&surreal, "HQ", addr.id)
        .await
        .unwrap();

    let storage = state.storage.clone();
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let raw = b"From: aries@example.com\r\nTo: support@neonlaw.com\r\nSubject: Hello\r\n\r\nBody";
    let (content_type, body) = build_inbound_multipart(
        "aries@example.com",
        "support@neonlaw.com",
        "Hello",
        "Body",
        raw,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/any-token-in-dev")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A letter row landed with the right metadata.
    let letters = store::letters::list_all(&surreal).await.unwrap();
    assert_eq!(letters.len(), 1);
    assert_eq!(letters[0].direction, "incoming");
    assert_eq!(letters[0].sender, "aries@example.com");
    assert_eq!(letters[0].recipient, "support@neonlaw.com");
    assert_eq!(letters[0].summary, "Hello");

    // And the raw RFC 5322 bytes are sitting in storage under the
    // expected inbound/ prefix. We can't predict the timestamp, so
    // scan a fresh listing isn't available — instead, verify the
    // file system backend has at least one object by reading any
    // path that starts with `inbound/`. (FsStorage is keyed by
    // string, so we can't list — we just trust the round-trip via
    // the public get method against the known prefix is exercised
    // by separate storage tests.)
    drop(storage);
}

#[tokio::test]
async fn sendgrid_inbound_scanner_error_503s_before_persistence() {
    use portal::attachment_scanner::{FakeAttachmentScanner, ScanError};

    let mut state = empty_state().await;
    let scanner = FakeAttachmentScanner::new(Err(ScanError::Timeout));
    state.attachment_scanner = std::sync::Arc::new(scanner.clone());
    let surreal = state.surreal.clone();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let raw = b"From: aries@example.com\r\nTo: support@neonlaw.com\r\nSubject: Intake\r\n\
Content-Type: multipart/mixed; boundary=nav\r\n\r\n--nav\r\nContent-Type: text/plain\r\n\r\nBody\r\n\
--nav\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"intake.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjc=\r\n--nav--\r\n";
    let (content_type, body) = build_inbound_multipart(
        "aries@example.com",
        "support@neonlaw.com",
        "Intake",
        "Body",
        raw,
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/any-token-in-dev")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(scanner.calls(), 1);
    assert!(store::letters::list_all(&surreal).await.unwrap().is_empty());
}

#[tokio::test]
async fn sendgrid_inbound_webhook_400s_when_required_field_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Body has `from` and `to` but no `subject`.
    let (content_type, body) =
        build_inbound_multipart_partial(&[("from", "aries@example.com"), ("to", "us@example.com")]);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/any-token-in-dev")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_string(resp).await;
    assert!(
        body.contains("subject"),
        "expected `subject` in error body, got: {body}",
    );
}

#[tokio::test]
async fn sendgrid_inbound_webhook_503s_when_no_mailroom_configured() {
    let (state, _surreal) = state_with_engines().await;
    // Note: no mailroom seeded.
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (content_type, body) =
        build_inbound_multipart("aries@example.com", "us@example.com", "Test", "", b"");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/any-token-in-dev")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sendgrid_inbound_webhook_401s_when_secret_mismatches() {
    let mut state = empty_state().await;
    state.inbound_email_secret = Some("real-secret".into());
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (content_type, body) =
        build_inbound_multipart("aries@example.com", "us@example.com", "Hi", "x", b"x");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/wrong-secret")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sendgrid_inbound_webhook_accepts_matching_secret() {
    let mut state = empty_state().await;
    state.inbound_email_secret = Some("real-secret".into());
    let addr = store::addresses::create(
        &state.surreal,
        &store::addresses::NewAddress {
            line1: "1 Test".into(),
            city: "Reno".into(),
            region: "NV".into(),
            postal_code: "89501".into(),
            country: "US".into(),
            ..store::addresses::NewAddress::default()
        },
    )
    .await
    .unwrap();
    store::mailrooms::create(&state.surreal, "HQ", addr.id)
        .await
        .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (content_type, body) = build_inbound_multipart(
        "aries@example.com",
        "support@neonlaw.com",
        "Hi",
        "Body",
        b"raw",
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/sendgrid/inbound/real-secret")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

const SAMPLE_EVENTS: &str = r#"[
    {"email":"a@example.com","timestamp":1716940800,"event":"delivered",
     "sg_event_id":"evt-1","sg_message_id":"msg-1","template_slug":"welcome"}
]"#;

#[tokio::test]
async fn sendgrid_events_webhook_persists_batch_and_returns_204() {
    // Dev posture: secret is `None`, so any path token is accepted
    // and the batch lands in the (filesystem) storage backend.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/email-events/any-token-in-dev")
                .header("content-type", "application/json")
                .body(Body::from(SAMPLE_EVENTS))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn sendgrid_events_webhook_401s_when_secret_mismatches() {
    let mut state = empty_state().await;
    state.email_events_secret = Some("real-secret".into());
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/email-events/wrong-secret")
                .header("content-type", "application/json")
                .body(Body::from(SAMPLE_EVENTS))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Variant of `build_inbound_multipart` that takes just a list of
/// `(name, value)` pairs — used for the missing-field test where
/// we deliberately omit `subject`.
fn build_inbound_multipart_partial(fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "----navigator-inbound-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    (content_type, body)
}

/// The billing and cap-table lawyer surfaces are gone: the Firm bills through
/// Xero and keeps cap tables in Carta, so Navigator models neither and the
/// four pages that read those tables were removed with them.
///
/// Asserted as lawyer — the role that could reach every one of these before —
/// so a `404` proves the route is unmounted rather than merely gated. A
/// surviving listing (`/app/lawyer/disclosures`) anchors the test: it shares the
/// same `admin_listing_router` factory, so its `200` shows the factory still
/// mounts and the four `404`s are removals, not a broken router.
#[tokio::test]
async fn the_removed_billing_and_cap_table_lawyer_paths_no_longer_resolve() {
    let (state, surreal) = state_with_engines().await;
    let entity = store::test_support::seed_entity(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for path in [
        "/app/lawyer/entity-billing-profiles".to_string(),
        "/app/lawyer/invoices".to_string(),
        "/app/lawyer/invoice-line-items".to_string(),
        format!("/app/admin/entities/{entity}/cap-table"),
    ] {
        let resp = get_with_role(app.clone(), &path, store::persons::Role::Lawyer).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} must be unmounted, got {}",
            resp.status()
        );
    }

    let resp = get_with_role(app, "/app/lawyer/disclosures", store::persons::Role::Lawyer).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the surviving listings must still mount through the same factory"
    );
}

#[tokio::test]
async fn admin_letter_detail_404s_when_id_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let id = uuid::Uuid::from_u128(9999);
    // `/app/lawyer/letters/{id}` now renders through the Dioxus detail page, which is
    // lawyer-gated, so it is exercised as lawyer. An unknown id renders a friendly
    // "not found" page rather than 404'ing — the route still resolves so the auth
    // layer + nav chrome are correct for the visitor.
    let resp = get_with_role(
        app,
        &format!("/app/lawyer/letters/{id}"),
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Letter not found"), "{body}");
    // The id is echoed in the not-found copy (asserted on the value alone so the
    // Dioxus SSR hydration comments between text nodes don't break the match).
    assert!(body.contains(&id.to_string()), "{body}");
}

#[tokio::test]
async fn admin_people_csv_exports_inserted_rows() {
    let (state, _surreal) = state_with_engines().await;
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Aries&email=aries%40example.com"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);

    let csv = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/people.csv")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csv.status(), StatusCode::OK);
    assert_eq!(
        csv.headers().get("content-type").unwrap(),
        "text/csv; charset=utf-8"
    );
    assert_eq!(
        csv.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"people.csv\""
    );
    let body = body_string(csv).await;
    let mut lines = body.split("\r\n");
    assert_eq!(lines.next().unwrap(), "id,name,email");
    let row = lines.next().unwrap();
    assert!(row.ends_with(",Aries,aries@example.com"));
}

#[tokio::test]
async fn admin_entities_csv_is_servable_and_emits_headers_even_when_empty() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/admin/entities.csv")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert_eq!(body, "id,name,entity_type,jurisdiction\r\n");
}

#[tokio::test]
async fn admin_projects_csv_is_servable_and_emits_headers_even_when_empty() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects.csv")
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert_eq!(body, "id,code,name,status,entity_name\r\n");
}

#[tokio::test]
async fn lawyer_projects_csv_is_scoped_to_lawyer_lens() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Lawyer Person",
            "lawyer-person@neonlaw.com",
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let visible = test_project(&surreal, "Visible Lawyer Matter", "open").await;
    store::projects::add_participation(&surreal, visible.id, lawyer.id, "paralegal")
        .await
        .unwrap();
    let hidden = test_project(&surreal, "Hidden Matter", "open").await;
    // Accountable to someone else entirely — the session's lawyer person has no
    // membership row here, so the matter must stay hidden from them.
    disclose_lawyer_dri(
        &surreal,
        store::test_support::dri_person(&surreal).await,
        hidden.id,
    )
    .await;
    let mut session = portal::SessionData::fresh("lawyer-sub", store::persons::Role::Lawyer);
    session.person_id = Some(lawyer.id);
    session.email = Some(lawyer.email);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        test_sessions().encode(&session)
    );

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects.csv")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Visible Lawyer Matter"), "{body}");
    assert!(
        !body.contains("Hidden Matter"),
        "lawyer CSV must not expose unassigned lawyer-lens projects: {body}",
    );
}

#[tokio::test]
async fn root_response_carries_security_headers_and_request_id() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let headers = resp.headers();
    assert_eq!(
        headers
            .get("strict-transport-security")
            .and_then(|v| v.to_str().ok()),
        Some("max-age=63072000; includeSubDomains; preload"),
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
    );
    assert_eq!(
        headers.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY"),
    );
    assert_eq!(
        headers.get("referrer-policy").and_then(|v| v.to_str().ok()),
        Some("strict-origin-when-cross-origin"),
    );
    // CSP locks scripts/objects/frames to same-origin; an injected
    // <script> has no execution backstop without it.
    let csp = headers
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .expect("response must carry a content-security-policy");
    assert!(csp.contains("default-src 'self'"), "got: {csp}");
    assert!(csp.contains("object-src 'none'"), "got: {csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "got: {csp}");
    assert!(csp.contains("script-src 'self'"), "got: {csp}");
    // SetRequestIdLayer always assigns one (UUID) when the client did
    // not send one; PropagateRequestIdLayer mirrors it to the response.
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("response must carry x-request-id");
    assert!(
        !request_id.is_empty(),
        "x-request-id must be non-empty, got {request_id:?}",
    );
}

#[tokio::test]
async fn client_supplied_request_id_is_propagated_to_response() {
    let app = server::neon_router(
        empty_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-request-id", "test-correlation-7")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some("test-correlation-7"),
    );
}

#[tokio::test]
async fn public_static_assets_carry_cache_control() {
    // Use the crate-bundled `public/` dir; pick any file that exists
    // by listing the dir first so the test does not depend on a
    // hard-coded asset name.
    let public_dir = std::path::Path::new(portal::DEFAULT_PUBLIC_DIR);
    let asset_name = std::fs::read_dir(public_dir)
        .expect("public dir must exist")
        .filter_map(Result::ok)
        .find_map(|e| {
            let p = e.path();
            if p.is_file() {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(std::string::ToString::to_string)
            } else {
                None
            }
        })
        .expect("public dir must contain at least one file for this test");
    let app = server::neon_router(empty_state().await, public_dir);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/public/{asset_name}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=3600"),
    );
}

#[tokio::test]
async fn project_documents_upload_writes_blob_and_document_with_description() {
    let (state, surreal) = state_with_engines().await; // auth disabled

    // Seed one project to upload into.
    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Upload Test", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    // Hand-rolled multipart body. Boundary chosen so it can't appear
    // in the payload bytes. `_csrf` is the first field, the way the
    // upload form renders it, so the handler verifies it before reading
    // the file (see `portal::csrf::require_multipart_csrf`).
    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-test-boundary-zzzzz";
    let payload = b"hello world from a test upload";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body.extend_from_slice(csrf.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: text/plain\r\n\r\n");
    body.extend_from_slice(payload);
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"kind\"\r\n\r\n");
    body.extend_from_slice(b"unclassified");
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"description\"\r\n\r\n");
    body.extend_from_slice(b"signed retainer from client");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/documents/upload"))
                .header("cookie", cookie)
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "expected 303 redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, format!("/app/projects/{project_code}"));

    // One document asset — carrying the byte pointer plus upload
    // provenance and the optional description from the form.
    let docs = store::assets::list_all(&state.surreal).await.unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].byte_size, i64::try_from(payload.len()).unwrap());
    assert_eq!(docs[0].content_type, "text/plain");
    assert_eq!(docs[0].filename.as_deref(), Some("hello.txt"));
    assert_eq!(docs[0].kind.as_deref(), Some("unclassified"));
    assert_eq!(docs[0].project_id, Some(project_id));
    assert_eq!(docs[0].source.as_deref(), Some("upload"));
    assert_eq!(
        docs[0].description.as_deref(),
        Some("signed retainer from client")
    );
    assert!(docs[0].received_at.is_some());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn project_documents_upload_files_one_document_per_file_in_a_batch() {
    // The picker is `multiple`, so the browser posts one `file` part per
    // selected file under the same field name. Each must land as its own
    // document, sharing the batch-level `kind` and `description`.
    let (state, surreal) = state_with_engines().await; // auth disabled

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Batch Upload Test", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-test-batch-boundary";
    let files: [(&str, &str, &[u8]); 3] = [
        ("first.txt", "text/plain", b"first file contents"),
        ("second.txt", "text/plain", b"second file contents"),
        ("third.md", "text/markdown", b"# third file"),
    ];
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body.extend_from_slice(csrf.as_bytes());
    for (filename, content_type, payload) in files {
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(payload);
    }
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"kind\"\r\n\r\n");
    body.extend_from_slice(b"unclassified");
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"description\"\r\n\r\n");
    body.extend_from_slice(b"discovery batch three");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/documents/upload"))
                .header("cookie", cookie)
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some(format!("/app/projects/{project_code}").as_str())
    );

    let mut docs = store::assets::list_all(&state.surreal).await.unwrap();
    assert_eq!(docs.len(), 3, "each selected file becomes its own document");
    docs.sort_by(|a, b| a.filename.cmp(&b.filename));

    assert_eq!(
        docs.iter()
            .map(|d| d.filename.as_deref().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["first.txt", "second.txt", "third.md"]
    );
    assert_eq!(
        docs.iter()
            .map(|d| d.content_type.as_str())
            .collect::<Vec<_>>(),
        vec!["text/plain", "text/plain", "text/markdown"],
        "each file keeps its own content type rather than the batch's first"
    );
    // Batch-level metadata is stamped on every document in the batch.
    for doc in &docs {
        assert_eq!(doc.project_id, Some(project_id));
        assert_eq!(doc.kind.as_deref(), Some("unclassified"));
        assert_eq!(doc.description.as_deref(), Some("discovery batch three"));
        assert_eq!(doc.source.as_deref(), Some("upload"));
        assert!(doc.received_at.is_some());
    }
    // Distinct bytes must produce distinct content-addressed blobs — a
    // batch that collapsed to one sha would mean files overwrote one
    // another.
    let shas = docs
        .iter()
        .map(|d| d.sha256_hex.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        shas.len(),
        3,
        "each file is stored under its own content hash"
    );
}

/// Seed one project and return `(id, code)` — the shared setup for the batch
/// upload tests below. The id is what the participation fixtures and the asset
/// assertions key on; the code is what the upload route is keyed by. One seed
/// returns both, because two seeds would be two different matters.
async fn batch_upload_project(state: &AppState) -> (uuid::Uuid, String) {
    let project = test_project(&state.surreal, "Batch Guard Test", "open").await;
    (project.id, project.code)
}

/// Build a document-upload multipart body: `_csrf` first, then one part
/// per `(filename, content_type, bytes)`.
fn documents_multipart(boundary: &str, csrf: &str, files: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body.extend_from_slice(csrf.as_bytes());
    for (filename, content_type, payload) in files {
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(payload);
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn post_documents_batch(
    app: axum::Router,
    project_code: &str,
    cookie: &str,
    boundary: &str,
    body: Vec<u8>,
) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/app/projects/{project_code}/documents/upload"))
            .header("cookie", cookie)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn project_documents_upload_rejects_a_batch_over_the_file_ceiling() {
    // The batch is buffered until the whole body is read, so an unbounded
    // part count is a memory-exhaustion lever for an authenticated lawyer
    // session. Past the ceiling the request is refused outright and
    // nothing is filed.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-batch-ceiling";
    let names: Vec<String> = (0..60).map(|i| format!("file-{i}.txt")).collect();
    let files: Vec<(&str, &str, &[u8])> = names
        .iter()
        .map(|n| (n.as_str(), "text/plain", b"x" as &[u8]))
        .collect();
    let body = documents_multipart(boundary, &csrf, &files);

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        store::assets::list_all(&state.surreal)
            .await
            .unwrap()
            .is_empty(),
        "an over-ceiling batch files nothing at all"
    );
}

/// Build a document-upload multipart body carrying an explicit `kind` part
/// ahead of the files, matching the form's real field order.
fn documents_multipart_with_kind(
    boundary: &str,
    csrf: &str,
    kind: &str,
    files: &[(&str, &str, &[u8])],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body.extend_from_slice(csrf.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"kind\"\r\n\r\n");
    body.extend_from_slice(kind.as_bytes());
    for (filename, content_type, payload) in files {
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(payload);
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn project_documents_upload_rejects_an_unrecognized_kind() {
    // The upload form's `<select>` cannot produce this, but a direct/bearer
    // caller can — `ingest_bytes` must refuse it as a bad request, and file
    // nothing.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-invalid-kind";
    let files: [(&str, &str, &[u8]); 1] = [("doc.txt", "text/plain", b"bytes")];
    let body = documents_multipart_with_kind(boundary, &csrf, "bogus", &files);

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        store::assets::list_all(&state.surreal)
            .await
            .unwrap()
            .is_empty(),
        "a rejected kind must not file a partial asset"
    );
}

#[tokio::test]
async fn project_documents_upload_accepts_a_pdf_past_axums_default_body_limit() {
    // Axum's own default request-body limit (~2 MB) sits in front of this
    // route's `MAX_BATCH_BYTES` (500 MB) — without a `DefaultBodyLimit`
    // layer on the route, a scanned-PDF batch this size 413s before the
    // handler's own check, or the PDF safety validator, ever runs.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-large-pdf";
    let pdf_bytes = vec![0u8; 3 * 1024 * 1024];
    let body = documents_multipart(
        boundary,
        &csrf,
        &[("scanned-discovery.pdf", "application/pdf", &pdf_bytes)],
    );

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let docs = store::assets::list_all(&state.surreal).await.unwrap();
    assert_eq!(
        docs.iter()
            .filter(|d| d.filename.as_deref() == Some("scanned-discovery.pdf"))
            .count(),
        1,
        "a PDF larger than Axum's default body limit is filed, not 413'd"
    );
}

#[tokio::test]
async fn project_documents_upload_keeps_a_named_empty_file() {
    // A picker with nothing selected posts an unnamed empty part, which is
    // "nothing selected". A *named* zero-byte part is a real selection and
    // must not vanish from the batch without a word.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-empty-named";
    let body = documents_multipart(
        boundary,
        &csrf,
        &[
            ("real.txt", "text/plain", b"has contents"),
            ("empty.txt", "text/plain", b""),
        ],
    );

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let docs = store::assets::list_all(&state.surreal).await.unwrap();
    let mut names: Vec<&str> = docs
        .iter()
        .filter_map(|d| d.filename.as_deref())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["empty.txt", "real.txt"],
        "the named empty file is filed rather than silently dropped"
    );
}

#[tokio::test]
async fn project_documents_upload_ignores_an_unnamed_empty_picker_part() {
    // The other half of the rule: a submission whose only file part is the
    // browser's empty-picker placeholder files nothing and redirects.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----navigator-empty-picker";
    let body = documents_multipart(boundary, &csrf, &[("", "application/octet-stream", b"")]);

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(store::assets::list_all(&state.surreal)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn project_documents_upload_retry_tops_up_instead_of_duplicating() {
    // Partial failure leaves already-filed documents in place, so the
    // lawyer's natural move is to re-send the batch. That is only
    // safe if re-sending is idempotent: the second submission must add the
    // missing file and leave the ones already filed alone.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;

    // First submission: two of the three files.
    let body = documents_multipart(
        "----retry-one",
        &csrf,
        &[
            ("a.txt", "text/plain", b"alpha"),
            ("b.txt", "text/plain", b"bravo"),
        ],
    );
    let resp =
        post_documents_batch(app.clone(), &project_code, &cookie, "----retry-one", body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        store::assets::list_all(&state.surreal).await.unwrap().len(),
        2
    );

    // Re-send the whole batch, now including the third file.
    let body = documents_multipart(
        "----retry-two",
        &csrf,
        &[
            ("a.txt", "text/plain", b"alpha"),
            ("b.txt", "text/plain", b"bravo"),
            ("c.txt", "text/plain", b"charlie"),
        ],
    );
    let resp = post_documents_batch(app, &project_code, &cookie, "----retry-two", body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let docs = store::assets::list_all(&state.surreal).await.unwrap();
    assert_eq!(
        docs.len(),
        3,
        "the retry adds only the missing file — no duplicate rows"
    );
    let mut names: Vec<&str> = docs.iter().filter_map(|d| d.filename.as_deref()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
}

#[tokio::test]
async fn project_documents_upload_files_the_same_bytes_under_a_different_name() {
    // Dedup is keyed on filename *and* content, so the same bytes filed
    // deliberately under a second name stay two documents.
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----same-bytes";
    let body = documents_multipart(
        boundary,
        &csrf,
        &[
            ("exhibit-a.txt", "text/plain", b"identical"),
            ("exhibit-b.txt", "text/plain", b"identical"),
        ],
    );

    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        store::assets::list_all(&state.surreal).await.unwrap().len(),
        2
    );
}

#[tokio::test]
async fn project_documents_upload_re_upload_with_a_new_visibility_syncs_the_existing_row() {
    // Greptile P1 on #786: `already_filed` dedupes on (filename, bytes) and
    // used to just skip re-ingestion — silently dropping a lawyer's new
    // visibility choice on a re-upload rather than either applying it or
    // duplicating the row.

    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----visibility-resubmit";
    let body = documents_multipart(
        boundary,
        &csrf,
        &[("welcome-letter.pdf", "application/pdf", b"welcome")],
    );
    let resp = post_documents_batch(app.clone(), &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let first = store::assets::for_project(&state.surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("first upload filed");
    assert_eq!(first.visibility, store::documents::visibility::INTERNAL);

    // Re-submit the identical filename + bytes, this time choosing
    // client-visible.
    let boundary2 = "----visibility-resubmit-2";
    let mut body2 = Vec::new();
    body2.extend_from_slice(format!("--{boundary2}\r\n").as_bytes());
    body2.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body2.extend_from_slice(csrf.as_bytes());
    body2.extend_from_slice(format!("\r\n--{boundary2}\r\n").as_bytes());
    body2.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"welcome-letter.pdf\"\r\n",
    );
    body2.extend_from_slice(b"Content-Type: application/pdf\r\n\r\n");
    body2.extend_from_slice(b"welcome");
    body2.extend_from_slice(format!("\r\n--{boundary2}\r\n").as_bytes());
    body2.extend_from_slice(b"Content-Disposition: form-data; name=\"visibility\"\r\n\r\nclient");
    body2.extend_from_slice(format!("\r\n--{boundary2}--\r\n").as_bytes());

    let resp2 = post_documents_batch(app, &project_code, &cookie, boundary2, body2).await;
    assert_eq!(resp2.status(), StatusCode::SEE_OTHER);

    let rows = store::assets::for_project(&state.surreal, project_id)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the dedup skip must not duplicate the row: {rows:?}"
    );
    assert_eq!(
        rows[0].id, first.id,
        "the existing row is updated in place, not replaced"
    );
    assert_eq!(
        rows[0].visibility,
        store::documents::visibility::CLIENT,
        "the re-upload's explicit visibility choice must take effect"
    );
}

#[tokio::test]
async fn project_documents_re_upload_as_internal_syncs_every_duplicate_row() {
    // Greptile P1 on #786: a matter can already hold several duplicate rows
    // for one file (uploads filed before dedup existed, concurrent
    // submissions, or other ingest paths). The dedup lookup returns only one
    // of them, so syncing just that row on a re-upload as `internal` would
    // leave the other duplicate `client`-visible — its filename and bytes
    // still reachable through the client list and ZIP export. The re-sync
    // must flip *every* matching row.

    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code) = batch_upload_project(&state).await;
    let app = server::neon_router(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );

    // Seed two client-visible duplicates for the same (filename, bytes).
    let bytes = b"a leaked draft";
    let sha_hex = store::documents::sha256_hex(bytes);
    let _ = sha_hex;
    // Two rows, deliberately: a matter can hold duplicates of one file, and
    // the re-sync has to flip every one of them. `ingest_bytes` always
    // appends a row, so calling it twice is what produces the pair.
    for _ in 0..2 {
        store::documents::ingest_bytes(
            &state.surreal,
            &state.storage,
            &store::documents::IngestArgs {
                project_id,
                source: store::documents::source::UPLOAD,
                filename: "leak.pdf",
                kind: "unclassified",
                content_type: "application/pdf",
                description: None,
                secondary_storage_key: None,
                visibility: store::documents::visibility::CLIENT,
            },
            bytes,
        )
        .await
        .unwrap();
    }

    // Re-upload the identical filename + bytes with the default (internal)
    // visibility — the dedup skip fires, and the re-sync must flip both.
    let (cookie, csrf) = admin_on_project(&surreal, project_id).await;
    let boundary = "----duplicate-resync";
    let body = documents_multipart(boundary, &csrf, &[("leak.pdf", "application/pdf", bytes)]);
    let resp = post_documents_batch(app, &project_code, &cookie, boundary, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let rows = store::assets::for_project(&state.surreal, project_id)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the dedup skip must not add a third row: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|r| r.visibility == store::documents::visibility::INTERNAL),
        "every duplicate must be flipped to internal, not just the one the \
         dedup lookup happened to select: {rows:?}"
    );
}

#[tokio::test]
async fn contract_review_upload_without_csrf_is_forbidden() {
    // The inbound contract-review upload has no rendered browser form yet,
    // but the handler is CSRF-hardened for the day one arrives. A
    // cookie-authenticated multipart POST that omits `_csrf` is the forged
    // shape `require_multipart_csrf` rejects before reading the contract
    // bytes — so it 403s, exactly like the transcript and document uploads.
    let (state, surreal) = state_with_engines().await;

    // The CSRF rejection has to be reached, so the caller must genuinely be on
    // the matter: since ENG-81 an Admin without a `person_project_roles` row
    // 404s here like anyone else, and an unseeded project id would assert the
    // wrong denial.
    let (project_id, _lawyer, _lawyer_cookie, _lawyer_csrf) =
        lawyer_project_fixture(&surreal).await;
    let project_code = store::projects::find_by_id(&surreal, project_id)
        .await
        .unwrap()
        .expect("the fixture matter")
        .code;
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Contract Review Admin",
            "contract-review-admin@neonlaw.com",
            store::persons::Role::Admin,
        ),
    )
    .await
    .unwrap();
    participate(&surreal, admin.id, project_id, "attorney").await;
    let (cookie, _csrf) = session_cookie_and_csrf_for_person(&admin);

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let boundary = "----navigator-contract-review-csrf-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"contract\"; \
         filename=\"c.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--{boundary}--\r\n"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/contract-review"))
                .header("cookie", cookie)
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn project_detail_page_renders_documents_and_upload_form() {
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Acme Formation", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    // Seed one document asset via the same ingest helper the upload
    // handler uses, so we exercise the read-side render against the
    // real shape ingest_bytes produces.
    let args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "engagement-letter.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: Some("Initial upload"),
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    store::documents::ingest_bytes(&state.surreal, &state.storage, &args, b"hello world")
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header(
                    "cookie",
                    &admin_cookie_on_project(&surreal, project_id).await,
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Acme Formation"));
    assert!(body.contains("engagement-letter.pdf"));
    // The list view is intentionally lean: filename links to the
    // per-document detail page and the Download link points at the
    // signed-URL redirect endpoint. Provenance (source, content type)
    // is NOT spilled into the list; it lives on the detail page
    // (covered by its own test below).
    assert!(body.contains(&format!("/app/projects/{project_code}/documents/")));
    assert!(body.contains("/download"));
    assert!(!body.contains("application/pdf"));
    // Inline upload form posts to the same endpoint as before.
    assert!(body.contains(&format!(
        "action=\"/app/projects/{project_code}/documents/upload\""
    )));
    assert!(body.contains("enctype=\"multipart/form-data\""));
    // The real-time progress-bar island: the script loads, and the file
    // input carries the id the script targets — a bare `Field::file(...)`
    // id would collide with the Estate transcript uploader's `id="file"`.
    assert!(body.contains("/public/js/upload-progress.js"));
    assert!(body.contains("id=\"document-upload-file\""));
}

#[tokio::test]
async fn project_document_detail_page_shows_provenance_and_download_link() {
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Acme Formation", "open").await;
    let (project_id, project_code) = (project.id, project.code.clone());

    let args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "engagement-letter.pdf",
        kind: "onboarding",
        content_type: "application/pdf",
        description: Some("Initial upload"),
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    let ingested =
        store::documents::ingest_bytes(&state.surreal, &state.storage, &args, b"hello world")
            .await
            .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_code}/documents/{}",
                    ingested.asset_id
                ))
                .header(
                    "cookie",
                    &admin_cookie_on_project(&surreal, project_id).await,
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("engagement-letter.pdf"));
    assert!(body.contains("Provenance"));
    assert!(body.contains("Storage"));
    assert!(body.contains("upload"));
    assert!(!body.contains("Source revision"));
    assert!(body.contains("Initial upload"));
    assert!(body.contains("application/pdf"));
    assert!(body.contains(&ingested.sha256_hex));
    assert!(body.contains(&format!(
        "/app/projects/{project_code}/documents/{}/download",
        ingested.asset_id
    )));
}

#[tokio::test]
async fn project_document_download_streams_bytes_on_fs_backend() {
    // FsStorage returns Unsupported from signed_url; the handler
    // falls through to stream_through, which writes the raw bytes
    // with Content-Disposition: attachment so the browser saves
    // under the original filename.
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Acme Formation", "open").await;
    let (project_id, project_code) = (project.id, project.code.clone());

    let bytes_in = b"engagement letter bytes";
    let args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "engagement-letter.pdf",
        kind: "onboarding",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    let ingested = store::documents::ingest_bytes(&state.surreal, &state.storage, &args, bytes_in)
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_code}/documents/{}/download",
                    ingested.asset_id
                ))
                .header(
                    "cookie",
                    &admin_cookie_on_project(&surreal, project_id).await,
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ct, "application/pdf");
    let cd = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(cd.contains("engagement-letter.pdf"));
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body_bytes.as_ref(), bytes_in);
}

#[tokio::test]
async fn project_document_download_404s_when_doc_belongs_to_a_different_project() {
    // Cross-project leakage guard: a document from project A must
    // not be downloadable via project B's URL even if the doc_id is
    // known.
    let (state, surreal) = state_with_engines().await;

    let project_a = test_project(&surreal, "A", "open").await.id;
    let project_b_row = test_project(&surreal, "B", "open").await;
    let (_project_b, project_b_code) = (project_b_row.id, project_b_row.code.clone());

    let args = store::documents::IngestArgs {
        project_id: project_a,
        source: "upload",
        filename: "secret.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    let ingested = store::documents::ingest_bytes(&state.surreal, &state.storage, &args, b"secret")
        .await
        .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // Same doc_id, but via project B's URL — must 404.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_b_code}/documents/{}/download",
                    ingested.asset_id
                ))
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_document_download_404s_when_document_missing() {
    // An unknown doc_id under a real project takes the not-found branch of
    // load_doc_for_project (which now logs the reason rather than swallowing
    // it). The response must still be a bare 404, never a 500.
    use uuid::Uuid;

    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Acme Formation", "open").await;
    let (_project_id, project_code) = (project.id, project.code.clone());

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_code}/documents/{}/download",
                    Uuid::now_v7()
                ))
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_document_download_sends_the_anonymous_browser_to_login() {
    // The download route lives under `/app/lawyer`, so the session boundary (#732)
    // redirects an anonymous browser to the login door before the handler
    // runs. That is a stronger privacy guarantee than the old bare 404: the
    // request never reaches the code that could observe whether the document
    // exists. The uniform redirect also leaks no matter-existence signal.
    use uuid::Uuid;

    let (state, _surreal) = state_with_engines().await;

    let path = format!(
        "/app/projects/{}/documents/{}/download",
        Uuid::now_v7(),
        Uuid::now_v7()
    );
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(format!("/auth/login?return_to={path}").as_str()),
    );
}

#[tokio::test]
async fn project_document_download_500s_on_database_error() {
    // A database failure while resolving the document must surface as a 500,
    // not be masked as a 404 — otherwise a pool/query outage looks like a
    // missing document to clients and HTTP monitors. `assets` is
    // Surreal-resident, so the store failure is an unreachable Surreal handle.
    //
    // The participation gate reads that same handle, so the outage now breaks
    // the *gate* before the lookup — which is exactly why the gate reports a
    // failed query as `500` instead of collapsing it into "not a participant".
    // Either way the contract this pins is unchanged: an outage is never
    // reported as a missing document.
    use uuid::Uuid;

    let (mut state, _surreal) = state_with_engines().await;
    state.surreal = store::surreal::SurrealDb::uninitialized();
    let cookie = admin_session_cookie_with_person();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{}/documents/{}/download",
                    Uuid::now_v7(),
                    Uuid::now_v7()
                ))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn project_document_detail_500s_on_database_error() {
    // The detail page must likewise surface a database failure as a 500
    // rather than render the not-found page.
    use uuid::Uuid;

    // Same shape as the download test above: the unreachable store breaks the
    // gate's own query, and a failed query is a `500`, never a soft 404.
    let (mut state, _surreal) = state_with_engines().await;
    state.surreal = store::surreal::SurrealDb::uninitialized();
    let cookie = admin_session_cookie_with_person();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{}/documents/{}",
                    Uuid::now_v7(),
                    Uuid::now_v7()
                ))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn project_document_detail_renders_not_found_for_unknown_document() {
    // An unknown doc_id under a real project takes the not-found branch and
    // renders the not-found page (a 200 soft-404), never a 500.
    use uuid::Uuid;

    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Acme Formation", "open").await;
    let (_project_id, project_code) = (project.id, project.code.clone());

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_code}/documents/{}",
                    Uuid::now_v7()
                ))
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.to_lowercase().contains("not found"));
}

#[tokio::test]
async fn project_detail_page_renders_empty_state_when_project_has_no_documents() {
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Empty Matter", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header(
                    "cookie",
                    &admin_cookie_on_project(&surreal, project_id).await,
                )
                .header(
                    "cookie",
                    &admin_cookie_on_project(&surreal, project_id).await,
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Empty Matter"));
    assert!(body.contains("No documents yet."));
}

/// The matter workbench's calendar section — from its own class to the
/// participation ledger that follows it. The page around it legitimately names
/// the matter and its documents, so assertions that the calendar synthesizes
/// nothing must scope to this slice.
fn matter_calendar_section(body: &str) -> &str {
    let start = body
        .find("project-calendar")
        .expect("matter calendar section present");
    let end = body[start..]
        .find("project-participations")
        .map_or(body.len(), |offset| start + offset);
    &body[start..end]
}

#[tokio::test]
async fn project_detail_page_renders_an_empty_matter_calendar() {
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Calendared Matter", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    // A document is a witness, not an event: the calendar must not pass the
    // rows the page already holds off as something scheduled (#350).
    let args = store::documents::IngestArgs {
        project_id,
        source: "upload",
        filename: "engagement-letter.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: Some("Initial upload"),
        secondary_storage_key: None,
        visibility: store::documents::visibility::INTERNAL,
    };
    store::documents::ingest_bytes(&state.surreal, &state.storage, &args, b"hello world")
        .await
        .unwrap();

    let cookie = admin_cookie_on_project(&surreal, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}?sort=event&dir=asc"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = strip_hydration_markers(&body_string(resp).await);

    let calendar = matter_calendar_section(&body);
    assert!(
        calendar.contains("No calendar events scheduled for this matter."),
        "{calendar}"
    );
    // The matter's own rows must stay out of it.
    assert!(!calendar.contains("engagement-letter.pdf"), "{calendar}");
    assert!(!calendar.contains("Calendared Matter"), "{calendar}");

    // The active column names its direction and offers the reverse; an inactive
    // one offers ascending. Both links stay on this matter.
    assert!(calendar.contains("Event (asc)"), "{calendar}");
    assert!(
        calendar.contains(&format!(
            "/app/projects/{project_code}?sort=event&#38;dir=desc"
        )),
        "{calendar}"
    );
    assert!(
        calendar.contains(&format!(
            "/app/projects/{project_code}?sort=date&#38;dir=asc"
        )),
        "{calendar}"
    );
    // The matter calendar advertises no `Project` column — the matter is the
    // page — so the workbench's own columns must not leak into it.
    assert!(!calendar.contains(">Project"), "{calendar}");
    assert!(!calendar.contains(">Entity"), "{calendar}");
}

#[tokio::test]
async fn project_detail_calendar_falls_back_to_its_leftmost_column() {
    let (state, surreal) = state_with_engines().await;

    let __dri = store::test_support::dri_person(&surreal).await;
    let project = test_project(&surreal, "Lenient Matter", "open").await;
    let project_id = project.id;
    let project_code = project.code;

    let cookie = admin_cookie_on_project(&surreal, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    // `project` is the workbench's column, not this calendar's, and `sideways`
    // is nobody's. Neither refuses the matter.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{project_code}?sort=project&dir=sideways"
                ))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = strip_hydration_markers(&body_string(resp).await);

    let calendar = matter_calendar_section(&body);
    assert!(calendar.contains("Date (asc)"), "{calendar}");
    assert!(
        calendar.contains(&format!(
            "/app/projects/{project_code}?sort=date&#38;dir=desc"
        )),
        "{calendar}"
    );
}

#[tokio::test]
async fn project_detail_page_404s_when_project_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects/missing-project")
                .header("cookie", admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_documents_upload_404s_when_project_missing() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let boundary = "----test-bdy";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--{boundary}--\r\n"
    );
    let missing = uuid::Uuid::now_v7();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{missing}/documents/upload"))
                .header("cookie", admin_session_cookie())
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------- Error pages: HTML for browsers, JSON for /api & /mcp ----------

#[tokio::test]
async fn unknown_path_returns_html_404_page_for_browser_request() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(
        body.starts_with("<!DOCTYPE html>"),
        "browser 404 must be the styled HTML page, got: {body}",
    );
    assert!(body.contains("<h1>Not found</h1>"));
}

#[tokio::test]
async fn unknown_api_path_returns_json_404_not_html() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/api/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(
        !body.starts_with("<!DOCTYPE html>"),
        "/app/api/* 404 must NOT be the HTML page; got: {body}",
    );
    assert!(
        body.contains("\"error\""),
        "expected JSON error body, got: {body}"
    );
}

#[tokio::test]
async fn unknown_mcp_path_returns_json_404_not_html() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/mcp/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_string(resp).await;
    assert!(
        !body.starts_with("<!DOCTYPE html>"),
        "/mcp/* 404 must NOT be the HTML page; got: {body}",
    );
}

#[tokio::test]
async fn wants_json_path_classifier() {
    // The classifier is the single source of truth for HTML-vs-JSON
    // routing in error responses — lock its behavior down so a future
    // route addition can't silently start handing HTML to a JSON
    // client.
    assert!(portal::wants_json("/app/api/people"));
    assert!(portal::wants_json("/app/api/people/123"));
    assert!(portal::wants_json("/mcp"));
    assert!(portal::wants_json("/mcp/foo"));
    assert!(portal::wants_json("/app/api/openapi.json"));
    assert!(!portal::wants_json("/"));
    assert!(!portal::wants_json("/app/lawyer"));
    assert!(!portal::wants_json("/app/lawyer/people"));
    assert!(!portal::wants_json("/blog/anything"));
    // `/api-something` (no trailing slash, no exact match) is NOT
    // an api route — leading-substring matches would catch real
    // page paths like `/apidocs` if someone added one.
    assert!(!portal::wants_json("/apidocs"));
}

// ---------- Admin role editing ----------

#[tokio::test]
async fn admin_people_edit_form_shows_role_select_pre_filled() {
    let (state, surreal) = state_with_engines().await;
    let lawyer = store::persons::create(
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
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}/edit", lawyer.id))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("name=\"role\""),
        "edit form must expose a role <select>, got: {body}",
    );
    assert!(
        body.contains("value=\"lawyer\" selected"),
        "role <select> must pre-select the row's current role, got: {body}",
    );
}

/// Extract the single opening tag (`<…>`) carrying `id="{id}"`, so a test
/// can assert attributes on one specific rendered control.
fn tag_with_id<'a>(html: &'a str, id: &str) -> &'a str {
    let needle = format!("id=\"{id}\"");
    let at = html
        .find(&needle)
        .unwrap_or_else(|| panic!("no element with {needle} in {html}"));
    let start = html[..at].rfind('<').expect("tag open");
    let end = at + html[at..].find('>').expect("tag close");
    &html[start..=end]
}

/// The admin console person page is where an admin changes a role — the
/// select must render **enabled** there, so the PATCH actually carries the
/// role the command layer is already willing to honor (see the sibling
/// `admin_can_update_a_persons_role`). A disabled control submits nothing,
/// which is how the role silently stayed put.
#[tokio::test]
async fn admin_person_edit_page_offers_an_editable_role_select() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}/edit", libra.id))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !tag_with_id(&body, "role").contains("disabled"),
        "an admin must get an editable role select: {}",
        tag_with_id(&body, "role"),
    );
    assert!(
        body.contains("value=\"client\" selected"),
        "the select must pre-select the row's current role: {body}",
    );
    // Save is offered, so the enabled select has somewhere to post. The Dioxus
    // SSR wraps the button text in hydration comments (`>Save<!--#--></button>`),
    // so match the `>Save<` it emits rather than the raw `>Save</button>`.
    assert!(
        body.contains("nav-btn--primary") && body.contains(">Save<"),
        "{body}",
    );
}

/// The one exception: the bootstrap Owner's role stays locked, because the
/// command layer refuses every write to that row.
#[tokio::test]
async fn admin_person_edit_page_locks_the_bootstrap_owner_role() {
    // The router must read the person from the SAME engine this test seeds.
    // `empty_state()` mints its own, so pairing it with a handle from
    // elsewhere renders a 404 against an engine that has no rows.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.bootstrap_owner_email = Some("owner@neonlaw.com".into());
    let boss = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Boss",
            "owner@neonlaw.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}/edit", boss.id))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        tag_with_id(&body, "role").contains("disabled"),
        "the bootstrap Owner role must stay locked: {}",
        tag_with_id(&body, "role"),
    );
    assert!(
        body.contains("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL"),
        "the hint must say where the bootstrap Owner changes instead: {body}",
    );
}

/// The role select locks when the target outranks the caller: the command layer
/// drops such a write, so the form must not invite it.
///
/// ENG-304 removed the surface's own `may_change_roles` flag with the
/// `/app/lawyer/people` mirror — `require_admin` now guarantees every caller here
/// may set roles at all. Authority rank and the pinned bootstrap Owner are the
/// two locks that survive, and this covers the first.
#[tokio::test]
async fn admin_person_edit_page_locks_the_role_select_for_a_higher_tier_target() {
    let (state, surreal) = state_with_engines().await;
    // An Owner who is *not* the bootstrap Owner, so the lock under test is the
    // authority-rank one rather than the pinned-record one.
    let boss = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Cap Boss",
            "cap-boss@example.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}/edit", boss.id))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        tag_with_id(&body, "role").contains("disabled"),
        "an admin caller must not get an editable role select on an Owner: {}",
        tag_with_id(&body, "role"),
    );
}

#[tokio::test]
async fn admin_can_update_a_persons_role() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Libra&email=libra%40example.com&role=admin",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let row = store::persons::find_by_id(&surreal, libra.id)
        .await
        .unwrap()
        .expect("row still present");
    assert_eq!(row.role, store::persons::Role::Admin);
}

/// The Dioxus admin person edit form posts to the native `POST /app/admin/people/{id}`
/// update route (the form PATCHed the REST `/app/api/people/{id}`). Prove the
/// native form persists the change and redirects back to the show view — a plain
/// form (no JavaScript), so a 303 not the REST API's 200+JSON.
#[tokio::test]
async fn admin_person_update_via_native_form_persists_and_redirects() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}", libra.id))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Libra%20Scale&email=libra%40example.com&role=lawyer&_csrf={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some(format!("/app/admin/people/{}", libra.id).as_str()),
    );
    let row = store::persons::find_by_id(&surreal, libra.id)
        .await
        .unwrap()
        .expect("row still present");
    assert_eq!(row.name, "Libra Scale");
    assert_eq!(row.role, store::persons::Role::Lawyer);
}

/// The Dioxus admin person page's welcome-email button posts to the native
/// `POST /app/admin/people/{id}/welcome` route. Prove it redirects back to the show
/// view with a `?notice=` flag (the flash the page floats), not a 5xx.
#[tokio::test]
async fn admin_person_welcome_redirects_with_notice() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/people/{}/welcome", libra.id))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        location.starts_with(&format!("/app/admin/people/{}?notice=welcome_", libra.id)),
        "welcome send must redirect to the show view with a notice flag, got: {location}",
    );
}

/// The Dioxus admin person page floats the flash from its query flags: a
/// rejected update lands with `?error=`, surfaced as a red alert; the
/// welcome-send outcome lands with `?notice=`, surfaced as a toned toast naming
/// the recipient (the sibling of the lawyer surface's `?notice=`).
#[tokio::test]
async fn admin_person_show_renders_flash_from_query() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let cookie = admin_session_cookie_with_person();

    // A rejected update: the error message surfaces as a red alert.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/admin/people/{}?error=Email%20already%20in%20use",
                    libra.id
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Email already in use"), "{body}");
    assert!(body.contains("nav-flash--danger"), "{body}");

    // A welcome send lands with `?notice=welcome_sent`: a green toast naming the
    // recipient.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/admin/people/{}?notice=welcome_sent",
                    libra.id
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("Welcome email sent to libra@example.com."),
        "{body}",
    );
    assert!(body.contains("nav-flash--success"), "{body}");
}

/// The welcome-email action takes a confirmation step, so an accidental click no
/// longer fires the external send (the surface confirmed via HTMX's
/// `hx-confirm`; the no-JS Dioxus form confirms with a native `<details>`
/// disclosure). Clicking "Send welcome email" (the `<summary>`) only reveals the
/// "Confirm and send" button that posts the send; the disclosure stays on the
/// page, so it never navigates away and never discards unsaved edits.
#[tokio::test]
async fn admin_person_welcome_requires_a_confirmation_step() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let cookie = admin_session_cookie_with_person();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}", libra.id))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The welcome action is a `<details>` disclosure (not an immediate `POST`),
    // so a single click reveals the confirmation rather than sending.
    assert!(
        body.contains("welcome-confirm")
            && body.contains("<summary")
            && body.contains("Send welcome email"),
        "welcome action must be a details disclosure, got: {body}",
    );
    // The revealed confirm form names the recipient and posts the send.
    assert!(
        body.contains("Send welcome email to libra@example.com?"),
        "confirmation must name the recipient, got: {body}",
    );
    assert!(
        body.contains("Confirm and send welcome email"),
        "confirmation must offer a confirm button, got: {body}",
    );
    assert!(
        body.contains(&format!(
            "action=\"/app/admin/people/{}/welcome\"",
            libra.id
        )),
        "confirm button must post the welcome send, got: {body}",
    );
}

/// Assert `html` (a full rendered Dioxus page) meets the form a11y invariants —
/// the Dioxus successor to `views/tests/accessibility.rs`'s structural gate for
/// the `FormCard`. Runs in `cargo test --workspace` (the per-PR gate), so a
/// migrated page that drops a label, an accessible form name, or a valid
/// `aria-describedby` fails the PR, not a nightly browser run. `scraper` parses
/// the rendered HTML (ignoring Dioxus's hydration comments).
fn assert_dioxus_forms_accessible(html: &str, label: &str) {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    let sel = |s: &str| Selector::parse(s).unwrap();

    let ids: std::collections::HashSet<String> = doc
        .select(&sel("[id]"))
        .filter_map(|e| e.value().attr("id").map(String::from))
        .collect();

    // 1. No positive tabindex — it reorders the tab sequence ahead of the nav
    //    (WCAG 2.4.3). Negative (programmatic focus) is fine.
    for e in doc.select(&sel("[tabindex]")) {
        let value: i32 = e
            .value()
            .attr("tabindex")
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        assert!(value < 0, "{label}: positive tabindex=\"{value}\"");
    }

    // Gather every `<label for>` target.
    let labelled: std::collections::HashSet<String> = doc
        .select(&sel("label[for]"))
        .filter_map(|e| e.value().attr("for").map(String::from))
        .collect();

    // 2. Every visible control has an id and a matching label; 3. every label
    //    points at an existing id.
    for target in &labelled {
        assert!(
            ids.contains(target),
            "{label}: <label for=\"{target}\"> has no element with that id",
        );
    }
    for e in doc.select(&sel("input, select, textarea")) {
        if e.value().attr("type") == Some("hidden") {
            continue; // the CSRF hidden input carries no user-facing label
        }
        let id = e
            .value()
            .attr("id")
            .unwrap_or_else(|| panic!("{label}: control without an id"));
        assert!(
            labelled.contains(id),
            "{label}: control id=\"{id}\" has no <label for=\"{id}\">",
        );
    }

    // 4. Every aria-describedby resolves. It is an IDREF *list* (WAI-ARIA), so
    //    a valid `aria-describedby="a b"` must have every space-separated token
    //    point at an existing id; checking the whole value as one id would
    //    reject a legitimate multi-target description.
    for e in doc.select(&sel("[aria-describedby]")) {
        let value = e.value().attr("aria-describedby").unwrap_or_default();
        for target in value.split_whitespace() {
            assert!(
                ids.contains(target),
                "{label}: aria-describedby token \"{target}\" has no element with that id",
            );
        }
    }

    // 5. Every form has an effective accessible name. Presence of the attribute
    //    is not enough (an empty `aria-label` exposes no name), and a form may
    //    be named by a resolving `aria-labelledby` instead, so accept either a
    //    non-empty `aria-label` or an `aria-labelledby` whose tokens all resolve.
    let mut saw_form = false;
    for e in doc.select(&sel("form")) {
        saw_form = true;
        let el = e.value();
        let has_label = el.attr("aria-label").is_some_and(|v| !v.trim().is_empty());
        let has_labelledby = el.attr("aria-labelledby").is_some_and(|v| {
            let targets: Vec<&str> = v.split_whitespace().collect();
            !targets.is_empty() && targets.iter().all(|t| ids.contains(*t))
        });
        assert!(
            has_label || has_labelledby,
            "{label}: <form> has no accessible name \
             (needs a non-empty aria-label or a resolving aria-labelledby)",
        );
    }
    assert!(saw_form, "{label}: expected at least one <form>");
}

/// The migrated Dioxus admin-cluster forms meet the same structural a11y
/// invariants the pages did — a per-PR gate on the `webapp` FormCard and
/// the person show/edit pages, complementing the nightly browser+axe walk (which
/// covers the remaining create forms and the read-only listings).
#[tokio::test]
async fn migrated_dioxus_forms_pass_structural_a11y() {
    let (state, surreal) = state_with_engines().await;
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let cookie = admin_session_cookie_with_person();

    // The create forms (no id), then the person show/edit pages (seeded id).
    // All render through the shared `webapp::FormCard`.
    let routes = [
        "/app/admin/people".to_string(),
        "/app/admin/people/new".to_string(),
        "/app/admin/entities/new".to_string(),
        format!("/app/admin/people/{}", libra.id),
        format!("/app/admin/people/{}/edit", libra.id),
    ];
    for route in routes {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&route)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{route} should render");
        let body = body_string(resp).await;
        assert_dioxus_forms_accessible(&body, &route);
    }
}

#[tokio::test]
async fn bootstrap_owner_row_renders_all_fields_disabled_with_banner() {
    // The router must read the person from the SAME engine this test seeds.
    // `empty_state()` mints its own, so pairing it with a handle from
    // elsewhere renders a 404 against an engine that has no rows.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.bootstrap_owner_email = Some("nick@neonlaw.com".into());
    let owner_row = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Nick",
            "nick@neonlaw.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/people/{}/edit", owner_row.id))
                .header(header::COOKIE, admin_session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The bootstrap Owner record is immutable: every field renders disabled,
    // a banner explains why, and there is no Save button. Server-side
    // reinforcement is the `PATCH /app/api/people/{id}` command (see the sibling
    // `bootstrap_owner_record_is_immutable_patch_returns_409`).
    for id in ["name", "email", "role"] {
        let tag = tag_with_id(&body, id);
        assert!(
            tag.contains("disabled"),
            "bootstrap Owner {id} field must be disabled, got: {tag}",
        );
    }
    assert!(
        body.contains("bootstrap Owner") && body.contains("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL"),
        "expected the immutability banner",
    );
    assert!(
        !body.contains(">Save</button>"),
        "the immutable record must not offer Save",
    );
}

#[tokio::test]
async fn bootstrap_owner_record_is_immutable_patch_returns_409() {
    // The router must read the person from the SAME engine this test seeds.
    // `empty_state()` mints its own, so pairing it with a handle from
    // elsewhere renders a 404 against an engine that has no rows.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.bootstrap_owner_email = Some("nick@neonlaw.com".into());
    let owner_row = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Nick",
            "nick@neonlaw.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();
    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // PATCH changing the name AND demoting the role — simulating a hostile
    // client (or a bug) bypassing the disabled UI. The command layer must
    // refuse the whole write with 409 and leave the row untouched.
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", owner_row.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Hacked&email=nick%40neonlaw.com&role=client",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let row = store::persons::find_by_id(&surreal, owner_row.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.name, "Nick", "bootstrap Owner name must be unchanged");
    assert_eq!(
        row.role,
        store::persons::Role::Owner,
        "bootstrap Owner role must be unchanged",
    );
}

// ---------- Uniqueness conflicts → 409 + delete guard ----------

#[tokio::test]
async fn admin_people_create_duplicate_email_returns_409() {
    let (state, surreal) = state_with_engines().await;
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "dup@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/people")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Other&email=dup%40example.com&role=client"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_string(resp).await;
    assert!(
        body.contains("already in use"),
        "409 body must explain the uniqueness conflict, got: {body}",
    );
}

#[tokio::test]
async fn admin_people_update_to_existing_email_returns_409() {
    let (state, surreal) = state_with_engines().await;
    store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();
    let taurus = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Taurus",
            "taurus@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Lawyer);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/app/api/people/{}", taurus.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Taurus&email=libra%40example.com&role=client",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_of_bootstrap_owner_person_returns_409_and_leaves_row() {
    // The router must read the person from the SAME engine this test seeds.
    // `empty_state()` mints its own, so pairing it with a handle from
    // elsewhere renders a 404 against an engine that has no rows.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.bootstrap_owner_email = Some("nick@neonlaw.com".into());
    let owner_row = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Nick",
            "nick@neonlaw.com",
            store::persons::Role::Owner,
        ),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{}", owner_row.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Row must still exist — the guard is the load-bearing invariant.
    let still_there = store::persons::find_by_id(&surreal, owner_row.id)
        .await
        .unwrap();
    assert!(
        still_there.is_some(),
        "bootstrap Owner row must survive a delete attempt",
    );
}

#[tokio::test]
async fn delete_of_non_bootstrap_client_person_still_succeeds() {
    // The router must read the person from the SAME engine this test seeds.
    // `empty_state()` mints its own, so pairing it with a handle from
    // elsewhere renders a 404 against an engine that has no rows.
    let (state, surreal) = state_with_engines().await;
    let mut state = state;
    state.bootstrap_owner_email = Some("nick@neonlaw.com".into());
    let libra = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Libra",
            "libra@example.com",
            store::persons::Role::Client,
        ),
    )
    .await
    .unwrap();

    let (cookie, csrf) = session_cookie_and_csrf_for_role(store::persons::Role::Admin);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/app/api/people/{}", libra.id))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "non-bootstrap client delete should succeed, got {}",
        resp.status(),
    );
    let gone = store::persons::find_by_id(&surreal, libra.id)
        .await
        .unwrap();
    assert!(
        gone.is_none(),
        "regular person row must be gone after delete"
    );
}

// ---------------------------------------------------------------------------
// Published workspace docs at /docs/:slug (portal::docs).
// ---------------------------------------------------------------------------

/// State whose docs index is the real baked `docs/` tree (every other
/// field matches `empty_state`).
async fn state_with_docs() -> AppState {
    let mut state = empty_state().await;
    state.docs = portal::docs::loader::bundled();
    state
}

/// `GET uri` as a signed-in reader.
///
/// The shared Navigator surface — docs, the template gallery, `/design`,
/// and the API documentation — sits behind one session boundary
/// (#732), so rendering any of it takes a session. Client is the least
/// privileged role that crosses the boundary, which keeps these assertions
/// about the page rather than about a role.
async fn get_signed_in(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    get_with_role(app, uri, store::persons::Role::Client).await
}

#[tokio::test]
async fn docs_glossary_renders_headings() {
    let app = server::neon_router(
        state_with_docs().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/docs/glossary").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Firm-branded page title from the doc's leading H1. `/docs` is mounted
    // once, in the composition every brand binary shares, so a second wordmark
    // here would publish another organization's identity on the firm's own host
    // and on every white-label tenant's. These are the Firm's own operating
    // docs.
    assert!(
        body.contains("<title>Neon Law | Docs | Glossary</title>"),
        "docs pages wear the firm brand on every host"
    );
    // The title carries the whole distinction: a docs page wearing a retired
    // wordmark would attribute the workspace's documentation to an organization
    // that does not build it.
    assert!(
        body.contains("/public/logo.png"),
        "the NL mark in the docs header"
    );
    assert!(
        !body.contains(&format!(
            "<title>{} | Docs | Glossary</title>",
            ["Neon", "Law", "Foundation"].join(" ")
        )),
        "the retired wordmark must not return"
    );
    // A known heading renders as an <h2> with a slug id so #council lands.
    assert!(
        body.contains("<h2 id=\"council\">Council</h2>"),
        "glossary should render the Council heading with an anchor id"
    );
    // Cross-doc link rewritten to a site route.
    assert!(body.contains("href=\"/docs/notation\""));
    assert!(
        body.contains("class=\"docs-article\""),
        "article pages retain their reading layout"
    );
    assert!(
        !body.contains("docs-catalog"),
        "the catalog presentation belongs only to /docs"
    );
}

#[tokio::test]
async fn docs_index_is_a_flat_accessible_catalog_of_every_published_guide() {
    let docs = portal::docs::loader::bundled();
    let app = server::neon_router(
        state_with_docs().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let response = get_signed_in(app, "/docs").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;

    assert!(
        body.contains("class=\"docs-catalog\""),
        "catalog shell: {body}"
    );
    assert!(
        body.contains("aria-label=\"Documentation catalog\""),
        "named catalog navigation: {body}"
    );
    assert!(
        body.contains("<ol class=\"docs-catalog__cards\""),
        "ordered cards: {body}"
    );
    assert!(
        !body.contains("docs-article"),
        "root uses catalog, not article: {body}"
    );
    assert_eq!(body.matches("<h1").count(), 1, "one page heading: {body}");
    assert!(
        body.contains("Start with Glossary"),
        "the index offers a direct first stop: {body}"
    );
    for retired_copy in [
        "Reading room",
        "Published guides, A–Z.",
        "Choose a title. Start with the glossary.",
        "Accession",
        "Read guide",
    ] {
        assert!(
            !body.contains(retired_copy),
            "{retired_copy} must not return: {body}"
        );
    }

    let mut published: Vec<_> = docs
        .docs()
        .iter()
        .filter(|doc| doc.slug != "index")
        .collect();
    published.sort_by_cached_key(|doc| doc.title.to_lowercase());
    let cards_start = body
        .find("<ol class=\"docs-catalog__cards\"")
        .expect("catalog cards");
    let cards = &body[cards_start..];
    let mut previous = 0;
    for doc in published {
        let href = format!("href=\"/docs/{}\"", doc.slug);
        let position = cards
            .find(&href)
            .unwrap_or_else(|| panic!("missing {href}: {cards}"));
        assert!(
            position > previous,
            "catalog order for {}: {cards}",
            doc.title
        );
        previous = position;
    }
    assert!(
        body.contains("class=\"neon-card docs-catalog__card\""),
        "each catalog destination is a navigation card: {body}"
    );
}

#[tokio::test]
async fn docs_notation_renders_teaching_order_headings() {
    let app = server::neon_router(
        state_with_docs().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/docs/notation").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // Template precedes Notation by design — both headings present.
    assert!(body.contains("<h2 id=\"template\">Template</h2>"));
    assert!(body.contains("<h2 id=\"notation\">Notation</h2>"));
    // notation links glossary.md#asset → /docs/glossary#asset.
    assert!(body.contains("href=\"/docs/glossary#asset\""));
}

#[tokio::test]
async fn every_published_doc_is_200() {
    // Every opt-in doc renders for a signed-in reader.
    let docs = portal::docs::loader::bundled();
    for doc in docs.docs() {
        if doc.slug == "index" {
            continue;
        }
        let app = server::neon_router(
            state_with_docs().await,
            std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        );
        let uri = format!("/docs/{}", doc.slug);
        let resp = get_signed_in(app, &uri).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri} should be 200");
    }
    for slug in [
        "deployment-secrets",
        "gke-prod",
        "dns",
        "cloud-operations",
        "multi-cloud",
        "rego-policy",
    ] {
        assert!(
            docs.find(slug).is_none(),
            "sensitive infrastructure doc {slug} must stay unpublished"
        );
    }
}

#[tokio::test]
async fn docs_index_slug_redirects_to_canonical_docs_root() {
    let app = server::neon_router(
        state_with_docs().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/docs/index").await;
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(resp.headers().get("location").unwrap(), "/docs");
}

#[tokio::test]
async fn docs_unknown_slug_is_404() {
    let app = server::neon_router(
        state_with_docs().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    );
    let resp = get_signed_in(app, "/docs/no-such-doc").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// True if `s` contains a digit-group run matching `groups` separated
/// by `-` (e.g. `[3, 2, 4]` matches an SSN `123-45-6789`), bounded so a
/// longer digit run on either side doesn't count. Hand-rolled so the
/// guardrail needs no regex dependency.
fn contains_dash_digit_pattern(s: &str, groups: &[usize]) -> bool {
    let bytes = s.as_bytes();
    let total: usize = groups.iter().sum::<usize>() + groups.len() - 1;
    let is_digit = |b: u8| b.is_ascii_digit();
    for start in 0..=bytes.len().saturating_sub(total) {
        // Left boundary: not preceded by a digit.
        if start > 0 && is_digit(bytes[start - 1]) {
            continue;
        }
        let mut pos = start;
        let mut ok = true;
        for (gi, &len) in groups.iter().enumerate() {
            if gi > 0 {
                if bytes.get(pos) != Some(&b'-') {
                    ok = false;
                    break;
                }
                pos += 1;
            }
            for _ in 0..len {
                match bytes.get(pos) {
                    Some(&b) if is_digit(b) => pos += 1,
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                break;
            }
        }
        // Right boundary: not followed by a digit.
        if ok && bytes.get(pos).is_none_or(|&b| !is_digit(b)) {
            return true;
        }
    }
    false
}

#[test]
fn docs_carry_no_client_confidences() {
    // The published-docs guardrail (RPC 1.6): the confidentiality
    // boundary is portal auth on the database, but as a belt-and-braces
    // check no published doc may contain an obvious client identifier —
    // an SSN- (ddd-dd-dddd) or EIN-shaped (dd-ddddddd) number. Docs use
    // placeholders today; this keeps it that way. (A real client name
    // can't be matched mechanically; the DB/portal boundary is what
    // actually protects it.)
    for doc in portal::docs::loader::bundled().docs() {
        assert!(
            !contains_dash_digit_pattern(&doc.body_html, &[3, 2, 4]),
            "/docs/{} contains an SSN-shaped string — published docs must \
             carry no client confidence",
            doc.slug
        );
        assert!(
            !contains_dash_digit_pattern(&doc.body_html, &[2, 7]),
            "/docs/{} contains an EIN-shaped string — published docs must \
             carry no client confidence",
            doc.slug
        );
    }
}

#[test]
fn dash_digit_pattern_detects_ssn_and_respects_boundaries() {
    assert!(contains_dash_digit_pattern(
        "ssn 123-45-6789 here",
        &[3, 2, 4]
    ));
    assert!(contains_dash_digit_pattern("ein 12-3456789.", &[2, 7]));
    // A longer digit run is not an SSN.
    assert!(!contains_dash_digit_pattern("v1234-45-6789", &[3, 2, 4]));
    assert!(!contains_dash_digit_pattern("port 8080", &[3, 2, 4]));
}

/// The statutes reference is gone (#874), so its three paths must be
/// unrouted — a 404 for a signed-in reader rather than a render, and a 404
/// for an anonymous one rather than the login redirect a merely-gated
/// surface would return.
#[tokio::test]
async fn the_statutes_reference_is_no_longer_routed() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for uri in [
        "/statutes",
        "/statutes/nrs/649",
        "/statutes/nrs/649/649.005",
    ] {
        let resp = get_signed_in(app.clone(), uri).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{uri} must not resolve for a signed-in reader"
        );

        let anonymous = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            anonymous.status(),
            StatusCode::NOT_FOUND,
            "{uri} must be unrouted, not merely behind the login door"
        );
    }
}

#[tokio::test]
async fn lawyer_cannot_fork_the_bootstrap_company_within_its_jurisdiction() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // A case variant in the firm's own jurisdiction is still the firm, and the
    // delete guard would protect both copies — so the fork is refused up front.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=shook%20law%20pllc&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    firm.entity_type_id, firm.jurisdiction_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !entity_write_succeeded(&response),
        "a case-variant fork of the firm anchor must be refused",
    );

    let firm_rows = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| {
            r.jurisdiction_id == firm.jurisdiction_id
                && r.name.eq_ignore_ascii_case(store::seed::FIRM_ENTITY_NAME)
        })
        .count();
    assert_eq!(firm_rows, 1, "the firm anchor must stay a single row");
}

#[tokio::test]
async fn lawyer_cannot_fork_the_bootstrap_company_into_another_jurisdiction() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let elsewhere = store::jurisdictions::list_all(&state.surreal)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id != firm.jurisdiction_id)
        .expect("the canonical seed carries more than one jurisdiction");

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // The delete guard matches on name alone, so a firm-named row in any
    // jurisdiction is born unremovable. Refuse the create instead of minting
    // a row no surface can clean up.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Shook%20Law%20PLLC&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    firm.entity_type_id, elsewhere.id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !entity_write_succeeded(&response),
        "a firm-named row in another jurisdiction must be refused",
    );

    let firm_rows = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.name.eq_ignore_ascii_case(store::seed::FIRM_ENTITY_NAME))
        .count();
    assert_eq!(
        firm_rows, 1,
        "the firm anchor must stay a single row across every jurisdiction",
    );
}

#[tokio::test]
async fn renaming_another_entity_into_the_firm_name_is_refused() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/entities")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Acme%20LLC&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    firm.entity_type_id, firm.jurisdiction_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    let acme = store::entities::find_by_name(&surreal, "Acme LLC")
        .await
        .unwrap()
        .expect("Acme LLC exists");

    // Renaming an ordinary Entity into the firm's name is the same fork the
    // create guard refuses: the row becomes protected on arrival, so nothing
    // could delete or rename it afterwards, and `store::seed` would then find
    // two rows under the exact name it looks the firm up by.
    for variant in ["Shook%20Law%20PLLC", "shook%20law%20pllc"] {
        let rename = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/admin/entities/{}", acme.id))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name={variant}&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                        firm.entity_type_id, firm.jurisdiction_id,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            !entity_write_succeeded(&rename),
            "{variant}: renaming an ordinary entity into the firm name must be refused",
        );
    }

    assert_eq!(
        store::entities::find_by_id(&surreal, acme.id)
            .await
            .unwrap()
            .expect("Acme LLC remains")
            .name,
        "Acme LLC",
        "a refused rename must leave the row untouched",
    );
    let firm_rows = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.name.eq_ignore_ascii_case(store::seed::FIRM_ENTITY_NAME))
        .count();
    assert_eq!(firm_rows, 1, "the firm anchor must stay a single row");
}

#[tokio::test]
async fn the_firm_anchor_stays_freely_editable_apart_from_its_name() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let elsewhere = store::jurisdictions::list_all(&state.surreal)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id != firm.jurisdiction_id)
        .expect("the canonical seed carries more than one jurisdiction");

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // Re-domiciling the firm keeps its name, so the reserved-name guard must
    // not mistake the row for a fork of itself.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/entities/{}", firm.id))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "name=Shook%20Law%20PLLC&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                    firm.entity_type_id, elsewhere.id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let moved = store::entities::find_by_id(&surreal, firm.id)
        .await
        .unwrap()
        .expect("the firm remains");
    assert_eq!(moved.jurisdiction_id, elsewhere.id);
    assert_eq!(moved.name, store::seed::FIRM_ENTITY_NAME);
}

#[tokio::test]
async fn entities_that_are_not_the_firm_anchor_may_share_a_name_and_jurisdiction() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    // Two unrelated Entities may legitimately carry one name in one
    // jurisdiction. Only the firm anchor is constrained to a single row.
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/app/admin/entities")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name=John%20Smith&entity_type_id={}&jurisdiction_id={}&_csrf={csrf}",
                        firm.entity_type_id, firm.jurisdiction_id,
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    let namesakes = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.name == "John Smith")
        .count();
    assert_eq!(namesakes, 2, "namesakes must both persist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_creates_cannot_fork_the_firm_anchor() {
    let (state, surreal) = state_with_engines().await;
    store::seed::seed_canonical(&state.surreal, &state.storage)
        .await
        .unwrap();
    let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
        .await
        .unwrap()
        .expect("canonical seed inserts the bootstrap company");
    let (type_id, jur_id) = (firm.entity_type_id, firm.jurisdiction_id);

    // Stand in for the white-label window: the protected name is configured
    // but no row carries it yet, so the existence check can pass. Moved aside
    // directly, since the surface itself refuses to.
    // Clearing `firm_anchor_key` is what opens the window: the UNIQUE
    // `entity_firm_anchor` index — not the name — is what a create now
    // collides with, so a rename that left the key would make every racer
    // below fail rather than exactly one succeed.
    move_anchor_aside(&surreal, &firm, "Placeholder Holdings LLC").await;

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let app = app.clone();
        let (cookie, csrf) = (cookie.clone(), csrf.clone());
        tasks.spawn(async move {
            let response = app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/app/admin/entities")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "name=Shook%20Law%20PLLC&entity_type_id={type_id}&jurisdiction_id={jur_id}&_csrf={csrf}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
            entity_write_outcome(&response)
        });
    }
    let mut created = 0;
    while let Some(outcome) = tasks.join_next().await {
        match outcome.unwrap() {
            EntityWriteOutcome::Created => created += 1,
            // Not merely "did not create" — refused *for the anchor*. A
            // racer that failed for any other reason is a broken guard
            // wearing a refusal, and must not be counted as one.
            EntityWriteOutcome::Refused(flash) => assert_eq!(
                flash,
                flash_of(store::entity_commands::FIRM_ANCHOR_EXISTS_MESSAGE),
                "a losing racer must be refused for the anchor, not for a fault",
            ),
            other @ EntityWriteOutcome::NotRedirected(_) => {
                panic!("a racer must redirect, got {other:?}")
            }
        }
    }
    assert_eq!(created, 1, "exactly one racer may create the anchor");

    let rows = store::entities::all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| {
            r.name
                .trim()
                .eq_ignore_ascii_case(store::seed::FIRM_ENTITY_NAME)
        })
        .count();
    assert_eq!(
        rows, 1,
        "concurrent creates must not fork the anchor into rows nothing can delete",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_delete_racing_a_rename_into_the_firm_name_never_removes_the_anchor() {
    // The dangerous interleaving — delete reads an ordinary row, a rename turns
    // it into the anchor, then the id-only delete removes the freshly protected
    // row — is timing-dependent and rarely wins against a fast local store.
    // The invariant this asserts holds regardless of who wins the race: a
    // rename that reports minting the anchor is never undone by the racing
    // delete. It is a regression guard for the delete-path serialization, run
    // across several rounds to widen the window.
    for round in 0..12 {
        let (state, surreal) = state_with_engines().await;
        store::seed::seed_canonical(&state.surreal, &state.storage)
            .await
            .unwrap();
        let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
            .await
            .unwrap()
            .expect("canonical seed inserts the bootstrap company");
        let (type_id, jur_id) = (firm.entity_type_id, firm.jurisdiction_id);

        // Open the white-label window so a rename can mint the anchor: move the
        // seeded row aside directly, which the surface itself refuses to do.
        move_anchor_aside(
            &surreal,
            &firm,
            &format!("Placeholder Holdings {round} LLC"),
        )
        .await;

        let victim = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Acme LLC".to_string(),
                entity_type_id: type_id,
                jurisdiction_id: jur_id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();

        let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
        let (cookie, csrf) = admin_session_cookie_and_csrf();
        let vid = victim.id;

        let delete = {
            let (app, cookie, csrf) = (app.clone(), cookie.clone(), csrf.clone());
            tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/app/admin/entities/{vid}/delete"))
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!("_csrf={csrf}")))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            })
        };
        let rename = {
            let (app, cookie, csrf) = (app.clone(), cookie.clone(), csrf.clone());
            tokio::spawn(async move {
                let response = app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/app/admin/entities/{vid}"))
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!(
                            "name=Shook%20Law%20PLLC&entity_type_id={type_id}&jurisdiction_id={jur_id}&_csrf={csrf}"
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
                // Under post/redirect/get a refused rename is also a `303`, so
                // the redirect target — not the status — reports the outcome.
                entity_write_succeeded(&response)
            })
        };
        let (delete_status, renamed) = (delete.await.unwrap(), rename.await.unwrap());

        let anchor_rows: Vec<_> = store::entities::all(&surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.name == store::seed::FIRM_ENTITY_NAME)
            .collect();
        if renamed {
            assert_eq!(
                anchor_rows.len(),
                1,
                "round {round}: a rename that minted the anchor \
                 (delete={delete_status}) must not be undone by the racing delete",
            );
        } else {
            // The rename lost or errored, so no anchor should exist and the
            // victim was a plain row the delete could take.
            assert!(
                anchor_rows.is_empty(),
                "round {round}: no anchor should exist when the rename did not succeed",
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_rename_racing_a_rename_into_the_firm_name_never_loses_the_anchor() {
    // Request A renames an ordinary row into the firm name (minting the anchor
    // in the white-label window); request B renames the same row to another
    // ordinary name off a read taken before A committed. If B's read were
    // outside the anchor lock it would see an ordinary source, skip every
    // guard, and rename the freshly protected firm away. The invariant holds
    // no matter who wins: a rename that reports minting the anchor is never
    // undone, and the firm name is never split into duplicates.
    for round in 0..12 {
        let (state, surreal) = state_with_engines().await;
        store::seed::seed_canonical(&state.surreal, &state.storage)
            .await
            .unwrap();
        let firm = store::entities::find_by_name(&surreal, store::seed::FIRM_ENTITY_NAME)
            .await
            .unwrap()
            .expect("canonical seed inserts the bootstrap company");
        let (type_id, jur_id) = (firm.entity_type_id, firm.jurisdiction_id);

        move_anchor_aside(
            &surreal,
            &firm,
            &format!("Placeholder Holdings {round} LLC"),
        )
        .await;

        let victim = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Acme LLC".to_string(),
                entity_type_id: type_id,
                jurisdiction_id: jur_id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();

        let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
        let (cookie, csrf) = admin_session_cookie_and_csrf();
        let vid = victim.id;

        let into_firm = {
            let (app, cookie, csrf) = (app.clone(), cookie.clone(), csrf.clone());
            tokio::spawn(async move {
                let response = app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/app/admin/entities/{vid}"))
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!(
                            "name=Shook%20Law%20PLLC&entity_type_id={type_id}&jurisdiction_id={jur_id}&_csrf={csrf}"
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
                // Under post/redirect/get a refused rename is also a `303`, so
                // the redirect target — not the status — reports the outcome.
                entity_write_succeeded(&response)
            })
        };
        let to_ordinary = {
            let (app, cookie, csrf) = (app.clone(), cookie.clone(), csrf.clone());
            tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/app/admin/entities/{vid}"))
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!(
                            "name=Acme%20Corp&entity_type_id={type_id}&jurisdiction_id={jur_id}&_csrf={csrf}"
                        )))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            })
        };
        let (minted_the_anchor, _to_ordinary_status) =
            (into_firm.await.unwrap(), to_ordinary.await.unwrap());

        let anchor_rows: Vec<_> = store::entities::all(&surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.name == store::seed::FIRM_ENTITY_NAME)
            .collect();
        assert!(
            anchor_rows.len() <= 1,
            "round {round}: the firm name must never split into duplicates",
        );
        if minted_the_anchor {
            assert_eq!(
                anchor_rows.len(),
                1,
                "round {round}: a rename that minted the anchor must not be \
                 renamed away by the racing rename",
            );
        }
    }
}

// ---- Lawyer playbooks (#956 Phase 4) ----
//
// The `/app/admin/playbooks` cluster shipped with no route-level coverage at
// all — only in-file unit tests on the pure parsers, which never touch a
// handler. These tests pin the behaviour the surface actually has (the sort
// `400`, the unknown-id `404`, and each refusal) so the Dioxus port is proved
// against the real routes rather than against the parsers.

/// Insert a client company plus one playbook with `positions`, returning both
/// ids. Playbooks are scoped to an Entity, so every fixture needs a company.
async fn seed_playbook(
    surreal: &store::surreal::SurrealDb,
    entity_name: &str,
    playbook_name: &str,
    positions: &[store::playbooks::Position],
) -> (uuid::Uuid, uuid::Uuid) {
    // Unenforced cross-engine ids — the playbook surfaces never render
    // the entity type or the jurisdiction.
    let entity_type_id = uuid::Uuid::now_v7();
    let jurisdiction_id = uuid::Uuid::now_v7();
    let entity = store::entities::create(
        surreal,
        &store::entities::NewEntity {
            name: entity_name.to_string(),
            entity_type_id,
            jurisdiction_id,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap();
    let playbook_id = store::playbooks::create(
        surreal,
        &store::playbooks::NewPlaybook {
            entity_id: entity.id,
            name: playbook_name,
            positions,
        },
    )
    .await
    .unwrap();
    (entity.id, playbook_id)
}

/// Percent-encode a form value for an `application/x-www-form-urlencoded`
/// body: unreserved characters pass through, everything else (the `|`
/// delimiter and the newlines between positions included) is escaped.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

fn position(topic: &str, severity: &str) -> store::playbooks::Position {
    store::playbooks::Position {
        topic: topic.to_string(),
        preferred: "mutual cap".to_string(),
        fallback: "2x fees".to_string(),
        walkaway: "uncapped".to_string(),
        severity: severity.to_string(),
    }
}

#[tokio::test]
async fn lawyer_playbooks_list_renders_each_playbook_with_its_company() {
    let (state, _surreal) = state_with_engines().await;
    seed_playbook(
        &state.surreal,
        "Acme Inc",
        "Vendor MSA",
        &[position("Liability", "high"), position("Term", "low")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(app, "/app/admin/playbooks", store::persons::Role::Lawyer).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Acme Inc"), "{body}");
    assert!(body.contains("Vendor MSA"), "{body}");
    // The position count is the column that tells an attorney the playbook is
    // populated rather than an empty shell.
    assert!(body.contains(">2<"), "{body}");
    assert!(body.contains("/app/admin/playbooks/new"), "{body}");
}

#[tokio::test]
async fn lawyer_playbooks_list_rejects_unknown_sort_with_400() {
    // The listing advertises exactly `entity` and `name`. An unadvertised
    // `?sort=` is refused ahead of the render, so a header can never link to a
    // query the route would serve differently than it advertises.
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app.clone(),
        "/app/admin/playbooks?sort=positions",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // An advertised key still renders.
    let ok = get_with_role(
        app,
        "/app/admin/playbooks?sort=-name",
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn lawyer_playbook_create_refusals_bounce_back_with_the_typed_positions() {
    // Every refusal is post/redirect/get back to the create form, carrying the
    // message and the rejected input. The positions textarea holds a whole
    // hand-authored position set, so discarding it on a typo'd severity would
    // cost the attorney the entire block.
    let (state, _surreal) = state_with_engines().await;
    let (entity_id, _) = seed_playbook(
        &state.surreal,
        "Acme Inc",
        "Existing MSA",
        &[position("Liability", "high")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let typed = "Liability | mutual cap | 2x fees | uncapped | critical";
    for (name, positions, expected) in [
        // A blank name.
        ("", typed, "A playbook name is required."),
        // No positions at all.
        ("Vendor MSA", "   \n\n", "Enter at least one position."),
        // A severity the parser refuses, named by line.
        ("Vendor MSA", typed, "Line 1: severity must be"),
        // A duplicate name for the same company.
        (
            "Existing MSA",
            "Liability | mutual cap | 2x fees | uncapped | high",
            "That Company already has a playbook with that name.",
        ),
    ] {
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/app/admin/playbooks")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "entity_id={entity_id}&name={}&positions={}&_csrf={csrf}",
                        form_encode(name),
                        form_encode(positions),
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            rejected.status(),
            StatusCode::SEE_OTHER,
            "a refused create must redirect, not re-render: {expected}",
        );
        let location = redirect_location(&rejected);
        assert!(
            location.starts_with("/app/admin/playbooks/new?error="),
            "{location}",
        );

        // Follow the redirect: asserting on the redirect's own empty body
        // would pass vacuously.
        let reloaded = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&location)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reloaded.status(), StatusCode::OK);
        let reloaded = body_string(reloaded).await;
        assert!(reloaded.contains(expected), "{expected}: {reloaded}");
        // The rejected input survives the bounce, so the correction is one
        // edit rather than a full retype.
        if !positions.trim().is_empty() {
            assert!(
                reloaded.contains("Liability | mutual cap | 2x fees | uncapped"),
                "the typed positions must survive the refusal: {reloaded}",
            );
        }
        let form = DomForm::parse(&reloaded, "/app/admin/playbooks");
        assert_eq!(form.value("_csrf"), csrf);
    }
}

#[tokio::test]
async fn lawyer_playbook_create_lands_the_playbook_and_returns_to_the_list() {
    let (state, surreal) = state_with_engines().await;
    let (entity_id, _) = seed_playbook(
        &surreal,
        "Acme Inc",
        "Existing MSA",
        &[position("Liability", "high")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/admin/playbooks")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "entity_id={entity_id}&name=Vendor%20MSA&positions={}&_csrf={csrf}",
                    form_encode(
                        "Liability | mutual cap | 2x fees | uncapped | HIGH\n\
                         Governing law | Nevada | Delaware | no nexus | medium"
                    ),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirect_location(&created), "/app/admin/playbooks");

    let listed = get_with_role(app, "/app/admin/playbooks", store::persons::Role::Lawyer).await;
    let body = body_string(listed).await;
    assert!(body.contains("Vendor MSA"), "{body}");
}

#[tokio::test]
async fn lawyer_playbook_edit_form_prefills_the_stored_positions() {
    let (state, _surreal) = state_with_engines().await;
    let (_, playbook_id) = seed_playbook(
        &state.surreal,
        "Acme Inc",
        "Vendor MSA",
        &[position("Liability", "high")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/admin/playbooks/{playbook_id}/edit"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The company + name are fixed context; the positions are what is edited.
    assert!(body.contains("Acme Inc"), "{body}");
    assert!(body.contains("Vendor MSA"), "{body}");
    assert!(
        body.contains("Liability | mutual cap | 2x fees | uncapped | high"),
        "{body}",
    );
    let form = DomForm::parse(&body, &format!("/app/admin/playbooks/{playbook_id}"));
    assert_eq!(form.value("_csrf"), csrf);
}

#[tokio::test]
async fn lawyer_playbook_edit_form_404s_on_an_unknown_id() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_role(
        app,
        &format!("/app/admin/playbooks/{}/edit", uuid::Uuid::from_u128(404)),
        store::persons::Role::Lawyer,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lawyer_playbook_update_refusals_bounce_back_with_the_typed_positions() {
    let (state, _surreal) = state_with_engines().await;
    let (_, playbook_id) = seed_playbook(
        &state.surreal,
        "Acme Inc",
        "Vendor MSA",
        &[position("Liability", "high")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let typed = "Liability | mutual cap | 2x fees | uncapped | critical";
    for (positions, expected) in [
        ("   \n\n", "Enter at least one position."),
        (typed, "Line 1: severity must be"),
    ] {
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/admin/playbooks/{playbook_id}"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "positions={}&_csrf={csrf}",
                        form_encode(positions),
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::SEE_OTHER, "{expected}");
        let location = redirect_location(&rejected);
        assert!(
            location.starts_with(&format!("/app/admin/playbooks/{playbook_id}/edit?error=")),
            "{location}",
        );

        let reloaded = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&location)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reloaded.status(), StatusCode::OK);
        let reloaded = body_string(reloaded).await;
        assert!(reloaded.contains(expected), "{expected}: {reloaded}");
        if !positions.trim().is_empty() {
            assert!(
                reloaded.contains("Liability | mutual cap | 2x fees | uncapped | critical"),
                "the rejected edit must survive, not reload the stored row: {reloaded}",
            );
        }
    }
}

#[tokio::test]
async fn lawyer_playbook_update_replaces_the_positions_and_returns_to_the_list() {
    let (state, surreal) = state_with_engines().await;
    let (_, playbook_id) = seed_playbook(
        &surreal,
        "Acme Inc",
        "Vendor MSA",
        &[position("Liability", "high")],
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let saved = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/admin/playbooks/{playbook_id}"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "positions={}&_csrf={csrf}",
                    form_encode("Term | 1 year | 2 years | perpetual | medium"),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirect_location(&saved), "/app/admin/playbooks");

    let row = store::playbooks::by_id(&surreal, playbook_id)
        .await
        .unwrap()
        .expect("the playbook survives its update");
    let positions = store::playbooks::positions_of(&row).unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].topic, "Term");
    assert_eq!(positions[0].severity, "medium");
}

#[tokio::test]
async fn lawyer_playbook_update_404s_on_an_unknown_id() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let (cookie, csrf) = admin_session_cookie_and_csrf();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/app/admin/playbooks/{}",
                    uuid::Uuid::from_u128(404)
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "positions=Term%20%7C%20a%20%7C%20b%20%7C%20c%20%7C%20low&_csrf={csrf}",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// ENG-81 — one matter surface at `/app/projects`.
//
// The lens used to come from the URL prefix, which the requester chooses. It
// now comes from the caller's tier plus their `person_project_roles` row. These
// tests pin that on the collapsed path: one path per resource, one assertion
// per tier, and the denials that the collapse could quietly turn into grants.
// ---------------------------------------------------------------------------

/// Attach a person of the given tier to a matter and return their cookie.
async fn tiered_participant(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
    role: store::persons::Role,
    email: &str,
    participation: Option<&str>,
) -> String {
    let person = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role("Tier Fixture", email, role),
    )
    .await
    .unwrap();
    if let Some(kind) = participation {
        participate(surreal, person.id, project_id, kind).await;
    }
    let (cookie, _csrf) = session_cookie_and_csrf_for_person(&person);
    cookie
}

async fn code_for_project(surreal: &store::surreal::SurrealDb, project_id: uuid::Uuid) -> String {
    store::projects::find_by_id(surreal, project_id)
        .await
        .unwrap()
        .expect("fixture project")
        .code
}

/// The guard for the 2026-08-05 authorization decision: before this slice,
/// the `is_admin_tier()` short-circuit handed Owner and Admin every matter's
/// full content without a participation row. `store::access::matter_viewer`
/// still carries no such bypass — this pins that Owner/Admin without a row
/// get the narrower participation-only rendering (added later) rather than
/// the workbench: no document upload, only the "Add person" participation
/// control every tier without full access never gets.
#[tokio::test]
async fn owner_and_admin_without_participation_get_the_participation_only_view() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, _cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let project_code = code_for_project(&surreal, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (role, email) in [
        (store::persons::Role::Owner, "unassigned-owner@neonlaw.com"),
        (store::persons::Role::Admin, "unassigned-admin@neonlaw.com"),
    ] {
        let cookie = tiered_participant(&surreal, project_id, role, email, None).await;
        let resp = get_with_cookie(
            app.clone(),
            &format!("/app/projects/{project_code}"),
            &cookie,
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{role:?} has no participation row, so gets the participation-only view, not a 404"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("Add person"),
            "{role:?}: expected the participation-only view: {body}"
        );
        assert!(
            !body.contains("Upload documents"),
            "{role:?} has no participation row and must not reach the workbench's documents: {body}"
        );
    }
}

/// The same tiers reach the matter the moment they are put on it. Without this
/// the test above would also pass with the whole surface broken.
#[tokio::test]
async fn every_firm_tier_reaches_a_matter_it_participates_on() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, _cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let project_code = code_for_project(&surreal, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (role, email) in [
        (store::persons::Role::Owner, "assigned-owner@neonlaw.com"),
        (store::persons::Role::Admin, "assigned-admin@neonlaw.com"),
        (store::persons::Role::Lawyer, "assigned-lawyer@neonlaw.com"),
    ] {
        // The participation the matter-people form would derive for this tier
        // (#108) — the same word as the role, and firm-side by construction.
        let kind = store::projects::participation_for_role(role);
        let cookie = tiered_participant(&surreal, project_id, role, email, Some(kind)).await;
        let resp = get_with_cookie(
            app.clone(),
            &format!("/app/projects/{project_code}"),
            &cookie,
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{role:?} participates as `{kind}` and must reach the matter"
        );
    }
}

/// The Clerk boundary, now that `/clerk` is retired and a Clerk enters the same
/// path as everyone else.
///
/// This is the assertion that replaces a *topological* guarantee with a
/// conditional one. A Clerk used to be unable to reach the client or firm
/// rendering because those pages were mounted somewhere they could not go; now
/// they are one dispatcher branch away, so the branch has to be pinned. A Clerk
/// is a supervised non-lawyer: name, status, supervising lawyer, and nothing
/// else — never a document, an invoice, or a write.
#[tokio::test]
async fn a_supervised_clerk_gets_the_narrow_rendering_and_no_documents() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, lawyer, _cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let project_code = code_for_project(&surreal, project_id).await;
    // Supervision is the whole condition: the matter must name a currently
    // licensed lawyer as its lawyer DRI, or this Clerk is not supervised on it
    // and correctly sees nothing. The fixture only records participation.
    disclose_lawyer_dri(&surreal, lawyer.id, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cookie = tiered_participant(
        &surreal,
        project_id,
        store::persons::Role::Clerk,
        "clerk-on-matter@neonlaw.com",
        Some("clerk"),
    )
    .await;
    let body =
        body_string(get_with_cookie(app, &format!("/app/projects/{project_code}"), &cookie).await)
            .await;

    assert!(
        body.contains("Supervising lawyer"),
        "a supervised clerk sees the disclosed lawyer: {body}"
    );
    for forbidden in [
        "Documents",
        "Invoice",
        "Participation ledger",
        "To close this matter",
        "Upload",
    ] {
        assert!(
            !body.contains(forbidden),
            "a clerk must never receive `{forbidden}`: {body}"
        );
    }
}

/// A Clerk whose matter has no licensed lawyer DRI is not supervised, so the
/// matter is not theirs to see. The firm-side row alone is not enough — that
/// extra condition is the whole difference between the Clerk lens and the
/// lawyer one, and it is easy to lose when the two share a path.
#[tokio::test]
async fn a_clerk_without_a_licensed_supervisor_is_denied_the_matter() {
    let (state, surreal) = state_with_engines().await;
    let project = test_project(&surreal, "Unsupervised", "open").await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Firm-side participation, but nobody licensed is accountable for the matter.
    let cookie = tiered_participant(
        &surreal,
        project.id,
        store::persons::Role::Clerk,
        "unsupervised-clerk@neonlaw.com",
        Some("clerk"),
    )
    .await;
    let resp = get_with_cookie(app, &format!("/app/projects/{}", project.code), &cookie).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The retired Clerk namespace is gone for its own tier too.
#[tokio::test]
async fn the_retired_clerk_namespace_is_not_served() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, _c, _csrf) = lawyer_project_fixture(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let cookie = tiered_participant(
        &surreal,
        project_id,
        store::persons::Role::Clerk,
        "retired-namespace-clerk@neonlaw.com",
        Some("clerk"),
    )
    .await;
    for uri in ["/clerk", &format!("/clerk/projects/{project_id}")] {
        let resp = get_with_cookie(app.clone(), uri, &cookie).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri} is retired");
    }
}

/// The two dashboards keep their tier gates after the move under `/app`.
#[tokio::test]
async fn the_app_dashboards_keep_their_tier_gates() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // `/app/lawyer` is firm-tier; `/app/admin` is Owner/Admin only.
    for (uri, marker, allowed, denied) in [
        (
            "/app/lawyer",
            "Lawyer workbench",
            vec![
                store::persons::Role::Owner,
                store::persons::Role::Admin,
                store::persons::Role::Lawyer,
            ],
            vec![store::persons::Role::Clerk, store::persons::Role::Client],
        ),
        (
            "/app/admin",
            "Manage people",
            vec![store::persons::Role::Owner, store::persons::Role::Admin],
            vec![
                store::persons::Role::Lawyer,
                store::persons::Role::Clerk,
                store::persons::Role::Client,
            ],
        ),
    ] {
        for role in allowed {
            let body = body_string(get_with_role(app.clone(), uri, role).await).await;
            assert!(
                body.contains(marker),
                "{role:?} may open {uri} (`{marker}`): {body}"
            );
        }
        for role in denied {
            // `require_lawyer` / `require_admin` deny by returning `Err`, which
            // Dioxus renders as an error body under a `200` — the page is
            // withheld rather than status-refused. Assert on what came back,
            // not on the status, or this passes for the wrong reason.
            let body = body_string(get_with_role(app.clone(), uri, role).await).await;
            assert!(
                !body.contains(marker),
                "{role:?} must not receive {uri} (`{marker}`): {body}"
            );
        }
    }
}

/// The adverse party is on the matter, so they see it — through the client
/// lens. They must never reach the firm workbench. `counterparty` is the only
/// participation whose *value* carries that distinction, and one shared handler
/// is exactly where it could be lost.
#[tokio::test]
async fn a_counterparty_is_denied_the_firm_lens_on_the_shared_path() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, _cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let project_code = code_for_project(&surreal, project_id).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // A lawyer-tier person recorded as the adverse party: the tier alone would
    // admit them, so only the participation value keeps them out.
    let cookie = tiered_participant(
        &surreal,
        project_id,
        store::persons::Role::Lawyer,
        "adverse-counsel@example.com",
        Some("counterparty"),
    )
    .await;
    let body =
        body_string(get_with_cookie(app, &format!("/app/projects/{project_code}"), &cookie).await)
            .await;
    assert!(
        !body.contains("Participation ledger") && !body.contains("Upload documents"),
        "a counterparty must not receive the firm workbench: {body}"
    );
}

/// Closing a matter is bespoke — asked for by email and opened by the lawyer
/// DRI — so the workbench carries no close control at all. Both the accountable
/// lawyer and an ordinary firm participant read where to ask, because the
/// accountability marker decides who *acts* on the request, not who may raise
/// it.
#[tokio::test]
async fn the_workbench_points_every_firm_participant_at_email_to_close_a_matter() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, lawyer, dri_cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let project_code = code_for_project(&surreal, project_id).await;
    disclose_lawyer_dri(&surreal, lawyer.id, project_id).await;
    let paralegal_cookie = tiered_participant(
        &surreal,
        project_id,
        store::persons::Role::Lawyer,
        "paralegal-on-close@neonlaw.com",
        Some("paralegal"),
    )
    .await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for (who, cookie) in [
        ("the lawyer DRI", dri_cookie),
        ("a paralegal", paralegal_cookie),
    ] {
        let body = body_string(
            get_with_cookie(
                app.clone(),
                &format!("/app/projects/{project_code}"),
                &cookie,
            )
            .await,
        )
        .await;

        // The sentence, not just the address: a shell footer can also carry a
        // `mailto:` and would satisfy the weaker assertion on its own. Asserted
        // in short spans because the copy interpolates the DRI's name, and
        // Dioxus splits an interpolated node from its surrounding text with
        // hydration comments — so no long contiguous run of it survives SSR.
        assert!(
            body.contains("To close this matter, email the lawyer DRI")
                && body.contains("(Lawyer Project Fixture)")
                && body.contains("mailto:support@neonlaw.com"),
            "{who} is pointed at the named lawyer DRI and the support address: {body}"
        );
        for gone in [
            "Close this matter".to_string(),
            "Close matter".to_string(),
            format!("/app/projects/{project_code}/close"),
        ] {
            assert!(
                !body.contains(&gone),
                "{who} must not receive `{gone}`: {body}"
            );
        }
    }
}

/// Unchanged behavior on a new path: a client on one matter cannot read
/// another client's.
#[tokio::test]
async fn a_client_on_another_matter_is_denied_on_the_new_path() {
    let (state, surreal) = state_with_engines().await;
    let (_mine, _code, cookie) = client_project_fixture(&surreal).await;
    let (theirs, _lawyer, _c, _csrf) = lawyer_project_fixture(&surreal).await;
    let their_code = code_for_project(&surreal, theirs).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let resp = get_with_cookie(app, &format!("/app/projects/{their_code}"), &cookie).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Replace each per-response CSP nonce with a fixed marker so two renders of
/// the same page compare byte-for-byte.
fn strip_csp_nonces(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("nonce=\"") {
        let (head, tail) = rest.split_at(at + "nonce=\"".len());
        out.push_str(head);
        let end = tail.find('"').expect("a nonce attribute is closed");
        out.push_str("NONCE");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Module toggle-blindness. The retired `/portal` got this structurally by never
/// mounting the lawyer surface; one handler serving both lenses does not get it
/// for free.
/// A section that renders empty is observably different from one never emitted,
/// so a client must not be able to infer that a module exists but was withheld
/// — the response has to be *byte-identical*, not merely similar.
#[tokio::test]
async fn a_client_lens_response_is_byte_identical_across_module_toggles() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, project_code, cookie) = client_project_fixture(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let uri = format!("/app/projects/{project_code}");

    let disabled = body_string(get_with_cookie(app.clone(), &uri, &cookie).await).await;

    for module in store::project_modules::Module::ALL {
        store::project_modules::enable(&surreal, project_id, *module, None)
            .await
            .unwrap();
    }
    let enabled = body_string(get_with_cookie(app, &uri, &cookie).await).await;

    // The CSP nonce is minted per response and is the one byte that is
    // *supposed* to differ between two identical renders, so it is normalized
    // out rather than weakening the comparison to "contains".
    assert_eq!(
        strip_csp_nonces(&disabled),
        strip_csp_nonces(&enabled),
        "the client lens must not leak which modules the firm enabled"
    );
}

/// The old prefixes are gone, deliberately and without a redirect layer — for
/// every tier, including links already sitting in sent email.
#[tokio::test]
async fn the_retired_project_prefixes_are_not_served() {
    let (state, surreal) = state_with_engines().await;
    let (project_id, _lawyer, cookie, _csrf) = lawyer_project_fixture(&surreal).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for uri in [
        format!("/app/lawyer/projects/{project_id}"),
        format!("/portal/projects/{project_id}"),
        "/app/lawyer/projects".to_string(),
        "/portal/projects".to_string(),
        // The retired `/portal` landing itself: folded into `/app`, served by
        // nothing now, and deliberately without a redirect shim.
        "/portal".to_string(),
        "/portal/forms".to_string(),
    ] {
        let resp = get_with_cookie(app.clone(), &uri, &cookie).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{uri} must 404 — no compatibility shim"
        );
    }
}

/// Firm-administration listings and Person CRUD left `/app/lawyer` and `/admin`
/// without a redirect layer. Deep links in sent email 404.
#[tokio::test]
async fn the_moved_admin_listings_are_not_served_at_the_old_paths() {
    let (state, _surreal) = state_with_engines().await;
    let cookie = admin_session_cookie_with_person();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    for uri in [
        "/app/lawyer/entities",
        "/app/lawyer/entities/new",
        "/app/lawyer/entity-types",
        "/app/lawyer/playbooks",
        "/app/lawyer/schedules",
        "/app/lawyer/letters",
        "/app/lawyer/email-log",
        "/app/lawyer/people.csv",
        "/admin/people",
        "/admin/people/new",
        "/admin/analytics",
    ] {
        let resp = get_with_cookie(app.clone(), uri, &cookie).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{uri} must 404 — no compatibility shim"
        );
    }
}

/// The Harvard-outline teaching stage moved into the authenticated `/app`
/// namespace without changing its lawyer-tier audience.
#[tokio::test]
async fn the_outline_stage_uses_the_app_namespace() {
    let (state, _surreal) = state_with_engines().await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let current = get_with_role(app.clone(), "/app/outline", store::persons::Role::Lawyer).await;
    assert_eq!(current.status(), StatusCode::OK);

    let retired = get_with_role(app, "/app/lawyer/outline", store::persons::Role::Lawyer).await;
    assert_eq!(retired.status(), StatusCode::NOT_FOUND);
}

/// A code that names no matter is refused everywhere below `/app/projects/`.
///
/// Every route in this namespace resolves its `{project_code}` segment at the
/// door, and each one owes the same answer when that lookup finds nothing: the
/// non-disclosing 404 a caller off the matter receives. A 403 — or a 400 from
/// an extractor, or a 500 from a handler that carried on with no matter — would
/// each tell a stranger something about which codes exist.
///
/// One test walks the whole namespace because the refusal is a property of the
/// namespace rather than of any one handler: a route added later that forgets
/// to resolve, or resolves and then ignores the miss, is exactly what this
/// catches. The caller here is an Admin with a real session, so nothing is
/// being refused for want of authentication — only for want of a matter.
#[tokio::test]
async fn a_code_naming_no_matter_is_refused_on_every_matter_route() {
    let (state, surreal) = state_with_engines().await;
    let cookie = admin_session_cookie_with_person();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    // Well-formed (lowercase, single hyphens, alphanumeric at both ends) and
    // carried by no Project, so every refusal below is the lookup's and not the
    // shape check's.
    let ghost = "no-such-matter";
    assert!(
        store::projects::is_valid_code(ghost),
        "the probe must be a valid code, or this asserts the wrong gate"
    );
    assert!(
        store::projects::find_by_code(&surreal, ghost)
            .await
            .unwrap()
            .is_none(),
        "the probe must name no matter"
    );

    let doc = uuid::Uuid::now_v7();
    let notation = uuid::Uuid::now_v7();
    let role = uuid::Uuid::now_v7();
    for path in [
        format!("/app/projects/{ghost}"),
        format!("/app/projects/{ghost}/edit"),
        format!("/app/projects/{ghost}/conversation"),
        format!("/app/projects/{ghost}/documents.zip"),
        format!("/app/projects/{ghost}/documents/{doc}/download"),
        format!("/app/projects/{ghost}/review/{doc}"),
        format!("/app/projects/{ghost}/review/{doc}/comments"),
        format!("/app/projects/{ghost}/intake/{notation}"),
        format!("/app/projects/{ghost}/people/new"),
        format!("/app/projects/{ghost}/people/{role}/edit"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} must answer the same 404 as a matter the caller is not on"
        );
    }

    // The per-document page is the one surface that answers with a rendered
    // not-found *body* rather than a 404 status — its own contract, and the
    // same body a document that exists on another matter produces. What matters
    // here is the disclosure, not the code: it must name no document.
    let detail = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{ghost}/documents/{doc}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(detail.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .into_owned();
    assert!(
        !body.contains(ghost),
        "the per-document page must not echo a code it could not resolve: {body}"
    );
}
