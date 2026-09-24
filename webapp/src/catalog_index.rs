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
///
/// `kind` and the two `category_*` fields are notation-only (LAW-53): the
/// template's own declared `kind:` frontmatter (via
/// `views::kind_catalog::declared_kind`, the same classifier `S103` runs) and
/// the category [`rules::kind::Kind::category`] groups it under. Empty for a
/// workshop or presentation, which carries no `kind:` at all — `Explore the
/// templates`' kind/category facet and gallery grouping render only when
/// `kind` is non-empty.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogMaterial {
    pub href: String,
    pub eyebrow: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub kind: String,
    /// The kind's title-cased display label (e.g. `"Letter"`), for the
    /// facet pill and the card's kind tag.
    #[serde(default)]
    pub kind_label: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub category_slug: String,
}

/// One kind-specific structural lint rule bound to a [`KindCatalogEntry`], as
/// a `(code, one-sentence requirement)` pair.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct KindRule {
    pub code: String,
    pub note: String,
}

/// One `kind:` value's `/notations` catalog entry (LAW-53) — its definition,
/// its category, and the structural rules bound to it. Built server-side
/// from `views::kind_catalog::entries()`, itself a projection of
/// `rules::kind::Kind` — the same enum `S103` validates a declared `kind:`
/// against — so this catalog cannot drift from the gate.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct KindCatalogEntry {
    pub kind: String,
    pub label: String,
    pub category: String,
    pub category_slug: String,
    pub definition: String,
    pub rules: Vec<KindRule>,
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
    /// Every `kind:` value `S103` accepts (LAW-53), for `/notations`' kind
    /// catalog section. Empty for every other catalog (workshops,
    /// presentations), which has no `kind:` vocabulary to document.
    #[serde(default)]
    pub kinds: Vec<KindCatalogEntry>,
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
            if !content.kinds.is_empty() {
                KindCatalogSection { kinds: content.kinds.clone() }
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
                if content.materials.iter().any(|m| !m.kind.is_empty()) {
                    TemplateGallery { materials: content.materials.clone() }
                } else {
                    CatalogMaterialList { materials: content.materials.clone() }
                }
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

/// The template gallery's category display order (LAW-53): correspondence
/// first — the letters most authors reach for — then the other
/// notation-family instruments, and the rarer families last. A category not
/// listed here sorts after every listed one, in first-seen order, so a
/// future category never disappears from the gallery silently.
const CATEGORY_ORDER: &[&str] = &[
    "correspondence",
    "instrument",
    "filing",
    "court-paper",
    "content",
    "engineering",
    "matter-dashboard",
];

/// Group `materials` by `category_slug`, in [`CATEGORY_ORDER`] order,
/// preserving each material's original relative order within its group. A
/// material with no `category_slug` (a workshop or presentation, which
/// carries no `kind:`) is dropped — this grouping exists only for the
/// notation gallery.
fn group_by_category(materials: &[CatalogMaterial]) -> Vec<(String, String, Vec<CatalogMaterial>)> {
    let mut groups: Vec<(String, String, Vec<CatalogMaterial>)> = Vec::new();
    for material in materials {
        if material.category_slug.is_empty() {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(slug, _, _)| *slug == material.category_slug)
        {
            Some(group) => group.2.push(material.clone()),
            None => groups.push((
                material.category_slug.clone(),
                material.category.clone(),
                vec![material.clone()],
            )),
        }
    }
    groups.sort_by_key(|(slug, _, _)| {
        CATEGORY_ORDER
            .iter()
            .position(|s| s == slug)
            .unwrap_or(CATEGORY_ORDER.len())
    });
    groups
}

/// Every distinct `(kind, kind_label, count)` present in `materials`, in
/// first-seen order — the kind-facet pills' data.
fn distinct_kinds(materials: &[CatalogMaterial]) -> Vec<(String, String, usize)> {
    let mut kinds: Vec<(String, String, usize)> = Vec::new();
    for material in materials {
        if material.kind.is_empty() {
            continue;
        }
        match kinds.iter_mut().find(|(kind, _, _)| *kind == material.kind) {
            Some(entry) => entry.2 += 1,
            None => kinds.push((material.kind.clone(), material.kind_label.clone(), 1)),
        }
    }
    kinds
}

/// The kind-filter pills' CSS, generated per bundled kind rather than
/// shipped as a static rule so a new bundled template's kind filters for
/// free. Checking a kind's pill hides every gallery card whose `data-kind`
/// does not match it, and hides a category section left with no visible
/// card. The "All" pill needs no rule: it is the default state every other
/// rule only overrides while its own pill is checked.
fn kind_filter_style(kinds: &[(String, String, usize)]) -> String {
    use std::fmt::Write;
    let mut css = String::new();
    for (kind, _, _) in kinds {
        let _ = writeln!(
            css,
            "#kind-filter-{kind}:checked ~ .template-gallery [data-kind]:not([data-kind=\"{kind}\"]) {{ display: none; }}"
        );
        let _ = writeln!(
            css,
            "#kind-filter-{kind}:checked ~ .template-gallery .template-gallery__group:not(:has([data-kind=\"{kind}\"])) {{ display: none; }}"
        );
    }
    css
}

/// The template gallery (LAW-53): a grid of preview cards grouped by
/// category, with a kind facet above it. Renders only for a catalog whose
/// materials carry a `kind` (today, `/notations` alone) — `CatalogMaterialList`
/// still serves a catalog with none.
#[component]
fn TemplateGallery(materials: Vec<CatalogMaterial>) -> Element {
    let kinds = distinct_kinds(&materials);
    let groups = group_by_category(&materials);
    let total = materials.len();
    let style = kind_filter_style(&kinds);
    rsx! {
        style { "{style}" }
        div {
            class: "template-gallery-wrapper",
            role: "radiogroup",
            "aria-label": "Filter templates by kind",
            input {
                r#type: "radio",
                name: "kind-filter",
                id: "kind-filter-all",
                class: "nav-template-filter__input",
                checked: true,
            }
            label { r#for: "kind-filter-all", class: "template-filter__pill", "All ({total})" }
            for (kind, label, count) in kinds.iter() {
                input {
                    r#type: "radio",
                    name: "kind-filter",
                    id: "kind-filter-{kind}",
                    class: "nav-template-filter__input",
                }
                label {
                    r#for: "kind-filter-{kind}",
                    class: "template-filter__pill",
                    "{label} ({count})"
                }
            }
            div { class: "template-gallery",
                for (slug, label, group_materials) in groups.iter() {
                    section {
                        class: "template-gallery__group",
                        key: "{slug}",
                        "data-category": "{slug}",
                        h3 { "{label}" }
                        ul { class: "template-gallery__cards",
                            for material in group_materials.iter() {
                                li {
                                    class: "template-card",
                                    key: "{material.href}",
                                    "data-kind": "{material.kind}",
                                    a { class: "template-card__link", href: "{material.href}",
                                        div { class: "template-card__face", "aria-hidden": "true",
                                            span { class: "template-card__face-kind", "{material.kind_label}" }
                                        }
                                        div { class: "template-card__body",
                                            p { class: "catalog-eyebrow", "{material.eyebrow}" }
                                            h3 { "{material.title}" }
                                            p { "{material.summary}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The `/notations` kind catalog (LAW-53): every `kind:` value `S103`
/// accepts, generated from [`rules::kind::Kind`] — the same enum `S103`
/// validates against — so it can never drift from the gate.
#[component]
fn KindCatalogSection(kinds: Vec<KindCatalogEntry>) -> Element {
    rsx! {
        section { id: "kinds", class: "kind-catalog",
            h2 { "Every notation kind" }
            p { class: "kind-catalog__intro",
                "Every value S103 accepts, generated from the same source the gate validates \
                 against — a definition, its category, and the structural rules bound to it."
            }
            ul { class: "kind-catalog__list",
                for entry in kinds.iter() {
                    li {
                        class: "kind-catalog__entry",
                        key: "{entry.kind}",
                        id: "kind-{entry.kind}",
                        "data-category": "{entry.category_slug}",
                        div { class: "kind-catalog__heading",
                            code { class: "kind-catalog__value", "{entry.kind}" }
                            span { class: "kind-catalog__category catalog-badge", "{entry.category}" }
                        }
                        p { class: "kind-catalog__definition", "{entry.definition}" }
                        if entry.rules.is_empty() {
                            p { class: "kind-catalog__rules kind-catalog__rules--none",
                                "No kind-specific structural rule beyond the shared Markdown \
                                 checks every file gets, regardless of kind."
                            }
                        } else {
                            ul { class: "kind-catalog__rules",
                                for rule in entry.rules.iter() {
                                    li { key: "{rule.code}",
                                        strong { "{rule.code}" }
                                        " — {rule.note}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
            ..CatalogMaterial::default()
        }
    }

    /// A kind-tagged material, for the gallery/facet tests — the shape
    /// `notation_card` builds once a real `kind:` is derived from the
    /// template's own frontmatter (LAW-53).
    fn kind_material(
        kind: &str,
        kind_label: &str,
        category: &str,
        category_slug: &str,
        title: &str,
        href: &str,
    ) -> CatalogMaterial {
        CatalogMaterial {
            href: href.to_string(),
            eyebrow: kind_label.to_string(),
            title: title.to_string(),
            summary: "A bundled sample.".to_string(),
            kind: kind.to_string(),
            kind_label: kind_label.to_string(),
            category: category.to_string(),
            category_slug: category_slug.to_string(),
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
            kinds: Vec::new(),
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
                        ..CatalogMaterial::default()
                    },
                    CatalogMaterial {
                        href: "https://github.com/neon-law-source-code/navigator/blob/main/templates/notations/forms/united_states/nevada/state/nv__llc_formation.md".to_string(),
                        eyebrow: "Form · Nevada".to_string(),
                        title: "Nevada LLC Formation".to_string(),
                        summary: "Articles of organization.".to_string(),
                        ..CatalogMaterial::default()
                    },
                ],
                contact_email: "support@example.org".to_string(),
                footnote: String::new(),
                include_testimonials: false,
                brand_key: String::new(),
            kinds: Vec::new(),
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

    fn kind_tagged_materials() -> Vec<CatalogMaterial> {
        vec![
            kind_material(
                "onboarding",
                "Onboarding",
                "Correspondence",
                "correspondence",
                "Onboarding Letter",
                "/notations/onboarding-letter",
            ),
            kind_material(
                "letter",
                "Letter",
                "Correspondence",
                "correspondence",
                "Nevada Engagement Letter",
                "/notations/nevada-engagement-letter",
            ),
            kind_material(
                "filing",
                "Filing",
                "Filing",
                "filing",
                "Nevada LLC Formation",
                "/notations/nevada-llc-formation",
            ),
        ]
    }

    #[test]
    fn group_by_category_orders_groups_and_keeps_material_order_within_each() {
        let groups = group_by_category(&kind_tagged_materials());
        let slugs: Vec<&str> = groups.iter().map(|(slug, _, _)| slug.as_str()).collect();
        // `correspondence` precedes `filing` in `CATEGORY_ORDER`.
        assert_eq!(slugs, vec!["correspondence", "filing"]);
        let correspondence = &groups[0].2;
        assert_eq!(correspondence.len(), 2);
        assert_eq!(correspondence[0].title, "Onboarding Letter");
        assert_eq!(correspondence[1].title, "Nevada Engagement Letter");
    }

    #[test]
    fn distinct_kinds_counts_each_kind_once_in_first_seen_order() {
        let kinds = distinct_kinds(&kind_tagged_materials());
        assert_eq!(
            kinds,
            vec![
                ("onboarding".to_string(), "Onboarding".to_string(), 1),
                ("letter".to_string(), "Letter".to_string(), 1),
                ("filing".to_string(), "Filing".to_string(), 1),
            ]
        );
    }

    #[test]
    fn kind_filter_style_hides_non_matching_cards_and_emptied_groups() {
        let css = kind_filter_style(&distinct_kinds(&kind_tagged_materials()));
        assert!(css.contains("#kind-filter-onboarding:checked"), "{css}");
        assert!(
            css.contains(r#"[data-kind]:not([data-kind="onboarding"])"#),
            "{css}"
        );
        assert!(
            css.contains(":not(:has([data-kind=\"onboarding\"]))"),
            "{css}"
        );
        // "All" needs no generated rule — it is the unfiltered default state.
        assert!(!css.contains("kind-filter-all"), "{css}");
    }

    fn notations_content_with_kind_facets() -> CatalogIndexContent {
        CatalogIndexContent {
            title: "Notations".to_string(),
            introduction: None,
            lede: "Every kind, documented.".to_string(),
            materials: kind_tagged_materials(),
            contact_email: "support@example.org".to_string(),
            footnote: String::new(),
            include_testimonials: false,
            brand_key: String::new(),
            kinds: vec![KindCatalogEntry {
                kind: "agreement".to_string(),
                label: "Agreement".to_string(),
                category: "Instrument".to_string(),
                category_slug: "instrument".to_string(),
                definition: "A private agreement (employment, contractor, LLC operating)"
                    .to_string(),
                rules: vec![KindRule {
                    code: "N123".to_string(),
                    note: "Depth-1 sections must be numbered with Roman numerals.".to_string(),
                }],
            }],
        }
    }

    #[test]
    fn the_gallery_renders_a_kind_filter_pill_per_bundled_kind() {
        fn app() -> Element {
            rsx! {
                CatalogIndexPage {
                    chrome: PublicChrome::default(),
                    content: notations_content_with_kind_facets(),
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains(r#"id="kind-filter-all""#), "all pill: {out}");
        assert!(
            out.contains(r#"id="kind-filter-onboarding""#),
            "onboarding pill: {out}"
        );
        assert!(
            out.contains(r#"for="kind-filter-onboarding""#),
            "onboarding pill label: {out}"
        );
    }

    #[test]
    fn the_gallery_groups_cards_by_category_with_a_heading_per_group() {
        fn app() -> Element {
            rsx! {
                CatalogIndexPage {
                    chrome: PublicChrome::default(),
                    content: notations_content_with_kind_facets(),
                }
            }
        }
        let out = ssr(app);
        assert!(
            out.contains(r#"data-category="correspondence""#),
            "correspondence group: {out}"
        );
        assert!(
            out.contains(r#"data-category="filing""#),
            "filing group: {out}"
        );
        assert!(
            out.contains(r#"data-kind="onboarding""#),
            "card carries its kind: {out}"
        );
        assert!(
            out.contains(r#"href="/notations/nevada-llc-formation""#),
            "card still links to its preview: {out}"
        );
    }

    #[test]
    fn the_kind_catalog_section_documents_a_kind_with_its_rule() {
        fn app() -> Element {
            rsx! {
                CatalogIndexPage {
                    chrome: PublicChrome::default(),
                    content: notations_content_with_kind_facets(),
                }
            }
        }
        let out = ssr(app);
        assert!(out.contains("Every notation kind"), "{out}");
        assert!(out.contains(r#"id="kind-agreement""#), "{out}");
        assert!(
            out.contains("A private agreement (employment, contractor, LLC operating)"),
            "{out}"
        );
        assert!(out.contains("N123"), "{out}");
        assert!(
            out.contains("Depth-1 sections must be numbered with Roman numerals."),
            "{out}"
        );
    }

    #[test]
    fn a_catalog_with_no_kinds_renders_no_kind_catalog_section() {
        // Workshops and presentations carry no `kind:` vocabulary — the
        // section must stay absent rather than rendering an empty heading.
        let out = html();
        assert!(!out.contains("Every notation kind"), "{out}");
    }
}
