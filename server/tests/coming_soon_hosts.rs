//! The Summons holding page is public on production and staging while its
//! service catalog stays unpublished.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use portal::CanonicalHost;
use tower::ServiceExt;
use views::brand::BrandKey;

#[tokio::test]
async fn summons_serves_only_coming_soon_on_production_and_staging() {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    for host in BrandKey::Summons.hosts() {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", *host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{host}");
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Summons Defense | Coming Soon"), "{host}");
        assert!(html.contains("Coming Soon"), "{host}");
        assert!(html.contains("holding-page"), "{host}");
        assert!(
            html.contains("/public/css/brand-summons-tokens.css"),
            "{host}"
        );
        assert!(html.contains("Libre Franklin"), "{host}");
        assert!(!html.contains("GORP"), "{host}");
        assert!(!html.contains("holding-page__paragraph"), "{host}");
        assert!(!html.contains("home-practice"), "{host}");
        assert!(html.contains("Attorney advertisement"), "{host}");
        for path in ["/services", "/contact"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("host", *host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{host}{path}");
        }
    }
}

#[tokio::test]
async fn summons_apex_redirects_to_its_own_site() {
    let state = portal::AppState {
        canonical_host: CanonicalHost::new(Some("www.neonlaw.com".into())),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", BrandKey::Summons.apex())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://www.summonsdefense.nyc/"
    );
}
