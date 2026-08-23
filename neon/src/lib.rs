//! `neonlaw.com` — Neon Law's public face, over the mounted Navigator
//! application.
//!
//! One crate, one binary, one site, one brand. Neon Law is the practice of
//! Shook Law PLLC, and every page this crate serves is the firm's.
//!
//! This crate owns the public surface outright: its marketing copy, its page
//! compositions, its path table, and the answers it still owes the URLs it no
//! longer publishes. `portal` owns the authenticated application underneath.
//!
//! The composition lives here rather than in `main` so the binary and the tests
//! that exercise its router are the same expression. A test that restated the
//! composition would pass while the deployed binary served something else,
//! which is exactly how a public surface goes quietly wrong.

// The public face is these three modules; the crate exports only the
// composition entry points below, so the site's copy is not API.
mod firm_copy;
mod firm_pages;
mod redirects;

use portal::hosting::{Brand, BrandSeed, PublicRouter};
use portal::AppState;

pub use firm_pages::firm_public_dioxus_routers;
pub use redirects::retired_path_routes;

/// Every path this site registers, public and gated alike.
///
/// A *declaration*, not an access rule: [`portal::bootstrap`] checks it against
/// `portal::RESERVED_PATH_PREFIXES` so it can never shadow a Navigator-owned
/// surface. Access is decided by the route layers and the embedded policy.
///
/// One population now: the firm's pages hold the root. The retired URLs trail
/// the table, answered by [`retired_path_routes`] rather than rendered.
pub const PUBLIC_PATHS: &[&str] = &[
    // --- The firm ---------------------------------------------------------
    "/",
    "/fractional-cto",
    "/services",
    "/litigation",
    "/fractional-gc",
    "/navigator",
    "/contact",
    "/blog",
    "/blog/{slug}",
    // The talks catalog and every talk beneath it. Anonymous like the rest of
    // this table: a talk is published to be read.
    "/presentations",
    "/presentations/{slug}",
    "/presentations/{slug}/slides",
    "/presentations/{slug}/step/{step}",
    "/presentations/{slug}/display/{step}",
    "/presentations/{slug}/certificate",
    "/presentations/{slug}/certificate/sent",
    // The public Navigator workshops.
    "/workshops",
    "/workshops/{slug}",
    "/workshops/{slug}/slides",
    "/workshops/{slug}/step/{step}",
    "/workshops/{slug}/display/{step}",
    "/workshops/{slug}/certificate",
    "/workshops/{slug}/certificate/sent",
    // --- Retired URLs, answered as `410 Gone` ------------------------------
    // The Neon Law Foundation's pages, at both the prefix they last held and
    // the site root they held before it. Every one is registered so the site
    // answers a backlink with "this is gone" rather than with the generic
    // `404` it gives a URL that never existed. See [`retired_path_routes`].
    "/foundation",
    "/foundation/mission",
    "/foundation/education",
    "/foundation/attorneys",
    "/foundation/notations",
    "/foundation/transparency",
    "/foundation/transparency/{slug}",
    "/foundation/transparency/minutes/{slug}",
    "/mission",
    "/education",
    "/attorneys",
    "/notations",
    "/transparency",
    "/transparency/{slug}",
    "/transparency/minutes/{slug}",
    // --- Shared -----------------------------------------------------------
    "/privacy",
    "/terms",
    "/robots.txt",
    "/sitemap.xml",
    "/llms.txt",
];

/// The site's crawlable pages: the firm's marketing surface, `/blog/{slug}`
/// expanded over the posts loaded at boot, the talks catalog expanded over the
/// `presentations` materials.
///
/// Derived from [`PUBLIC_PATHS`] but not equal to it. That table declares
/// everything the site registers, including gated pages, the crawler documents
/// `portal` adds for itself, and the `{slug}` patterns a crawler cannot follow;
/// this is the subset a stranger can actually read, at concrete URLs.
///
/// A retired URL is deliberately absent: a `410` is an answer, not a document,
/// and a sitemap entry pointing at one is worse than no entry at all.
///
/// A talk's projector face (`/display/{step}`) and its certificate confirmation
/// are left out for the same reason a crawler is not sent to a print dialog:
/// they are states of a session, not documents.
#[must_use]
pub fn sitemap_paths(state: &AppState) -> std::collections::BTreeSet<String> {
    let mut paths: std::collections::BTreeSet<String> = [
        "/",
        "/fractional-cto",
        "/services",
        "/litigation",
        "/fractional-gc",
        "/navigator",
        "/contact",
        "/blog",
        "/workshops",
        "/presentations",
    ]
    .iter()
    .map(|path| (*path).to_string())
    .collect();
    for post in state.blog.posts() {
        paths.insert(format!("/blog/{}", post.slug));
    }
    for material in state
        .workshops
        .materials()
        .iter()
        .filter(|material| matches!(material.category.as_str(), "presentations" | "workshops"))
    {
        let path = format!("/{}/{}", material.category, material.slug);
        // The hub, its raw-Markdown twin, the light table, and the classroom
        // step faces.
        paths.insert(path.clone());
        paths.insert(format!("{path}.md"));
        paths.insert(format!("{path}/slides"));
        for step in 1..=material.sections.len() {
            paths.insert(format!("{path}/step/{step}"));
        }
    }
    paths
}

