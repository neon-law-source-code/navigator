//! `/{category}/{slug}/display/{n}` — the slide-only,
//! full-screen display face a presenter opens on a second screen, migrated to
//! Dioxus SSR (#956 Phase 4).
//!
//! Built for a two-screen setup: the presenter keeps `…/step/{n}` (notes plus
//! the progress rail) on their laptop while an external monitor opens this
//! route and advances it independently. There is no websocket and no
//! cross-window sync — each browser navigates its own deck, by design.
//!
//! This page wears no site chrome at all: no header, no footer, no shell. A
//! projector should show the slide and nothing else, so it renders the theme
//! stylesheet and the slide canvas directly rather than going through
//! [`crate::components::PublicShell`].
//!
//! `catalog-display.js` is what makes it a deck: it activates the
//! `data-catalog-nav` anchors on click, arrow keys, Space, and PageUp/PageDown,
//! and turns a click anywhere on `data-catalog-advance` into "next". The
//! `PageLayout` loaded it on every render; a Dioxus page loads only what it
//! names, so without the hoist below the projector face becomes a still image.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::catalog_slide_body::CatalogSlideBody;
use crate::catalog_step::CATALOG_DISPLAY_SCRIPT_HREF;
use crate::components::{CATALOG_STYLESHEET_HREF, THEME_STYLESHEET_HREF};
use crate::home::PracticeLink;

/// Everything one projected slide renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DisplayContent {
    pub workshop_title: String,
    /// This section's title, which becomes the document title.
    pub title: String,
    /// Rendered HTML for the slide face (carries its own heading).
    pub body_html: String,
    /// The previous slide's display face, when there is one.
    pub prev_href: Option<String>,
    /// The next slide's display face, when there is one.
    pub next_href: Option<String>,
    /// Back to the presenter step for this same slide.
    pub step_href: String,
}

/// The [`DisplayContent`] the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedDisplay(pub DisplayContent);

/// Everything the projector face renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogDisplayView {
    pub content: DisplayContent,
    pub practices: Vec<PracticeLink>,
}

/// Resolve this slide's content. There is no chrome to resolve — the display
/// face deliberately wears none.
#[server]
pub async fn catalog_display_view() -> Result<CatalogDisplayView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedDisplay>, _>()
            .await
            .map(|axum::Extension(c)| c.0)
            .unwrap_or_default();
    let practices = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::catalog_slide_body::InjectedPracticeCatalog>,
        _,
    >()
    .await
    .map(|axum::Extension(c)| c.0)
    .unwrap_or_default();
    Ok(CatalogDisplayView { content, practices })
}

/// The page's route entry.
#[component]
pub fn CatalogDisplayEntry() -> Element {
    let resource = use_server_future(catalog_display_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        CatalogDisplayPage {
            content: view.content,
            practices: view.practices,
        }
    }
}

