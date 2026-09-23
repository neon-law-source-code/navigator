//! `/workshops`, `/presentations`, and `/notations` — the firm's material indexes.
//!
//! Categories share the public shell and material list. The portal pre-layer
//! injects each category's content; Notations also carries a format introduction
//! above its templates.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    CatalogHero, PlainCodeBlock, PublicShell, SiteHeader, SiteNavLink, SocialMeta, TestimonialCard,
    TestimonialSection, CATALOG_STYLESHEET_HREF,
};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// One material in an index — a workshop or a presentation. `eyebrow` is the
/// small uppercase line above the title, naming the audience the material is
/// written for.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogMaterial {
    pub href: String,
    pub eyebrow: String,
    pub title: String,
    pub summary: String,
}

/// One category index's resolved content, built per request by the portal
/// pre-layer and injected for [`catalog_index_view`]. The wasm-safe carrier
/// across the server-function boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogIndexContent {
    /// The category's heading, and the page title after the brand name.
    pub title: String,
    /// Optional long-form introduction above the material catalog.
    #[serde(default)]
    pub introduction: Option<crate::marketing_page::PageContent>,
    /// The hero paragraph, reused as the page's meta description.
    pub lede: String,
    pub materials: Vec<CatalogMaterial>,
    /// The inbox the empty state writes to.
    pub contact_email: String,
    /// The line under the list. Empty renders nothing.
    pub footnote: String,
    /// Whether the catalog ends with the public testimonials section.
    #[serde(default)]
    pub include_testimonials: bool,
    /// The request's resolved brand, used only for the server-side store read.
    #[serde(default)]
    pub brand_key: String,
}

