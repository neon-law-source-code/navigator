//! The estate offer and the firm's two practice doors through the shipped router.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use portal::{AppState, CanonicalHost};
use tower::ServiceExt;
use views::brand::BrandKey;

/// An arbitrary local port, standing in for whatever
/// `NAVIGATOR_LOCAL_VESTA_PORT` a developer exports. The value is
/// meaningless; that it is *a port the deployment was told to bind* is the
/// whole point.
const PREVIEW_PORT: u16 = 20_640;

/// Fetch `path` as a public visitor on `host`.
///
/// Only a launched brand's host reaches a page this way. `Vesta` has not
/// launched, so `portal::canonical_host` refuses `staging.vestaestateplanning.com`
/// with a `404` — see `server::tests::routes`'s launch-gate tests, which is
/// where that refusal is asserted. Vesta's own page is read through
/// [`preview`] instead.
async fn page(host: &str, path: &str) -> String {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    fetch(state, path, host).await
}

/// Fetch `path` as a developer previewing `key` through its own local port —
/// the one door into a brand that the launch gate does not close, and the
/// only way to read a held-out brand's page.
///
/// A held-out brand is built, staged, and reviewed long before it is
/// approved, and this is the seam that review runs through:
/// `BrandKey::local_port_env_var` names a variable, `portal::hosting::run`
/// binds the port it holds, and a request arriving on that port wears that
/// brand whatever hostname it carries. Nothing here is reachable from the
/// public internet, because nothing binds the port unless an operator sets
/// the variable.
async fn preview(key: BrandKey, path: &str) -> String {
    let state = AppState {
        canonical_host: CanonicalHost::new(None)
            .with_local_ports([(PREVIEW_PORT, key)].into_iter().collect()),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    fetch(state, path, &format!("localhost:{PREVIEW_PORT}")).await
}

async fn fetch(state: AppState, path: &str, host: &str) -> String {
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
    assert_eq!(response.status(), StatusCode::OK, "{host} {path}");
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
    let html = preview(BrandKey::Vesta, "/").await;
    for expected in [
        "$5,000",
        "Unlimited edits",
        "for life",
        "$5",
        "per transaction",
        "Available by request",
        "possible individual",
        "trustee appointment",
        "https://calendar.notion.so/meet/nick-shook/or15n4yy7",
        "/public/brand/vesta.svg",
        "Shook Law PLLC",
        "site-footer",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("$3,000"));
    assert!(!html.contains("calendar.app.google"));
    assert!(!html.contains("Coming soon"));
    assert!(!html.contains("not yet available"));
    assert!(html.contains("do not replace the signatures"));
    assert!(!html.contains("Read the video transcript"));
}

#[tokio::test]
async fn lawyer_shook_lists_the_launched_family_and_keeps_the_firm_notice() {
    let html = page("staging.lawyershook.com", "/").await;
    for expected in [
        "holding-page",
        "Shook Law PLLC is an American law firm",
        "home-practice",
        "Neon Law",
        "DeleteYourData.com",
        "DeleteYourDebt.com",
        "Vesta Estate Planning",
        "Misericordia Injury Law",
        "Abhaya Immigration",
        "Summons Defense",
        "emerging tech",
        "site-footer",
        "https://www.deleteyourdata.com",
        "https://www.vestaestateplanning.com",
        "https://www.summonsdefense.nyc",
        "/public/brand/lawyer-shook/logo.svg",
        "/public/brand/lawyer-shook/logo.png",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    let head = html.split_once("</head>").expect("document head").0;
    assert!(head.contains(r#"rel="icon""#), "missing favicon: {head}");
    assert!(
        head.contains(r#"href="/public/brand/lawyer-shook/logo.svg""#),
        "the Lawyer Shook tab must use its supplied mark: {head}"
    );
}

#[tokio::test]
async fn vesta_services_agree_with_the_lifetime_offer() {
    let html = preview(BrandKey::Vesta, "/services").await;
    assert!(html.contains("$5,000"));
    assert!(html.contains("unlimited edits"));
    assert!(html.contains("Nicholas Shook may accept appointment"));
    assert!(html.contains("qualified Nevada trustee"));
    assert!(html.contains("Life-insurance trusts"));
    assert!(html.contains("$5 per transaction, available by request"));
    assert!(!html.contains("$3,000"));
    assert!(!html.contains("Coming soon"));
    assert!(!html.contains("not yet available"));
}

#[tokio::test]
async fn vesta_and_lawyer_shook_omit_the_lawyer_shook_trademark_notice() {
    for html in [
        preview(BrandKey::Vesta, "/").await,
        page("staging.lawyershook.com", "/").await,
    ] {
        let footer = html.split("<footer").nth(1).expect("shared footer");
        assert!(footer.contains("https://www.lawyershook.com"));
        assert!(footer.contains("Everyone deserves to be seen."));
        assert!(!footer.contains("Our family"));
        assert!(!footer.contains("LAWYER SHOOK"));
        assert!(!footer.contains("common-law mark of Shook Law PLLC"));
    }
}

#[tokio::test]
async fn vesta_has_an_explainer_instead_of_numbered_steps() {
    let html = preview(BrandKey::Vesta, "/").await;
    assert!(!html.contains("vesta-step__number"));
    assert!(html.contains("<video"));
    assert!(html.contains("/public/img/vesta-home/vesta-explainer.mp4"));
    assert!(!html.contains("Read the video transcript"));
    assert!(!html.contains("does not replace required signing"));
    let brand = BrandKey::Vesta;
    assert_eq!(brand.default_typeface().id, "eb-garamond");
    assert!(brand.display_typeface().is_none());
}
