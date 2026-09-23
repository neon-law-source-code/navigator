use std::collections::BTreeMap;

use axum::{body::Body, http::Request};
use portal::CanonicalHost;
use tower::ServiceExt;
use views::brand::BrandKey;

async fn router(canonical_host: CanonicalHost) -> axum::Router {
    let mut state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    state.canonical_host = canonical_host;
    portal::bootstrap(
        state.clone(),
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        neon::public_dioxus_routers(&state),
    )
    .expect("public router")
}

async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec(),
    )
    .expect("UTF-8 response body")
}

async fn get(app: &axum::Router, host: &str, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", host)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

/// A published brand answers an empty testimonials page and an empty home.
/// Neither invents a quote card when the store has no approved row.
async fn assert_clean_empty(app: &axum::Router, host: &str, key: BrandKey) {
    let response = get(app, host, "/testimonials").await;
    assert_eq!(
        response.status(),
        200,
        "{} testimonials route",
        key.as_str()
    );
    let testimonials = body(response).await;
    assert!(
        testimonials.contains("Testimonials"),
        "{host}: {testimonials}"
    );
    assert!(
        !testimonials.contains("testimonial-card"),
        "{host}: {testimonials}"
    );

    let response = get(app, host, "/").await;
    assert_eq!(response.status(), 200, "{} home route", key.as_str());
    let home = body(response).await;
    assert!(!home.contains("testimonial-card"), "{host}: {home}");
}

#[tokio::test]
async fn testimonials_and_home_are_cleanly_empty_for_every_brand() {
    let app = router(CanonicalHost::new(None)).await;
    // A held-out brand's public hosts stay unpublished. The local port map is
    // the door that still serves the brand, so the empty page is checked there.
    let mut preview_ports = BTreeMap::new();
    let mut port = 20_710_u16;

    for key in BrandKey::ALL {
        let host = key.hosts()[0];
        if key.is_live() {
            assert_clean_empty(&app, host, *key).await;
            continue;
        }
        for path in ["/", "/testimonials"] {
            let response = get(&app, host, path).await;
            assert_eq!(
                response.status(),
                404,
                "{} stays unpublished at {path}",
                key.as_str()
            );
        }
        preview_ports.insert(port, *key);
        port += 1;
    }

    if preview_ports.is_empty() {
        return;
    }
    let preview = router(CanonicalHost::new(None).with_local_ports(preview_ports.clone())).await;
    for (port, key) in preview_ports {
        assert_clean_empty(&preview, &format!("localhost:{port}"), key).await;
    }
}

#[tokio::test]
async fn retired_workshops_index_redirects_to_presentations() {
    let response = router(CanonicalHost::new(None))
        .await
        .oneshot(
            Request::builder()
                .uri("/workshops")
                .header("host", "staging.neonlaw.com")
                .body(Body::empty())
                .expect("workshops request"),
        )
        .await
        .expect("workshops response");

    assert_eq!(response.status(), 301);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/presentations"
    );
}
