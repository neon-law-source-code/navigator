//! Link-driven tabs, as a Dioxus component.
//!
//! The CSS already lives on `.nav-tabs` / `.nav-tab` / `.nav-tab.is-active`.
//! This component owns that markup so a page can server-render a selected
//! panel from a query parameter without a client-side widget. Each tab is a
//! plain anchor whose `href` the caller supplies, so the theme stays a leaf:
//! no router import, and the selected panel is in the HTML before hydration.

use dioxus::prelude::*;

/// One tab in a [`Tabs`] strip.
#[derive(Clone, PartialEq, Eq)]
pub struct Tab {
    pub label: String,
    pub href: String,
    pub selected: bool,
}

impl Tab {
    #[must_use]
    pub fn new(label: impl Into<String>, href: impl Into<String>, selected: bool) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
            selected,
        }
    }
}

/// A navigation strip of link tabs. The selected tab carries `aria-current="page"`
/// and `.is-active`. The caller renders the matching panel beside this strip.
#[component]
pub fn Tabs(aria_label: String, tabs: Vec<Tab>) -> Element {
    rsx! {
        nav { class: "nav-tabs", aria_label: "{aria_label}",
            for tab in tabs.iter() {
                if tab.selected {
                    a {
                        class: "nav-tab is-active",
                        href: "{tab.href}",
                        aria_current: "page",
                        "{tab.label}"
                    }
                } else {
                    a { class: "nav-tab", href: "{tab.href}", "{tab.label}" }
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

    #[test]
    fn renders_one_anchor_per_tab_and_marks_the_selected_page() {
        fn app() -> Element {
            rsx! {
                Tabs {
                    aria_label: "Matter sections".to_string(),
                    tabs: vec![
                        Tab::new("Documents", "/app/projects/acme?tab=documents", true),
                        Tab::new("Testimonial", "/app/projects/acme?tab=testimonial", false),
                    ],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"aria-label="Matter sections""#), "{html}");
        assert_eq!(html.matches("<a ").count(), 2, "{html}");
        assert!(
            html.contains(r#"href="/app/projects/acme?tab=documents""#),
            "{html}"
        );
        assert!(
            html.contains(r#"href="/app/projects/acme?tab=testimonial""#),
            "{html}"
        );
        assert!(html.contains("Documents"), "{html}");
        assert!(html.contains("Testimonial"), "{html}");
        assert!(html.contains(r#"aria-current="page""#), "{html}");
        assert!(html.contains("nav-tab is-active"), "{html}");
    }

    #[test]
    fn a_single_tab_still_renders_its_anchor() {
        fn app() -> Element {
            rsx! {
                Tabs {
                    aria_label: "Matter sections".to_string(),
                    tabs: vec![Tab::new("Documents", "/app/projects/acme?tab=documents", true)],
                }
            }
        }
        let html = ssr(app);
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert!(html.contains("Documents"), "{html}");
        assert!(html.contains(r#"aria-current="page""#), "{html}");
    }
}
