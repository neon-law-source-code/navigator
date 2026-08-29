//! `/app/outline` — Harvard-outline narration stage for a firm template.
//!
//! The lawyer workbench's recording surface: bundled documents already in
//! Harvard form, rendered as highlightable units. `?doc=` selects among them
//! (onboarding letter, offboarding letter). Keyboard and click handling
//! live in `harvard-outline-narrate.js`, so the page works without the wasm
//! hydration bundle. Arbitrary drafts stay on the operator's machine via
//! `navigator template narrate`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Stylesheet for the stage highlight fills.
pub const HARVARD_OUTLINE_STYLESHEET_HREF: &str = "/public/css/harvard-outline.css";

/// First-party script that steps the current unit.
pub const HARVARD_OUTLINE_SCRIPT_HREF: &str = "/public/js/harvard-outline-narrate.js";

/// The pre-rendered library the portal injects at construction.
#[derive(Clone, Default)]
pub struct InjectedOutlineStage(pub Vec<OutlineStageContent>);

/// Query that picks which bundled document the stage shows.
#[derive(Deserialize, Default)]
pub struct OutlineStageQuery {
    #[serde(default)]
    pub doc: Option<String>,
}

/// One bundled document ready to narrate.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OutlineStageContent {
    pub slug: String,
    pub title: String,
    /// The `<article data-harvard-outline>` fragment.
    pub stage_html: String,
}

/// A switcher row: slug in `?doc=` and the document title.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OutlineDocLink {
    pub slug: String,
    pub title: String,
}

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OutlineStageView {
    pub firm_name: String,
    pub current_slug: String,
    pub library: Vec<OutlineDocLink>,
    pub content: OutlineStageContent,
}

/// Pick the requested document, or the first one when `doc` is missing or unknown.
#[must_use]
pub fn select_stage<'a>(
    library: &'a [OutlineStageContent],
    doc: Option<&str>,
) -> Option<&'a OutlineStageContent> {
    if library.is_empty() {
        return None;
    }
    if let Some(slug) = doc.filter(|s| !s.is_empty()) {
        if let Some(found) = library.iter().find(|item| item.slug == slug) {
            return Some(found);
        }
    }
    library.first()
}

/// Resolve the injected stage and the viewer chrome.
#[server]
pub async fn outline_stage_view() -> Result<OutlineStageView, ServerFnError> {
    let _role = crate::admin_listing::require_lawyer().await?;
    let query = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<OutlineStageQuery>,
        _,
    >()
    .await
    .map(|axum::extract::Query(q)| q)
    .unwrap_or_default();
    let library = consume_context::<InjectedOutlineStage>().0;
    let content = select_stage(&library, query.doc.as_deref())
        .cloned()
        .unwrap_or_default();
    let current_slug = content.slug.clone();
    Ok(OutlineStageView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        current_slug,
        library: library
            .into_iter()
            .map(|item| OutlineDocLink {
                slug: item.slug,
                title: item.title,
            })
            .collect(),
        content,
    })
}

/// Route entry.
#[component]
pub fn OutlineStage() -> Element {
    let resource = use_server_future(outline_stage_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "harvard-outline", p { "This stage is available to the lawyer tier." } }
            }
        }
        None => {
            return rsx! {
                main { id: "harvard-outline", p { "Loading…" } }
            }
        }
    };
    outline_stage_body(&view)
}

