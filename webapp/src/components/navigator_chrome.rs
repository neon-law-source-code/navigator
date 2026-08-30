//! Shared authenticated Navigator chrome.
//!
//! Every authenticated Dioxus page needs the same global application
//! navigation and footer. The caller supplies an already-authorized model: this
//! component renders links but never makes an access decision. That keeps the
//! route middleware and the lawyer/client project lenses authoritative.

use dioxus::prelude::*;

/// One global Navigator destination, prepared by the server for the current
/// authenticated viewer.
#[derive(Clone, PartialEq, Eq)]
pub struct NavigatorDestination {
    pub label: String,
    pub href: String,
    pub active: bool,
}

impl NavigatorDestination {
    #[must_use]
    pub fn new(label: impl Into<String>, href: impl Into<String>, active: bool) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
            active,
        }
    }
}

/// One compact link in the global footer.
#[derive(Clone, PartialEq, Eq)]
pub struct NavigatorFooterLink {
    pub label: String,
    pub href: String,
}

impl NavigatorFooterLink {
    #[must_use]
    pub fn new(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
        }
    }
}

/// The one global navbar for authenticated Navigator pages.
///
/// `destinations` is deliberately a supplied list rather than a role enum. The
/// portal adapter decides which destinations an authenticated viewer may see;
/// this presentation component never infers a system tier from the URL.
#[component]
pub fn NavigatorNavbar(
    brand_name: String,
    brand_href: String,
    destinations: Vec<NavigatorDestination>,
    #[props(default = "/auth/logout".to_string())] sign_out_href: String,
) -> Element {
    rsx! {
        header { class: "navigator-chrome__header",
            nav { class: "navigator-navbar", "aria-label": "Navigator",
                a { class: "navigator-navbar__brand", href: "{brand_href}", "{brand_name}" }
                ul { class: "navigator-navbar__destinations",
                    for destination in destinations {
                        li {
                            if destination.active {
                                a {
                                    class: "navigator-navbar__link navigator-navbar__link--active",
                                    href: "{destination.href}",
                                    "aria-current": "page",
                                    "{destination.label}"
                                }
                            } else {
                                a {
                                    class: "navigator-navbar__link",
                                    href: "{destination.href}",
                                    "{destination.label}"
                                }
                            }
                        }
                    }
                }
                a { class: "navigator-navbar__sign-out", href: "{sign_out_href}", "Sign out" }
            }
        }
    }
}

/// The global footer for authenticated Navigator pages.
///
/// The legal attribution and release label are supplied by the host adapter so
/// the footer stays host-aware and white-label safe.
#[component]
pub fn NavigatorFooter(
    legal_attribution: String,
    release_label: String,
    links: Vec<NavigatorFooterLink>,
) -> Element {
    rsx! {
        footer { class: "navigator-footer",
            p { class: "navigator-footer__legal", "{legal_attribution}" }
            nav { class: "navigator-footer__links", "aria-label": "Footer",
                for link in links {
                    a { href: "{link.href}", "{link.label}" }
                }
            }
            p { class: "navigator-footer__release", "{release_label}" }
        }
    }
}

