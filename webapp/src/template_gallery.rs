//! `/templates` — the template gallery, migrated to Dioxus SSR (#956 Phase 4).
//!
//! The successor to the `views::pages::templates`. The conversion
//! centerpiece of the "our legal documents are plain markdown" pitch: a
//! stretched nonprofit staffer browses a curated, client-safe subset of the
//! workspace `templates/` tree, sees the notation format itself (the YAML
//! frontmatter, verbatim), and downloads the raw `.md` to take with them.
//!
//! Every page carries the shared [`LegalBlueprintDisclaimer`] UPL guardrail and
//! ends with a "start a matter" call to action, so a download is never a dead
//! end. Firm-branded: this is a firm document-services surface that routes a
//! serious prospect into an opened matter.
//!
//! The curated allow-list lives in `portal::template_gallery` and is fixed at
//! compile time, so the index content is injected at construction; one
//! template's detail is selected by the `{*path}` parameter, so the portal
//! pre-layer resolves it per request — and owns the legacy alias, the
//! kebab-case redirect, the `/download` raw response, and the 404.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Card, LegalBlueprintDisclaimer, PublicShell, SiteHeader, SiteNavLink};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The gallery index's `<meta description>`, as the page set it.
const INDEX_DESCRIPTION: &str = "Browse and download Neon Law's legal templates — \
                                 plain-markdown notation you can take with you.";

/// One template's display fields, in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TemplateCard {
    /// Public detail URL for this template.
    pub href: String,
    /// File stem (`form990_annual_report`), shown verbatim as the download
    /// filename base.
    pub name: String,
    /// Human title, parsed from the template's frontmatter `title`.
    pub title: String,
    /// Plain-language "what it's for".
    pub blurb: String,
    /// Loud jurisdiction label (`Federal · United States`, `Nevada`).
    pub jurisdiction_label: String,
    /// The theme badge classes denoting the jurisdiction's weight — federal
    /// reads neutral, a state-specific filing reads as a caution so nobody
    /// assumes nationwide reach.
    pub badge_class: String,
}

/// The gallery index content, resolved by the portal router at construction.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GalleryContent {
    /// The localized gallery title, used for both the `<h1>` and the `<title>`.
    pub title: String,
    pub cards: Vec<TemplateCard>,
}

/// One template's detail page content, resolved per request.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TemplateDetailContent {
    pub card: TemplateCard,
    /// The YAML frontmatter block (inner, between the `---` fences), shown
    /// verbatim so the visitor sees the notation contract.
    pub frontmatter: String,
    /// `/templates/…/download` — the raw `.md` route.
    pub download_href: String,
    /// Where "start a matter" routes a serious prospect.
    pub start_matter_href: String,
}

/// The gallery content the portal router injects.
#[derive(Clone, Default)]
pub struct InjectedGallery(pub GalleryContent);

/// The detail content the portal pre-layer injects for the matched path.
#[derive(Clone, Default)]
pub struct InjectedTemplateDetail(pub TemplateDetailContent);

/// Everything the gallery index renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GalleryView {
    pub chrome: PublicChrome,
    pub content: GalleryContent,
}

/// Everything a detail page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TemplateDetailView {
    pub chrome: PublicChrome,
    pub content: TemplateDetailContent,
}

/// Resolve the gallery index.
#[server]
pub async fn gallery_view() -> Result<GalleryView, ServerFnError> {
    Ok(GalleryView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content: consume_context::<InjectedGallery>().0,
    })
}

