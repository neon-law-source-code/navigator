//! The CyberInjuryLaw preview renders through the shared Dioxus router.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use portal::{AppState, CanonicalHost};
use tower::ServiceExt;
use views::brand::BrandKey;

#[tokio::test]
async fn cyber_injury_preview_serves_its_copy_assets_and_fonts() {
    let state = AppState {
        canonical_host: CanonicalHost::new(None)
            .with_local_ports([(20_645, BrandKey::CyberInjuryLaw)].into()),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "localhost:20645")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    for expected in [
        "CyberInjuryLaw",
        "WE USE AI TO",
        "YOUR POCKET.",
        "20% ATTORNEY FEE",
        "$76,000",
        "$63,650",
        "+$12,350",
        "cyber-injury-law.css",
        "BarlowCondensed-ExtraBold.woff2",
        "DMSans-Regular.woff2",
        "cyber-warrior.png",
        "cyber-incident",
        "cyber-timing",
        "Shook Law PLLC",
        "site-footer",
        "Law.com",
        "cyber-consultation-qr.svg",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("fonts.googleapis.com"));
    assert!(!html.contains("fonts.gstatic.com"));
    assert!(!html.contains("assets/app.js"));
}

#[tokio::test]
async fn printed_consultation_url_redirects_to_the_brands_booking_provider() {
    let state = AppState {
        canonical_host: CanonicalHost::new(None)
            .with_local_ports([(20_645, BrandKey::CyberInjuryLaw)].into()),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/consultation?next=https://example.org")
                .header("host", "localhost:20645")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()["location"],
        views::brand::CYBER_INJURY_BRANDING.consultation_url
    );
}

#[test]
fn campaign_stays_out_of_the_public_launch_set() {
    assert!(!BrandKey::CyberInjuryLaw.is_live());
    assert!(
        portal::plausible::PlausibleSite::for_brand(BrandKey::CyberInjuryLaw)
            .script_tags()
            .is_empty()
    );
}
