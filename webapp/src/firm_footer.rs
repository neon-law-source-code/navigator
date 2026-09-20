//! One data-driven firm footer, resolved per request from the Firm that
//! wears the current brand (ENG-589) — replacing the process-wide compiled
//! `/app` footer and the `FIRM_BRAND.legal_entity` constant the public
//! chrome's footer used to name.
//!
//! [`FirmFooterModel`] is the one resolved shape both surfaces draw from:
//! [`FirmFooter`] renders it directly for `/app`, and
//! `crate::public_chrome::firm_public_chrome_from_context` maps its
//! `legal_entity`/`brands` onto `crate::components::SiteFooterLegal`'s own
//! props rather than nesting this component. One resolver, not one piece of
//! markup.
//!
//! Both footers carry the same two affiliation rows: "Our Family", every
//! house brand the Firm wears with the current one unlinked, and a "Proud
//! member of …" line per association the firm belongs to. A brand key no
//! Firm wears (the pre-seed database, or a compiled key with no `firm_brand`
//! row yet) falls back to the compiled `Branding`'s family
//! ([`compiled_family_brands`]), so the row is on every page from first boot
//! rather than only once a Firm is seeded.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{ExternalLink, GitHubStars, FOOTER_TAGLINE, POWERED_BY_NEON_LAW_NAVIGATOR};

/// One brand the resolved Firm wears, for the footer's "Our Family" row.
///
/// `href` is empty for a runtime-created brand with no dedicated host to
/// link to yet (a later issue gives one) — rendered as plain text, the same
/// treatment the current brand gets, rather than an inert `<a href="">`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterBrand {
    pub label: String,
    pub href: String,
    pub current: bool,
    /// What this brand actually does, in a few words.
    ///
    /// A cold reader learns nothing from "Vesta" or "Abhaya", so the family
    /// list is close to useless as bare wordmarks. Empty for a
    /// runtime-created brand that has no compiled line, which renders the
    /// wordmark alone rather than an empty dash.
    #[serde(default)]
    pub byline: String,
}

/// One association the firm belongs to, for the footer's "Proud member of
/// the {label}" line. Mirrors `views::brand::FirmMembership`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterMembership {
    pub label: String,
    pub href: String,
}

/// The data every Navigator footer needs, resolved once per request from the
/// Firm that wears the current brand — never a process-wide constant.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterModel {
    pub legal_entity: String,
    /// Every brand the Firm wears, in registry order with the current one
    /// flagged. Empty or a single entry renders no "Our Family" row.
    pub brands: Vec<FirmFooterBrand>,
    /// The associations the firm belongs to. Empty renders no line.
    #[serde(default)]
    pub memberships: Vec<FirmFooterMembership>,
    pub disclaimer: String,
    pub trademark: String,
    pub trademark_registration: String,
    pub trademark_record_url: String,
    pub copyright_year: i32,
    pub source_repo: String,
    pub source_href: String,
    pub source_stars: Option<u64>,
    /// The published release this deployment runs. Empty under `cargo run`.
    pub navigator_version: String,
    pub navigator_href: String,
}

/// Entry count at which the family list splits into two columns.
///
/// Any multi-brand family uses two columns on desktop and one on mobile.
const FAMILY_TWO_COLUMN_THRESHOLD: usize = 2;

/// The family list's class, widened for every multi-brand family.
#[must_use]
fn family_list_class(entries: usize) -> &'static str {
    if entries >= FAMILY_TWO_COLUMN_THRESHOLD {
        "app-footer__family-list app-footer__family-list--two-column"
    } else {
        "app-footer__family-list"
    }
}