fn outline_stage_body(view: &OutlineStageView) -> Element {
    let title = view.content.title.clone();
    let stage_html = view.content.stage_html.clone();
    let current_slug = view.current_slug.clone();
    let library = view.library.clone();
    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Outline stage | {title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: HARVARD_OUTLINE_STYLESHEET_HREF }
        document::Script { src: HARVARD_OUTLINE_SCRIPT_HREF, defer: true }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/lawyer", "Workbench" }
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "harvard-outline", class: "nav-theme",
            p { class: "harvard-stage-intro nav-muted",
                "Press H to hide this chrome for a recording. "
                "Offline drafts: "
                code { "navigator template narrate" }
                "."
            }
            nav { class: "harvard-doc-switcher", aria_label: "Bundled outlines",
                for link in library {
                    a {
                        class: if link.slug == current_slug { "nav-link is-current" } else { "nav-link" },
                        href: "/app/outline?doc={link.slug}",
                        aria_current: if link.slug == current_slug { Some("page") } else { None },
                        "{link.title}"
                    }
                }
            }
            div { dangerous_inner_html: "{stage_html}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> OutlineStageView {
        OutlineStageView {
            firm_name: "Example Law".to_string(),
            current_slug: "onboarding".to_string(),
            library: vec![
                OutlineDocLink {
                    slug: "onboarding".to_string(),
                    title: "Retainer Agreement".to_string(),
                },
                OutlineDocLink {
                    slug: "offboarding".to_string(),
                    title: "Closing Letter".to_string(),
                },
            ],
            content: OutlineStageContent {
                slug: "onboarding".to_string(),
                title: "Retainer Agreement".to_string(),
                stage_html: "<article class=\"harvard-stage\" data-harvard-outline>\
                    <section class=\"harvard-unit harvard-unit--depth-1\" data-harvard-path=\"I\">\
                    <h2>Scope of the engagement</h2></section></article>"
                    .to_string(),
            },
        }
    }

    #[test]
    fn the_stage_landmark_carries_the_outline_root() {
        let html = dioxus_ssr::render_element(outline_stage_body(&view()));
        assert!(html.contains("id=\"harvard-outline\""), "{html}");
        assert!(html.contains("data-harvard-outline"), "{html}");
        assert!(html.contains("data-harvard-path=\"I\""), "{html}");
        assert!(html.contains("Scope of the engagement"), "{html}");
        assert!(html.contains("navigator template narrate"), "{html}");
        assert!(html.contains("Press H to hide"), "{html}");
        assert!(html.contains("/app/outline?doc=onboarding"), "{html}");
        assert!(html.contains("/app/outline?doc=offboarding"), "{html}");
        assert!(html.contains("aria-label=\"Bundled outlines\""), "{html}");
    }

    #[test]
    fn select_stage_defaults_and_matches_slug() {
        let library = vec![
            OutlineStageContent {
                slug: "onboarding".into(),
                title: "Retainer".into(),
                stage_html: "r".into(),
            },
            OutlineStageContent {
                slug: "offboarding".into(),
                title: "Closing Letter".into(),
                stage_html: "c".into(),
            },
        ];
        assert_eq!(
            select_stage(&library, None).map(|d| d.slug.as_str()),
            Some("onboarding")
        );
        assert_eq!(
            select_stage(&library, Some("offboarding")).map(|d| d.slug.as_str()),
            Some("offboarding")
        );
        assert_eq!(
            select_stage(&library, Some("nope")).map(|d| d.slug.as_str()),
            Some("onboarding")
        );
        assert!(select_stage(&[], Some("retainer")).is_none());
    }

    #[test]
    fn the_stage_assets_match_the_public_files() {
        assert_eq!(
            HARVARD_OUTLINE_SCRIPT_HREF,
            "/public/js/harvard-outline-narrate.js"
        );
        assert_eq!(
            HARVARD_OUTLINE_STYLESHEET_HREF,
            "/public/css/harvard-outline.css"
        );
        let js = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/public/js/harvard-outline-narrate.js"
        ));
        assert!(js.contains("data-harvard-outline"), "{js}");
        let css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/public/css/harvard-outline.css"
        ));
        assert!(css.contains(".harvard-unit.is-current"), "{css}");
        assert!(css.contains(".harvard-doc-switcher"), "{css}");
    }

    #[test]
    fn the_page_hoists_the_stage_assets() {
        let src = include_str!("harvard_outline.rs");
        assert!(
            src.contains("document::Script { src: HARVARD_OUTLINE_SCRIPT_HREF"),
            "{src}"
        );
        assert!(
            src.contains("document::Stylesheet { href: HARVARD_OUTLINE_STYLESHEET_HREF"),
            "{src}"
        );
    }
}