/// Resolve one template's detail page from the injected extension.
#[server]
pub async fn template_detail_view() -> Result<TemplateDetailView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedTemplateDetail>,
        _,
    >()
    .await
    .map_or_else(
        |_| TemplateDetailContent::default(),
        |axum::Extension(injected)| injected.0,
    );
    Ok(TemplateDetailView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The gallery index's route entry.
#[component]
pub fn GalleryEntry() -> Element {
    let resource = use_server_future(gallery_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    gallery_body(&view)
}

/// A detail page's route entry.
#[component]
pub fn TemplateDetailEntry() -> Element {
    let resource = use_server_future(template_detail_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    template_detail_body(&view)
}

/// The firm public-shell header for a gallery page.
fn gallery_header(chrome: &PublicChrome) -> Element {
    rsx! {
        SiteHeader {
            brand_name: chrome.brand_name.clone(),
            home_href: chrome.home_href.clone(),
            logo_href: chrome.logo_href.clone(),
            destinations: chrome
                .destinations
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
            utility: chrome
                .utility
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
        }
    }
}

/// The shared unified footer for a gallery page.
fn gallery_footer(chrome: &PublicChrome) -> Element {
    rsx! {
        PublicFooter { chrome: chrome.clone() }
    }
}

/// The jurisdiction badge — the loud "which law is this" marker.
#[component]
fn JurisdictionBadge(label: String, badge_class: String) -> Element {
    rsx! {
        span { class: "{badge_class}", "{label}" }
    }
}

/// `GET /templates` — the gallery index.
pub fn gallery_body(view: &GalleryView) -> Element {
    let chrome = &view.chrome;
    let header = gallery_header(chrome);
    let footer = gallery_footer(chrome);
    let head_title = format!("{} | {}", chrome.brand_name, view.content.title);
    let title = view.content.title.clone();
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: INDEX_DESCRIPTION }
        PublicShell { header, footer,
            article {
                header {
                    h1 { "{title}" }
                    p { class: "lead",
                        "Our legal documents are plain-markdown "
                        em { "notation" }
                        " — no proprietary format, no lock-in. Open one to "
                        "see the format, then download the raw "
                        code { ".md" }
                        " and take it with you."
                    }
                }
                LegalBlueprintDisclaimer {}
                div { class: "template-gallery",
                    for card in view.content.cards.iter() {
                        GalleryCard { card: card.clone() }
                    }
                }
            }
        }
    }
}

/// One template's card on the index.
#[component]
fn GalleryCard(card: TemplateCard) -> Element {
    rsx! {
        Card {
            JurisdictionBadge {
                label: card.jurisdiction_label.clone(),
                badge_class: card.badge_class.clone(),
            }
            h2 { class: "template-card__title",
                a { href: "{card.href}", "{card.title}" }
            }
            p { class: "nav-muted", "{card.blurb}" }
            p {
                a { class: "nav-btn nav-btn--secondary", href: "{card.href}", "View notation" }
            }
        }
    }
}

/// `GET /templates/{*path}` — one template's detail page.
pub fn template_detail_body(view: &TemplateDetailView) -> Element {
    let chrome = &view.chrome;
    let header = gallery_header(chrome);
    let footer = gallery_footer(chrome);
    let card = &view.content.card;
    let head_title = format!("{} | {}", chrome.brand_name, card.title);
    // The frontmatter is shown verbatim between its fences, which rsx! cannot
    // express against an interpolation.
    let fenced = format!("---\n{}\n---", view.content.frontmatter);
    let download_label = format!("Download {}.md", card.name);
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: "{card.blurb}" }
        PublicShell { header, footer,
            article {
                p {
                    a { href: "/templates", "← All templates" }
                }
                header {
                    JurisdictionBadge {
                        label: card.jurisdiction_label.clone(),
                        badge_class: card.badge_class.clone(),
                    }
                    h1 { "{card.title}" }
                    p { class: "lead", "{card.blurb}" }
                }
                LegalBlueprintDisclaimer {}
                section { class: "template-notation",
                    h2 { "The notation format" }
                    p {
                        "Every Neon Law Navigator template is plain markdown with a YAML "
                        "header — the machine-readable contract the questionnaire "
                        "and workflow run on. Here is this template's, verbatim:"
                    }
                    pre {
                        code { "{fenced}" }
                    }
                }
                p {
                    a {
                        class: "nav-btn nav-btn--primary",
                        href: "{view.content.download_href}",
                        "{download_label}"
                    }
                }
                section { class: "template-start-matter",
                    Card {
                        h2 { "Want a lawyer to stand behind it?" }
                        p { class: "nav-muted",
                            "A template is a blueprint. To have a licensed attorney "
                            "prepare, review, and sign a document for your situation, "
                            "start a matter with the firm."
                        }
                        p {
                            a {
                                class: "nav-btn nav-btn--secondary",
                                href: "{view.content.start_matter_href}",
                                "Start a matter"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".to_string(),
            home_href: "/".to_string(),
            firm_name: "Neon Law".to_string(),
            ..PublicChrome::default()
        }
    }

    fn card() -> TemplateCard {
        TemplateCard {
            href: "/templates/united-states/federal/irs/taxation/form990-annual-report".to_string(),
            name: "form990_annual_report".to_string(),
            title: "IRS Form 990".to_string(),
            blurb: "The annual federal information return.".to_string(),
            jurisdiction_label: "Federal · United States".to_string(),
            badge_class: "nav-badge".to_string(),
        }
    }

    fn index_html() -> String {
        dioxus_ssr::render_element(gallery_body(&GalleryView {
            chrome: chrome(),
            content: GalleryContent {
                title: "Template gallery".to_string(),
                cards: vec![card()],
            },
        }))
    }

    fn detail_html() -> String {
        dioxus_ssr::render_element(template_detail_body(&TemplateDetailView {
            chrome: chrome(),
            content: TemplateDetailContent {
                card: card(),
                frontmatter: "code: form990\ntitle: IRS Form 990".to_string(),
                download_href: "/templates/united-states/federal/irs/taxation/\
                                form990-annual-report/download"
                    .to_string(),
                start_matter_href: "/contact".to_string(),
            },
        }))
    }

    #[test]
    fn the_index_lists_each_template_with_its_jurisdiction() {
        let out = index_html();
        assert!(out.contains("Template gallery"), "gallery title: {out}");
        // The detail link is kebab-cased even though the file stem keeps its
        // underscores.
        assert!(
            out.contains("/templates/united-states/federal/irs/taxation/form990-annual-report"),
            "kebab-cased detail link: {out}"
        );
        assert!(
            !out.contains("form990_annual_report</a>"),
            "the underscored stem is not the link text: {out}"
        );
        assert!(
            out.contains("Federal · United States"),
            "jurisdiction badge"
        );
    }

    /// The UPL guardrail rides every gallery page — a template is a blueprint,
    /// not legal advice, and the page may never suggest otherwise.
    #[test]
    fn both_pages_carry_the_legal_blueprint_disclaimer() {
        for out in [index_html(), detail_html()] {
            assert!(out.contains("not legal advice"), "disclaimer: {out}");
        }
    }

    #[test]
    fn the_detail_page_shows_the_frontmatter_verbatim_between_its_fences() {
        let out = detail_html();
        assert!(out.contains("code: form990"), "frontmatter body: {out}");
        assert!(out.contains("---"), "the fences frame it");
        assert!(out.contains("The notation format"), "section heading");
    }

    #[test]
    fn the_detail_page_offers_the_download_and_the_way_back() {
        let out = detail_html();
        assert!(out.contains("Download form990_annual_report.md"), "{out}");
        assert!(out.contains("/download"), "download href: {out}");
        assert!(out.contains(r#"href="/templates""#), "back to the index");
        assert!(out.contains("← All templates"), "back-link label");
    }

    /// A download must never be a dead end: every detail page routes a serious
    /// prospect into an opened matter.
    #[test]
    fn the_detail_page_ends_with_the_start_a_matter_call_to_action() {
        let out = detail_html();
        assert!(out.contains("Start a matter"), "CTA label: {out}");
        assert!(out.contains(r#"href="/contact""#), "CTA href: {out}");
    }

    #[test]
    fn both_pages_wear_the_firm_public_shell() {
        for out in [index_html(), detail_html()] {
            assert!(out.contains("site-header"), "header chrome: {out}");
            assert!(out.contains("site-footer__legal"), "footer chrome");
        }
        // Theme classes, not Bootstrap's grid/card/button vocabulary.
        let out = index_html();
        assert!(out.contains("nav-card"), "themed card: {out}");
        assert!(!out.contains("row-cols-md-2"), "no Bootstrap grid: {out}");
        assert!(!out.contains("btn-outline-primary"), "no Bootstrap buttons");
    }
}
