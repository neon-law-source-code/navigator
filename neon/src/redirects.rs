//! Every retired URL the site still answers, and what it answers with.
//!
//! Two kinds of answer, and which one a path gets is decided by whether the
//! firm publishes an equivalent page.
//!
//! - A **`301`** goes to a path whose content moved. A redirect is a promise
//!   that what the reader wanted is at the other end of it.
//! - A **`410 Gone`** goes to a path whose content is retired outright. The
//!   Neon Law Foundation's public surface is that case: the firm publishes no
//!   mission letter, no CLE curriculum, no volunteer-attorney page, and no
//!   nonprofit transparency disclosures, so there is nowhere honest to send a
//!   reader who asks for one.
//!
//! `410` rather than `404`, and rather than a `301` to the site root. A `404`
//! says "we have no idea what this is", which invites a crawler to keep asking
//! and a reader to assume they mistyped; a `301` to `/` says "this is the
//! homepage now", which is false and costs the reader a round trip to find out.
//! `410` says the thing existed and has been withdrawn, which is what happened,
//! and it is the one answer a search engine treats as a signal to drop the URL.
//!
//! Every entry here is a route handler. The pages themselves are Dioxus routers
//! in [`crate::firm_pages`]; a site's public surface is both halves together.

use portal::hosting::PublicRouter as Router;
use portal::AppState;

use axum::http::StatusCode;
use axum::routing::get;

/// A `GET` route answering one retired path with `410 Gone`.
fn gone(path: &str) -> Router<AppState> {
    Router::new().route(path, get(|| async { StatusCode::GONE }))
}

/// The retired-path table: every URL this site answers but no longer publishes.
///
/// The `presentations` certificate `POST` is the one write on the public
/// surface and is deliberately absent: it stays the application's, merged in by
/// [`crate::public_routes`] from `portal::presentation_command_routes`, because
/// who may claim a certificate is an authorization question rather than a
/// brand's.
pub fn retired_path_routes() -> Router<AppState> {
    foundation_gone_routes()
}

/// The Neon Law Foundation's retired URLs, at both the prefix they last held
/// and the site root they held before it.
///
/// Both generations are here because both were live. The pages sat at the site
/// root for as long as the Foundation had a host of its own; they moved beneath
/// `/foundation` when one binary took over both faces. A backlink from either
/// era reaches the same answer.
///
/// The `{slug}` families are registered as patterns rather than enumerated:
/// each transparency document had its own URL, and none of them is published
/// now, so the whole shape is gone rather than some listed subset of it. The
/// minutes prefix is registered before the bare `{slug}` for the same reason it
/// always was — a quarter key must never be read as a governance slug.
fn foundation_gone_routes() -> Router<AppState> {
    [
        "/foundation",
        "/foundation/mission",
        "/foundation/education",
        "/foundation/attorneys",
        "/foundation/notations",
        "/foundation/transparency",
        "/foundation/transparency/minutes/{slug}",
        "/foundation/transparency/{slug}",
        "/mission",
        "/education",
        "/attorneys",
        "/notations",
        "/transparency",
        "/transparency/minutes/{slug}",
        "/transparency/{slug}",
    ]
    .into_iter()
    .fold(Router::new(), |router, path| router.merge(gone(path)))
}
