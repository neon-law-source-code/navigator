//! `/immigration` — Neon's own gateway page to Abhaya Immigration.
//!
//! Neon-only: the same path 404s on every other registered brand host, the
//! same mechanism that already gates `/delete-your-debt`, `/delete-your-data`,
//! `/business`, `/services`, and `/disputes` off a house-brand host. See
//! `neon::firm_copy::immigration_gateway` for the content and
//! `views::brand::BrandKey::publishes_firm_path` for the gate.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use tower::ServiceExt;

async fn state() -> portal::AppState {
    portal::test_support::app_state(store::test_support::mem_surreal().await).await
}

async fn state_for_deployment(deployment_host: &str) -> portal::AppState {
    let mut state = state().await;
    state.canonical_host = portal::CanonicalHost::new(Some(deployment_host.to_string()));
    state
}

fn app(state: portal::AppState) -> Router {
    let host_dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        host_dioxus,
    )
    .expect("Neon Law public routes must not collide with Navigator")
}

async fn get_on_host(app: &Router, path: &str, host: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().uri(path);
    if let Some(host) = host {
        builder = builder.header("host", host);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = String::from_utf8(
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

/// `GET /immigration` on a Neon host returns 200, wears Neon chrome, and
/// carries the approved original copy — the destination named factually,
/// the USCIS-fee/no-approval-promise clarifier, and a destination-named CTA
/// whose href resolves to this deployment's own Abhaya Immigration host.
#[tokio::test]
async fn the_gateway_renders_on_neon_hosts_with_neon_chrome_and_the_approved_copy() {
    let app = app(state().await);

    for host in [None, Some("www.neonlaw.com")] {
        let (status, body) = get_on_host(&app, "/immigration", host).await;
        assert_eq!(status, StatusCode::OK, "{host:?}: {body}");
        for expected in [
            "<title>Neon Law | Immigration</title>",
            r#"content="Immigration | Neon Law" property="og:title""#,
            "Immigration help, through Abhaya.",
            "Abhaya Immigration",
            "Shook Law PLLC",
            "cannot promise",
            "Visit Abhaya Immigration",
            "site-footer",
            "/public/css/brand-neon-tokens.css",
        ] {
            assert!(body.contains(expected), "{host:?} is missing {expected:?}");
        }
        assert!(
            body.contains(r#"href="https://www.abhayaimmigration.com""#),
            "{host:?}: the CTA does not open the production Abhaya Immigration destination: {body}"
        );
    }
}

/// A staging deployment resolves the CTA to the staging sibling.
#[tokio::test]
async fn a_staging_deployment_resolves_the_cta_to_the_staging_sibling() {
    let app = app(state_for_deployment("staging.neonlaw.com").await);
    let (status, body) = get_on_host(&app, "/immigration", Some("www.neonlaw.com")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(r#"href="https://staging.abhayaimmigration.com""#),
        "the CTA must resolve to the staging sibling on a staging deployment: {body}"
    );
}

/// The same path is a `404` on every other registered brand host —
/// including Abhaya Immigration's own — because only Neon's
/// `publishes_firm_path` admits it.
#[tokio::test]
async fn the_gateway_is_404_on_every_non_neon_brand_host() {
    let app = app(state().await);
    for host in [
        "www.deleteyourdata.com",
        "www.lawyershook.com",
        "www.vestaestateplanning.com",
        "www.misericordialaw.com",
        "www.abhayaimmigration.com",
        "staging.abhayaimmigration.com",
        "www.deleteyourdebt.com",
        "www.summonsdefense.nyc",
        "www.daybridgedivorce.com",
    ] {
        let (status, body) = get_on_host(&app, "/immigration", Some(host)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{host} must not serve the Neon-only gateway: {body}"
        );
    }
}
