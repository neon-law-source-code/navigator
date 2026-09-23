//! End-to-end title and content coverage for the four launched practice sites.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

async fn page(host: &str, path: &str) -> String {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", host)
                .body(Body::empty())
                .expect("brand page request"),
        )
        .await
        .expect("brand page response");
    assert_eq!(response.status(), StatusCode::OK, "{host} {path}");
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("brand page body")
            .to_vec(),
    )
    .expect("brand page is UTF-8")
}

async fn assert_brand_pages(
    host: &str,
    title: &str,
    home_copy: &str,
    services_copy: &str,
    tokens_css: &str,
) {
    let home = page(host, "/").await;
    for expected in [
        title,
        home_copy,
        "site-footer",
        tokens_css,
        "Shook Law PLLC",
    ] {
        assert!(
            home.contains(expected),
            "{host} home is missing {expected:?}"
        );
    }
    assert!(
        !home.contains("Coming Soon"),
        "{host} still renders a holding page"
    );

    let services = page(host, "/services").await;
    for expected in [services_copy, "site-footer", tokens_css] {
        assert!(
            services.contains(expected),
            "{host} services is missing {expected:?}"
        );
    }
    assert!(
        !services.contains("Coming Soon"),
        "{host} services still renders a holding page"
    );
}

#[tokio::test]
async fn vesta_publishes_estate_planning_home_and_services() {
    assert_brand_pages(
        "staging.vestaestateplanning.com",
        "<title>Vesta Estate Planning | Home</title>",
        "For the life you build.",
        "Your lifetime estate plan",
        "/public/css/brand-vesta-tokens.css",
    )
    .await;
}

#[tokio::test]
async fn misericordia_publishes_injury_home_and_services() {
    assert_brand_pages(
        "staging.misericordialaw.com",
        "<title>Misericordia Injury Law | Home</title>",
        "You were hurt. Talk to a lawyer.",
        "How we are paid",
        "/public/css/brand-misericordia-tokens.css",
    )
    .await;
}

#[tokio::test]
async fn abhaya_publishes_immigration_home_and_services() {
    assert_brand_pages(
        "staging.abhayaimmigration.com",
        "<title>Abhaya Immigration | Home</title>",
        "Help with your immigration case.",
        "What we handle",
        "/public/css/brand-abhaya-tokens.css",
    )
    .await;
}

#[tokio::test]
async fn delete_your_debt_publishes_collection_defense_home_and_services() {
    assert_brand_pages(
        "staging.deleteyourdebt.com",
        "<title>DeleteYourDebt.com | Home</title>",
        "We defend you against debt collectors.",
        "Collection defense",
        "/public/css/brand-delete-your-debt-tokens.css",
    )
    .await;
}
