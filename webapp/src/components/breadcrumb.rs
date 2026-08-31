//! Breadcrumb navigation, as Dioxus components (issue #641, Phase 2).
//!
//! The successor to the `views::components::breadcrumb`. A small "back to
//! parent page" breadcrumb shown at the top of detail pages: an accessible
//! breadcrumb landmark, a muted link, and the left-pointing arrow
//! ([`IconName::ArrowLeft`] inline SVG). [`LawyerPortalBreadcrumb`] is the
//! standard return link for a lawyer workbench page, so lawyer views share one
//! target and treatment.

use dioxus::prelude::*;

use crate::components::{Icon, IconName};

/// A one-step breadcrumb back to a parent page.
#[component]
pub fn BackBreadcrumb(href: String, label: String) -> Element {
    rsx! {
        nav { class: "nav-breadcrumb", "aria-label": "Breadcrumb",
            a { class: "nav-breadcrumb__link", href: "{href}",
                Icon { name: IconName::ArrowLeft }
                " "
                "{label}"
            }
        }
    }
}

/// The standard return link for a lawyer workbench page.
#[allow(non_snake_case)]
pub fn LawyerPortalBreadcrumb() -> Element {
    rsx! {
        BackBreadcrumb { href: "/app/lawyer".to_string(), label: "Lawyer portal".to_string() }
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
    fn back_breadcrumb_links_to_parent_with_arrow() {
        fn app() -> Element {
            rsx! {
                BackBreadcrumb { href: "/team".to_string(), label: "All team profiles".to_string() }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-label="Breadcrumb""#), "{html}");
        assert!(html.contains(r#"href="/team""#), "{html}");
        assert!(html.contains("All team profiles"), "{html}");
        // The back arrow is an inline SVG.
        assert!(html.contains("nav-icon"), "{html}");
    }

    #[test]
    fn lawyer_portal_breadcrumb_returns_to_the_workbench() {
        fn app() -> Element {
            rsx! { LawyerPortalBreadcrumb {} }
        }
        let html = ssr(app);
        assert!(html.contains(r#"href="/app/lawyer""#), "{html}");
        assert!(html.contains("Lawyer portal"), "{html}");
    }
}