/// The `/app` footer: the requested legal lines, the "Our Family" row (a
/// runtime-created brand listed exactly like a compiled one), the firm's
/// membership lines, and the shared platform line.
#[component]
pub fn FirmFooter(model: FirmFooterModel) -> Element {
    rsx! {
        footer { class: "app-footer",
            p { class: "app-footer__disclaimer", "{model.disclaimer}" }
            p { class: "app-footer__copyright", "© {model.copyright_year} {model.legal_entity}" }
            if !model.trademark.is_empty() {
                p { class: "app-footer__trademark",
                    "{model.trademark}"
                    if model.trademark_registration.is_empty() {
                        sup { "™" }
                        " is a common-law mark of {model.legal_entity}"
                    } else {
                        sup { "®" }
                        " is a registered trademark of {model.legal_entity}, "
                        if model.trademark_record_url.is_empty() {
                            "U.S. Reg. No. {model.trademark_registration}"
                        } else {
                            ExternalLink {
                                class: "link-secondary".to_string(),
                                href: model.trademark_record_url.clone(),
                                "U.S. Reg. No. {model.trademark_registration}"
                            }
                        }
                    }
                }
            }
            // The same row the public footer renders, in the `/app` footer's
            // own quieter dress: a landmark named by its visible heading, the
            // current brand as text marked `aria-current`, a brand with no
            // host yet as text rather than an empty anchor.
            if model.brands.len() > 1 {
                nav { class: "app-footer__family", "aria-label": "Our family",
                    h2 { class: "app-footer__family-heading", "Our Family" }
                    ul { class: family_list_class(model.brands.len()),
                        for brand in model.brands.iter() {
                            li { class: "app-footer__family-item", key: "{brand.label}",
                                if brand.current {
                                    span {
                                        class: "app-footer__family-current",
                                        "aria-current": "true",
                                        "{brand.label}"
                                    }
                                } else if brand.href.is_empty() {
                                    span { class: "app-footer__family-current", "{brand.label}" }
                                } else {
                                    a {
                                        class: "app-footer__family-link",
                                        href: "{brand.href}",
                                        "{brand.label}"
                                    }
                                }
                                if !brand.byline.is_empty() {
                                    span { class: "app-footer__family-byline", " · {brand.byline}" }
                                }
                            }
                        }
                    }
                }
            }
            // "Proud member of …", linking the association's own site with
            // the off-site treatment every outbound link carries.
            for membership in model.memberships.iter() {
                p { class: "app-footer__membership", key: "{membership.href}",
                    ExternalLink {
                        class: "app-footer__membership-link".to_string(),
                        href: membership.href.clone(),
                        "Proud member of the {membership.label}"
                    }
                }
            }
            p { class: "app-footer__platform",
                if !model.source_repo.is_empty() && !model.source_href.is_empty() {
                    "Powered by"
                    GitHubStars {
                        href: model.source_href.clone(),
                        repo: model.source_repo.clone(),
                        stars: model.source_stars,
                    }
                } else {
                    "{POWERED_BY_NEON_LAW_NAVIGATOR}"
                }
                if !model.navigator_version.is_empty() && !model.navigator_href.is_empty() {
                    a {
                        class: "app-footer__release",
                        href: "{model.navigator_href}",
                        "#{model.navigator_version}"
                    }
                }
            }
            p { class: "app-footer__tagline", "{FOOTER_TAGLINE}" }
        }
    }
}

