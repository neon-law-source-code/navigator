//! The minimal footer every authenticated `/app` page carries.
//!
//! Distinct from [`crate::components::NavigatorFooter`], which composes a
//! host-supplied legal attribution, release label, and link row for the not
//! yet widely adopted `NavigatorShell`. The eight real `/app` pages render
//! their own markup directly rather than through that shell (see each page's
//! own `AppNavbar` call), so this footer is injected once into every `/app`
//! HTML response by `portal::dioxus_app::dioxus_document_head` — the same
//! seam [`crate::components::SampleMattersBanner`] rides — rather than added
//! to eight page bodies one at a time.
//!
//! It carries the copyright line the entity of record requires and the
//! shared platform line [`crate::components::POWERED_BY_NEON_LAW_NAVIGATOR`].
//! No navigation. A stamped deploy appends `views::brand::deployed_release`;
//! under `cargo run` that stamp is absent, so this publishes no version.

use dioxus::prelude::*;

use super::POWERED_BY_NEON_LAW_NAVIGATOR;

/// The centered copyright line naming the entity of record, then the
/// platform line every Navigator footer carries.
///
/// `legal_entity`, `copyright_year`, and `navigator_version` are supplied
/// by the caller rather than resolved here, matching
/// [`crate::components::SiteFooterLegal`]'s convention: a leaf component
/// takes data as props, and the host resolves `views::brand` where the
/// request/brand context is live. An empty `navigator_version` is the
/// ordinary local case.
#[component]
pub fn AppFooter(
    legal_entity: String,
    copyright_year: i32,
    #[props(default)] navigator_version: String,
) -> Element {
    rsx! {
        footer { class: "app-footer",
            p { class: "app-footer__copyright", "© {copyright_year} {legal_entity}" }
            p { class: "app-footer__platform",
                "{POWERED_BY_NEON_LAW_NAVIGATOR}"
                if !navigator_version.is_empty() {
                    span { class: "app-footer__release", " #{navigator_version}" }
                }
            }
        }
    }
}

/// Render the footer to a standalone HTML string.
///
/// `portal` injects this into every `/app` HTML response rather than
/// composing the component into each page tree — see the module docs for
/// why. Server-only: `dioxus-ssr` is not in the wasm client bundle.
///
/// Cheap to call, but call it once: `views::brand::FIRM_BRAND` is a
/// process-wide constant, so the markup is the same on every request.
#[cfg(feature = "server")]
#[must_use]
pub fn render_app_footer() -> String {
    fn footer() -> Element {
        rsx! {
            AppFooter {
                legal_entity: views::brand::FIRM_BRAND.legal_entity.to_string(),
                // Fixed for now, matching `public_chrome::firm_public_chrome`'s
                // footer year until a deploy-time value replaces the constant.
                copyright_year: 2026,
                navigator_version: views::brand::deployed_release()
                    .unwrap_or_default()
                    .to_string(),
            }
        }
    }
    let mut dom = VirtualDom::new(footer);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
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
    fn the_footer_renders_a_centered_copyright_line() {
        fn app() -> Element {
            rsx! {
                AppFooter {
                    legal_entity: "Shook Law PLLC".to_string(),
                    copyright_year: 2026,
                }
            }
        }

        let html = ssr(app);
        assert!(html.contains("© 2026 Shook Law PLLC"), "{html}");
        assert!(html.contains("app-footer__copyright"), "{html}");
        assert!(html.starts_with("<footer"), "{html}");
    }

    #[test]
    fn the_footer_renders_the_shared_platform_line_without_a_version() {
        fn app() -> Element {
            rsx! {
                AppFooter {
                    legal_entity: "Shook Law PLLC".to_string(),
                    copyright_year: 2026,
                }
            }
        }

        let html = ssr(app);
        assert!(
            html.contains(POWERED_BY_NEON_LAW_NAVIGATOR),
            "the shared wording: {html}"
        );
        assert!(
            !html.contains("app-footer__release"),
            "no version under cargo run: {html}"
        );
        assert!(
            !html.contains("href="),
            "the /app footer has no navigation: {html}"
        );
    }

    #[test]
    fn the_footer_appends_the_release_stamp_without_navigation() {
        fn app() -> Element {
            rsx! {
                AppFooter {
                    legal_entity: "Shook Law PLLC".to_string(),
                    copyright_year: 2026,
                    navigator_version: "26.8.20".to_string(),
                }
            }
        }

        let html = ssr(app);
        assert!(html.contains(POWERED_BY_NEON_LAW_NAVIGATOR), "{html}");
        assert!(html.contains("#26.8.20"), "{html}");
        assert!(
            !html.contains("href="),
            "a stamped deploy still has no navigation: {html}"
        );
    }

    /// The server render resolves the real entity of record, not a fixture.
    #[cfg(feature = "server")]
    #[test]
    fn the_rendered_footer_names_the_firm_of_record() {
        let html = render_app_footer();
        assert!(html.contains("Shook Law PLLC"), "{html}");
        assert!(html.contains('©'), "{html}");
        assert!(
            html.contains(POWERED_BY_NEON_LAW_NAVIGATOR),
            "injected /app footer carries the platform line: {html}"
        );
        assert!(
            !html.contains("href="),
            "injected /app footer has no navigation: {html}"
        );
        // `NAVIGATOR_RELEASE_TAG` is unset in this test process, matching
        // `cargo run`.
        assert!(
            !html.contains("app-footer__release"),
            "no version under cargo run: {html}"
        );
    }

    /// Every class the footer emits is styled by the theme it ships with.
    /// Mirrors `sample_matters_banner`'s own guard: a renamed class with no
    /// matching rule renders as unstyled text and nothing else catches it.
    #[test]
    fn every_class_the_footer_emits_is_styled_by_the_theme() {
        fn app() -> Element {
            rsx! {
                AppFooter {
                    legal_entity: "Shook Law PLLC".to_string(),
                    copyright_year: 2026,
                    navigator_version: "26.8.20".to_string(),
                }
            }
        }

        let theme =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/public/css/theme.css");
        let css = std::fs::read_to_string(&theme)
            .unwrap_or_else(|e| panic!("the theme stylesheet must be readable: {e}"));
        let out = ssr(app);

        let mut classes = Vec::new();
        let mut rest = out.as_str();
        while let Some(at) = rest.find("class=\"") {
            rest = &rest[at + 7..];
            let Some(end) = rest.find('"') else { break };
            classes.extend(rest[..end].split_whitespace().map(str::to_string));
            rest = &rest[end..];
        }
        assert!(!classes.is_empty(), "the footer emits classes: {out}");

        for class in classes {
            assert!(
                css.contains(&format!(".{class}")),
                "`{class}` is emitted by the footer but has no rule in \
                 server/public/css/theme.css"
            );
        }
    }
}