/// The pure display face.
#[component]
pub fn CatalogDisplayPage(
    content: DisplayContent,
    #[props(default)] practices: Vec<PracticeLink>,
) -> Element {
    rsx! {
        document::Title { "{content.title} | {content.workshop_title}" }
        // No `PublicShell` here, so the theme stylesheet is this page's own to
        // hoist — it carries the `--nav-*` tokens every rule below reads.
        document::Stylesheet { href: THEME_STYLESHEET_HREF }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        // Without this the projector face stops being a deck.
        document::Script { src: CATALOG_DISPLAY_SCRIPT_HREF, defer: true }
        // A `main` rather than a bare `div`: the projector face carries no site
        // chrome, so without it the page has no main landmark at all and a
        // screen-reader user has nothing to jump to. Every rule in
        // `catalog.css` selects the class, so the element change is invisible.
        main { class: "nav-theme catalog-display", "data-catalog-display": true,
            // The same 16:9 canvas the step page and the light table use, so a
            // slide reads identically here — just scaled to the viewport. A
            // click anywhere on the surface advances.
            section {
                class: "workshop-slide catalog-display-slide",
                "data-catalog-advance": true,
                CatalogSlideBody {
                    title: content.title.clone(),
                    body_html: content.body_html.clone(),
                    practices: practices.clone(),
                }
            }
            // Near-invisible controls that fade in on hover or focus, so a
            // full-screen slide stays uncluttered. The ends are inert spans
            // rather than links: the first slide has no previous and the last
            // has no next, and an arrow key there falls through untouched.
            nav { class: "catalog-display-controls", "aria-label": "Slide navigation",
                if let Some(prev) = content.prev_href.clone() {
                    a {
                        class: "catalog-display-nav",
                        href: "{prev}",
                        "data-catalog-nav": "prev",
                        "aria-label": "Previous slide",
                        span { "aria-hidden": "true", "‹" }
                    }
                } else {
                    // `role="link"` is load-bearing, not decoration: a bare
                    // `span` carries the implicit `generic` role, which
                    // prohibits both `aria-label` and `aria-disabled`, so a
                    // screen reader drops the name and announces nothing at
                    // all. Naming it as a disabled link is the standard
                    // pattern and keeps the deck's ends legible — axe reports
                    // the bare span as `aria-prohibited-attr`.
                    span {
                        class: "catalog-display-nav catalog-display-nav--disabled",
                        role: "link",
                        "aria-disabled": "true",
                        "aria-label": "Previous slide",
                        span { "aria-hidden": "true", "‹" }
                    }
                }
                a {
                    class: "catalog-display-nav",
                    href: "{content.step_href}",
                    "data-catalog-nav": "exit",
                    "aria-label": "Exit display mode",
                    span { "aria-hidden": "true", "✕" }
                }
                if let Some(next) = content.next_href.clone() {
                    a {
                        class: "catalog-display-nav",
                        href: "{next}",
                        "data-catalog-nav": "next",
                        "aria-label": "Next slide",
                        span { "aria-hidden": "true", "›" }
                    }
                } else {
                    // Named as a disabled link for the same reason as the
                    // previous control above.
                    span {
                        class: "catalog-display-nav catalog-display-nav--disabled",
                        role: "link",
                        "aria-disabled": "true",
                        "aria-label": "Next slide",
                        span { "aria-hidden": "true", "›" }
                    }
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

    fn content() -> DisplayContent {
        DisplayContent {
            workshop_title: "Runbook".into(),
            title: "Build the template".into(),
            body_html: "<h3>Build the template</h3><p>do it</p>".into(),
            prev_href: Some("/workshops/use-the-navigator/display/1".into()),
            next_href: Some("/workshops/use-the-navigator/display/3".into()),
            step_href: "/workshops/use-the-navigator/step/2".into(),
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CatalogDisplayPage { content: content() }
            }
        }
        ssr(app)
    }

    #[test]
    fn the_middle_slide_offers_previous_next_and_exit() {
        let out = html();
        assert!(out.contains("<h3>Build the template</h3>"), "slide: {out}");
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator/display/1""#),
            "previous: {out}"
        );
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator/display/3""#),
            "next: {out}"
        );
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator/step/2""#),
            "exit to the presenter step: {out}"
        );
        for hook in ["prev", "next", "exit"] {
            assert!(
                out.contains(&format!(r#"data-catalog-nav="{hook}""#)),
                "missing {hook} hook: {out}"
            );
        }
    }

    #[test]
    fn every_hook_catalog_display_js_reads_is_present() {
        // The script finds the page by the root attribute and the click surface
        // by the advance attribute. A rename here leaves a still image that
        // still looks correct.
        let out = html();
        assert!(out.contains("data-catalog-display"), "root: {out}");
        assert!(out.contains("data-catalog-advance"), "click surface: {out}");
        assert!(
            out.contains("workshop-slide catalog-display-slide"),
            "reuses the shared 16:9 canvas: {out}"
        );
    }

    #[test]
    fn the_deck_ends_render_inert_controls_rather_than_links() {
        fn first() -> Element {
            rsx! {
                CatalogDisplayPage {
                    content: DisplayContent { prev_href: None, ..content() },
                }
            }
        }
        fn last() -> Element {
            rsx! {
                CatalogDisplayPage {
                    content: DisplayContent { next_href: None, ..content() },
                }
            }
        }
        let out = ssr(first);
        assert!(
            !out.contains(r#"data-catalog-nav="prev""#),
            "no previous anchor at the first slide: {out}"
        );
        assert!(
            out.contains(r#"data-catalog-nav="next""#),
            "still advances: {out}"
        );
        assert!(out.contains(r#"aria-disabled="true""#), "inert end: {out}");

        let out = ssr(last);
        assert!(
            !out.contains(r#"data-catalog-nav="next""#),
            "no next anchor at the last slide: {out}"
        );
        assert!(
            out.contains(r#"data-catalog-nav="prev""#),
            "still goes back: {out}"
        );
        assert!(out.contains(r#"aria-disabled="true""#), "inert end: {out}");
    }

    #[test]
    fn the_projector_face_carries_no_site_chrome_and_no_presenter_notes() {
        let out = html();
        assert!(!out.contains("<header"), "no site header: {out}");
        assert!(!out.contains("<footer"), "no site footer: {out}");
        assert!(!out.contains("public-shell"), "no public shell: {out}");
        assert!(!out.contains("Presenter notes"), "no notes: {out}");
    }
}
