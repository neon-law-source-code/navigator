//! Navigator's cross-cutting HTTP integration suite.
//!
//! This crate builds no binary. Its ~42,000 lines of `tests/` drive the whole
//! application through the brand crates' composed routers, which is the only
//! reason it still exists as a package: Cargo needs a library target to hang
//! an integration-test directory on. The two constructors below are the
//! composition those tests share.
//!
//! The `server` binary it is named for is gone. `--site` selected a public face
//! at runtime; one brand binary — `neon` — now serves
//! exactly one, so the binary is the site and no flag can point a deployment at
//! another entity's face (#974).
//!
//! The name is therefore a misnomer, and moving this suite into `portal` or
//! renaming the package would be the honest follow-up. Neither is done here:
//! relocating 42,000 lines of tests forces every open branch that touches them
//! to rebase across the move, and active branches still touch this suite.

use std::path::Path;

use axum::Router;
use portal::AppState;

/// The router the `neon` binary serves: the Navigator application under the
/// public face.
///
/// Most tests in this suite exercise the *application* — `/app`, `/app/api`, the
/// auth surfaces — and not the face in front of it. They need some brand
/// composed to have a router at all, and the site's is the one they
/// take.
///
/// Composed through `neon`'s own entry points rather than restated here, so a
/// test cannot pass against a surface the binary does not serve. That is the
/// same reason `host_legal_pages.rs` and `firm_routes.rs` compose their hosts
/// this way.
///
/// # Panics
///
/// Panics when the site's declared paths collide with a Navigator-owned
/// prefix, which is a composition bug rather than a test condition.
pub fn neon_router(state: AppState, public_dir: &Path) -> Router {
    let dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        public_dir,
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        dioxus,
    )
    .expect("the public host must not claim Navigator-owned routes")
}

/// The router the `tenant` binary serves: the application with no public face
/// at all.
///
/// A white-label deploy publishes none of the first-party brands' pages and
/// answers its bare host with a redirect into the portal. Tests that set
/// `portal_only` use this rather than [`neon_router`], because a portal-only
/// deploy's whole point is that the marketing surface is absent.
///
/// # Panics
///
/// Panics when the tenant root redirect collides with a Navigator-owned
/// prefix.
pub fn tenant_router(state: AppState, public_dir: &Path) -> Router {
    portal::bootstrap(
        state,
        public_dir,
        portal::tenant::public_routes(),
        &["/"],
        Vec::new(),
    )
    .expect("the tenant root redirect does not collide with Navigator")
}
