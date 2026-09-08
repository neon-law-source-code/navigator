//! The shared navbar used by the authenticated `/app` pages that render the
//! Dioxus application chrome. Legacy native-form pages reuse its
//! [`AppProfileMenu`] leaf so the viewer affordance stays consistent there too.
//!
//! Before this component each `/app` page hand-wrote its own `nav.lawyer-nav`,
//! and they drifted: the workbench offered Admin, the matter list offered
//! neither Lawyer nor Admin, and the filed-document page still pointed at the
//! retired `/lawyer` and `/admin` prefixes. One component renders the row so a
//! new page cannot invent a fourth variant.
//!
//! Presentation only, and a leaf: it takes the destinations it should render and
//! never derives them from a role or a URL. [`crate::app_chrome`] owns that
//! mapping and is where the role gate is unit-tested.
//!
//! The brand mark is a prop for the same reason [`crate::components::SiteHeader`]
//! takes one — this crate is shared, and a white-label deploy mounting these
//! pages publishes its own mark, not the firm's.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One destination in the `/app` navbar.
///
/// `Serialize`/`Deserialize` because it crosses the server-function boundary
/// inside a page's view struct; `PartialEq` because Dioxus diffs props.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppNavLink {
    pub label: String,
    pub href: String,
}

impl AppNavLink {
    #[must_use]
    pub fn new(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
        }
    }
}

/// The brand mark the navbar renders at its trailing edge.
///
/// `src` is the image, `href` where the mark links, and `brand_name` names the
/// deploy for the anchor's accessible label. The image itself is decorative
/// (`alt=""`) — the label already carries the name, so a screen reader that
/// announced both would say it twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLogo {
    pub src: String,
    pub href: String,
    pub brand_name: String,
}

/// The `/app` navbar: a profile menu containing the viewer's destinations,
/// then the deploy's brand mark.
///
/// `logo` is `None` for a deploy that configures no mark, which renders the row
/// of links alone rather than a broken image.
#[component]
pub fn AppNavbar(
    destinations: Vec<AppNavLink>,
    #[props(default)] logo: Option<AppLogo>,
) -> Element {
    rsx! {
        nav { class: "lawyer-nav", "aria-label": "Application",
            AppProfileMenu { destinations }
            if let Some(logo) = logo.as_ref() {
                a {
                    class: "lawyer-nav__brand",
                    href: "{logo.href}",
                    "aria-label": "{logo.brand_name} home",
                    img {
                        class: "lawyer-nav__logo",
                        src: "{logo.src}",
                        alt: "",
                        width: "28",
                        height: "28",
                    }
                }
            }
        }
    }
}

/// The avatar trigger and disclosure shared by migrated and legacy `/app`
/// routes. Keeping this leaf separate lets a native-form page retain its
/// existing links while gaining the same authenticated profile affordance.
#[component]
pub fn AppProfileMenu(destinations: Vec<AppNavLink>) -> Element {
    rsx! {
        details { class: "lawyer-nav__profile",
            summary {
                class: "lawyer-nav__profile-trigger",
                "aria-label": "Open profile menu",
                img {
                    class: "lawyer-nav__avatar",
                    src: "/app/me/avatar",
                    alt: "Your profile photo",
                    width: "36",
                    height: "36",
                }
                span { class: "lawyer-nav__profile-chevron", "⌄" }
            }
            div { class: "lawyer-nav__profile-menu",
                for link in destinations.iter() {
                    a { class: "nav-link", href: "{link.href}", "{link.label}" }
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

    fn logo() -> AppLogo {
        AppLogo {
            src: "/public/img/logo.svg".to_string(),
            href: "/".to_string(),
            brand_name: "Example Law".to_string(),
        }
    }

    #[test]
    fn renders_every_supplied_destination_in_order() {
        fn app() -> Element {
            rsx! {
                AppNavbar {
                    destinations: vec![
                        AppNavLink::new("Projects", "/app/projects"),
                        AppNavLink::new("Lawyer", "/app/lawyer"),
                        AppNavLink::new("Sign out", "/auth/logout"),
                    ],
                }
            }
        }

        let html = ssr(app);
        let projects = html.find("Projects").expect("Projects present");
        let lawyer = html.find("Lawyer").expect("Lawyer present");
        let sign_out = html.find("Sign out").expect("Sign out present");
        assert!(projects < lawyer && lawyer < sign_out, "{html}");
        assert!(html.contains(r#"class="lawyer-nav""#), "{html}");
        assert!(html.contains(r#"aria-label="Application""#), "{html}");
    }

    #[test]
    fn renders_the_configured_brand_mark_after_the_destinations() {
        fn app() -> Element {
            rsx! {
                AppNavbar {
                    destinations: vec![AppNavLink::new("Sign out", "/auth/logout")],
                    logo: Some(logo()),
                }
            }
        }

        let html = ssr(app);
        assert!(html.contains(r#"src="/public/img/logo.svg""#), "{html}");
        assert!(html.contains(r#"aria-label="Example Law home""#), "{html}");
        // The mark is decorative: the anchor's label carries the brand name.
        assert!(html.contains(r#"alt="""#), "{html}");
        assert!(html.contains(r#"src="/app/me/avatar""#), "{html}");
        assert!(html.contains(r#"aria-label="Open profile menu""#), "{html}");
        // It trails the profile menu, which is what the `margin-left: auto` on
        // `.lawyer-nav__brand` pushes to the right-hand edge.
        let sign_out = html.find("Sign out").expect("Sign out present");
        let mark = html.find("lawyer-nav__brand").expect("mark present");
        assert!(sign_out < mark, "{html}");
    }

    #[test]
    fn renders_no_mark_when_the_deploy_configures_none() {
        fn app() -> Element {
            rsx! {
                AppNavbar { destinations: vec![AppNavLink::new("Sign out", "/auth/logout")] }
            }
        }

        let html = ssr(app);
        assert!(!html.contains("lawyer-nav__brand"), "{html}");
        assert!(!html.contains("lawyer-nav__logo"), "{html}");
    }
}
