//! The public site header, as a Dioxus component (issue #641, Phase 2).
//!
//! The successor to the public marketing navbar in the `views::layout` —
//! the brand mark (logo + site name, linked home) and the primary navigation
//! every public page carries, plus an optional utility group for the
//! auth-aware links (Sign in, or Portal/Lawyer/Sign out) the server resolves per
//! request. It is the public counterpart to [`crate::components::NavigatorNavbar`],
//! which is the *authenticated* application chrome (#792).
//!
//! Prop-driven like [`crate::components::SiteFooterLegal`]: the process brand
//! (`views::brand`) and the request's auth state map onto its props server-side,
//! so the wasm client never links the view layer and a white-label deploy
//! emits its own identity and destinations.
//!
//! **The mobile menu carries no JavaScript.** Public pages are server-rendered
//! and ship no hydration bundle, so a toggle held in a signal would render as an
//! inert button. The disclosure is therefore a hidden checkbox the burger label
//! flips, and CSS reveals the link rows from `:checked` — the one mechanism that
//! needs no script and no `::details-content` support. Above the breakpoint the
//! burger is `display: none` and the rows are always shown, so the checkbox
//! state cannot strand a desktop visitor's navigation.

#![allow(clippy::doc_markdown)]

use dioxus::prelude::*;

/// One navigation destination. `current` marks the active page so the anchor
/// carries `aria-current="page"` and the active-link styling.
#[derive(Clone, PartialEq, Eq)]
pub struct SiteNavLink {
    pub label: String,
    pub href: String,
    pub current: bool,
}

impl SiteNavLink {
    /// A non-current destination.
    #[must_use]
    pub fn new(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
            current: false,
        }
    }

    /// Mark this destination as the active page.
    #[must_use]
    pub fn current(mut self) -> Self {
        self.current = true;
        self
    }
}

/// The public site header.
///
/// - `brand_name` / `home_href` / `logo_href`: the brand mark, linked home. The
///   logo is decorative (`alt=""`) — the brand name follows in text and the
///   anchor is labelled, so the mark would otherwise be announced twice.
/// - `destinations`: the primary marketing nav, in order.
/// - `utility`: the auth-aware links (empty for an anonymous reader),
///   rendered as a trailing group set off from the primary nav.
/// - `menu_id`: the id tying the narrow-viewport burger label to its checkbox.
///   A page renders one header and wants the default; the `/design` gallery
///   renders two, and a repeated id would make the second burger toggle the
///   first header's menu.
#[component]
pub fn SiteHeader(
    brand_name: String,
    home_href: String,
    logo_href: String,
    destinations: Vec<SiteNavLink>,
    #[props(default)] utility: Vec<SiteNavLink>,
    #[props(default = "site-menu".to_string())] menu_id: String,
) -> Element {
    rsx! {
        // The browser-tab icon is the same mark the header paints two lines
        // below, so it cannot drift from it and a white-label deploy's tab
        // carries its own logo rather than someone else's. It rides with the
        // header rather than sitting in each page's own `<head>` block because
        // every page that renders a header wants it and none wants a different
        // one; `document::Link` hoists it into `<head>` from here.
        //
        // The MIME type is derived from the mark rather than hardcoded: the
        // built-in mark is a PNG and a white-label bundle may mount an SVG, and
        // a `type` that disagrees with the bytes is a tab icon the browser
        // declines to draw. A browser that cannot use it falls back to
        // requesting `/favicon.ico`, which this deployment does not serve — one
        // 404 for a tab icon, not a broken page.
        document::Link {
            rel: "icon",
            r#type: if std::path::Path::new(&logo_href)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
            {
                "image/svg+xml"
            } else {
                "image/png"
            },
            href: "{logo_href}",
        }
        // Without this a phone lays the page out in a 980px imaginary window and
        // scales the result down, so every `max-width` breakpoint in the
        // stylesheet is measured against a viewport the device does not have —
        // the burger below never appears, and the whole site renders as a
        // shrunken desktop. It rides here for the same reason the icon does:
        // every page that renders a header needs it and none needs a different
        // one.
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1",
        }
        header { class: "site-header",
            nav { class: "site-header__nav", "aria-label": "Primary",
                a {
                    class: "site-header__brand",
                    href: "{home_href}",
                    "aria-label": "{brand_name} home",
                    img {
                        class: "site-header__logo",
                        src: "{logo_href}",
                        alt: "",
                        width: "32",
                        height: "32",
                    }
                    strong { "{brand_name}" }
                }
                // The narrow-viewport disclosure. The input holds the state and
                // the accessible name; the label is the visual burger and is
                // hidden from assistive technology because the input it drives
                // is already announced. Both are inert above the breakpoint,
                // where the rows below render unconditionally.
                input {
                    class: "site-header__toggle",
                    r#type: "checkbox",
                    id: "{menu_id}",
                    "aria-label": "Menu",
                }
                label {
                    class: "site-header__burger",
                    r#for: "{menu_id}",
                    "aria-hidden": "true",
                    span { class: "site-header__burger-bar" }
                    span { class: "site-header__burger-bar" }
                    span { class: "site-header__burger-bar" }
                }
                ul { class: "site-header__links",
                    for link in destinations.iter() {
                        SiteHeaderLink { link: link.clone() }
                    }
                }
                if !utility.is_empty() {
                    ul { class: "site-header__utility",
                        for link in utility.iter() {
                            SiteHeaderLink { link: link.clone() }
                        }
                    }
                }
            }
        }
    }
}

