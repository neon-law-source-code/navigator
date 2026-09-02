//! The public-page chrome resolved from the request's scoped brand — the reusable
//! header + footer data every ported public marketing page needs (issue #641 /
//! #730 PR6).
//!
//! Extracted from the first page port (`team_nick`) once a second page needed
//! the same header nav + footer legal strip. The DTOs are wasm-safe (plain
//! serde); the resolver reads `views::brand` and so is server-only, called from
//! each page's `#[server]` view function.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    FooterAttorney, FooterBarLicense, FooterNavLink, FooterOffice, SiteFooterLegal,
};

/// One nav destination, resolved from the brand for the header.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ChromeNavLink {
    pub label: String,
    pub href: String,
}

/// The header's auth-aware utility links (Portal / role-gated Lawyer·Admin /
/// Sign out for a signed-in viewer, or Sign in for an anonymous visitor on a
/// law-firm brand), resolved from the request session. The wasm-safe request
/// extension the portal router injects (`portal::dioxus_app`), read by each
/// page's `#[server]` view function — the session lives on `portal`'s
/// `SessionData`, which `webapp` cannot see. Empty when no session and the
/// brand is not a law firm.
#[derive(Clone, Default)]
pub struct PublicUtility(pub Vec<ChromeNavLink>);

/// One published office, resolved from the brand for the footer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ChromeOffice {
    pub state: String,
    pub address: String,
    /// A qualification published under the address, e.g. an admission that has
    /// not issued yet. Mirrors `views::brand::FirmOffice::note`.
    pub note: Option<String>,
}

/// One bar license, resolved from the brand for the footer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ChromeBarLicense {
    pub jurisdiction: String,
    pub number: String,
    pub license_url: String,
}

/// One licensed attorney and their bar licenses, resolved from the brand.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ChromeAttorney {
    pub name: String,
    pub licenses: Vec<ChromeBarLicense>,
}

/// The public-page chrome: everything the [`crate::components::SiteHeader`] and
/// [`crate::components::SiteFooterLegal`] need, resolved from the process brand
/// per request so the wasm client never links the view layer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PublicChrome {
    pub brand_name: String,
    pub home_href: String,
    pub logo_href: String,
    /// The absolute URL of the brand's raster mark, for `og:image` — scrapers
    /// drop relative image URLs, so it is resolved against the site origin on
    /// the server.
    pub social_image: String,
    pub destinations: Vec<ChromeNavLink>,
    pub utility: Vec<ChromeNavLink>,
    /// The public pages the footer links rather than the header — the two
    /// organizations' homes, Navigator, the Blog, Contact, and the rest. The
    /// same row on both faces, because there is one footer.
    pub footer_links: Vec<ChromeNavLink>,
    pub firm_name: String,
    /// The firm's own mark, which is the one the footer opens on. Distinct
    /// from `logo_href`, the header's mark: a white-label bundle can publish a
    /// different mark in each place.
    pub firm_logo_href: String,
    /// Where the footer's mark links — the firm's own home. Distinct from
    /// `home_href`, which is the header's.
    pub firm_home_href: String,
    /// The legal person the footer's copyright names, resolved from the firm
    /// brand on both faces.
    pub legal_entity: String,
    pub disclaimer: String,
    /// The firm's registered word mark, its U.S. registration number, and the
    /// register's own record for it — the footer's trademark notice. Resolved
    /// from the firm brand on both faces, like `legal_entity` beside it, since
    /// the notice names that entity as the registrant.
    pub trademark: String,
    pub trademark_registration: String,
    pub trademark_record_url: String,
    pub copyright_year: i32,
    /// The firm's inbound support address — the footer's contact CTA.
    pub firm_email: String,
    /// The firm's published voice line, dialled from the footer's `tel:` link.
    pub firm_phone: String,
    /// Every office the firm publishes, in the order the brand gives them.
    pub offices: Vec<ChromeOffice>,
    /// The firm's attorneys and the bar licenses each holds.
    pub attorneys: Vec<ChromeAttorney>,
    /// The public repository the platform is developed in, as both footers
    /// publish it: the `owner/name` a reader sees and the address it links to.
    /// Constants rather than brand fields, for the same reason the platform
    /// line names Neon Law Navigator outright — a white-label deployment wears
    /// its own wordmark but runs this software, developed here.
    pub source_repo: String,
    pub source_href: String,
    /// The published release this deployment runs (`YY.M.D`), and the page
    /// describing the platform. The release is `views::brand::deployed_release`
    /// — empty under `cargo run`, where the image stamp does not exist, so the
    /// footer publishes no version rather than an empty one.
    pub navigator_version: String,
    pub navigator_href: String,
    /// How many people have starred that repository, or `None` when the
    /// process has not fetched it yet.
    ///
    /// Read from `crate::source_repository`'s cache when the chrome is built,
    /// so a page render never waits on GitHub. `None` is the ordinary state
    /// before the first refresh lands and whenever GitHub is unreachable; the
    /// footer publishes the repository link without a number.
    pub source_stars: Option<u64>,
}

