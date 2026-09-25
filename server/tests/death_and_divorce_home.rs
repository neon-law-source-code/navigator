//! Death & Divorce's public Dioxus home and services pages.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

const HOSTS: [&str; 2] = [
    "www.deathanddivorcelaw.com",
    "staging.deathanddivorcelaw.com",
];

async fn page(host: &str, path: &str) -> String {
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
async fn death_and_divorce_home_carries_the_brand_direction() {
    for host in HOSTS {
        let html = page(host, "/").await;
        for expected in [
            "<title>Death & Divorce</title>",
            "For endings, transitions, and the beyond",
            "Divorce",
            "Estate Planning",
            "Probate",
            "These are sad moments",
            "death-and-divorce",
            "/public/css/brand-death-and-divorce-tokens.css",
            "/public/css/death-and-divorce.css",
            "/public/brand/death-and-divorce/mark.svg",
            "Pirata One",
            "Attorney advertisement",
        ] {
            assert!(
                html.contains(expected),
                "{host} missing {expected:?}: {html}"
            );
        }
        assert!(!html.contains(">01<") && !html.contains(">02<") && !html.contains(">03<"));
    }
}

#[tokio::test]
async fn death_and_divorce_services_explain_the_three_practice_areas() {
    let html = page(HOSTS[1], "/services").await;
    for expected in [
        "Death &#38; Divorce services",
        "Divorce",
        "Estate planning",
        "Probate",
        "What comes after",
    ] {
        assert!(html.contains(expected), "missing {expected:?}: {html}");
    }
}
