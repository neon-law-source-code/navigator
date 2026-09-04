#![allow(clippy::doc_markdown)]
//! Route tests for the template gallery + the LSP showcase.
//!
//! Drives the router via `tower::ServiceExt::oneshot` (no socket). The
//! load-bearing claims:
//!
//! - the gallery is a shared Navigator tool: it renders for a signed-in
//!   reader and turns an anonymous one back to the login door (#732);
//! - a template downloads as verbatim `text/markdown` bytes with an
//!   attachment filename;
//! - the curated allow-list is enforced — a `confidential: true`
//!   template (Retainer) 404s rather than leaking;
//! - a detail page carries the disclaimer partial + the start-a-matter
//!   CTA;
//! - the LSP showcase renders with the install command + disclaimer.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::AppState;
use tower::ServiceExt;

async fn empty_state() -> AppState {
    portal::test_support::app_state(portal::test_support::embedded_surreal().await).await
}

async fn get(state: AppState, uri: &str) -> axum::http::Response<Body> {
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY).encode(
            &portal::SessionData::fresh("gallery-reader", store::persons::Role::Client)
        )
    );
    server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR))
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(axum::http::header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The same request with no session at all.
async fn get_anonymous(state: AppState, uri: &str) -> axum::http::Response<Body> {
    server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR))
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn gallery_index_renders_for_a_signed_in_reader() {
    let resp = get(empty_state().await, "/templates").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(&body.contains("Template gallery"));
    // Leads with the federal Form 990, labeled federal.
    assert!(body.contains("IRS Form 990"));
    assert!(body.contains("Federal · United States"));
    // The two Nevada filings are loudly labeled.
    assert!(body.contains("Nevada"));
    // The disclaimer rides the page.
    assert!(body.contains("not legal advice"));
}

#[tokio::test]
async fn template_detail_has_frontmatter_disclaimer_and_start_a_matter_cta() {
    let resp = get(
        empty_state().await,
        "/templates/notations/forms/united-states/federal/irs/us--form-990",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    // The notation format itself — the rendered frontmatter.
    assert!(body.contains("code: us__form_990"));
    // The UPL disclaimer partial.
    assert!(body.contains("does not create an attorney"));
    // A download must not be a dead end.
    assert!(&body.contains("Start a matter"));
    assert!(body.contains("href=\"mailto:contact@neonlaw.com\""));
    // And the raw-download link — kebab-cased, like every asset URL.
    assert!(
        body.contains("/templates/notations/forms/united-states/federal/irs/us--form-990/download")
    );
}

#[tokio::test]
async fn template_underscore_url_redirects_to_kebab() {
    // The on-disk stem keeps its underscores; the URL is kebab-case. A
    // request for the legacy underscore form permanently redirects to the
    // hyphenated home.
    let resp = get(
        empty_state().await,
        "/templates/notations/forms/united_states/federal/irs/us__form_990",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/templates/notations/forms/united-states/federal/irs/us--form-990"),
    );

    // The download route redirects too, preserving the trailing segment.
    let resp = get(
        empty_state().await,
        "/templates/notations/forms/united_states/federal/irs/us__form_990/download",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/templates/notations/forms/united-states/federal/irs/us--form-990/download"),
    );
}

#[tokio::test]
async fn legacy_gallery_url_redirects_to_deep_taxonomy_path() {
    let resp = get(
        empty_state().await,
        "/templates/nonprofit/form990-annual-report",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/templates/notations/forms/united-states/federal/irs/us--form-990"),
    );
}

#[tokio::test]
async fn template_downloads_verbatim_markdown_as_an_attachment() {
    let resp = get(
        empty_state().await,
        "/templates/notations/forms/united-states/federal/irs/us--form-990/download",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "text/markdown; charset=utf-8"
    );
    // The downloaded file keeps its on-disk underscore name (the bytes a
    // git reader sees), even though the route that serves it is kebab.
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .unwrap(),
        "attachment; filename=\"us__form_990.md\""
    );
    let body = body_string(resp).await;
    // Verbatim bytes: the same source the git reader sees, frontmatter
    // fence and all.
    let source =
        include_str!("../../templates/notations/forms/united_states/federal/irs/us__form_990.md");
    assert_eq!(body, source);
}

#[tokio::test]
async fn confidential_template_404s_rather_than_leaking() {
    // Retainer is `confidential: true` and not on the allow-list. The
    // route must 404 — never serve it by guessing the path.
    let resp = get(empty_state().await, "/templates/neon-law/shared/retainer").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = get(
        empty_state().await,
        "/templates/neon-law/shared/retainer/download",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn off_list_template_path_404s() {
    let resp = get(empty_state().await, "/templates/nonprofit/MadeUp").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn gallery_turns_an_anonymous_visitor_back_to_the_login_door() {
    // The gallery serves firm notation, not host marketing, so it composes
    // behind the shared session boundary (#732). Anonymous access is a
    // browser redirect, never a rendered page — including for the raw
    // markdown twin under `/app/api/templates/*`, which answers a machine
    // caller with a parseable document instead.
    for path in [
        "/templates",
        "/templates/notations/forms/united-states/federal/irs/us--form-990",
        "/templates/notations/forms/united-states/federal/irs/us--form-990/download",
    ] {
        let resp = get_anonymous(empty_state().await, path).await;
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "{path} must redirect an anonymous browser"
        );
        assert_eq!(
            resp.headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(format!("/auth/login?return_to={path}").as_str()),
            "{path} must return the reader to where they were headed"
        );
    }

    let raw = get_anonymous(
        empty_state().await,
        "/app/api/templates/notations/forms/united-states/federal/irs/us--form-990",
    )
    .await;
    assert_eq!(raw.status(), StatusCode::UNAUTHORIZED);
    let document: serde_json::Value = serde_json::from_str(&body_string(raw).await).unwrap();
    assert_eq!(document["error"], "unauthenticated");
}