/// The public footer, mapped from an already-resolved [`PublicChrome`].
///
/// ONE footer, byte-identical on every public page: this mapping reads no
/// field a white-label bundle varies per header, so
/// [`crate::components::SiteFooterLegal`] renders the same mark, wordmark,
/// copyright, link row, bar licenses, offices, and contact channels wherever a
/// reader reaches it. `every_page_renders_the_one_shared_footer` compares two
/// renders byte for byte.
///
/// The header half of the chrome is where a bundle differs — `brand_name`,
/// `logo_href`, `home_href`, `social_image`, and `destinations`. None of them
/// belongs here: `firm_name` and `firm_logo_href` are the footer's identity.
///
/// Pages call `rsx! { PublicFooter { chrome } }` and pass the result to
/// [`crate::components::PublicShell`].
#[component]
pub fn PublicFooter(chrome: PublicChrome) -> Element {
    rsx! {
        // The firm's colour, hoisted here because this is the one place that
        // already knows which brand the page wears. Navigator's shared tokens
        // are teal; the firm is orange. Loading it here means every public page
        // gets the palette — including `/contact`, `/blog`, and the legal
        // pages, which hoist no marketing layer.
        document::Stylesheet { href: crate::brand_style::BRAND_TOKENS_HREF }
        SiteFooterLegal {
            // The copyright names the legal person that renders the legal
            // services, resolved from the firm brand, so it is the same name at
            // the bottom of every page.
            copyright_holder: chrome.legal_entity.clone(),
            disclaimer: chrome.disclaimer.clone(),
            // The mark notice reads `copyright_holder` as its registrant, so
            // it is handed in beside that name rather than resolved apart from
            // it — the registration and its owner are one claim.
            trademark: chrome.trademark.clone(),
            trademark_registration: chrome.trademark_registration.clone(),
            trademark_record_url: chrome.trademark_record_url.clone(),
            copyright_year: chrome.copyright_year,
            logo_href: chrome.firm_logo_href.clone(),
            // The wordmark beside the footer mark is the firm's, on every
            // page, with the line naming the legal person below it.
            brand_name: chrome.firm_name.clone(),
            // And that mark is the door home. The header's is off screen by the
            // time a reader reaches the footer, so this is the one they click.
            home_href: chrome.firm_home_href.clone(),
            contact_email: chrome.firm_email.clone(),
            phone: chrome.firm_phone.clone(),
            offices: chrome
                .offices
                .iter()
                .map(|office| FooterOffice {
                    state: office.state.clone(),
                    address: office.address.clone(),
                    note: office.note.clone(),
                })
                .collect(),
            nav: chrome
                .footer_links
                .iter()
                .map(|link| FooterNavLink {
                    label: link.label.clone(),
                    href: link.href.clone(),
                })
                .collect(),
            attorneys: chrome
                .attorneys
                .iter()
                .map(|attorney| FooterAttorney {
                    name: attorney.name.clone(),
                    licenses: attorney
                        .licenses
                        .iter()
                        .map(|license| FooterBarLicense {
                            jurisdiction: license.jurisdiction.clone(),
                            number: license.number.clone(),
                            license_url: license.license_url.clone(),
                        })
                        .collect(),
                })
                .collect(),
            source_repo: chrome.source_repo.clone(),
            source_href: chrome.source_href.clone(),
            source_stars: chrome.source_stars,
            navigator_version: chrome.navigator_version.clone(),
            navigator_href: chrome.navigator_href.clone(),
        }
    }
}

