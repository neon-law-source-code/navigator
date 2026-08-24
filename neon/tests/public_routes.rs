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

/// Every URL from an earlier generation of the firm's own site, and the page
/// that carries what the reader came for now.
///
/// The content behind each of these is still published — only the path changed
/// — which is what separates them from the `410` half above and what makes a
/// `301` an honest answer rather than a guess.
const SUPERSEDED_PATHS: &[(&str, &str)] = &[
    ("/services/litigation", "/litigation"),
    ("/for-lawyers", "/fractional-cto"),
    ("/support", "/contact"),
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

/// Every superseded firm URL answers `301` to its successor.
///
/// `301` specifically, not merely "a redirect": it is the status a search
/// engine treats as permanent and follows when consolidating a stale result
/// onto the live page, which is the whole reason these routes exist. A `302`
/// or a `303` here would keep the dead URL in the index.
#[tokio::test]
async fn every_superseded_firm_url_answers_a_permanent_redirect() {
    let app = neon::retired_path_routes().with_state(state().await);

    for (path, target) in SUPERSEDED_PATHS {
        let response = anonymous_get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY,
            "{path} must answer 301 Moved Permanently"
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(*target),
            "{path} must point at {target}"
        );
    }
}

/// Every redirect target is a page the site actually publishes.
///
/// A redirect is a promise that what the reader wanted is at the other end of
/// it. A target that 404s or 410s breaks the promise twice: the reader spends a
/// round trip to find out, and the crawler consolidates a stale result onto a
/// dead page. So the targets are checked against the live path table rather
/// than merely spelled correctly.
#[test]
fn every_redirect_target_is_a_published_page() {
    for (path, target) in SUPERSEDED_PATHS {
        assert!(
            neon::PUBLIC_PATHS.contains(target),
            "{path} redirects to {target}, which the site does not declare"
        );
        assert!(
            !RETIRED_PATHS.contains(target),
            "{path} redirects to {target}, which is retired and answers 410"
        );
    }
}

/// A superseded URL redirects without a session, which is what a crawler and a
/// stale search result both are.
#[tokio::test]
async fn a_superseded_url_redirects_rather_than_asking_for_a_login() {
    let app = neon::retired_path_routes().with_state(state().await);

    for (path, _) in SUPERSEDED_PATHS {
        let status = anonymous_get(&app, path).await.status();
        assert_eq!(
            status,
            StatusCode::MOVED_PERMANENTLY,
            "{path} must redirect anonymously, not answer {status}"
        );
    }
}

/// No superseded path is advertised to a crawler.
///
/// The sitemap names pages, and a redirect is not a page. Advertising the old
/// URL would ask a crawler to index the hop instead of its destination, which
/// the destination is already advertised as in its own right.
#[tokio::test]
async fn no_superseded_path_reaches_the_sitemap() {
    let state = state().await;
    let sitemap = neon::sitemap_paths(&state);

    for (path, _) in SUPERSEDED_PATHS {
        assert!(
            !sitemap.contains(*path),
            "{path} is superseded and must not be advertised in the sitemap"
        );
    }
}

/// Every superseded path is declared in `PUBLIC_PATHS`, for the same reason
/// every retired one is: a route nothing describes is a route nobody maintains.
#[test]
fn every_superseded_path_is_declared() {
    for (path, _) in SUPERSEDED_PATHS {
        assert!(
            neon::PUBLIC_PATHS.contains(path),
            "{path} is answered but not declared"
        );
    }
}

/// `/mission` stays `410` and is never redirected.
///
/// It is a Foundation URL, and the firm publishes no mission letter to send a
/// reader to. This is the boundary between the two halves of the table, and it
/// is asserted rather than left to the reader of the module doc because the
/// tempting wrong fix — "it is in search results, so redirect it" — is exactly
/// what `every_retired_foundation_url_answers_gone` already forbids.
#[tokio::test]
async fn the_mission_url_is_gone_rather_than_redirected() {
    let app = neon::retired_path_routes().with_state(state().await);
    let response = anonymous_get(&app, "/mission").await;

    assert_eq!(response.status(), StatusCode::GONE);
    assert!(
        !SUPERSEDED_PATHS.iter().any(|(path, _)| *path == "/mission"),
        "/mission has no successor and must not be given one"
    );
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
