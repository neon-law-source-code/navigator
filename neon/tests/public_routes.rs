//! What the site's retired-path table answers.
//!
//! The table is only half of the public surface — the pages themselves are
//! Dioxus routers in `neon::firm_pages`. This file pins the half that is a
//! table: every URL that was live before the Neon Law Foundation's public
//! surface was retired, and now answers `410 Gone`. The two halves together are
//! covered against the real composition in `server/tests/routes.rs`.
//!
//! `410` rather than `404` is the whole point of keeping the table, so these
//! assert the status rather than merely that something answers. A retired URL
//! that fell out of the table would 404 like a typo, which tells a crawler to
//! keep asking and tells a reader they mistyped.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// Every URL the retired-path table answers, at both generations of the
/// nonprofit's public surface: the `/foundation` prefix it last held, and the
/// site root it held while it had a host of its own.
const RETIRED_PATHS: &[&str] = &[
    "/foundation",
    "/foundation/mission",
    "/foundation/education",
    "/foundation/attorneys",
    "/foundation/notations",
    "/foundation/transparency",
    "/foundation/transparency/bylaws",
    "/foundation/transparency/minutes/2026-q2",
    "/mission",
    "/education",
    "/attorneys",
    "/notations",
    "/transparency",
    "/transparency/bylaws",
    "/transparency/minutes/2026-q2",
];

async fn state() -> portal::AppState {
    portal::test_support::app_state(mem_surreal().await).await
}

async fn anonymous_get(app: &axum::Router, path: &str) -> Response<Body> {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// Every retired URL answers `410 Gone`, and none of them redirects.
///
/// A `301` here would be the wrong answer twice over: there is no firm page
/// that carries a nonprofit's mission letter or its governance disclosures, so
/// any destination would be a promise the other end cannot keep, and a hop into
/// a page about something else costs the reader a round trip to find out.
#[tokio::test]
async fn every_retired_foundation_url_answers_gone() {
    let app = neon::retired_path_routes().with_state(state().await);

    for path in RETIRED_PATHS {
        let response = anonymous_get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::GONE,
            "{path} must answer 410 Gone"
        );
        assert!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .is_none(),
            "{path} must not redirect: a retired page has no successor"
        );
    }
}

/// A retired URL answers `410` without a session, which is what a crawler and a
/// stale backlink both are.
///
/// Two of these paths were gated before they were retired — the mission letter
/// and the transparency documents read only for a signed-in visitor. A gate
/// left behind on a retired path would answer a `303` to the login door, which
/// sends a search engine at a sign-in form instead of dropping the URL.
#[tokio::test]
async fn a_retired_gated_url_answers_gone_rather_than_a_login_redirect() {
    let app = neon::retired_path_routes().with_state(state().await);

    for path in [
        "/foundation/mission",
        "/foundation/transparency",
        "/mission",
    ] {
        let response = anonymous_get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::GONE,
            "{path} answers gone rather than bouncing an anonymous reader to login"
        );
    }
}

/// The retired-path table owns retired URLs and nothing else. A live page that
/// appeared here would shadow the Dioxus router that actually renders it, and
/// the visitor would get `410 Gone` on a page the site publishes.
#[tokio::test]
async fn the_retired_path_table_owns_no_live_page() {
    let app = neon::retired_path_routes().with_state(state().await);

    for path in [
        "/",
        "/services",
        "/litigation",
        "/blog",
        "/contact",
        "/navigator",
        "/fractional-cto",
        "/fractional-gc",
        "/workshops",
        "/presentations",
    ] {
        assert_eq!(
            anonymous_get(&app, path).await.status(),
            StatusCode::NOT_FOUND,
            "{path} is a live page, so the retired-path table must not own it"
        );
    }
}

/// Every retired path is declared in `PUBLIC_PATHS`.
///
/// The declaration is what `portal::bootstrap` checks against the reserved
/// prefixes, and it is what a reader of the crate sees as the site's whole
/// surface. A path answered by the table but missing from the table of
/// declarations is a route nothing describes.
#[test]
fn every_retired_path_is_declared() {
    for path in RETIRED_PATHS {
        let declared = neon::PUBLIC_PATHS.iter().any(|candidate| {
            *candidate == *path
                || (candidate.contains('{')
                    && path.starts_with(candidate.split('{').next().unwrap_or_default()))
        });
        assert!(declared, "{path} is answered but not declared");
    }
}

/// No retired path is advertised to a crawler.
///
/// A `410` is an answer, not a document. A sitemap entry pointing at one is
/// worse than no entry: it invites the crawl that the status code exists to
/// stop.
#[tokio::test]
async fn no_retired_path_reaches_the_sitemap() {
    let state = state().await;
    let sitemap = neon::sitemap_paths(&state);

    for path in RETIRED_PATHS {
        assert!(
            !sitemap.contains(*path),
            "{path} is retired and must not be advertised in the sitemap"
        );
    }
}
