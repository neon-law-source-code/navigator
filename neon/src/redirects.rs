//! Every retired URL the site still answers, kept alive as permanent
//! redirects.
//!
//! Two consolidations left backlinks behind, and this module is where both
//! land. The Foundation's pages served at the site root while it had a host of
//! its own, so `/mission`, `/notations`, and `/transparency…`,
//! and its surviving audience pages now `301` beneath `/foundation`. Workshops
//! and presentations have only their canonical top-level homes.
//!
//! `/legal-aid` is deliberately absent: the page it pointed at is retired, and a
//! `301` into a `404` is worse for a backlink than the `404` itself — it costs
//! the reader a round trip to reach the same dead end.
//!
//! Every entry here is a `301` or a redirect handler. The pages themselves are
//! Dioxus routers in [`crate::pages`] and [`crate::firm_pages`]; a site's
//! public surface is both halves together.
//!
//! One host serves everything now, so every destination is relative. While the
//! firm and the Foundation were separate deployments a redirect that crossed
//! between them had to be absolute; that seam is gone, and with it the whole
//! class of bug where a relative hop landed on the other host's `404`.

use portal::hosting::PublicRouter as Router;
use portal::{dioxus_app, AppState};

use axum::extract::Path as AxumPath;
use axum::routing::get;

/// A `GET` route answering one retired path with a permanent redirect to
/// `destination`.
fn moved(from: &str, destination: &'static str) -> Router<AppState> {
    Router::new().route(
        from,
        get(move || async move { axum::response::Redirect::permanent(destination) }),
    )
}

/// Build the retired-path table for the Foundation's former root URLs.
///
/// The `presentations` certificate `POST` is the one write on this surface and
/// is deliberately absent: it stays the application's, merged in by
/// [`crate::public_routes`] from `portal::presentation_command_routes`,
/// because who may claim a certificate is an authorization question rather
/// than a brand's.
pub fn retired_path_routes() -> Router<AppState> {
    foundation_root_redirects()
}

/// The Foundation's former root URLs, each `301`ing to its `/foundation`
/// replacement.
///
/// These were live pages on `neonlaw.org` for as long as the Foundation had a
/// host of its own, so they are the most-linked retired URLs on the site.
/// `/foundation` itself is deliberately absent: it is a real page now, not a
/// redirect, which is the whole point of the consolidation.
fn foundation_root_redirects() -> Router<AppState> {
    moved("/mission", dioxus_app::FOUNDATION_MISSION_PATH)
        .merge(moved("/education", dioxus_app::FOUNDATION_EDUCATION_PATH))
        .merge(moved("/attorneys", dioxus_app::FOUNDATION_ATTORNEYS_PATH))
        .merge(moved("/notations", dioxus_app::NOTATIONS_PATH))
        .merge(moved("/transparency", dioxus_app::TRANSPARENCY_PATH))
        // The minutes prefix is registered before the bare `{slug}` so a
        // quarter key can never be read as a governance slug.
        .merge(Router::new().route(
            "/transparency/minutes/{slug}",
            get(|AxumPath(slug): AxumPath<String>| async move {
                axum::response::Redirect::permanent(&format!(
                    "/foundation/transparency/minutes/{slug}"
                ))
            }),
        ))
        .merge(Router::new().route(
            "/transparency/{slug}",
            get(|AxumPath(slug): AxumPath<String>| async move {
                axum::response::Redirect::permanent(&format!("/foundation/transparency/{slug}"))
            }),
        ))
}
