//! `CyberInjuryLaw`'s public Dioxus home, on its own launched hosts.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use views::brand::BrandKey;

const HOSTS: [&str; 2] = ["www.cyberinjurylaw.com", "staging.cyberinjurylaw.com"];

async fn page(host: &str, path: &str) -> (StatusCode, String) {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", host)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec(),
    )
    .expect("UTF-8 response");
    (status, body)
}

#[tokio::test]
async fn cyber_injury_home_serves_its_copy_assets_and_fonts() {
    for host in HOSTS {
        let (status, html) = page(host, "/").await;
        assert_eq!(status, StatusCode::OK, "{host}");
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
            "canal-st-ad.png",
            "cyber-incident",
            "cyber-timing",
            "Shook Law PLLC",
            "site-footer",
            "Law.com",
            "cyber-consultation-qr.svg",
        ] {
            assert!(html.contains(expected), "{host}: missing {expected}");
        }
        assert!(!html.contains("fonts.googleapis.com"), "{host}");
        assert!(!html.contains("fonts.gstatic.com"), "{host}");
        assert!(!html.contains("assets/app.js"), "{host}");
    }
}

#[tokio::test]
async fn printed_consultation_url_redirects_to_the_brands_booking_provider() {
    for host in HOSTS {
        let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
        let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/consultation?next=https://example.org")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{host}");
        assert_eq!(
            response.headers()["location"],
            views::brand::CYBER_INJURY_BRANDING.consultation_url,
            "{host}"
        );
    }
}

/// The campaign has launched, but its own Plausible site is not wired up
/// yet — [`portal::plausible::script_id`] still answers its unconfigured
/// placeholder, so it renders no analytics tags. Mirrors `DeathAndDivorce`'s
/// launch, which shipped the same way.
#[test]
fn campaign_is_live_without_analytics_configured_yet() {
    assert!(BrandKey::CyberInjuryLaw.is_live());
    assert!(
        portal::plausible::PlausibleSite::for_brand(BrandKey::CyberInjuryLaw)
            .script_tags()
            .is_empty()
    );
}