/// The [`CatalogIndexContent`] the portal pre-layer injects, read back in
/// [`catalog_index_view`].
#[derive(Clone, Default)]
pub struct InjectedCatalogIndex(pub CatalogIndexContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogIndexView {
    pub chrome: PublicChrome,
    pub content: CatalogIndexContent,
    #[serde(default)]
    pub testimonials: Vec<TestimonialCard>,
}

/// Resolve the shared chrome and this category's injected content.
#[server]
pub async fn catalog_index_view() -> Result<CatalogIndexView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedCatalogIndex>,
        _,
    >()
    .await
    .map(|axum::Extension(c)| c.0)
    .unwrap_or_default();
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let testimonials = if content.include_testimonials {
        store::testimonials::published_for_brand(&surreal, &content.brand_key, 12)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?
            .into_iter()
            .map(|testimonial| TestimonialCard {
                quote: testimonial.quote,
                attribution: testimonial.attribution_label.unwrap_or_default(),
                detail: None,
                profile_image_url: None,
                product_label: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(CatalogIndexView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
        testimonials,
    })
}

/// The page's route entry, mounted once per category.
#[component]
pub fn CatalogIndexEntry() -> Element {
    let resource = use_server_future(catalog_index_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        CatalogIndexPage {
            chrome: view.chrome,
            content: view.content,
            testimonials: view.testimonials,
        }
    }
}

/// The pure index page. Prop-driven, so it server-renders and unit-tests
/// without a server future.
#[component]
pub fn CatalogIndexPage(
    chrome: PublicChrome,
    content: CatalogIndexContent,
    #[props(default)] testimonials: Vec<TestimonialCard>,
) -> Element {
    let header = rsx! {
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
    };
    let footer = rsx! {
        PublicFooter { chrome: chrome.clone() }
    };
    let mailto = format!("mailto:{}", content.contact_email);
    let head_title = format!("{} | {}", chrome.brand_name, content.title);
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: content.lede.clone() }
        // The share card. Load-bearing on the presentations index in
        // particular: the talks are the firm's most-shared public surface, so
        // this is the preview every pasted link to a talk index renders.
        //
        // Not assertable from the unit tests below: `document::*` hoists into
        // `<head>` during the real SSR pipeline and never appears in
        // `dioxus_ssr::render` output. The covering test is the
        // `brand_routing.feature` scenario that greps `og:site_name` off `/`
        // through the real router.
        SocialMeta {
            title: head_title.clone(),
            description: content.lede.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        PublicShell { header, footer,
            if let Some(introduction) = content.introduction.clone() {
                NotationsIntroduction { content: introduction }
            } else {
                CatalogHero {
                    eyebrow: chrome.brand_name.clone(),
                    title: content.title.clone(),
                    lede: content.lede.clone(),
                }
            }
            section { id: "templates", class: "catalog-library",
            if content.introduction.is_some() {
                h2 { "Explore the templates" }
            }
            if content.materials.is_empty() {
                p { class: "catalog-empty",
                    "This catalog is still loading. Email "
                    a { href: "{mailto}", "{content.contact_email}" }
                    " for the runbook in the meantime."
                }
            } else {
                CatalogMaterialList { materials: content.materials.clone() }
                if !content.footnote.is_empty() {
                    p { class: "catalog-more", "{content.footnote}" }
                }
            }
            }
            if content.include_testimonials {
                TestimonialSection {
                    heading: "Testimonials".to_string(),
                    lead: String::new(),
                    cards: testimonials,
                }
            }
        }
    }
}

/// The format's opening specimen, followed by the shared marketing bands.
#[component]
fn NotationsIntroduction(content: crate::marketing_page::PageContent) -> Element {
    let specimen = "---\nkind: agreement\ntitle: Sample agreement\nprompts:\n  delivery: How should notices arrive?\nchoices:\n  delivery:\n    email: By email\n    post: By post\nquestionnaire:\n  BEGIN: { _: custom_single_choice__delivery }\n  custom_single_choice__delivery: { _: END }\n  END: {}\n# … workflow and other metadata\n---\n\n## I. Notices\n\nDelivery method: {{custom_single_choice__delivery}}.";
    rsx! {
        document::Stylesheet { href: crate::marketing_page::MARKETING_STYLESHEET_HREF }
        document::Stylesheet { href: "/public/css/notations.css" }
        div { class: "notations-page",
            section { class: "notations-hero",
                div { class: "notations-hero__copy",
                    h1 { "{content.tagline}" }
                    div { class: "notations-lead",
                        crate::marketing_page::Prose { runs: content.hero_lead_runs.clone() }
                    }
                    div { class: "notations-actions",
                        if let Some(cta) = &content.hero_cta {
                            a { class: "nav-btn nav-btn--primary", href: "{cta.href}", "{cta.label} ↗" }
                        }
                        a { href: "#notation-flow", "Follow the format ↓" }
                    }
                }
                figure { class: "notations-specimen", id: "notation-source",
                    figcaption { span { "agreement.md" } }
                    PlainCodeBlock { code: specimen.to_string() }
                    div { class: "notations-specimen__footer",
                        span { "YAML Frontmatter" }
                        span { "+" }
                        span { "Markdown body" }
                    }
                }
            }
            crate::marketing_page::Bands { items: content.bands, notation_mark: true }
        }
    }
}

/// One list of materials.
#[component]
fn CatalogMaterialList(materials: Vec<CatalogMaterial>) -> Element {
    rsx! {
        ul { class: "catalog-materials",
            for material in materials.iter() {
                li { class: "catalog-material",
                    p { class: "catalog-eyebrow", "{material.eyebrow}" }
                    h3 {
                        a { href: "{material.href}", "{material.title}" }
                    }
                    p { "{material.summary}" }
                }
            }
        }
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

    fn material(title: &str, href: &str) -> CatalogMaterial {
        CatalogMaterial {
            href: href.to_string(),
            eyebrow: "For lawyers".to_string(),
            title: title.to_string(),
            summary: "What you take away.".to_string(),
        }
    }

    fn workshops() -> CatalogIndexContent {
        CatalogIndexContent {
            title: "Workshops".to_string(),
            introduction: None,
            lede: "Hands-on classes.".to_string(),
            materials: vec![
                material("Using Neon Law Navigator", "/workshops/use-the-navigator"),
                material(
                    "Operating Neon Law Navigator",
                    "/workshops/deploy-the-navigator",
                ),
            ],
            contact_email: "support@example.org".to_string(),
            footnote: "More classes land here as we run them.".to_string(),
            include_testimonials: false,
            brand_key: String::new(),
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CatalogIndexPage { chrome: PublicChrome::default(), content: workshops() }
            }
        }
        ssr(app)
    }

    #[test]
    fn the_heading_names_the_category() {
        let out = html();
        assert!(out.contains("Workshops"), "category heading: {out}");
    }

    #[test]
    fn each_material_links_to_its_own_page() {
        let out = html();
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator""#),
            "first material href: {out}"
        );
        assert!(
            out.contains(r#"href="/workshops/deploy-the-navigator""#),
            "second material href: {out}"
        );
    }

    #[test]
    fn the_index_advertises_a_gated_class_it_cannot_open() {
        // The index is public while the material behind it is not: an
        // anonymous reader must still learn the class exists. Losing the
        // title or the summary here turns the gate into a dead end.
        let out = html();
        assert!(out.contains("Operating Neon Law Navigator"), "title: {out}");
        assert!(out.contains("What you take away."), "summary: {out}");
    }

    #[test]
    fn the_footnote_renders_under_the_list() {
        let out = html();
        assert!(
            out.contains("More classes land here as we run them."),
            "footnote: {out}"
        );
    }

    #[test]
    fn an_empty_category_offers_the_inbox_instead() {
        fn app() -> Element {
            let content = CatalogIndexContent {
                title: "Presentations".to_string(),
                contact_email: "support@example.org".to_string(),
                ..CatalogIndexContent::default()
            };
            rsx! {
                CatalogIndexPage { chrome: PublicChrome::default(), content }
            }
        }
        let out = ssr(app);
        assert!(
            out.contains("support@example.org") && out.contains("mailto:support@example.org"),
            "the empty state must offer the inbox: {out}"
        );
    }

    #[test]
    fn notations_catalog_links_the_letters_and_a_form() {
        fn app() -> Element {
            let content = CatalogIndexContent {
                title: "Notations".to_string(),
                introduction: None,
                lede: "One markdown file is the template, questionnaire, and workflow.".to_string(),
                materials: vec![
                    CatalogMaterial {
                        href: "/notations/onboarding-letter".to_string(),
                        eyebrow: "Letter".to_string(),
                        title: "Onboarding Letter".to_string(),
                        summary: "Opens a matter.".to_string(),
                    },
                    CatalogMaterial {
                        href: "https://github.com/neon-law-source-code/navigator/blob/main/templates/notations/forms/united_states/nevada/state/nv__llc_formation.md".to_string(),
                        eyebrow: "Form · Nevada".to_string(),
                        title: "Nevada LLC Formation".to_string(),
                        summary: "Articles of organization.".to_string(),
                    },
                ],
                contact_email: "support@example.org".to_string(),
                footnote: String::new(),
                include_testimonials: false,
                brand_key: String::new(),
            };
            rsx! {
                CatalogIndexPage { chrome: PublicChrome::default(), content }
            }
        }
        let out = ssr(app);
        assert!(out.contains("catalog-hero"), "catalog hero: {out}");
        assert!(out.contains("Onboarding Letter"), "letter title: {out}");
        assert!(
            out.contains(r#"href="/notations/onboarding-letter""#),
            "letter's default link opens the preview: {out}"
        );
        assert!(out.contains("nv__llc_formation.md"), "form href: {out}");
    }
}
