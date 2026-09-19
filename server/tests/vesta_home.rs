//! The estate offer and the firm's two practice doors through the shipped router.
use std::collections::BTreeMap;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use views::brand::BrandKey;

/// Local preview port for the held-out estate brand. Public Host matching
/// no longer admits that key, so the suite reaches it the same way a
/// developer does: `localhost` plus a bound local port.
const VESTA_LOCAL_PORT: u16 = 20_640;

async fn page(host: &str, path: &str) -> String {
    page_with_ports(host, path, BTreeMap::new()).await
}

async fn vesta_page(path: &str) -> String {
    page_with_ports(
        &format!("localhost:{VESTA_LOCAL_PORT}"),
        path,
        [(VESTA_LOCAL_PORT, BrandKey::Vesta)].into(),
    )
    .await
}

async fn page_with_ports(host: &str, path: &str, local_ports: BTreeMap<u16, BrandKey>) -> String {
    let mut state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    state.canonical_host = portal::CanonicalHost::new(None).with_local_ports(local_ports);
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_held_out_brand_host_does_not_render_that_brand() {
    let html = page("staging.vestaestateplanning.com", "/").await;
    assert!(
        !html.contains("/public/img/vesta-home/vesta-explainer.mp4"),
        "a held-out Host must not admit the estate brand: {html}"
    );
    assert!(
        !html.contains("/public/brand/vesta.svg"),
        "a held-out Host must not admit the estate mark: {html}"
    );
}

#[tokio::test]
async fn vesta_offers_lifetime_edits_and_the_shared_booking_calendar() {
    let html = vesta_page("/").await;
    for expected in [
        "$5,000",
        "Unlimited edits",
        "for life",
        "$5",
        "per transaction",
        "Coming soon",
        "https://calendar.notion.so/meet/shicholas/or15n4yy7",
        "/public/brand/vesta.svg",
        "Shook Law PLLC",
        "site-footer",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("$3,000"));
    assert!(!html.contains("calendar.app.google"));
    assert!(html.contains("does not replace"));
}

#[tokio::test]
async fn lawyer_shook_has_two_brand_cards_and_keeps_the_firm_notice() {
    let html = page("staging.lawyershook.com", "/").await;
    for expected in [
        "holding-page",
        "Unless you have an active retainer",
        "home-practice",
        "Neon Law",
        "Vesta Estate Planning",
        "emerging technology companies",
        "$5,000",
        "unlimited edits",
        "site-footer",
        "https://www.vestaestateplanning.com",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[tokio::test]
async fn vesta_services_agree_with_the_lifetime_offer() {
    let html = vesta_page("/services").await;
    assert!(html.contains("$5,000"));
    assert!(html.contains("unlimited edits"));
    assert!(!html.contains("$3,000"));
}

#[tokio::test]
async fn vesta_and_lawyer_shook_share_the_company_footer_treatment() {
    for html in [
        vesta_page("/").await,
        page("staging.lawyershook.com", "/").await,
    ] {
        let footer = html.split("<footer").nth(1).expect("shared footer");
        assert!(footer.contains("https://www.lawyershook.com"));
        assert!(footer.contains("Everyone deserves to be seen."));
        assert!(!footer.contains("Our family"));
    }
}

#[tokio::test]
async fn vesta_has_an_explainer_instead_of_numbered_steps() {
    let html = vesta_page("/").await;
    assert!(!html.contains("vesta-step__number"));
    assert!(html.contains("<video"));
    assert!(html.contains("/public/img/vesta-home/vesta-explainer.mp4"));
    assert!(html.contains("Read the video transcript"));
    assert!(html.contains("does not replace required signing"));
    let brand = BrandKey::Vesta;
    assert_eq!(brand.default_typeface().id, "eb-garamond");
    assert!(brand.display_typeface().is_none());
}
