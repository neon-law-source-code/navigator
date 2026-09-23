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
        assert!(html.contains(r#"<main class="holding-page">"#), "{host}");
        let heading = html
            .split_once("<h1")
            .and_then(|(_, rest)| rest.split_once("</h1>").map(|(heading, _)| heading));
        assert!(
            heading.is_some_and(|heading| {
                heading.contains("holding-page__heading") && heading.contains("Coming Soon")
            }),
            "{host}: the holding page has a named h1"
        );
        assert!(
            html.contains("/public/css/brand-summons-tokens.css"),
            "{host}"
        );
        let head = html.split_once("</head>").expect("document head").0;
        assert!(head.contains(r#"rel="icon""#), "{host}: {head}");
        assert!(
            head.contains(r#"href="/public/brand/summons/logo.svg""#),
            "{host} must use the Summons mark as its favicon: {head}"
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
            assert!(
                response.headers().get(header::LOCATION).is_none(),
                "{host}{path} must not redirect anywhere, got Location: {:?}",
                response.headers().get(header::LOCATION)
            );
            let body = String::from_utf8(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(
                !body.contains("Coming Soon"),
                "{host}{path} must not leak the holding page's own notice"
            );
            assert!(
                !body.contains("Summons Defense | Coming Soon"),
                "{host}{path} must not wear the holding page's title"
            );
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