/// Resolve the firm host's public chrome from the request's scoped brand. Server-only
/// (`views::brand` does not compile to wasm); each page's `#[server]` view
/// function calls this, and the macro stubs those bodies for the wasm client.
///
/// `utility` is the auth-aware header utility group the portal router resolves
/// from the request session (see [`PublicUtility`]) and the page's server
/// function passes through; it is empty for an anonymous visitor on a
/// non-firm brand.
///
#[cfg(feature = "server")]
#[must_use]
pub fn firm_public_chrome(utility: Vec<ChromeNavLink>) -> PublicChrome {
    chrome_for(&views::brand::FIRM_BRAND, utility)
}

/// Build the public chrome for `brand`'s header, with the firm's footer data.
///
/// One brand reaches this — the firm's — because the site publishes one
/// header. The parameter stays because a white-label bundle still mounts its
/// own brand through it.
#[cfg(feature = "server")]
fn chrome_for(brand: &views::brand::SiteBrand, utility: Vec<ChromeNavLink>) -> PublicChrome {
    use views::brand::FIRM_BRAND;

    // The mark, its registration, and the register's record, resolved together
    // — the notice is one claim and a footer holding two thirds of it makes an
    // ownership statement a reader cannot check.
    let (trademark, registration, record) = views::brand::firm_trademark();

    let destinations = brand
        .nav
        .iter()
        .map(|link| ChromeNavLink {
            label: link.label.to_string(),
            href: link.href.to_string(),
        })
        .collect();

    PublicChrome {
        brand_name: brand.site_name.to_string(),
        home_href: brand.home_href.to_string(),
        logo_href: brand.logo_href.to_string(),
        social_image: views::assets::absolute_url(brand.social_image),
        destinations,
        utility,
        footer_links: views::brand::firm_footer_nav()
            .iter()
            .map(|link| ChromeNavLink {
                label: link.label.to_string(),
                href: link.href.to_string(),
            })
            .collect(),
        firm_name: FIRM_BRAND.site_name.to_string(),
        firm_logo_href: FIRM_BRAND.logo_href.to_string(),
        firm_home_href: FIRM_BRAND.home_href.to_string(),
        legal_entity: FIRM_BRAND.legal_entity.to_string(),
        disclaimer: views::brand::firm_disclaimer().to_string(),
        trademark: trademark.to_string(),
        trademark_registration: registration.to_string(),
        trademark_record_url: record.to_string(),
        // The footer fixes the joint-copyright year too; a deploy-time
        // value replaces the constant when the footer year is wired through.
        copyright_year: 2026,
        // The contact band is firm-anchored for the same reason the legal strip
        // is: one footer serves the firm, and the firm is the entity a
        // visitor calls, writes to, or walks in on.
        firm_email: views::brand::firm_email().to_string(),
        firm_phone: views::brand::firm_phone().to_string(),
        offices: views::brand::firm_offices()
            .iter()
            .map(|office| ChromeOffice {
                state: office.state.to_string(),
                address: office.address.to_string(),
                note: office.note.map(str::to_string),
            })
            .collect(),
        attorneys: views::brand::firm_attorneys()
            .iter()
            .map(|attorney| ChromeAttorney {
                name: attorney.name.to_string(),
                licenses: attorney
                    .licenses
                    .iter()
                    .map(|license| ChromeBarLicense {
                        jurisdiction: license.jurisdiction.to_string(),
                        number: license.number.to_string(),
                        license_url: license.license_url.to_string(),
                    })
                    .collect(),
            })
            .collect(),
        // The repository the platform is developed in.
        //
        // The star count is a CACHE READ, deliberately. This function runs in
        // the request path — once per public page render — so it must not
        // reach the network: `source_repository::spawn_refresh` keeps the value
        // current from a background task, and an empty cache renders the
        // repository link with no number rather than delaying the page.
        source_repo: crate::source_repository::REPOSITORY_SLUG.to_string(),
        source_href: crate::source_repository::REPOSITORY_HREF.to_string(),
        source_stars: crate::source_repository::star_count(),
        // The release stamp, read from the environment the image was published
        // with. `None` on a local `cargo run`, which renders no version line at
        // all — see `SiteFooterLegal`'s `navigator_version`.
        navigator_version: views::brand::deployed_release()
            .unwrap_or_default()
            .to_string(),
        navigator_href: "/navigator".to_string(),
    }
}

