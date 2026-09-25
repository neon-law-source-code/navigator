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
    // Answers a 301 to the data-removal practice, not a page. The consumer
    // plan it served is retired; the path stays declared because it is still
    // a route this site answers, and its published links must resolve.
    //
    // It is deliberately absent from `sitemap_paths`, which is a different
    // question: this table is what the app answers, a sitemap is what we ask
    // a crawler to index, and a redirect belongs in the first but not the
    // second.
    "/personal",
    "/services",
    "/start/{service_id}",
    "/disputes",
    "/business",
    "/navigator",
    // The Neon-hosted gateway to the DeleteYourDebt.com practice. Neon-only —
    // 404s on every other registered brand host; see
    // `views::brand::BrandKey::publishes_firm_path`.
    "/delete-your-debt",
    // The Neon-hosted gateway to the DeleteYourData.com practice. Same
    // Neon-only shape as `/delete-your-debt` above.
    "/delete-your-data",
    // The Neon-hosted gateway to the Abhaya Immigration practice. Same
    // Neon-only shape as `/delete-your-debt` above.
    "/immigration",
    // The Neon-hosted gateway to the Vesta Estate Planning practice. Same
    // Neon-only shape as `/delete-your-debt` above.
    "/estate-planning",
    // The Neon-hosted gateway to the Misericordia Injury Law practice. Same
    // Neon-only shape as `/delete-your-debt` above.
    "/accidents",
    // The Neon-hosted gateway to the Daybridge Divorce Law practice. Same
    // Neon-only shape as `/delete-your-debt` above.
    "/divorce",
    "/notations",
    "/notations/{slug}",
    "/contact",
    "/contact/sent",
    "/leads",
    "/team",
    "/blog",
    "/blog/{slug}",
    "/testimonials",
    // The talks catalog and every talk beneath it. Anonymous like the rest of
    // this table: a talk is published to be read.
    "/presentations",
    "/presentations/{slug}",
    "/presentations/{slug}/slides",
    "/presentations/{slug}/step/{step}",
    "/presentations/{slug}/display/{step}",
    "/presentations/{slug}/certificate",
    "/presentations/{slug}/certificate/sent",
    // The public Navigator workshop materials remain under their original
    // paths; the retired index itself redirects to `/presentations`.
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
#[must_use]
pub fn sitemap_paths(state: &AppState, key: BrandKey) -> std::collections::BTreeSet<String> {
    match key {
        // Privacy keeps the annual product on one page, with office details separate.
        BrandKey::DeleteYourData => ["/", "/contact", "/testimonials"]
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        BrandKey::Vesta
        | BrandKey::Misericordia
        | BrandKey::Abhaya
        | BrandKey::DeleteYourDebt
        | BrandKey::Summons
        | BrandKey::Daybridge
        | BrandKey::DeathAndDivorce => ["/", "/services", "/contact", "/testimonials"]
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        // Lawyer Shook is a bare holding page: `/` is the whole surface.
        BrandKey::LawyerShook => ["/", "/testimonials"]
            .iter()
            .map(|path| (*path).to_string())
            .collect(),
        BrandKey::Neon => {
            let mut paths: std::collections::BTreeSet<String> = [
                "/",
                "/navigator",
                "/delete-your-debt",
                "/delete-your-data",
                "/immigration",
                "/estate-planning",
                "/accidents",
                "/divorce",
                "/notations",
                "/contact",
                "/team",
                "/blog",
                "/presentations",
                "/testimonials",
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

/// The `llms.txt` index for a practice brand: the two pages it serves,
/// described in its own compiled `Branding`.
///
/// Lifted out of [`llms_txt`] because every practice brand answers this the
/// same way — the arm would otherwise repeat once per brand, and the five of
/// them pushed that function past its length limit.
fn practice_brand_llms_txt(key: BrandKey) -> portal::LlmsTxt {
    let branding = key.resolve_branding(&views::brand::DEFAULT_BRANDING);
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
                title: "Services".to_string(),
                path: "/services".to_string(),
                description: branding.service_description.to_string(),
            },
        ],
        sections: Vec::new(),
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
/// it as generic "legal services" would understate it. The index carries the
/// same $5, $10/day, and $100-starting pricing summary the public pages publish.
#[must_use]
pub fn llms_txt(state: &AppState, key: BrandKey) -> portal::LlmsTxt {
    match key {
        // Each practice brand indexes the two pages it serves, from its own
        // compiled `Branding` — the descriptions are the brand's own words,
        // never another brand's.
        key @ (BrandKey::Vesta
        | BrandKey::Misericordia
        | BrandKey::Abhaya
        | BrandKey::DeleteYourDebt
        | BrandKey::Summons
        | BrandKey::Daybridge
        | BrandKey::DeathAndDivorce) => practice_brand_llms_txt(key),
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
        BrandKey::LawyerShook => {
            // Lawyer Shook is a bare holding page: the same statement
            // `resolve_firm_home_content` puts on the screen is the whole of
            // what a crawler reads here too, so this reuses it rather than
            // keeping a second copy of the sentence in step.
            let content =
                firm_pages::resolve_firm_home_content(&views::brand::LAWYER_SHOOK_BRANDING, None);
            let bare = content
                .bare
                .as_ref()
                .expect("invariant: Lawyer Shook's home content is always bare");
            portal::LlmsTxt {
                title: bare.heading.clone(),
                summary: bare.paragraph.clone(),
                pages: vec![portal::LlmsTxtLink {
                    title: bare.heading.clone(),
                    path: "/".to_string(),
                    description: bare.paragraph.clone(),
                }],
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
            "Counsel for technology companies. $5,000 flat-fee setup. $50 a day with five-business-day contract turnaround. $500 express review in one day. Contracts include 50 pages; each additional page is $5. Separate $10,000 retainer.",
        ),
        page(
            "Debt-collection defense",
            "/delete-your-debt",
            "Neon's gateway page naming DeleteYourDebt.com, a Shook Law PLLC practice, as \
             the destination for collection-lawsuit defense, FDCPA claims, validation \
             demands, and credit-report disputes. It does not settle debts or negotiate \
             balances.",
        ),
        page(
            "Privacy and data removal",
            "/delete-your-data",
            "Neon's gateway page naming DeleteYourData.com, a Shook Law PLLC practice, as \
             the destination for data-removal and privacy-protection work: one year of \
             data removal and privacy protection for $50, with credit monitoring available \
             as an opt-in. It does not promise every piece of personal information can be \
             removed.",
        ),
        page(
            "Immigration",
            "/immigration",
            "Neon's gateway page naming Abhaya Immigration, a Shook Law PLLC practice, as \
             the destination for family petitions, employment visas, green cards, \
             naturalization, and consular processing. Government filing fees are \
             separate, and no approval or timeline is promised.",
        ),
        page(
            "Estate planning",
            "/estate-planning",
            "Neon's gateway page naming Vesta Estate Planning, a Shook Law PLLC practice, \
             as the destination for wills, trusts, powers of attorney, health-care \
             directives, and lifetime plan updates: $5,000 once, with unlimited edits \
             for life, and court and recording fees separate. No tax, probate, or \
             asset-protection outcome is promised.",
        ),
        page(
            "Accidents and injury claims",
            "/accidents",
            "Neon's gateway page naming Misericordia Injury Law, a Shook Law PLLC \
             practice, as the destination for car, truck, motorcycle, pedestrian, \
             premises, and wrongful-death claims: a free first conversation and a \
             contingency fee. No recovery amount, speed, or unqualified no-fee result \
             is promised.",
        ),
        page(
            "Divorce",
            "/divorce",
            "Neon's gateway page naming Daybridge Divorce Law, a Shook Law PLLC \
             practice, as the destination for divorce planning, agreements, motions, \
             and court papers: a one-time $500 setup fee, then $10 a day while \
             retained, $50 per court appearance, and $5,000 per trial day, with \
             legal costs paid separately. No result or timeline is promised.",
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
