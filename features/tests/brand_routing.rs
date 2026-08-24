//! Cucumber runner for `features/brand_routing.feature`.
//!
//! Boots the real composed router against an in-memory `SQLite`
//! and grep-asserts the per-brand `og:site_name` on every public
//! route, since brand selection is per-handler (no middleware). The
//! footer is unified site-wide and is no longer a brand marker.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::{policy::PolicyClient, SessionStore};
use tower::ServiceExt;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct BrandWorld {
    app: Option<axum::Router>,
    last_status: Option<StatusCode>,
    last_location: Option<String>,
    last_body: String,
}

impl std::fmt::Debug for BrandWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrandWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

#[given("the Neon Law Navigator public site is running")]
async fn build(world: &mut BrandWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("brand").await;
    let state = app_state(
        runtime,
        storage,
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[when(regex = r"^a visitor opens (.+)$")]
async fn visit(world: &mut BrandWorld, path: String) {
    let resp = world
        .app
        .as_ref()
        .expect("app not built")
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    world.last_body = body_string(resp).await;
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status(world: &mut BrandWorld, code: u16) {
    assert_eq!(world.last_status.expect("no status").as_u16(), code);
}

// The footer is unified site-wide now (firm-anchored copyright on every
// page), so it is no longer a per-brand marker. The reliable per-brand
// signal the layout still emits is the Open Graph `og:site_name`, set
// from the page's `SiteBrand`. The trailing quote in the needle keeps
// "Neon Law" from matching a longer site-name tag as a prefix.
/// Whether `body` declares `brand` as its `og:site_name`.
///
/// HTML attributes may arrive in either order: `property="og:site_name"
/// content="…"` or `content="…" property="og:site_name"`. A Gherkin step
/// matches raw bytes, so both spellings are accepted. Each
/// needle keeps the brand's closing quote, which is what stops "Neon Law" from
/// matching a longer site-name tag as a prefix.
fn declares_brand(body: &str, brand: &str) -> bool {
    body.contains(&format!("og:site_name\" content=\"{brand}\""))
        || body.contains(&format!("content=\"{brand}\" property=\"og:site_name\""))
}

#[then(regex = r#"^the page is branded "([^"]+)"$"#)]
async fn page_is_branded(world: &mut BrandWorld, brand: String) {
    assert!(
        declares_brand(&world.last_body, &brand),
        "expected page branded {brand:?} (og:site_name); body did not contain it",
    );
}

#[then(regex = r#"^the page is not branded "([^"]+)"$"#)]
async fn page_is_not_branded(world: &mut BrandWorld, brand: String) {
    assert!(
        !declares_brand(&world.last_body, &brand),
        "page unexpectedly branded {brand:?} (og:site_name)",
    );
}

#[then(regex = r#"^the response redirects to "([^"]+)"$"#)]
async fn redirects_to(world: &mut BrandWorld, target: String) {
    assert_eq!(
        world.last_location.as_deref(),
        Some(target.as_str()),
        "expected Location header {target:?}, got {:?}",
        world.last_location,
    );
}

/// A withdrawn URL answers `410` and points nowhere.
///
/// The status alone is not the assertion. A `410` carrying a `Location` is the
/// shape a half-finished retirement takes — the route table says gone while the
/// handler still names a successor — and a browser given both will follow the
/// header, so the reader never sees the `410` at all.
#[then("the response carries no redirect")]
async fn carries_no_redirect(world: &mut BrandWorld) {
    assert_eq!(
        world.last_location, None,
        "a withdrawn URL must point nowhere, got {:?}",
        world.last_location,
    );
}

#[then(regex = r#"^the response body contains "(.+)"$"#)]
async fn body_contains(world: &mut BrandWorld, needle: String) {
    // Feature files use `\"` to embed a literal double-quote; un-escape
    // so the assertion matches the actual HTML output.
    let needle = needle.replace("\\\"", "\"");
    assert!(
        world.last_body.contains(&needle),
        "expected response body to contain {needle:?}",
    );
}

#[tokio::main]
async fn main() {
    BrandWorld::cucumber()
        // Every scenario composes the Dioxus-backed public router. Serializing
        // them keeps the pinned Tokio worker pools bounded on developer
        // machines, matching the CI gate's constrained execution topology.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/brand_routing.feature")
        .await;
}