/// Resolve the firm host's public chrome, reading the auth-aware utility group
/// from the request's injected [`PublicUtility`] extension (empty when the
/// portal router injected none). The single entry point each public page's
/// `#[server]` view function calls.
#[cfg(feature = "server")]
pub async fn firm_public_chrome_from_context() -> PublicChrome {
    // Prefer the chrome the portal pre-layer (`inject_public_utility`) resolved
    // on the request task, where the brand `task_local` is live — this server-fn
    // runs on a task that does not inherit it, so building the chrome here would
    // read the default brand under a mounted white-label bundle. Fall back to
    // building it from context if the extension is absent.
    if let Ok(axum::Extension(chrome)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PublicChrome>, _>().await
    {
        return chrome;
    }
    let utility =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PublicUtility>, _>()
            .await
            .map_or_else(|_| Vec::new(), |axum::Extension(utility)| utility.0);
    firm_public_chrome(utility)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// A firm chrome fixture carrying the firm's regulated footer copy.
    fn firm_chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".to_string(),
            logo_href: "/public/logo.svg".to_string(),
            firm_name: "Neon Law".to_string(),
            firm_logo_href: "/public/logo.svg".to_string(),
            firm_home_href: "/".to_string(),
            legal_entity: "Shook Law PLLC".to_string(),
            disclaimer: "This is an attorney advertisement.".to_string(),
            // The real registration, as `chrome_for` resolves it from the firm
            // brand: the notice's registrant is `legal_entity` above, so a
            // fixture inventing a number here would document the one pairing
            // this footer must never publish.
            trademark: "NEON LAW".to_string(),
            trademark_registration: "6,325,650".to_string(),
            trademark_record_url: "https://tmsearch.uspto.gov/search/search-results/90039224"
                .to_string(),
            copyright_year: 2026,
            firm_email: "support@neonlaw.com".to_string(),
            firm_phone: "+1 555 010 0100".to_string(),
            attorneys: vec![ChromeAttorney {
                name: "Ada Lovelace".to_string(),
                licenses: vec![ChromeBarLicense {
                    jurisdiction: "California".to_string(),
                    number: "100001".to_string(),
                    license_url: "https://example.com/bar/100001".to_string(),
                }],
            }],
            ..PublicChrome::default()
        }
    }

    /// A chrome whose header half names a white-label tenant, sharing the
    /// firm's footer half.
    ///
    /// It is what makes `a_different_header_renders_the_same_footer` an
    /// assertion rather than a tautology: the footer must read no header field,
    /// and only a fixture whose header fields differ can prove it.
    fn tenant_chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Acme Law".to_string(),
            logo_href: "/public/brand/firm-logo.svg".to_string(),
            ..firm_chrome()
        }
    }

    /// Firm chrome renders the firm's footer: the copyright that names the
    /// regulated entity, and the firm's own contact channels.
    #[test]
    fn firm_chrome_renders_the_firm_footer() {
        fn app() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        let out = ssr(app);
        assert!(out.contains("\u{a9} 2026 Shook Law PLLC"), "{out}");
        assert!(out.contains("mailto:support@neonlaw.com"), "{out}");
        assert!(
            out.contains(r#"class="site-footer__logo" src="/public/logo.svg" alt="""#),
            "the firm footer carries the firm's mark: {out}"
        );
        assert!(!out.contains("site-footer--foundation"), "{out}");
    }

    /// The footer's mark is a link to the firm's home, under any header.
    ///
    /// It used to be an inert `<div>`, which made the bottom-of-page logo the
    /// one copy of the mark on a long page that did nothing when clicked — the
    /// header's is off screen by the time a reader gets there, so the footer's
    /// is the one they reach for. The destination is the firm's home even under
    /// a tenant's header, matching the firm wordmark and mark beside it.
    #[test]
    fn the_footer_mark_links_to_the_firms_home_under_any_header() {
        fn firm() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        fn tenant() -> Element {
            rsx! { PublicFooter { chrome: tenant_chrome() } }
        }
        for face in [firm as fn() -> Element, tenant] {
            let out = ssr(face);
            let brand = out
                .split(r#"class="site-footer__brand""#)
                .nth(1)
                .and_then(|rest| rest.split('>').next())
                .expect("the footer renders a brand mark");
            assert!(
                brand.contains(r#"href="/""#),
                "the footer mark opens the firm's home: {out}"
            );
            assert!(
                brand.contains(r#"aria-label="Neon Law home""#),
                "and is announced as that door: {out}"
            );
            assert!(
                !out.contains(r#"<div class="site-footer__brand""#),
                "the mark is a link, not an inert box: {out}"
            );
        }
    }

    /// The footer's identity is the firm's under any header: the firm's mark,
    /// the firm's wordmark beside it, and the legal person named below.
    ///
    /// A header wordmark and mark that reached the footer would put one
    /// organization's name at the bottom of a page with another's copyright
    /// directly under it. One footer means one identity, and this keeps the
    /// header's out of it.
    #[test]
    fn the_footer_identity_is_the_firms_under_any_header() {
        fn app() -> Element {
            rsx! { PublicFooter { chrome: tenant_chrome() } }
        }
        let out = ssr(app);
        assert!(
            out.contains(r#"<strong class="site-footer__wordmark">Neon Law</strong>"#),
            "the footer wordmark is the firm's: {out}"
        );
        assert!(
            out.contains(r#"src="/public/logo.svg""#),
            "and so is the mark: {out}"
        );
        assert!(
            !out.contains("Acme Law"),
            "the tenant's header wordmark does not reach the footer: {out}"
        );
    }

    /// The footer's copyright names the legal person, not the wordmark:
    /// "Neon Law" is a brand and cannot hold a copyright.
    #[test]
    fn the_firm_copyright_names_the_legal_entity() {
        fn app() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        let out = ssr(app);
        assert!(out.contains("© 2026 Shook Law PLLC"), "{out}");
    }

    /// The mark notice names the registrant the copyright line names, and
    /// points at the register rather than at the site's own word for it.
    ///
    /// The registrant is the assertion that matters: U.S. Reg. No. 6,325,650 is
    /// the Firm's, and a footer citing that number beside any other name would
    /// hand a reader permission nobody gave them.
    #[test]
    fn the_mark_notice_names_the_registrant_and_links_the_record() {
        fn app() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        let out = ssr(app);
        assert!(
            out.contains("NEON LAW") && out.contains("®"),
            "the mark renders as registered: {out}"
        );
        assert!(
            out.contains("is a registered trademark of Shook Law PLLC"),
            "the registrant is the legal person, not the wordmark: {out}"
        );
        assert!(
            out.contains("U.S. Reg. No. 6,325,650")
                && out.contains(
                    r#"href="https://tmsearch.uspto.gov/search/search-results/90039224""#
                ),
            "the registration is cited and linked to the register: {out}"
        );
    }

    /// A different header renders the one shared footer, byte-identical to the
    /// firm's: the bar licenses, the offices, and the contact band all carry
    /// over unchanged.
    #[test]
    fn a_different_header_renders_the_same_footer() {
        fn firm_app() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        fn tenant_app() -> Element {
            rsx! { PublicFooter { chrome: tenant_chrome() } }
        }
        let firm_out = ssr(firm_app);
        let tenant_out = ssr(tenant_app);
        assert_eq!(
            firm_out, tenant_out,
            "the two headers render the same footer"
        );
        assert!(
            tenant_out.contains("Bar No."),
            "the bar license carries over: {tenant_out}"
        );
        assert!(
            tenant_out.contains("mailto:support@neonlaw.com"),
            "the contact band carries over: {tenant_out}"
        );
    }

    /// The public footer carries the firm's own disclaimer.
    #[test]
    fn the_footer_carries_the_firms_disclaimer() {
        fn app() -> Element {
            rsx! { PublicFooter { chrome: firm_chrome() } }
        }
        let out = ssr(app);
        assert_eq!(
            out.matches(r#"role="contentinfo""#).count(),
            1,
            "exactly one footer landmark: {out}"
        );
        assert!(
            out.contains("attorney advertisement") || out.contains("attorney advertising"),
            "the firm's own disclaimer is there: {out}"
        );
    }
}
