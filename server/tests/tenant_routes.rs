//! Route parity for the white-label tenant shape (`portal::tenant`).
//!
//! A tenant is a firm running Navigator behind its own marketing site. What it
//! must *not* serve is the point: none of the first-party brands' public pages,
//! because a tenant's own site owns the public web and two front doors on one
//! domain is the defect.
//!
//! These tests compose the router exactly as the `tenant` binary does, through
//! `portal::tenant::public_routes`, so they cover the binary's real surface
//! rather than one this file assembled.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// The router the `tenant` binary serves. `portal_only` is set the way
/// `hosting::run` sets it from `Brand::portal_only`, so the assertions below
/// describe the deployed shape rather than a default one.
async fn tenant_router() -> Router {
    let mut state = portal::test_support::app_state(mem_surreal().await).await;
    state.portal_only = portal::PortalOnly::new(true);
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        portal::tenant::public_routes(),
        &["/"],
        Vec::new(),
    )
    .expect("the tenant root redirect does not collide with Navigator")
}

async fn get(app: &Router, path: &str) -> (StatusCode, Option<String>) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    (status, location)
}

#[tokio::test]
async fn the_bare_host_redirects_into_the_portal() {
    let app = tenant_router().await;

    let (status, location) = get(&app, "/").await;

    assert!(
        status.is_redirection(),
        "a tenant serves no home page of its own; got {status}"
    );
    assert_eq!(location.as_deref(), Some("/app/projects"));
}

#[tokio::test]
async fn no_first_party_brand_page_is_published() {
    let app = tenant_router().await;

    // The firm's pages, its retired URLs, and the shared legal/crawler
    // documents all belong to a brand host. A tenant publishing any of them
    // would be serving another company's site under its own domain — and a
    // retired URL is the sharper case: a tenant answering `410 Gone` would be
    // telling its own visitors that a page it never published was withdrawn.
    for path in [
        "/contact",
        "/team",
        "/team/jacob",
        "/team/nick",
        "/blog",
        "/foundation",
        "/foundation/transparency",
        "/privacy",
        "/terms",
        "/robots.txt",
        "/sitemap.xml",
    ] {
        let (status, _) = get(&app, path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} must not be published by a tenant"
        );
    }
}

#[tokio::test]
async fn the_application_is_still_mounted_and_still_gated() {
    let app = tenant_router().await;

    // The tenant exists to serve the application, so `/app/projects` must be
    // there — and must still send an anonymous visitor to log in.
    let (status, location) = get(&app, "/app/projects").await;
    assert!(
        status.is_redirection(),
        "anonymous /app/projects must redirect to login; got {status}"
    );
    assert_eq!(
        location.as_deref(),
        Some("/auth/login?return_to=/app/projects"),
        "the tenant must not weaken the anonymous-access boundary"
    );
}

#[tokio::test]
async fn the_json_api_stays_unauthenticated_401() {
    let app = tenant_router().await;

    let (status, _) = get(&app, "/app/api/projects").await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the API answers 401 for a tenant exactly as it does for a brand host"
    );
}
