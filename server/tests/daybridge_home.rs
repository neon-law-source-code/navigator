//! The launched Daybridge divorce practice, reached through its real public
//! hosts (`www.daybridgedivorce.com`, `staging.daybridgedivorce.com`) — the
//! same door a browser or crawler uses now that `Daybridge` is admitted by
//! `BrandKey::LIVE`.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

const HOSTS: [&str; 2] = ["www.daybridgedivorce.com", "staging.daybridgedivorce.com"];

async fn get(host: &str, path: &str) -> axum::http::Response<Body> {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    app.oneshot(
        Request::builder()
            .uri(path)
            .header("host", host)
            .body(Body::empty())
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn body_of(response: axum::http::Response<Body>) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec(),
    )
    .expect("UTF-8 response")
}

async fn page(host: &str, path: &str) -> String {
    let response = get(host, path).await;
    assert_eq!(response.status(), StatusCode::OK, "{host} {path}");
    body_of(response).await
}

/// The document head must wear Daybridge's own identity — its `og:site_name`,
/// its favicon, and its scoped tokens stylesheet — never Neon Law's.
fn assert_own_brand_head(html: &str, host: &str) {
    let head = html.split_once("</head>").expect("document head").0;
    assert!(
        head.contains(r#"og:site_name" content="Daybridge Divorce Law""#)
            || head.contains(r#"content="Daybridge Divorce Law" property="og:site_name""#),
        "{host} must declare its own og:site_name: {head}"
    );
    assert!(
        head.contains(r#"href="/public/brand/daybridge/logo.svg""#),
        "{host} must use its own favicon/logo: {head}"
    );
    assert!(
        head.contains("/public/css/brand-daybridge-tokens.css"),
        "{host} must load its own scoped brand tokens: {head}"
    );
    assert!(
        !head.contains(r#"og:site_name" content="Neon Law""#)
            && !head.contains(r#"content="Neon Law" property="og:site_name""#),
        "{host} must not fall back to Neon Law's og:site_name: {head}"
    );
}

#[tokio::test]
async fn daybridge_home_publishes_the_reviewed_daily_fee_offer() {
    for host in HOSTS {
        let html = page(host, "/").await;
        assert_own_brand_head(&html, host);
        for expected in [
            "Daybridge Divorce Law",
            "A way through divorce.",
            "$10",
            "$500",
            "five business days",
            "legal costs",
            "/public/brand/daybridge/logo.svg",
            "/public/css/daybridge.css",
            "Attorney advertisement",
            "Shook Law PLLC",
        ] {
            assert!(html.contains(expected), "{host} missing {expected:?}");
        }
        // Daybridge is a launched practice brand now, like Vesta,
        // Misericordia, Abhaya, DeleteYourDebt, and Summons before it: its
        // footer is the single-focus "practice" variant
        // (`PublicChrome::is_practice`, keyed off `BrandKey::LIVE`), which
        // omits the "Our Family" cross-sell and points the copyright at
        // Lawyer Shook instead. See `server/tests/vesta_home.rs`'s
        // `vesta_and_lawyer_shook_omit_the_lawyer_shook_trademark_notice`
        // for the same shape on an already-launched sibling.
        let footer = html.split("<footer").nth(1).expect("shared footer");
        assert!(
            !footer.contains("site-footer__family") && !footer.contains("Our family"),
            "{host} a launched practice brand must not cross-sell the family row: {footer}"
        );
        assert!(
            footer.contains(r#"href="https://www.lawyershook.com""#),
            "{host} the practice footer's copyright links Lawyer Shook: {footer}"
        );
        assert!(
            !html.contains("A practice of Shook Law PLLC"),
            "{host} {html}"
        );
        assert!(!html.to_lowercase().contains("guaranteed result"));
    }
}

#[tokio::test]
async fn daybridge_services_explain_scope_timing_and_costs() {
    for host in HOSTS {
        let html = page(host, "/services").await;
        assert_own_brand_head(&html, host);
        for expected in [
            "Divorce services",
            "Agreements and proposed resolutions",
            "Motions and court papers",
            "after we receive the information and documents we need",
            "filing fees",
            "not a particular result",
        ] {
            assert!(html.contains(expected), "{host} missing {expected:?}");
        }
    }
}

/// `/contact` is a shared route every brand answers on its own hosts; it must
/// still publish Daybridge's own mailbox and head, never Neon's.
#[tokio::test]
async fn daybridge_contact_publishes_its_own_mailbox_and_head() {
    for host in HOSTS {
        let html = page(host, "/contact").await;
        assert_own_brand_head(&html, host);
        assert!(
            html.contains("<title>Daybridge Divorce Law | Contact</title>"),
            "{host} must title its own contact page: {html}"
        );
        assert!(
            html.contains("contact@daybridgedivorce.com"),
            "{host} must publish its own mailbox: {html}"
        );
        assert!(
            !html.contains("contact@neonlaw.com"),
            "{host} must not publish Neon Law's mailbox: {html}"
        );
    }
}

/// `/llms.txt` is a crawler's other door into the site. It must index
/// Daybridge's own two pages under its own name and description, never
/// Neon's.
#[tokio::test]
async fn daybridge_llms_txt_indexes_its_own_pages() {
    for host in HOSTS {
        let response = get(host, "/llms.txt").await;
        assert_eq!(response.status(), StatusCode::OK, "{host} /llms.txt");
        let body = body_of(response).await;
        assert!(
            body.starts_with("# Daybridge Divorce Law\n"),
            "{host} llms.txt must name Daybridge as its own site: {body}"
        );
        assert!(
            body.contains("$10 a day while retained"),
            "{host} llms.txt summary must explain the daily-fee offer: {body}"
        );
        assert!(
            body.contains(&format!("https://{host}/)")),
            "{host} llms.txt must index its own home page: {body}"
        );
        assert!(
            body.contains(&format!("https://{host}/services)")),
            "{host} llms.txt must index its own services page: {body}"
        );
        assert!(
            !body.contains("Neon Law is a consumer law firm"),
            "{host} llms.txt must not fall back to Neon Law's summary: {body}"
        );
    }
}
