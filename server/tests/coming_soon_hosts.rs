//! The held-out summons channel renders one "Coming Soon" landing page,
//! wearing its own typeface and palette, and publishes nothing else.
//!
//! Router-driven rather than unit-level on purpose: the thing under test is
//! the product of the brand resolver, the home router, and the
//! unpublished-path rejection together. `neon::firm_pages` covers the copy;
//! this covers the wiring.
//!
//! **These pages are not public, and this file must not be read as saying
//! they are.** The key is held out of [`BrandKey::LIVE`], so
//! `portal::canonical_host` refuses its real hostname with a `404` before
//! any of this renders — [`the_launch_gate_still_refuses_every_one_of_these_hosts`]
//! pins that, and `server::tests::routes`'s launch-gate tests assert it over
//! the whole registry. What is under test here is what each host will serve
//! on the day its key joins the approved set, read through the one door the
//! gate leaves open.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use portal::{AppState, CanonicalHost};
use tower::ServiceExt;
use views::brand::BrandKey;

/// An arbitrary local port, standing in for whatever
/// `BrandKey::local_port_env_var` names for the key under test. The value is
/// meaningless; that it is *a port the deployment was told to bind* is the
/// whole point.
const PREVIEW_PORT: u16 = 20_641;

/// The held-out channel, as `(key, public host, site name)`.
///
/// Staging hosts throughout: this repository names only staging, and the
/// registry serves the same brand on both.
const UNLAUNCHED: &[(BrandKey, &str, &str)] = &[
    // The NYC host wears the firm's own name: New York Rule 7.5(b) bars a
    // trade name for private practice.
    (
        BrandKey::Summons,
        "staging.summonsdefense.nyc",
        "Shook Law PLLC",
    ),
];

/// The `<title>` element's markup, for a failure message worth reading.
fn title_of(html: &str) -> &str {
    html.split_once("<title")
        .and_then(|(_, rest)| rest.split_once("</title>"))
        .map_or("<no title element>", |(inner, _)| inner)
}

/// Fetch `path` as a developer previewing `key` through its own local port —
/// the one door into a brand that the launch gate does not close, and so the
/// only way to read a held-out brand's page.
///
/// A held-out brand is built, staged, and reviewed long before it is
/// approved, and this is the seam that review runs through:
/// `BrandKey::local_port_env_var` names a variable, `portal::hosting::run`
/// binds the port it holds, and a request arriving on that port wears that
/// brand whatever hostname it carries. Nothing here is reachable from the
/// public internet, because nothing binds the port unless an operator sets
/// the variable.
async fn preview(key: BrandKey, path: &str) -> (StatusCode, String) {
    let state = AppState {
        canonical_host: CanonicalHost::new(None)
            .with_local_ports([(PREVIEW_PORT, key)].into_iter().collect()),
        ..portal::test_support::app_state(store::test_support::mem_surreal().await).await
    };
    fetch(state, path, &format!("localhost:{PREVIEW_PORT}")).await
}

/// Fetch `path` as a public visitor on `host`, with the launch gate in force
/// and no local port bound.
async fn public_request(host: &str, path: &str) -> (StatusCode, String) {
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    fetch(state, path, host).await
}

async fn fetch(state: AppState, path: &str, host: &str) -> (StatusCode, String) {
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
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
async fn each_unlaunched_practice_serves_a_coming_soon_notice_in_its_own_brand() {
    for (key, _, site_name) in UNLAUNCHED {
        let (status, html) = preview(*key, "/").await;
        assert_eq!(status, StatusCode::OK, "{key:?}");

        assert!(html.contains("Coming Soon"), "{key:?} says Coming Soon");
        assert!(html.contains("holding-page"), "{key:?} is the bare variant");
        // Matched on the text rather than on `<title>…</title>`: this is SSR
        // output and Dioxus interleaves hydration markers inside the element.
        assert!(
            html.contains(&format!("{site_name} | Coming Soon")),
            "{key:?} titles itself {site_name}, got {:?}",
            title_of(&html)
        );

        // The brand's own font and colour: the per-key tokens sheet carries
        // both `--nav-font-family` and the palette. A stub in the wrong
        // brand's skin is the failure this catches.
        assert!(
            html.contains(&format!("/public/css/brand-{}-tokens.css", key.as_str())),
            "{key:?} loads its own tokens, not another brand's"
        );
    }
}

/// A holding page for a law practice is still attorney advertising, so the
/// shared footer and its disclaimer stay under the notice.
#[tokio::test]
async fn the_notice_keeps_the_attorney_advertisement_disclaimer() {
    for (key, _, _) in UNLAUNCHED {
        let (status, html) = preview(*key, "/").await;
        assert_eq!(status, StatusCode::OK, "{key:?}");
        assert!(html.contains("site-footer"), "{key:?} carries the footer");
        assert!(
            html.contains("Attorney advertisement"),
            "{key:?} carries the disclaimer"
        );
        assert!(
            html.contains("Shook Law PLLC"),
            "{key:?} names the firm that would be retained"
        );
    }
}

/// One landing page and nothing under it.
///
/// Read through the preview door deliberately: on the public host every path
/// `404`s because the *gate* refuses the host, which would make this pass
/// without `publishes_firm_path` being involved at all.
#[tokio::test]
async fn nothing_but_the_landing_page_answers_for_an_unlaunched_practice() {
    for (key, _, _) in UNLAUNCHED {
        for path in ["/services", "/contact"] {
            let (status, _) = preview(*key, path).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{key:?} {path} must not answer before launch"
            );
        }
    }
}

/// The stub must not leak the held-out marketing copy it sits in front of.
#[tokio::test]
async fn the_notice_does_not_publish_the_unlaunched_offer() {
    let (_, summons) = preview(BrandKey::Summons, "/").await;
    assert!(
        !summons.contains("home-practice"),
        "the held-out stub lists no sibling practice cards"
    );
}

/// Authoring the holding page does not publish it.
///
/// The held-out hostname is refused outright — no notice, no brand, no
/// redirect. Launching a key belongs in the change that makes its content
/// reachable.
#[tokio::test]
async fn the_launch_gate_still_refuses_every_one_of_these_hosts() {
    for (key, host, site_name) in UNLAUNCHED {
        assert!(!key.is_live(), "{key:?} is held out of the approved set");
        for path in ["/", "/services", "/contact"] {
            let (status, body) = public_request(host, path).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{host}{path} is not public yet"
            );
            assert!(
                !body.contains("Coming Soon"),
                "{host}{path} does not serve the notice to the public yet"
            );
            assert!(
                !body.contains(&format!("{site_name} | Coming Soon")),
                "{host}{path} must not wear {site_name}"
            );
        }
    }
}
