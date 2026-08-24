//! The white-label tenant shape: the application with no public face.
//!
//! A tenant deploy is a firm that runs Navigator behind its own marketing site.
//! It publishes none of the firm's public-facing pages and answers its bare
//! host with a redirect into the portal, because the only thing it serves is
//! the application.
//!
//! This is what `--site app` selected before #974 gave every face its own
//! binary. It lights [`crate::PortalOnly`] rather than inventing a second way to
//! express "no public surface": a mounted brand manifest can already enable that
//! mode with `portal_only: true`, and both routes into the behavior converge
//! here.
//!
//! Unlike a brand crate, this lives inside the application crate. A
//! tenant has no brand of its own to compose — that is the entire point — so
//! there is nothing for a thin crate to hold.

use axum::response::Redirect;
use axum::routing::get;

use crate::hosting::{Brand, BrandSeed, PublicRouter};
use crate::AppState;

/// The tenant's whole public surface: a redirect from `/` into the app.
///
/// The tenant's own marketing site owns the public web; serving a competing
/// home page from Navigator would put two front doors on one domain. Everything
/// else 404s by construction, since nothing else is registered.
///
/// The target is `/app/projects`, the one authenticated surface every tier may
/// reach: an anonymous visitor is bounced to sign-in, and the render adapts to
/// whoever lands (a client sees their matters, a firm tier the workbench).
pub fn public_routes() -> PublicRouter<AppState> {
    PublicRouter::new().route("/", get(|| async { Redirect::to("/app/projects") }))
}

/// The tenant brand: what the `tenant` binary hands to the shared run loop.
///
/// `service_name` is a default, not a fixed identity — a white-label deploy is
/// one of many, so it sets `OTEL_SERVICE_NAME` to name itself and
/// `telemetry::init` prefers that over this value.
#[must_use]
pub fn brand() -> Brand {
    Brand {
        key: "tenant",
        seed: BrandSeed::Tenant,
        service_name: "tenant-server",
        portal_only: true,
        public_routes: public_routes(),
        public_paths: &["/"],
        public_dioxus: Box::new(|_| Vec::new()),
    }
}
