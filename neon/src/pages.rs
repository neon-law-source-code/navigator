//! The Foundation's public and gated Dioxus SSR pages, and the content each
//! one renders.
//!
//! Two populations live here. The anonymous set — the marketing home and the
//! two audience pages — is what the Foundation presents to
//! the world. The gated set carries the application's own session and policy
//! layers through [`portal::gated`], so a brand declares *that* a page is
//! gated while the application still decides what gating means.

use portal::hosting::PublicRouter as Router;
use portal::{dioxus_app, git_meta, AppState, DocCategory, MarketingIndex};

use crate::copy as foundation_copy;

/// `GET /notations` — the notations page: a Foundation-brand
/// story about what a notation is, above the notation tree README.
/// Resolve the mission letter for the Dioxus router: the body and the
/// git-derived freshness date.
///
/// The letter lives with the other marketing fragments under
/// `server/content/marketing/`; `portal`'s `CARGO_MANIFEST_DIR` is `…/portal`, so
/// it resolves relative to it. The "last edited" line is `None` in production,
/// where distroless carries no git binary.
fn mission_content(marketing: &MarketingIndex) -> webapp::mission::MissionContent {
    let source_file = "../server/content/marketing/mission.md";
    let doc = marketing.find("mission");
    let last_edited = git_meta::last_touched(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(source_file),
    )
    .map(|date| date.format("%B %-d, %Y").to_string());
    webapp::mission::MissionContent {
        title: "Mission".to_string(),
        description: doc.map_or_else(
            || views::brand::mission_description().to_string(),
            |d| d.description.clone(),
        ),
        body_html: doc.map_or_else(
            || {
                "<p>Mission copy is loaded from <code>web/content/marketing/mission.md</code>.</p>"
                    .to_string()
            },
            |d| d.body_html.clone(),
        ),
        last_edited,
    }
}

/// The Foundation host's public Dioxus SSR pages, as raw routers for
/// [`bootstrap`]'s `host_dioxus` argument — the Dioxus half of the Foundation's
/// public surface, whose half is [`foundation_public_routes`]. A host's
/// public surface is these two halves together; a composition that assembles
/// only one serves nothing on the other's paths.
///
/// These mount *outside* [`session_boundary`], because they are the brand's
/// anonymous surface: the marketing pages that explain what the Foundation
/// does, and the talks it gives. Every other Foundation page sits behind the
/// boundary in [`foundation_gated_dioxus_routers`].
///
/// `/` is the marketing home. It replaces the static
/// `marketing/neon-law-foundation` site, whose four pages this surface
/// absorbs (ENG-139), and it is what a stranger arriving at `neonlaw.org`
/// reads first. The talks catalog is no longer here: `/presentations` and
/// every talk beneath it moved to the firm's host, so this surface is the
/// marketing home and its two audience pages.
#[must_use]
pub fn foundation_public_dioxus_routers(_state: &AppState) -> Vec<Router> {
    vec![
        // The Foundation's front door: what it does, for whom, and how to
        // start. The brand mark links here from every page.
        dioxus_app::foundation_home_router(dioxus_app::MISSION_PATH, foundation_copy::home()),
        // The audience pages the retired static site published.
        dioxus_app::marketing_page_router(
            dioxus_app::FOUNDATION_EDUCATION_PATH,
            foundation_copy::education(),
        ),
        dioxus_app::marketing_page_router(
            dioxus_app::FOUNDATION_ATTORNEYS_PATH,
            foundation_copy::attorneys(),
        ),
    ]
}

/// The Foundation pages that read only for a signed-in visitor.
///
/// The Foundation gates the mission letter, Notations, and transparency disclosures.
/// [`bootstrap`] merges each through [`session_boundary`], so an anonymous
/// reader gets the login door rather than a `404`, and the embedded policy each
/// router carries decides the rest — `foundation_reading_surface` in
/// `navigator.rego` opens these to any authenticated person. The workshop and
/// presentation catalogs are not here: both mount anonymously from
/// [`crate::firm_public_dioxus_routers`].
///
/// The nav still names them ([`views::brand`]), which is what keeps a gated
/// page discoverable rather than invisible.
#[must_use]
pub fn foundation_gated_dioxus_routers(state: &AppState) -> Vec<Router> {
    // The gate is the application's, not the brand's: `portal::gated` carries
    // the same stack `session_boundary` applies, so these routers can ride the
    // brand's `public_dioxus` slot without this crate composing an
    // authorization layer of its own.
    let gate = |router: Router| -> Router { portal::gated(state, router) };
    [
        dioxus_app::mission_router(
            dioxus_app::FOUNDATION_MISSION_PATH,
            mission_content(&state.marketing),
        ),
        dioxus_app::notations_router(),
        dioxus_app::transparency_index_router(state.transparency.clone()),
        dioxus_app::transparency_doc_router(
            dioxus_app::TRANSPARENCY_DOC_PATH,
            DocCategory::Governance,
            state.transparency.clone(),
        ),
        dioxus_app::transparency_doc_router(
            dioxus_app::TRANSPARENCY_MINUTES_PATH,
            DocCategory::Minutes,
            state.transparency.clone(),
        ),
    ]
    .into_iter()
    .map(gate)
    .collect()
}
