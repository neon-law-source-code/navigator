//! Cucumber runner for `features/deploy_the_navigator_walkthrough.feature`.
//!
//! Grounds the *renderable* claims of the "Operating Neon Law Navigator"
//! workshop (`server/content/workshops/navigator/DEPLOY.md`) in the running
//! web app: it is registered on the Catalog surface, renders under
//! the site brand, opens with an Agenda, splits into stepped
//! content, and shows the reader the real
//! `cargo run -p cli -- ops gcp setup` command. The pipeline-grounding half
//! — that the services, buckets, and command the prose names match what
//! `navigator ops gcp setup` actually calls — lives in `cli/src/devx/gcp/mod.rs`,
//! the only place `cli`'s `devx::gcp::run` is reachable.

#![allow(clippy::unused_async)]

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::policy::PolicyClient;
use portal::{SessionStore, WorkshopIndex, WorkshopMaterial, DEFAULT_PUBLIC_DIR};
use tower::ServiceExt;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct DeployWorld {
    app: Option<axum::Router>,
    materials: Vec<WorkshopMaterial>,
    last_status: Option<StatusCode>,
    last_ctype: String,
    last_location: String,
    last_body: String,
}

impl std::fmt::Debug for DeployWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeployWorld")
            .field("last_status", &self.last_status)
            .field("materials", &self.materials.len())
            .finish_non_exhaustive()
    }
}

impl DeployWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    /// The deploy workshop material, looked up by category and slug. Panics with a
    /// clear message when the manifest entry is missing — that is the
    /// failure the registration scenario exists to surface.
    fn deploy(&self) -> &WorkshopMaterial {
        self.materials
            .iter()
            .find(|m| m.category == "workshops" && m.slug == "deploy-the-navigator")
            .expect("the `deploy-the-navigator` workshop must be registered in the manifest")
    }
}

#[given("the \"Operating Neon Law Navigator\" workshop is loaded from the content directory")]
async fn load_workshops(world: &mut DeployWorld) {
    // Load the *real* on-disk content so the scenarios ground the file
    // that actually ships, not a synthetic fixture.
    let materials =
        portal::workshops::loader::load_navigator(Path::new(portal::DEFAULT_WORKSHOPS_DIR))
            .expect("load workshop materials from the content directory");
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("deploy-the-navigator-walkthrough").await;
    let mut state = app_state(
        runtime,
        storage,
        PolicyClient::passthrough(),
        None,
        SessionStore::new(TEST_SESSION_KEY),
    )
    .await;
    state.workshops = WorkshopIndex::new(materials.clone());
    // The firm's host: the Navigator classes mount there, public like the
    // talks beside them.
    world.app = Some(features::neon_router(state, Path::new(DEFAULT_PUBLIC_DIR)));
    world.materials = materials;
}

/// The session key the world's [`SessionStore`] is built with, so a minted
/// cookie verifies against the running app.
const TEST_SESSION_KEY: &str = "test-session-key-not-for-production";

async fn record(world: &mut DeployWorld, req: Request<Body>) {
    let resp = world.app().oneshot(req).await.unwrap();
    world.last_status = Some(resp.status());
    world.last_ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    world.last_location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    world.last_body = body_string(resp).await;
}

#[when(regex = r#"^a reader visits "([^"]+)"$"#)]
async fn visit(world: &mut DeployWorld, path: String) {
    record(
        world,
        Request::builder().uri(&path).body(Body::empty()).unwrap(),
    )
    .await;
}

/// The class is public, so the read must not hand back the session boundary's
/// login redirect. Guards the gate from returning: re-mounting the material
/// behind `require_auth` would answer this anonymous read with a `303` to
/// `/auth/login`, and the certificate `POST` — which does keep its gate — is
/// pinned in `server/tests/routes.rs` against the embedded policy.
#[then("the response carries no login redirect")]
async fn no_login_redirect(world: &mut DeployWorld) {
    assert!(
        world.last_location.is_empty(),
        "a public class must not redirect anywhere, got {:?}",
        world.last_location,
    );
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut DeployWorld, code: u16) {
    assert_eq!(
        world.last_status.expect("no request made").as_u16(),
        code,
        "unexpected status",
    );
}

#[then(regex = r#"^the page title is "([^"]+)"$"#)]
async fn title_is(world: &mut DeployWorld, title: String) {
    let needle = format!("<title>{title}</title>");
    assert!(
        world.last_body.contains(&needle),
        "page <title> must be {title:?}",
    );
}

#[then(regex = r#"^the page shows no "([^"]+)" banner$"#)]
async fn no_banner(world: &mut DeployWorld, phrase: String) {
    assert!(
        !world.last_body.contains(&phrase),
        "the workshop page must not render the {phrase:?} banner",
    );
}

#[then(regex = r#"^the workshop's first section is titled "([^"]+)"$"#)]
async fn first_section(world: &mut DeployWorld, title: String) {
    let first = world
        .deploy()
        .sections
        .first()
        .expect("the workshop must have at least one section");
    assert_eq!(first.title, title, "first section title");
}

#[then(regex = r"^the workshop splits into at least (\d+) sections$")]
async fn at_least_sections(world: &mut DeployWorld, n: usize) {
    let count = world.deploy().sections.len();
    assert!(count >= n, "expected at least {n} sections, got {count}");
}

#[then("the rendered body carries no duplicate top-level heading")]
async fn no_duplicate_h1(world: &mut DeployWorld) {
    assert!(
        !world.deploy().body_html.contains("<h1"),
        "rendered body must carry no <h1>; the page chrome owns the sole title",
    );
}

/// The visible text of rendered HTML, with tags stripped. Server-side syntect
/// highlighting wraps a fenced command's tokens in `<span style=…>` elements, so
/// a verbatim command is no longer a raw substring of `body_html`; matching
/// against the tag-stripped text asserts the reader still sees the command to
/// copy, independent of how it is coloured.
fn visible_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[then(regex = r#"^the rendered workshop shows the command "([^"]+)"$"#)]
async fn shows_command(world: &mut DeployWorld, cmd: String) {
    // The command lives in a highlighted step-body code block, so assert against
    // the workshop's full rendered HTML with tags stripped — stable no matter
    // which step the beat lands on or how syntect colours the tokens.
    assert!(
        visible_text(&world.deploy().body_html).contains(&cmd),
        "rendered workshop must show the command {cmd:?} for the reader to copy",
    );
}

#[then(regex = r#"^the rendered workshop shows the "([^"]+)" flag$"#)]
async fn shows_flag(world: &mut DeployWorld, flag: String) {
    assert!(
        visible_text(&world.deploy().body_html).contains(&flag),
        "rendered workshop must show the {flag:?} flag",
    );
}

#[then(regex = r#"^the response content-type is "([^"]+)"$"#)]
async fn ctype_is(world: &mut DeployWorld, ctype: String) {
    assert_eq!(world.last_ctype, ctype, "content-type");
}

#[then(regex = r#"^the markdown twin contains "([^"]+)"$"#)]
async fn twin_contains(world: &mut DeployWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "markdown twin must contain {needle:?}",
    );
}

#[tokio::main]
async fn main() {
    DeployWorld::run("tests/features/deploy_the_navigator_walkthrough.feature").await;
}