/// Render the footer to a standalone HTML string for injection into every
/// `/app` HTML response. Server-only: `dioxus-ssr` is not in the wasm bundle.
#[cfg(feature = "server")]
#[must_use]
pub fn render_firm_footer(model: FirmFooterModel) -> String {
    let mut dom = VirtualDom::new_with_props(FirmFooter, FirmFooterProps { model });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// The compiled "Our Family" row for a request on `current`: every house
/// brand the request-scoped firm trades under (`views::brand::firm_family`),
/// in registry order, each named by its compiled wordmark and linking its
/// production home, with `current` flagged. Empty under a bundle that renamed
/// the firm, whose footer must not list another firm's brands.
///
/// Shared by the `/app` footer's compiled fallback and the public chrome, so
/// the two cannot disagree about who is in the family before a Firm row
/// overrides both.
#[cfg(feature = "server")]
#[must_use]
pub fn compiled_family_brands(current: views::brand::BrandKey) -> Vec<FirmFooterBrand> {
    views::brand::firm_family()
        .iter()
        // "Our Family" is a set of links, so it lists only brands a reader
        // can actually reach — the launch gate's set, the same one
        // `portal::canonical_host` admits and `cli::devx::ship` renders
        // certificates for. A brand is registered here well before its host
        // serves anything; see `views::brand::BrandKey::is_live`.
        //
        // `current` is retained beyond that set for exactly one caller: a
        // developer previewing a held-out brand through its
        // `local_port_env_var` door, where a footer that omitted the brand
        // being previewed would be the wrong preview. A public request can
        // no longer arrive wearing a held-out key at all — the router
        // refuses those hosts — so on a real host this clause is
        // unreachable, and `the_family_row_lists_only_admitted_brands`
        // holds that.
        .filter(|key| key.is_live() || **key == current)
        .map(|key| FirmFooterBrand {
            label: compiled_footer_label(*key),
            href: key.public_home_href(),
            current: *key == current,
            byline: key.family_byline().to_string(),
        })
        .collect()
}

/// The concise names the footer uses for the two public family entries whose
/// masthead names are longer or more specific than their family labels.
#[cfg(feature = "server")]
fn compiled_footer_label(key: views::brand::BrandKey) -> String {
    match key {
        views::brand::BrandKey::Neon => "Emerging Technologies Counsel".to_string(),
        views::brand::BrandKey::DeleteYourData => "Protect your info".to_string(),
        _ => key
            .resolve_branding(&views::brand::DEFAULT_BRANDING)
            .firm
            .site_name
            .to_string(),
    }
}

#[cfg(feature = "server")]
#[must_use]
fn firm_memberships_from(
    memberships: &'static [views::brand::FirmMembership],
) -> Vec<FirmFooterMembership> {
    memberships
        .iter()
        .map(|membership| FirmFooterMembership {
            label: membership.name.to_string(),
            href: membership.href.to_string(),
        })
        .collect()
}

/// The no-store fallback: the compiled `Branding` for `current`, with the
/// compiled family as its "Our Family" row and the firm's memberships. What a
/// fresh deployment (no `firm` rows yet) or a brand key no Firm wears renders.
#[cfg(feature = "server")]
#[must_use]
pub fn compiled_firm_footer_model(
    current: views::brand::BrandKey,
    copyright_year: i32,
    navigator_version: String,
) -> FirmFooterModel {
    let branding = current.resolve_branding(&views::brand::DEFAULT_BRANDING);
    let mut brands = compiled_family_brands(current);
    // A firm with no compiled family (a renamed white-label bundle) still
    // names its one brand, so the model is never empty of identity.
    if brands.is_empty() {
        brands.push(FirmFooterBrand {
            label: compiled_footer_label(current),
            href: String::new(),
            current: true,
            byline: current.family_byline().to_string(),
        });
    }
    FirmFooterModel {
        legal_entity: branding.firm.legal_entity.to_string(),
        brands,
        memberships: firm_memberships_from(branding.firm_memberships),
        disclaimer: branding.firm_disclaimer.to_string(),
        trademark: branding.firm_trademark.to_string(),
        trademark_registration: branding.firm_trademark_registration.to_string(),
        trademark_record_url: branding.firm_trademark_record_url.to_string(),
        copyright_year,
        source_repo: crate::source_repository::REPOSITORY_SLUG.to_string(),
        source_href: crate::source_repository::REPOSITORY_HREF.to_string(),
        source_stars: crate::source_repository::star_count(),
        navigator_version,
        navigator_href: crate::source_repository::NAVIGATOR_HREF.to_string(),
    }
}

/// Resolve [`FirmFooterModel`] for `current`: the Firm wearing that brand
/// key, its Entity's legal name, and every brand key it wears (in registry
/// order, current first), each named from its live `brand` row — a
/// runtime-created brand is listed exactly like a compiled one, never
/// silently dropped. No Firm wearing `current` (or no live rows at all)
/// falls back to [`compiled_firm_footer_model`].
///
/// # Errors
///
/// Never returns an error: any store failure degrades to the compiled
/// fallback rather than breaking every page's footer.
#[cfg(feature = "server")]
pub async fn resolve_firm_footer_model(
    surreal: &store::surreal::SurrealDb,
    current: views::brand::BrandKey,
    copyright_year: i32,
    navigator_version: String,
) -> FirmFooterModel {
    let branding = current.resolve_branding(&views::brand::DEFAULT_BRANDING);
    let fallback =
        || compiled_firm_footer_model(current, copyright_year, navigator_version.clone());

    let Ok(Some(firm_id)) = store::firms::firm_id_for_brand_key(surreal, current.as_str()).await
    else {
        return fallback();
    };
    let Ok(Some(firm)) = store::firms::find_by_id(surreal, firm_id).await else {
        return fallback();
    };
    let Some(entity_id) = firm.entity_id else {
        return fallback();
    };
    let Ok(Some(entity)) = store::entities::find_by_id(surreal, entity_id).await else {
        return fallback();
    };
    let Ok(keys) = store::firms::brand_keys_for_firm(surreal, firm_id).await else {
        return fallback();
    };
    if keys.is_empty() {
        return fallback();
    }

    let mut brands = Vec::with_capacity(keys.len());
    for key in &keys {
        let Ok(Some(brand)) = store::brands::find_by_key(surreal, key).await else {
            continue;
        };
        let compiled = views::brand::BrandKey::ALL
            .iter()
            .find(|candidate| candidate.as_str() == key);
        // Same reachability rule the compiled fallback applies: a row in the
        // `brand` table does not mean a host serves it. A runtime-created
        // brand has no compiled key and so no launch state — it is listed,
        // because nothing here knows better than the operator who made it.
        if compiled.is_some_and(|candidate| !candidate.is_live())
            && key.as_str() != current.as_str()
        {
            continue;
        }
        brands.push(FirmFooterBrand {
            label: compiled
                .map(|key| compiled_footer_label(*key))
                .unwrap_or(brand.name),
            href: compiled
                .map(|key| key.public_home_href())
                .unwrap_or_default(),
            current: key == current.as_str(),
            // A runtime brand carries no compiled line; a compiled one does.
            byline: compiled
                .map(|key| key.family_byline().to_string())
                .unwrap_or_default(),
        });
    }
    if brands.is_empty() {
        return fallback();
    }

    FirmFooterModel {
        legal_entity: entity.name,
        brands,
        memberships: firm_memberships_from(branding.firm_memberships),
        disclaimer: branding.firm_disclaimer.to_string(),
        trademark: branding.firm_trademark.to_string(),
        trademark_registration: branding.firm_trademark_registration.to_string(),
        trademark_record_url: branding.firm_trademark_record_url.to_string(),
        copyright_year,
        source_repo: crate::source_repository::REPOSITORY_SLUG.to_string(),
        source_href: crate::source_repository::REPOSITORY_HREF.to_string(),
        source_stars: crate::source_repository::star_count(),
        navigator_version,
        navigator_href: crate::source_repository::NAVIGATOR_HREF.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn model(brands: Vec<FirmFooterBrand>) -> FirmFooterModel {
        FirmFooterModel {
            legal_entity: "Shook Law PLLC".to_string(),
            brands,
            memberships: Vec::new(),
            disclaimer: "Attorney advertisement. Nothing here is legal advice without a signed retainer for an active project. Past results do not guarantee future outcomes.".to_string(),
            trademark: "NEON LAW".to_string(),
            trademark_registration: "6,325,650".to_string(),
            trademark_record_url:
                "https://tmsearch.uspto.gov/search/search-results/90039224".to_string(),
            copyright_year: 2026,
            source_repo: crate::source_repository::REPOSITORY_SLUG.to_string(),
            source_href: crate::source_repository::REPOSITORY_HREF.to_string(),
            source_stars: None,
            navigator_version: String::new(),
            navigator_href: crate::source_repository::NAVIGATOR_HREF.to_string(),
        }
    }

    fn one_membership() -> Vec<FirmFooterMembership> {
        vec![FirmFooterMembership {
            label: "Justice Technology Association".to_string(),
            href: "https://justicetechassociation.org/".to_string(),
        }]
    }

    #[test]
    fn the_footer_renders_a_centered_copyright_line() {
        fn app() -> Element {
            rsx! {
                FirmFooter { model: model(vec![]) }
            }
        }
        let html = ssr(app);
        assert!(html.contains("© 2026 Shook Law PLLC"), "{html}");
        assert!(html.contains("app-footer__copyright"), "{html}");
        assert!(html.starts_with("<footer"), "{html}");
    }

    #[test]
    fn the_footer_ends_with_the_tagline_after_all_brands_and_platform_details() {
        let html = ssr(|| rsx! { FirmFooter { model: model(vec![]) } });
        let disclaimer = html
            .find("Attorney advertisement. Nothing here is legal advice without a signed retainer for an active project. Past results do not guarantee future outcomes.")
            .expect("the disclaimer renders");
        let copyright = html
            .find("© 2026 Shook Law PLLC")
            .expect("the copyright renders");
        let trademark = html.find("NEON LAW").expect("the trademark renders");
        let platform = html
            .find("app-footer__platform")
            .expect("the platform line renders");
        let tagline = html
            .rfind(FOOTER_TAGLINE)
            .expect("the final tagline renders");
        assert!(disclaimer < copyright && copyright < trademark);
        assert!(trademark < platform && platform < tagline);
        assert_eq!(html.matches(FOOTER_TAGLINE).count(), 1);
        assert!(
            html.contains(r#"href="https://tmsearch.uspto.gov/search/search-results/90039224""#)
        );
        assert!(html.contains(r#"href="https://github.com/neon-law-source-code/navigator""#));
    }

    #[test]
    fn the_footer_renders_the_source_repository_without_a_version() {
        fn app() -> Element {
            rsx! {
                FirmFooter { model: model(vec![]) }
            }
        }
        let html = ssr(app);
        assert!(html.contains("Powered by"), "the platform wording: {html}");
        assert!(
            html.contains("neon-law-source-code/navigator"),
            "the source repository: {html}"
        );
        assert!(
            !html.contains("app-footer__release"),
            "no version under cargo run: {html}"
        );
    }

    #[test]
    fn the_footer_appends_the_release_stamp() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: FirmFooterModel {
                        navigator_version: "26.8.20".to_string(),
                        ..model(vec![])
                    },
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("#26.8.20"), "{html}");
    }

    /// A single brand — a firm with no family to name — renders no "Our
    /// Family" row.
    #[test]
    fn a_single_brand_renders_no_family_row() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![FirmFooterBrand {
                        label: "Neon Law".to_string(),
                        href: "https://www.neonlaw.com".to_string(),
                        current: true,
                        byline: String::new(),
                    }]),
                }
            }
        }
        let html = ssr(app);
        assert!(!html.contains("app-footer__family"), "{html}");
        assert!(
            !html.contains("Proud member"),
            "no membership given: {html}"
        );
    }

    /// A Firm wearing several brands lists every one, in the given order,
    /// with the current brand unlinked and every other one a link.
    #[test]
    fn several_brands_render_in_order_with_the_current_one_unlinked() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        FirmFooterBrand {
                            label: "Emerging Technologies Counsel".to_string(),
                            href: "https://www.neonlaw.com".to_string(),
                            current: true,
                            byline: String::new(),
                        },
                        FirmFooterBrand {
                            label: "Protect your info".to_string(),
                            href: "https://www.deleteyourdata.com".to_string(),
                            current: false,
                            byline: String::new(),
                        },
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(r#"<nav class="app-footer__family" aria-label="Our family">"#)
                && html.contains(r#"<h2 class="app-footer__family-heading">Our Family</h2>"#),
            "a landmark named by its visible heading: {html}"
        );
        let neon = html.find("Emerging Technologies Counsel").expect("neon");
        let dyd = html.find("Protect your info").expect("dyd");
        assert!(neon < dyd, "registry order: {html}");
        assert!(
            html.contains(
                r#"<span class="app-footer__family-current" aria-current="true">Emerging Technologies Counsel</span>"#
            ),
            "current brand is text, marked current: {html}"
        );
        assert!(
            !html.contains(r#"href="https://www.neonlaw.com""#),
            "current brand is unlinked: {html}"
        );
        assert!(
            html.contains(r#"href="https://www.deleteyourdata.com""#),
            "other brands link their home host: {html}"
        );
    }

    /// A runtime-created brand with no dedicated host yet renders as plain
    /// text rather than an inert `<a href="">`.
    #[test]
    fn a_runtime_brand_with_no_href_renders_unlinked_not_as_an_empty_anchor() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        FirmFooterBrand {
                            label: "Neon Law".to_string(),
                            href: "https://www.neonlaw.com".to_string(),
                            current: true,
                            byline: String::new(),
                        },
                        FirmFooterBrand {
                            label: "Acme Runtime Brand".to_string(),
                            href: String::new(),
                            current: false,
                            byline: String::new(),
                        },
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("Acme Runtime Brand"), "{html}");
        assert!(!html.contains(r#"href="""#), "no empty anchor: {html}");
    }

    /// Every class the footer emits is styled by the theme it ships with.
    /// Mirrors `sample_matters_banner`'s own guard: a renamed class with no
    /// matching rule renders as unstyled text and nothing else catches it.
    #[test]
    fn every_class_the_footer_emits_is_styled_by_the_theme() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: FirmFooterModel {
                        navigator_version: "26.8.20".to_string(),
                        memberships: one_membership(),
                        ..model(vec![
                            FirmFooterBrand {
                                label: "Neon Law".to_string(),
                                href: "https://www.neonlaw.com".to_string(),
                                current: true,
                                byline: String::new(),
                            },
                            FirmFooterBrand {
                                label: "DeleteYourData.com".to_string(),
                                href: "https://www.deleteyourdata.com".to_string(),
                                current: false,
                                byline: String::new(),
                            },
                        ])
                    },
                }
            }
        }

        let theme =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/public/css/theme.css");
        let css = std::fs::read_to_string(&theme)
            .unwrap_or_else(|e| panic!("the theme stylesheet must be readable: {e}"));
        let out = ssr(app);

        let mut classes = Vec::new();
        let mut rest = out.as_str();
        while let Some(at) = rest.find("class=\"") {
            rest = &rest[at + 7..];
            let Some(end) = rest.find('"') else { break };
            classes.extend(rest[..end].split_whitespace().map(str::to_string));
            rest = &rest[end..];
        }
        assert!(!classes.is_empty(), "the footer emits classes: {out}");

        for class in classes
            .iter()
            .filter(|class| class.starts_with("app-footer"))
        {
            assert!(
                css.contains(&format!(".{class}")),
                "`{class}` is emitted by the footer but has no rule in \
                 server/public/css/theme.css"
            );
        }
    }

    /// The membership line names the association, links its own site, and
    /// wears the off-site treatment — a new tab, the OWASP `rel` pair, the
    /// arrow — between the family row and the platform line.
    #[test]
    fn a_membership_renders_as_a_proud_member_line_linking_off_site() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: FirmFooterModel {
                        memberships: one_membership(),
                        ..model(vec![])
                    },
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(
                r#"<p class="app-footer__membership"><a href="https://justicetechassociation.org/" class="app-footer__membership-link""#
            ),
            "{html}"
        );
        assert!(
            html.contains("Proud member of the Justice Technology Association"),
            "{html}"
        );
        assert!(
            html.contains(r#"target="_blank""#)
                && html.contains(r#"rel="noopener noreferrer""#)
                && html.contains("<title>opens in a new tab</title>"),
            "off-site treatment: {html}"
        );
        let member = html.find("Proud member").expect("membership");
        let platform = html.find("app-footer__platform").expect("platform");
        assert!(
            member < platform,
            "membership before the platform line: {html}"
        );
    }

    /// The compiled fallback lists the live compiled family — every house
    /// brand, in registry order, the request's key current and every other
    /// linking its production home — and the firm's membership, so the rows
    /// render before any Firm row exists.
    #[cfg(feature = "server")]
    #[test]
    fn the_compiled_fallback_lists_the_whole_family_and_the_membership() {
        let model =
            compiled_firm_footer_model(views::brand::BrandKey::DeleteYourData, 2026, String::new());
        let labels: Vec<&str> = model.brands.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Emerging Technologies Counsel",
                "Protect your info",
                "DeleteYourDebt.com",
                "Vesta Estate Planning",
                "Misericordia Injury Law",
                "Abhaya Immigration",
                "Lawyer Shook"
            ]
        );
        let current: Vec<bool> = model.brands.iter().map(|b| b.current).collect();
        assert_eq!(current, [false, true, false, false, false, false, false]);
        assert_eq!(model.brands[0].href, "https://www.neonlaw.com");
        assert_eq!(model.brands[6].href, "https://www.lawyershook.com");
        assert_eq!(model.legal_entity, "Shook Law PLLC");
        assert_eq!(model.memberships.len(), 1);
        assert_eq!(model.memberships[0].label, "Justice Technology Association");
    }

    #[cfg(feature = "server")]
    #[test]
    fn the_footer_uses_the_requested_public_family_labels() {
        assert_eq!(
            compiled_footer_label(views::brand::BrandKey::Neon),
            "Emerging Technologies Counsel"
        );
        assert_eq!(
            compiled_footer_label(views::brand::BrandKey::DeleteYourData),
            "Protect your info"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn a_non_default_brand_does_not_inherit_the_neon_trademark_notice() {
        let model =
            compiled_firm_footer_model(views::brand::BrandKey::DeleteYourData, 2026, String::new());
        assert!(model.trademark.is_empty());
        assert!(model.trademark_registration.is_empty());
        assert!(!render_firm_footer(model).contains("NEON LAW"));
    }

    /// ENG-589: the resolver reads the live Firm/Entity/brand rows, not a
    /// process-wide constant — a Firm wearing the compiled `neon` key names
    /// its own Entity, in registry order, current first.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn resolves_the_firm_wearing_the_compiled_key() {
        let surreal = store::surreal::test_support::mem().await;
        let admin = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm Admin",
                "firm-footer-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Shook Law PLLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Shook Law".to_string(),
                status: "active".to_string(),
                entity_id: entity.id,
                admin_dri_person_id: admin.id,
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm.id, "neon")
            .await
            .unwrap();
        store::firms::attach_brand(&surreal, firm.id, "delete-your-data")
            .await
            .unwrap();

        let model =
            resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026, String::new())
                .await;

        assert_eq!(model.legal_entity, "Shook Law PLLC");
        assert_eq!(model.brands.len(), 2);
        assert!(model.brands[0].current);
        assert_eq!(model.brands[0].label, "Emerging Technologies Counsel");
    }

    /// A second Firm wearing a runtime-created brand (never one of the compiled
    /// compiled keys) alongside a compiled one: the footer for its compiled
    /// key names *that* Firm's Entity and lists both of *its* brands by their
    /// live names — never the other, seeded Firm's name or brands.
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_second_firm_with_a_runtime_brand_is_isolated_from_the_first() {
        let surreal = store::surreal::test_support::mem().await;

        // The first Firm, wearing the other compiled key.
        let admin_a = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm A Admin",
                "firm-a-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity_a = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Firm A Legal Entity LLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm_a = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Firm A".to_string(),
                status: "active".to_string(),
                entity_id: entity_a.id,
                admin_dri_person_id: admin_a.id,
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm_a.id, "neon")
            .await
            .unwrap();

        // The second Firm, wearing the reachable compiled key plus a
        // runtime-created one with no dedicated host yet.
        let admin_b = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm B Admin",
                "firm-b-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity_b = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Firm B Legal Entity LLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm_b = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Firm B".to_string(),
                status: "active".to_string(),
                entity_id: entity_b.id,
                admin_dri_person_id: admin_b.id,
            },
        )
        .await
        .unwrap();
        store::brands::create(
            &surreal,
            store::persons::Role::Owner,
            None,
            &store::brands::NewBrand {
                name: "Acme Runtime Brand".to_string(),
                key: "acme-runtime-brand".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm_b.id, "lawyer-shook")
            .await
            .unwrap();
        store::firms::attach_brand(&surreal, firm_b.id, "acme-runtime-brand")
            .await
            .unwrap();

        let model = resolve_firm_footer_model(
            &surreal,
            views::brand::BrandKey::LawyerShook,
            2026,
            String::new(),
        )
        .await;

        assert_eq!(model.legal_entity, "Firm B Legal Entity LLC");
        assert_ne!(model.legal_entity, "Firm A Legal Entity LLC");
        let labels: Vec<&str> = model.brands.iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"Lawyer Shook"));
        assert!(labels.contains(&"Acme Runtime Brand"));
        assert!(
            !labels.iter().any(|label| label.contains("neon")),
            "Firm A's brand must never appear on Firm B's footer: {labels:?}"
        );
    }

    /// A brand key no Firm wears — the pre-seed database — falls back to the
    /// compiled `Branding`, unchanged from every deployment's footer today.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn a_brand_key_no_firm_wears_falls_back_to_the_compiled_branding() {
        let surreal = store::surreal::test_support::mem().await;

        let model =
            resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026, String::new())
                .await;

        assert_eq!(
            model.legal_entity,
            views::brand::DEFAULT_BRANDING.firm.legal_entity
        );
        // Only the reachable brands: the registry holds more than the
        // footer advertises. See `BrandKey::is_live`.
        assert_eq!(
            model.brands.len(),
            views::brand::BrandKey::ALL
                .iter()
                .filter(|key| key.is_live())
                .count()
        );
        assert!(model.brands[0].current);
        assert_eq!(model.memberships.len(), 1);
    }

    // --- ENG-741: bylines and the two-column split ------------------------

    fn brand(label: &str, href: &str, current: bool, byline: &str) -> FirmFooterBrand {
        FirmFooterBrand {
            label: label.to_string(),
            href: href.to_string(),
            current,
            byline: byline.to_string(),
        }
    }

    /// The whole point of the block: a wordmark alone tells a cold reader
    /// nothing, so every entry carries what it does.
    #[test]
    fn a_brands_byline_renders_beside_its_name() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        brand("Neon Law", "", true, "flat-fee legal services for emerging tech"),
                        brand(
                            "Vesta Estate Planning",
                            "https://www.vestaestateplanning.com",
                            false,
                            "wills, trusts, and probate",
                        ),
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("wills, trusts, and probate"), "{html}");
        assert!(
            html.contains("flat-fee legal services for emerging tech"),
            "the current brand keeps its byline too: {html}"
        );
    }

    /// A runtime brand with no compiled line renders its wordmark alone
    /// rather than a dangling em dash.
    #[test]
    fn a_brand_without_a_byline_renders_no_dash() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        brand("Neon Law", "", true, ""),
                        brand("Someone Else", "https://example.test", false, ""),
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(!html.contains("app-footer__family-byline"), "{html}");
        assert!(!html.contains(" · "), "{html}");
    }

    /// Every multi-brand family gets the desktop two-column class; the
    /// responsive stylesheet collapses it to one column on mobile.
    #[test]
    fn the_family_list_splits_into_two_columns_only_once_it_is_long() {
        assert_eq!(family_list_class(1), "app-footer__family-list");
        assert_eq!(
            family_list_class(2),
            "app-footer__family-list app-footer__family-list--two-column"
        );
        assert_eq!(
            family_list_class(8),
            "app-footer__family-list app-footer__family-list--two-column"
        );
    }

    /// A multi-brand family renders with the two-column class end to end, not
    /// just in the class helper.
    #[test]
    fn a_multi_brand_family_renders_two_columns() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(
                        (0..7)
                            .map(|i| brand(&format!("Brand {i}"), "https://example.test", i == 0, "does a thing"))
                            .collect(),
                    ),
                }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains("app-footer__family-list--two-column"),
            "{html}"
        );
    }

    /// `DeleteYourDebt`'s byline is a regulatory boundary, not a style choice.
    ///
    /// "Defend against debt collectors" describes FDCPA and
    /// collection-defence work. Wording that promises to settle, reduce, or
    /// negotiate down a balance describes debt settlement — a separate
    /// regulated activity under the FTC Telemarketing Sales Rule's
    /// advance-fee provisions and state debt-adjuster licensing, whose
    /// attorney exemption is narrower than it is usually assumed to be. A
    /// copy edit here is a scope change, so it fails the build.
    #[test]
    fn no_family_byline_drifts_toward_debt_settlement() {
        for key in views::brand::BrandKey::ALL {
            let byline = key.family_byline().to_lowercase();
            for banned in [
                "settle",
                "settlement",
                "reduce what you owe",
                "negotiate",
                "pennies",
                "write off",
                "forgive",
                "eliminate your debt",
            ] {
                assert!(
                    !byline.contains(banned),
                    "{} byline contains settlement framing {banned:?}: {byline:?}",
                    key.as_str(),
                );
            }
        }
    }

    /// A brand nobody can visit stays out of "Our Family".
    ///
    /// The row is a link, so listing an unreachable host advertises a
    /// practice a reader cannot get to. For the NYC summons practice it is
    /// sharper than a dead link: holding out a New York practice before
    /// admission is a licensing problem, not a cosmetic one. The brand whose
    /// page the reader is *on* is exempt, since it renders as plain text
    /// rather than a link and a site that omitted itself would be stranger
    /// still.
    #[cfg(feature = "server")]
    #[test]
    fn an_unreachable_brand_stays_out_of_the_family_row() {
        let rows = compiled_family_brands(views::brand::BrandKey::Neon);
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();

        for key in views::brand::BrandKey::ALL {
            let label = compiled_footer_label(*key);
            if key.is_live() {
                assert!(
                    labels.contains(&label.as_str()),
                    "{label} is live and is listed: {labels:?}"
                );
            } else {
                assert!(
                    !labels.contains(&label.as_str()),
                    "{label} is not reachable and must not be advertised: {labels:?}"
                );
            }
        }
    }

    /// The seeded `brand` row wears the same wordmark the compiled
    /// `Branding` publishes.
    ///
    /// The two footer paths read different sources for one label:
    /// [`compiled_family_brands`] takes `SiteBrand::site_name`, and
    /// [`resolve_firm_footer_model`] takes `brand.name` from the row
    /// `store::seed` wrote. `store` cannot depend on `views`, so that row's
    /// name is a hand-copied constant — and this is the only place in the
    /// workspace that can see both halves and hold them together. Without
    /// it, a deployment with a seeded Firm quietly publishes a different
    /// wordmark than the same page renders before seeding.
    #[cfg(feature = "server")]
    #[test]
    fn every_compiled_brand_seeds_its_published_wordmark() {
        for key in views::brand::BrandKey::ALL {
            let published = key
                .resolve_branding(&views::brand::DEFAULT_BRANDING)
                .firm
                .site_name;
            assert_eq!(
                store::seed::compiled_brand_name(key.as_str()),
                Some(published),
                "{} seeds a brand row named for the wordmark it publishes",
                key.as_str(),
            );
        }
    }

    /// Every compiled brand carries a line, so the family block can never
    /// render a bare wordmark for a brand the firm actually ships.
    #[test]
    fn every_compiled_brand_has_a_byline() {
        for key in views::brand::BrandKey::ALL {
            assert!(
                !key.family_byline().is_empty(),
                "{} has no family byline",
                key.as_str(),
            );
        }
    }
}
