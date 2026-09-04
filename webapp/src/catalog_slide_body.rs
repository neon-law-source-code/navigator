//! Shared rendering for a Catalog slide face.
//!
//! Most slides are authored Markdown and arrive as rendered HTML. A deliberately
//! small marker lets a slide opt into a first-party Dioxus component without
//! allowing arbitrary author-written HTML. The step page, projector display,
//! and light table all call this component so the three faces cannot drift.
//!
//! The firm-practice marker renders the same practice list the home page
//! publishes. That list is the English YAML catalog (`locales/en/home.yaml`);
//! this module does not keep a second copy of the doors.

use dioxus::prelude::*;

use crate::brand_style::{BRAND_STYLESHEET_HREF, BRAND_TOKENS_HREF};
use crate::components::{PracticeCard, PracticeMark, PracticeMarkGlyph};
use crate::home::{PracticeLink, HOME_STYLESHEET_HREF};

/// Markdown marker that replaces the ordinary slide body with the firm's
/// practice catalog, the same boxes `/` renders from YAML.
pub const FIRM_PRODUCT_CARDS_MARKER: &str = "{{firm-product-cards}}";

/// Markdown marker that renders the Navigator identity slide.
pub const NAVIGATOR_PRODUCT_MARKER: &str = "{{navigator-product}}";

/// The practice doors injected by the brand crate from `locales/en/home.yaml`.
///
/// Workshop faces that expand [`FIRM_PRODUCT_CARDS_MARKER`] read this rather
/// than a Rust list, so a YAML edit is the only copy change.
#[derive(Clone, Default)]
pub struct InjectedPracticeCatalog(pub Vec<PracticeLink>);

/// Render one slide body, selecting a first-party component only when its
/// explicit marker is present.
///
/// `practices` is the YAML catalog. Ordinary Markdown slides ignore it; the
/// firm-practice marker renders exactly those cards, in catalog order.
#[component]
pub fn CatalogSlideBody(
    title: String,
    body_html: String,
    #[props(default)] practices: Vec<PracticeLink>,
) -> Element {
    if body_html.contains(NAVIGATOR_PRODUCT_MARKER) {
        return rsx! {
            document::Stylesheet { href: BRAND_TOKENS_HREF }
            document::Stylesheet { href: BRAND_STYLESHEET_HREF }
            div { class: "material-body workshop-navigator-slide",
                h3 { "{title}" }
                div { class: "workshop-navigator-slide__mark",
                    PracticeMarkGlyph {
                        mark: PracticeMark::Helm,
                        class: "workshop-navigator-slide__wheel".to_string(),
                    }
                }
                a {
                    class: "workshop-navigator-slide__repo",
                    href: crate::source_repository::REPOSITORY_HREF,
                    "github.com/neon-law-source-code/navigator"
                }
            }
        };
    }

    if !body_html.contains(FIRM_PRODUCT_CARDS_MARKER) {
        return rsx! {
            div { class: "material-body", dangerous_inner_html: "{body_html}" }
        };
    }

    rsx! {
        // The cards are the same component and stylesheet used on `/`. Hoist
        // the firm's tokens too because the projector face carries no public
        // footer to do that work for it.
        document::Stylesheet { href: BRAND_TOKENS_HREF }
        document::Stylesheet { href: BRAND_STYLESHEET_HREF }
        document::Stylesheet { href: HOME_STYLESHEET_HREF }
        div { class: "material-body workshop-product-slide",
            h3 { "{title}" }
            div {
                class: "home-practices__grid workshop-product-cards",
                "aria-label": "{title}",
                for (index, product) in practices.iter().enumerate() {
                    PracticeCard {
                        mark: product.mark,
                        heading: product.heading.clone(),
                        body: String::new(),
                        href: product.href.clone(),
                        heading_id: format!("workshop-product-heading-{index}"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::PracticeLink;

    fn render(title: &str, body_html: &str, practices: Vec<PracticeLink>) -> String {
        fn app() -> Element {
            let (title, body_html, practices) =
                consume_context::<(String, String, Vec<PracticeLink>)>();
            rsx! { CatalogSlideBody { title, body_html, practices } }
        }

        let title = title.to_string();
        let body_html = body_html.to_string();
        let mut dom = VirtualDom::new(app);
        dom.insert_any_root_context(Box::new((title, body_html, practices)));
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn sample_catalog() -> Vec<PracticeLink> {
        vec![
            PracticeLink {
                mark: PracticeMark::Scales,
                heading: "Litigation".into(),
                body: String::new(),
                href: "/litigation".into(),
            },
            PracticeLink {
                mark: PracticeMark::Technology,
                heading: "Personal Plan".into(),
                body: String::new(),
                href: "/personal-plan".into(),
            },
        ]
    }

    #[test]
    fn the_marker_renders_the_supplied_practice_catalog() {
        let html = render(
            "What our firm does",
            &format!("<h3>What our firm does</h3><p>{FIRM_PRODUCT_CARDS_MARKER}</p>"),
            sample_catalog(),
        );
        assert!(html.contains("workshop-product-cards"), "{html}");
        assert!(
            html.contains(r#"aria-label="What our firm does""#),
            "{html}"
        );
        assert_eq!(html.matches("neon-card home-practice").count(), 2, "{html}");
        for (heading, href) in [
            ("Litigation", "/litigation"),
            ("Personal Plan", "/personal-plan"),
        ] {
            assert!(html.contains(heading), "missing {heading}: {html}");
            assert!(html.contains(&format!(r#"href="{href}""#)), "{html}");
        }
        assert!(!html.contains("Fractional GC"), "{html}");
        assert!(!html.contains(FIRM_PRODUCT_CARDS_MARKER), "{html}");
    }

    #[test]
    fn the_marker_without_a_catalog_renders_no_hardcoded_doors() {
        let html = render(
            "What our firm does",
            &format!("<p>{FIRM_PRODUCT_CARDS_MARKER}</p>"),
            Vec::new(),
        );
        assert!(html.contains("workshop-product-cards"), "{html}");
        assert_eq!(html.matches("neon-card home-practice").count(), 0, "{html}");
        assert!(!html.contains("Personal Plan"), "{html}");
        assert!(!html.contains("One-time services"), "{html}");
    }

    #[test]
    fn ordinary_markdown_html_passes_through_unchanged() {
        let html = render("Ordinary", "<h3>Ordinary</h3><p>Body</p>", sample_catalog());
        assert!(html.contains("<h3>Ordinary</h3><p>Body</p>"), "{html}");
        assert!(!html.contains("workshop-product-cards"), "{html}");
        assert!(!html.contains("Litigation"), "{html}");
    }

    #[test]
    fn the_navigator_marker_renders_the_wheel_and_repository() {
        let html = render(
            "NeonLawNavigator",
            &format!("<h3>NeonLawNavigator</h3><p>{NAVIGATOR_PRODUCT_MARKER}</p>"),
            Vec::new(),
        );
        assert!(html.contains("workshop-navigator-slide"), "{html}");
        assert!(html.contains(r#"data-practice-mark="helm""#), "{html}");
        assert!(html.contains("NeonLawNavigator"), "{html}");
        assert!(
            html.contains(r#"href="https://github.com/neon-law-source-code/navigator""#),
            "{html}"
        );
        assert!(
            html.contains("github.com/neon-law-source-code/navigator"),
            "{html}"
        );
        assert!(!html.contains(NAVIGATOR_PRODUCT_MARKER), "{html}");
    }
}
