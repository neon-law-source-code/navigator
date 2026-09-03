//! `neonlaw.com` — Neon Law's public face, over the mounted Navigator
//! application.
//!
//! One crate, one binary — the `neon-server` image — serving every house
//! brand this repository registers. Each request's `Host:` header resolves
//! to its own [`views::brand::BrandKey`], so the same running process renders
//! Neon Law's chrome on its own hosts and a second house brand's on its own,
//! from the one composed router this crate declares. Neon Law is the practice
//! of Shook Law PLLC, and every page this crate serves is the firm's or a
//! house brand the firm operates.
//!
//! This crate owns the public surface outright: its marketing copy, its page
//! compositions, and its path table. `portal` owns the authenticated
//! application underneath.
//!
//! The composition lives here rather than in `main` so the binary and the tests
//! that exercise its router are the same expression. A test that restated the
//! composition would pass while the deployed binary served something else,
//! which is exactly how a public surface goes quietly wrong.

// The public face is these three modules; the crate exports only the
// composition entry points below, so the site's copy is not API.
mod firm_copy;
mod firm_pages;
mod locales;

use portal::hosting::{BrandSeed, PublicRouter, Site};
use portal::AppState;
use views::brand::BrandKey;

pub use firm_pages::firm_public_dioxus_routers;

/// Every path this site registers, public and gated alike.
///
/// A *declaration*, not an access rule: [`portal::bootstrap`] checks it against
/// `portal::RESERVED_PATH_PREFIXES` so it can never shadow a Navigator-owned
/// surface. Access is decided by the route layers and the embedded policy.
///
/// The firm's pages hold the root, which is the site's whole surface.
pub const PUBLIC_PATHS: &[&str] = &[
    // --- The firm ---------------------------------------------------------
    "/",
    "/fractional-cto",
    "/services",
    "/litigation",
    "/fractional-gc",
    "/navigator",
    "/notations",
    "/notations/{slug}",
    "/contact",
    "/team",
    "/team/{slug}",
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
/// A talk's projector face (`/display/{step}`) and its certificate confirmation
/// are left out for the same reason a crawler is not sent to a print dialog:
/// they are states of a session, not documents.
///
/// `/team/{slug}` is not expanded here the way `/blog/{slug}` is: the blog
/// roster is loaded once into `state.blog` at boot, so listing it is a plain
/// sync read, while the team roster is a live `Person` query this function's
/// `fn` (not `async fn`) signature cannot make. `/team` itself is listed, and
/// its index page links every current profile, so a crawler still reaches
/// them — just not with their own sitemap `<url>` entry.
#[must_use]
pub fn sitemap_paths(state: &AppState, key: BrandKey) -> std::collections::BTreeSet<String> {
    match key {
        BrandKey::DeleteYourData => ["/", "/services", "/contact"]
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        BrandKey::Neon => {
            let mut paths: std::collections::BTreeSet<String> = [
                "/",
                "/fractional-cto",
                "/services",
                "/litigation",
                "/fractional-gc",
                "/navigator",
                "/notations",
                "/contact",
                "/team",
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
            for material in state.workshops.materials().iter().filter(|material| {
                matches!(material.category.as_str(), "presentations" | "workshops")
            }) {
                let path = format!("/{}/{}", material.category, material.slug);
                paths.insert(path.clone());
                paths.insert(format!("{path}.md"));
                paths.insert(format!("{path}/slides"));
                for step in 1..=material.sections.len() {
                    paths.insert(format!("{path}/step/{step}"));
                }
            }
            paths
        }
    }
}

/// The site's `/llms.txt`: what a crawler has reached at `neonlaw.com`, and the
/// pages it may read there.
///
/// The firm's practice and its flat-fee routine work, in the order a reader
/// meets them. Every entry is a page this host serves anonymously; the
/// individual posts a crawler walks from `/blog` are enumerated by
/// [`sitemap_paths`] rather than curated here.
///
/// `/services` is named as the fee schedule it is. The firm charges a fixed fee
/// per matter, which is the thing that page exists to say — an index describing
/// it as generic "legal services" would understate it. What it does not say is
/// a dollar figure: the site publishes no fee amounts, and `/services` names
/// its schedule's matters and scope without one. An index that told a crawler
/// otherwise would send it looking for numbers the page does not carry.
#[must_use]
pub fn llms_txt(state: &AppState, key: BrandKey) -> portal::LlmsTxt {
    match key {
        BrandKey::DeleteYourData => {
            let branding = &views::brand::DELETE_YOUR_DATA_BRANDING;
            let mark = branding.firm.site_name;
            portal::LlmsTxt {
                title: mark.to_string(),
                summary: branding.mission_description.to_string(),
                pages: vec![
                    portal::LlmsTxtLink {
                        title: mark.to_string(),
                        path: "/".to_string(),
                        description: branding.mission_description.to_string(),
                    },
                    portal::LlmsTxtLink {
                        title: "Data-deletion requests".to_string(),
                        path: "/services".to_string(),
                        description: branding.service_description.to_string(),
                    },
                    portal::LlmsTxtLink {
                        title: "Contact".to_string(),
                        path: "/contact".to_string(),
                        description: format!(
                            "How to reach {mark}, a practice of Shook Law PLLC, about a \
                             data-deletion request."
                        ),
                    },
                ],
                sections: Vec::new(),
            }
        }
        BrandKey::Neon => {
            let mark = views::brand::FIRM_BRAND.site_name;
            portal::LlmsTxt {
                title: mark.to_string(),
                summary: format!(
                    "{mark} is a consumer law firm working on flat fees: wills, trusts, name changes, \
                     formations, and the other routine matters a person actually walks in with, \
                     alongside a litigation and company-counsel practice quoted per engagement."
                ),
                pages: indexed_pages(mark),
                sections: [("Workshop Corpus", "workshops")]
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
            "The firm's practice — flat-fee consumer legal work, litigation on both sides of \
                 the v., and company counsel for emerging technology companies.",
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
            "The flat-fee schedule: wills, trusts, name changes, formations, trademarks, \
                 tenant defense, and demand letters, each a fixed fee agreed before work \
                 begins and reviewed by a licensed attorney. The site names no dollar figure; \
                 email the firm for the fee on your matter.",
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
            "The firm's legal project platform, source-available under BUSL-1.1, with \
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
            "Notations",
            "/notations",
            "The firm's sample engagement letters and the government forms Navigator files.",
        ),
        page(
            "Contact",
            "/contact",
            "How to reach the firm about a matter, and what to include in the first email.",
        ),
        page(
            "Team",
            "/team",
            "The people at the firm, each with their own page naming an email and a LinkedIn \
                 profile.",
        ),
    ]
}

/// The site's public Axum table: the crawler documents, and the one write on
/// each workshop or presentation surface.
///
/// Every *page* renders through the Dioxus SSR port, so it arrives via
/// [`public_dioxus_routers`] rather than this table; a certificate `POST` is
/// not a page, which is why it mounts here.
pub fn public_routes() -> PublicRouter<AppState> {
    portal::catalog_presentation_command_routes().merge(portal::host_crawler_and_legal_routes(
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

/// The whole site: what `main` hands to the shared run loop.
#[must_use]
pub fn brand() -> Site {
    Site {
        key: "neon",
        seed: BrandSeed::Neon,
        service_name: "neon-server",
        portal_only: false,
        public_routes: public_routes(),
        public_paths: PUBLIC_PATHS,
        public_dioxus: Box::new(public_dioxus_routers),
    }
}
