//! The estate offer and the firm's two practice doors through the shipped router.
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
async fn vesta_offers_lifetime_edits_and_the_shared_booking_calendar() {
    let html = page("staging.vestaestateplanning.com", "/").await;
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
    let html = page("staging.vestaestateplanning.com", "/services").await;
    assert!(html.contains("$5,000"));
    assert!(html.contains("unlimited edits"));
    assert!(!html.contains("$3,000"));
}

#[tokio::test]
async fn vesta_and_lawyer_shook_share_the_company_footer_treatment() {
    for host in ["staging.vestaestateplanning.com", "staging.lawyershook.com"] {
        let html = page(host, "/").await;
        let footer = html.split("<footer").nth(1).expect("shared footer");
        assert!(footer.contains("https://www.lawyershook.com"));
        assert!(footer.contains("Everyone deserves to be seen."));
        assert!(!footer.contains("Our family"));
    }
}
