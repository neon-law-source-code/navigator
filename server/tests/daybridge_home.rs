//! The held-out Daybridge divorce practice, reviewed through its local preview door.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use portal::{AppState, CanonicalHost};
use tower::ServiceExt;
use views::brand::BrandKey;

const PREVIEW_PORT: u16 = 20_641;

async fn preview(path: &str) -> String {
    let state = AppState {
        canonical_host: CanonicalHost::new(None)
            .with_local_ports([(PREVIEW_PORT, BrandKey::Daybridge)].into_iter().collect()),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let host = format!("localhost:{PREVIEW_PORT}");
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", &host)
                .body(Body::empty())
                .expect("preview request"),
        )
        .await
        .expect("preview response");
    assert_eq!(response.status(), StatusCode::OK, "{host} {path}");
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec(),
    )
    .expect("UTF-8 response")
}

#[tokio::test]
async fn daybridge_home_publishes_the_reviewed_daily_fee_offer() {
    let html = preview("/").await;
    for expected in [
        "Daybridge Divorce Law",
        "A way through divorce.",
        "$10",
        "/ day",
        "five business days",
        "case costs",
        "/public/brand/daybridge/logo.svg",
        "/public/css/daybridge.css",
        "Attorney advertisement",
        "Neon Law",
        "DeleteYourData.com",
        "/public/logo.svg",
        "/public/brand/delete-your-data/logo.svg",
        "Shook Law PLLC",
    ] {
        assert!(html.contains(expected), "missing {expected:?}");
    }
    assert!(
        html.contains("site-footer__family"),
        "the footer family row renders"
    );
    assert!(!html.contains("Lawyer Shook"), "{html}");
    assert!(!html.contains("A practice of Shook Law PLLC"), "{html}");
    assert!(
        html.matches(r#"class="site-footer__family-logo""#).count() == 8,
        "each of the eight Daybridge family entries has its own logo"
    );
    assert!(!html.to_lowercase().contains("guaranteed result"));
}

#[tokio::test]
async fn daybridge_services_explain_scope_timing_and_costs() {
    let html = preview("/services").await;
    for expected in [
        "Divorce services",
        "Agreements and proposed resolutions",
        "Motions and court papers",
        "after we receive the information and documents we need",
        "Court filing fees",
        "No promised outcome",
    ] {
        assert!(html.contains(expected), "missing {expected:?}");
    }
}

#[tokio::test]
async fn daybridge_hosts_stay_held_out_until_launch_is_approved() {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    for host in BrandKey::Daybridge.hosts() {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", *host)
                    .body(Body::empty())
                    .expect("held-out host request"),
            )
            .await
            .expect("held-out host response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{host}");
    }
}
