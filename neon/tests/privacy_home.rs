//! The annual privacy offer through the same host router used by the binary.
use axum::{body::Body, http::Request};
use tower::ServiceExt;

#[tokio::test]
async fn privacy_home_publishes_one_annual_offer_and_shared_footer() {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let dioxus = neon::public_dioxus_routers(&state);
    let app = portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        dioxus,
    )
    .expect("public router");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "staging.deleteyourdata.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    for required in [
        "Your Life. Less Exposed.",
        "$50",
        "/ year",
        "Gift cards available",
        "Privacy &#38; Credit",
        "Solana",
        "Record your removal requests on Solana.",
        "Our litigator friends will happily help you out.",
        "pause-privacy-motion",
        "privacy-cloak",
        "site-footer__legal",
        "https://www.lawyershook.com",
    ] {
        assert!(html.contains(required), "missing {required}");
    }
    assert_eq!(html.matches("<h1").count(), 1);
    assert!(!html.contains("A little less out there."));
    assert!(!html.contains("Privacy in motion"));
    assert!(!html.contains("$10"));
    assert!(!html.contains("Coming soon"));
    assert!(!html.contains("Removal depends on the company"));
    assert!(!html.contains("href=\"/services\""));
    assert_eq!(
        html.matches("href=\"https://calendar.notion.so/meet/nick-shook/or15n4yy7\"")
            .count(),
        3
    );
    assert!(!html.contains("calendar.app.google"));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/services")
                .header("host", "staging.deleteyourdata.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}