/// One rendered nav `<li>`. The active page carries `aria-current="page"` and
/// the `--active` modifier; the rest are plain links.
#[component]
fn SiteHeaderLink(link: SiteNavLink) -> Element {
    let class = if link.current {
        "site-header__link site-header__link--active"
    } else {
        "site-header__link"
    };
    rsx! {
        li {
            a {
                class: "{class}",
                href: "{link.href}",
                "aria-current": if link.current { Some("page") } else { None },
                "{link.label}"
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

    /// The anonymous public header: brand + primary nav, no utility group.
    fn anonymous_html() -> String {
        fn app() -> Element {
            rsx! {
                SiteHeader {
                    brand_name: "Lawyer Shook".to_string(),
                    home_href: "/".to_string(),
                    logo_href: "/public/img/logo.svg".to_string(),
                    destinations: vec![
                        SiteNavLink::new("Services", "/#services"),
                        SiteNavLink::new("Blog", "/blog").current(),
                        SiteNavLink::new("Team", "/team"),
                        SiteNavLink::new("Notations", "/notations"),
                    ],
                }
            }
        }
        ssr(app)
    }

    /// The signed-in header: the same nav plus a utility group.
    fn signed_in_html() -> String {
        fn app() -> Element {
            rsx! {
                SiteHeader {
                    brand_name: "Lawyer Shook".to_string(),
                    home_href: "/".to_string(),
                    logo_href: "/public/img/logo.svg".to_string(),
                    destinations: vec![SiteNavLink::new("Services", "/#services")],
                    utility: vec![SiteNavLink::new("Portal", "/app/projects")],
                }
            }
        }
        ssr(app)
    }

    #[test]
    fn renders_the_brand_mark_linked_home() {
        let out = anonymous_html();
        assert!(out.contains(r#"href="/""#), "brand links home: {out}");
        assert!(
            out.contains("Lawyer Shook home"),
            "brand anchor is labelled"
        );
        // The logo is decorative — the brand text and label carry the name.
        assert!(out.contains(r#"alt="""#), "logo is decorative: {out}");
    }

    #[test]
    fn lists_every_destination_in_order() {
        let out = anonymous_html();
        let services = out.find("Services").expect("Services present");
        let blog = out.find("Blog").expect("Blog present");
        let team = out.find("Team").expect("Team present");
        let notations = out.find("Notations").expect("Notations present");
        assert!(
            services < blog && blog < team && team < notations,
            "in order: {out}"
        );
    }

    #[test]
    fn marks_the_current_page() {
        let out = anonymous_html();
        assert!(
            out.contains(r#"aria-current="page""#),
            "the active page carries aria-current: {out}",
        );
        assert!(
            out.contains("site-header__link--active"),
            "the active page carries the active modifier",
        );
    }

    #[test]
    fn renders_the_utility_group_only_when_present() {
        // An anonymous reader passes no utility links, so the group is absent.
        assert!(!anonymous_html().contains("site-header__utility"));
        // A signed-in reader gets the utility group with their links.
        let out = signed_in_html();
        assert!(out.contains("site-header__utility"));
        assert!(out.contains("Portal"));
    }

    /// The mobile menu is a checkbox the burger label flips, and it has to be
    /// exactly that: public pages ship no hydration bundle, so a toggle held in
    /// a signal would render as a button that does nothing.
    ///
    /// Three things make it usable, and each is asserted because each is easy to
    /// lose in a refactor: the input carries the accessible name, the label
    /// points at the input by `for` (a click target that is not nested inside
    /// it), and the burger's own bars are hidden from assistive technology so it
    /// is announced once rather than four times.
    #[test]
    fn the_mobile_menu_toggles_without_javascript() {
        let out = anonymous_html();
        assert!(
            out.contains(r#"type="checkbox""#) && out.contains(r#"id="site-menu""#),
            "the disclosure is a checkbox: {out}"
        );
        assert!(
            out.contains(r#"aria-label="Menu""#),
            "the input carries the accessible name: {out}"
        );
        assert!(
            out.contains(r#"for="site-menu""#),
            "the burger label drives it: {out}"
        );
        assert!(
            out.contains(r#"class="site-header__burger" for="site-menu" aria-hidden="true""#),
            "the burger is hidden from assistive technology: {out}"
        );
        assert_eq!(
            out.matches("site-header__burger-bar").count(),
            3,
            "three bars: {out}"
        );
        // No script, on a page that could not run one.
        assert!(!out.contains("<script"), "no script: {out}");
        assert!(!out.contains("onclick"), "no inline handler: {out}");
    }

    /// Two headers on one page carry two distinct menu ids.
    ///
    /// Only the `/design` gallery renders more than one header, and it used to
    /// render both with the hard-coded `site-menu`: a duplicate id in an ARIA
    /// and label relationship (axe's `duplicate-id-aria`), which also pointed
    /// the second burger at the first header's checkbox.
    #[test]
    fn a_second_header_on_the_page_gets_its_own_menu_id() {
        fn app() -> Element {
            rsx! {
                SiteHeader {
                    brand_name: "Lawyer Shook".to_string(),
                    home_href: "/".to_string(),
                    logo_href: "/public/img/logo.svg".to_string(),
                    destinations: vec![SiteNavLink::new("Blog", "/blog")],
                }
                SiteHeader {
                    brand_name: "Lawyer Shook".to_string(),
                    home_href: "/".to_string(),
                    logo_href: "/public/img/logo.svg".to_string(),
                    destinations: vec![SiteNavLink::new("Blog", "/blog")],
                    menu_id: "shell-menu".to_string(),
                }
            }
        }
        let out = ssr(app);
        assert_eq!(
            out.matches(r#"id="site-menu""#).count(),
            1,
            "the default id is used once: {out}"
        );
        assert_eq!(
            out.matches(r#"id="shell-menu""#).count(),
            1,
            "the second header holds its own state: {out}"
        );
        assert_eq!(
            out.matches(r#"for="shell-menu""#).count(),
            1,
            "and its burger drives that checkbox, not the first one: {out}"
        );
    }
}