/// Page framing for a globally navigable authenticated Navigator page.
///
/// `main_landmark` controls whether the content region is the document `<main>`
/// landmark. Real authenticated pages leave it `true` so the shell owns the one
/// page landmark. A preview that renders the shell *inside* another page's main
/// passes `false`, so the frame renders without introducing a second, nested
/// `<main>` landmark.
#[component]
pub fn NavigatorShell(
    navbar: Element,
    footer: Element,
    children: Element,
    #[props(default = true)] main_landmark: bool,
) -> Element {
    rsx! {
        div { class: "navigator-shell nav-theme",
            {navbar}
            if main_landmark {
                main { class: "navigator-shell__main", {children} }
            } else {
                div { class: "navigator-shell__main", {children} }
            }
            {footer}
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
    fn navbar_marks_only_the_server_selected_destination_as_current() {
        fn app() -> Element {
            rsx! {
                NavigatorNavbar {
                    brand_name: "Neon Law Navigator".to_string(),
                    brand_href: "/app/projects".to_string(),
                    destinations: vec![
                        NavigatorDestination::new("Portal", "/app/projects", false),
                        NavigatorDestination::new("Lawyer", "/app/lawyer", true),
                    ],
                }
            }
        }

        let html = ssr(app);
        assert!(html.contains(r#"aria-label="Navigator""#), "{html}");
        assert!(html.contains(r#"href="/app/projects""#), "{html}");
        assert!(html.contains(r#"href="/app/lawyer""#), "{html}");
        assert!(html.contains(r#"aria-current="page""#), "{html}");
        assert_eq!(html.matches(r#"aria-current="page""#).count(), 1, "{html}");
        assert!(!html.contains(r#"aria-current="false""#), "{html}");
        assert!(html.contains("navigator-navbar__link--active"), "{html}");
        assert!(html.contains(r#"href="/auth/logout""#), "{html}");
    }

    #[test]
    fn footer_renders_host_supplied_legal_and_release_content() {
        fn app() -> Element {
            rsx! {
                NavigatorFooter {
                    legal_attribution: "Legal services rendered by Example PLLC.".to_string(),
                    release_label: "Navigator 26.7.24".to_string(),
                    links: vec![
                        NavigatorFooterLink::new("Privacy", "/privacy"),
                        NavigatorFooterLink::new("Terms", "/terms"),
                    ],
                }
            }
        }

        let html = ssr(app);
        assert!(
            html.contains("Legal services rendered by Example PLLC."),
            "{html}"
        );
        assert!(html.contains("Navigator 26.7.24"), "{html}");
        assert!(html.contains(r#"aria-label="Footer""#), "{html}");
        assert!(html.contains(r#"href="/privacy""#), "{html}");
        assert!(html.contains(r#"href="/terms""#), "{html}");
    }

    #[test]
    fn shell_orders_global_chrome_around_main_content() {
        fn app() -> Element {
            rsx! {
                NavigatorShell {
                    navbar: rsx! { div { "global navigation" } },
                    footer: rsx! { div { "global footer" } },
                    p { "page content" }
                }
            }
        }

        let html = ssr(app);
        let navigation = html.find("global navigation").unwrap();
        let content = html.find("page content").unwrap();
        let footer = html.find("global footer").unwrap();
        assert!(navigation < content && content < footer, "{html}");
        assert!(html.contains("navigator-shell__main"), "{html}");
        // A real authenticated page defaults to owning the `<main>` landmark.
        assert!(
            html.contains("<main class=\"navigator-shell__main\""),
            "{html}"
        );
    }

    /// The authenticated shell must not read as a public page.
    ///
    /// `portal::dioxus_app` decides where the support-chat widget goes by
    /// looking for the public shell's root class in the rendered document, and
    /// both roots carry `nav-theme`. If this shell ever rendered the public
    /// marker, every `/app` and `/app/lawyer` page would grow a support bubble and
    /// have its CSP widened to a third-party origin — on the surfaces that
    /// display a client's matter.
    #[test]
    fn the_authenticated_shell_is_not_marked_as_a_public_page() {
        fn app() -> Element {
            rsx! {
                NavigatorShell {
                    navbar: rsx! { div { "nav" } },
                    footer: rsx! { div { "foot" } },
                    p { "content" }
                }
            }
        }

        let html = ssr(app);
        assert!(
            !html.contains(&format!(
                "class=\"{}\"",
                crate::components::PUBLIC_SHELL_MARKER
            )),
            "{html}"
        );
    }

    #[test]
    fn shell_omits_the_main_landmark_when_previewed_inside_another_main() {
        fn app() -> Element {
            rsx! {
                NavigatorShell {
                    main_landmark: false,
                    navbar: rsx! { div { "global navigation" } },
                    footer: rsx! { div { "global footer" } },
                    p { "page content" }
                }
            }
        }

        let html = ssr(app);
        // The content region keeps its class (so styling is unchanged) but is a
        // plain `<div>`, not a nested `<main>` landmark.
        assert!(html.contains("navigator-shell__main"), "{html}");
        assert!(!html.contains("<main"), "{html}");
    }
}