/// The site's `/llms.txt`: what a crawler has reached at `neonlaw.com`, and the
/// pages it may read there.
///
/// The firm's practice and its published fee schedule, in the order a reader
/// meets them. Every entry is a page this host serves anonymously; the
/// individual posts a crawler walks from `/blog` are enumerated by
/// [`sitemap_paths`] rather than curated here.
///
/// `/services` is named as the fee schedule it is. The firm charges a fixed fee
/// per matter, which is the thing that page exists to say — an index describing
/// it as generic "legal services" would understate it.
#[must_use]
pub fn llms_txt(state: &AppState) -> portal::LlmsTxt {
    let mark = views::brand::FIRM_BRAND.site_name;
    portal::LlmsTxt {
        title: mark.to_string(),
        summary: format!(
            "{mark} is a consumer law firm working on flat fees: wills, trusts, name changes, \
             formations, and the other routine matters a person actually walks in with, \
             alongside a litigation and company-counsel practice quoted per engagement."
        ),
        pages: indexed_pages(mark),
        sections: [
            ("Workshop Corpus", "workshops"),
            ("Presentation Corpus", "presentations"),
        ]
        .into_iter()
        .map(|(heading, category)| portal::LlmsTxtSection {
            heading: heading.to_string(),
            links: state
                .workshops
                .materials()
                .iter()
                .filter(|material| material.category == category)
                .map(|material| portal::LlmsTxtLink {
                    title: material.title.clone(),
                    path: format!("/{}/{}.md", material.category, material.slug),
                    description: material.description.clone(),
                })
                .collect(),
        })
        .collect(),
    }
}

/// The pages `/llms.txt` lists, in the order a reader meets them: the firm's
/// practice, then its fee schedule, then everything it publishes beside them.
fn indexed_pages(mark: &str) -> Vec<portal::LlmsTxtLink> {
    let page = |title: &str, path: &str, description: &str| portal::LlmsTxtLink {
        title: title.to_string(),
        path: path.to_string(),
        description: description.to_string(),
    };
    vec![
        page(
            mark,
            "/",
            "The firm's practice — flat-fee consumer legal work with every price published, \
                 litigation on both sides of the v., and company counsel for emerging technology \
                 companies.",
        ),
        page(
            "Fractional CTO",
            "/fractional-cto",
            "The firm runs the technology function for a law firm: AI enablement delivered \
                 through the firm, the privacy and compliance work under it, and complex counsel \
                 beside it. A law-related service, quoted per engagement.",
        ),
        page(
            "Legal Services and fees",
            "/services",
            "The published flat-fee schedule: what each routine matter costs, in dollars, \
                 before you call. Wills, trusts, name changes, formations, trademarks, tenant \
                 defense, and demand letters, each reviewed by a licensed attorney.",
        ),
        page(
            "Litigation",
            "/litigation",
            "Plaintiff and defense: complex technology disputes for companies, and fraud \
                 cases for the people on the receiving end. Quoted per engagement.",
        ),
        page(
            "Fractional General Counsel",
            "/fractional-gc",
            "Company counsel on a flat monthly fee — cap table, employee agreements, and \
                 state tax filings, with a one-business-day redline turnaround.",
        ),
        page(
            "Neon Law Navigator",
            "/navigator",
            "The firm's legal project platform, free software under the AGPL-3.0, with \
                 an open invitation to co-counsel a pro bono case.",
        ),
        page(
            "Presentations",
            "/presentations",
            "Talks we give on building legal software; every talk below reads beneath it.",
        ),
        page(
            "Writing",
            "/blog",
            "Posts from the firm on litigation, company counsel, and building legal software.",
        ),
        page(
            "Contact",
            "/contact",
            "How to reach the firm about a matter, and what to include in the first email.",
        ),
    ]
}

/// The site's public Axum table: the retired URLs, the crawler documents, and
/// the one write on each workshop or presentation surface.
///
/// Every *page* renders through the Dioxus SSR port, so it arrives via
/// [`public_dioxus_routers`] rather than this table; a `410` and a certificate
/// `POST` are not pages, which is why they mount here.
pub fn public_routes() -> PublicRouter<AppState> {
    portal::catalog_presentation_command_routes()
        .merge(retired_path_routes())
        .merge(portal::host_crawler_and_legal_routes(
            sitemap_paths,
            llms_txt,
        ))
}

/// Every Dioxus SSR router the site mounts: the firm's pages at the root, and
/// the two legal documents beside them.
///
/// This is the Dioxus half of the surface; [`public_routes`] is the Axum half.
/// Both must be composed, or the binary serves nothing on the missing half's
/// paths.
#[must_use]
pub fn public_dioxus_routers(state: &AppState) -> Vec<PublicRouter> {
    let mut routers = legal_dioxus_routers();
    routers.append(&mut firm_public_dioxus_routers(state));
    routers
}

/// The site's two legal documents (`/privacy`, `/terms`), rendered from this
/// crate's own `CommonMark` bodies.
///
/// Both carry the text-messaging (SMS) program terms, which is where the
/// message-frequency, opt-out, and carrier-liability disclosures live.
fn legal_dioxus_routers() -> Vec<PublicRouter> {
    portal::dioxus_app::legal_dioxus_routers(
        views::brand::FIRM_BRAND.site_name,
        include_str!("../content/privacy.md"),
        include_str!("../content/terms.md"),
    )
}

/// The whole brand: what `main` hands to the shared run loop.
#[must_use]
pub fn brand() -> Brand {
    Brand {
        key: "neon",
        seed: BrandSeed::Neon,
        service_name: "neon-server",
        portal_only: false,
        public_routes: public_routes(),
        public_paths: PUBLIC_PATHS,
        public_dioxus: Box::new(public_dioxus_routers),
    }
}
