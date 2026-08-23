//! Shared rendering for a Catalog slide face.
//!
//! Most slides are authored Markdown and arrive as rendered HTML. A deliberately
//! small marker lets a slide opt into a first-party Dioxus component without
//! allowing arbitrary author-written HTML. The step page, projector display,
//! and light table all call this component so the three faces cannot drift.

use dioxus::prelude::*;

use crate::brand_style::{BRAND_STYLESHEET_HREF, BRAND_TOKENS_HREF};
use crate::components::{PracticeCard, PracticeMark, PracticeMarkGlyph};
use crate::home::HOME_STYLESHEET_HREF;

/// Markdown marker that replaces the ordinary slide body with the firm's four
/// product cards.
pub const FIRM_PRODUCT_CARDS_MARKER: &str = "{{firm-product-cards}}";

/// Markdown marker that renders the Navigator identity slide.
pub const NAVIGATOR_PRODUCT_MARKER: &str = "{{navigator-product}}";

struct FirmProduct {
    mark: PracticeMark,
    heading: &'static str,
    href: &'static str,
}

const FIRM_PRODUCTS: &[FirmProduct] = &[
    FirmProduct {
        mark: PracticeMark::Technology,
        heading: "Fractional CTO",
        href: "/fractional-cto",
    },
    FirmProduct {
        mark: PracticeMark::Scales,
        heading: "Litigation",
        href: "/litigation",
    },
    FirmProduct {
        mark: PracticeMark::Handshake,
        heading: "Fractional GC",
        href: "/fractional-gc",
    },
    FirmProduct {
        mark: PracticeMark::Gavel,
        heading: "One-time services",
        href: "/services",
    },
];

/// Render one slide body, selecting a first-party component only when its
/// explicit marker is present.
#[component]
pub fn CatalogSlideBody(title: String, body_html: String) -> Element {
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
                for (index, product) in FIRM_PRODUCTS.iter().enumerate() {
                    PracticeCard {
                        mark: product.mark,
                        heading: product.heading.to_string(),
                        body: String::new(),
                        href: product.href.to_string(),
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

    fn render(title: &str, body_html: &str) -> String {
        fn app() -> Element {
            let (title, body_html) = consume_context::<(String, String)>();
            rsx! { CatalogSlideBody { title, body_html } }
        }

        let title = title.to_string();
        let body_html = body_html.to_string();
        let mut dom = VirtualDom::new(app);
        dom.insert_any_root_context(Box::new((title, body_html)));
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn the_marker_renders_the_four_homepage_practice_cards() {
        let html = render(
            "What our firm does",
            &format!("<h3>What our firm does</h3><p>{FIRM_PRODUCT_CARDS_MARKER}</p>"),
        );
        assert!(html.contains("workshop-product-cards"), "{html}");
        assert!(
            html.contains(r#"aria-label="What our firm does""#),
            "{html}"
        );
        assert_eq!(html.matches("neon-card home-practice").count(), 4, "{html}");
        for (heading, href) in [
            ("Fractional CTO", "/fractional-cto"),
            ("Litigation", "/litigation"),
            ("Fractional GC", "/fractional-gc"),
            ("One-time services", "/services"),
        ] {
            assert!(html.contains(heading), "missing {heading}: {html}");
            assert!(html.contains(&format!(r#"href="{href}""#)), "{html}");
        }
        assert!(!html.contains(FIRM_PRODUCT_CARDS_MARKER), "{html}");
    }

    #[test]
    fn ordinary_markdown_html_passes_through_unchanged() {
        let html = render("Ordinary", "<h3>Ordinary</h3><p>Body</p>");
        assert!(html.contains("<h3>Ordinary</h3><p>Body</p>"), "{html}");
        assert!(!html.contains("workshop-product-cards"), "{html}");
    }

    #[test]
    fn the_navigator_marker_renders_the_wheel_and_repository() {
        let html = render(
            "NeonLawNavigator",
            &format!("<h3>NeonLawNavigator</h3><p>{NAVIGATOR_PRODUCT_MARKER}</p>"),
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
