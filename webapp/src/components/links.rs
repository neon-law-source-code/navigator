//! Off-site link, as a Dioxus component (issue #641, Phase 2).
//!
//! The successor to the `views::components::ExternalLink`. An anchor that
//! leaves our domains opens in a new tab and carries the OWASP `rel` pair
//! (`noopener noreferrer`), with an upper-right arrow (the
//! [`IconName::BoxArrowUpRight`] inline SVG) so the reader knows it goes
//! off-site. The glyph carries its own accessible name — "opens in a new
//! tab" — read after the anchor text, rather than being purely decorative:
//! that is the one thing about the destination the link text itself cannot
//! say.

use dioxus::prelude::*;

use crate::components::{Icon, IconName};

/// An off-site anchor around `children`. `class` sets the `<a>` class (e.g.
/// `link-secondary` for muted footer links); `title` sets a hover tooltip.
/// `current` marks this as the active page in a nav row it sits in — the
/// same `aria-current="page"` an in-app [`crate::components::SiteHeader`]
/// destination carries, for the off-site destinations that sit beside them.
#[component]
pub fn ExternalLink(
    href: String,
    #[props(default)] class: Option<String>,
    #[props(default)] title: Option<String>,
    #[props(default)] current: bool,
    children: Element,
) -> Element {
    rsx! {
        a {
            href: "{href}",
            class: class,
            title: title,
            "aria-current": if current { Some("page") } else { None },
            target: "_blank",
            rel: "noopener noreferrer",
            {children}
            " "
            Icon { name: IconName::BoxArrowUpRight, label: "opens in a new tab".to_string() }
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

    #[test]
    fn opens_new_tab_with_owasp_rel_and_offsite_glyph() {
        fn app() -> Element {
            rsx! {
                ExternalLink { href: "https://example.com".to_string(), "Example" }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"href="https://example.com""#), "{html}");
        assert!(html.contains(r#"target="_blank""#), "{html}");
        assert!(html.contains(r#"rel="noopener noreferrer""#), "{html}");
        assert!(html.contains("Example"), "{html}");
        // The off-site arrow is an inline SVG, carrying its own accessible name.
        assert!(html.contains("nav-icon"), "{html}");
        assert!(
            html.contains("<title>opens in a new tab</title>"),
            "the glyph names what leaving the anchor text cannot: {html}"
        );
        assert!(
            !html.contains(r#""aria-current""#),
            "not marked current by default: {html}"
        );
    }

    /// An off-site destination sitting in a nav row can still be the active
    /// page, exactly like an internal [`crate::components::SiteHeader`]
    /// destination.
    #[test]
    fn marks_the_current_page_when_asked() {
        fn app() -> Element {
            rsx! {
                ExternalLink {
                    href: "https://example.com".to_string(),
                    current: true,
                    "Example"
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-current="page""#), "{html}");
    }

    #[test]
    fn carries_an_optional_class_and_title() {
        fn app() -> Element {
            rsx! {
                ExternalLink {
                    href: "https://example.com".to_string(),
                    class: "link-secondary".to_string(),
                    title: "Opens example.com".to_string(),
                    "Docs"
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"class="link-secondary""#), "{html}");
        assert!(html.contains(r#"title="Opens example.com""#), "{html}");
    }
}
