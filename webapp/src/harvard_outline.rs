//! `/lawyer/outline` — Harvard-outline narration stage for a firm template.
//!
//! The lawyer workbench's recording surface: a document already in Harvard
//! form (the bundled retainer) rendered as highlightable units. Keyboard and
//! click handling live in `harvard-outline-narrate.js`, so the page works
//! without the wasm hydration bundle. Arbitrary drafts stay on the operator's
//! machine via `navigator template narrate`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Stylesheet for the stage highlight fills.
pub const HARVARD_OUTLINE_STYLESHEET_HREF: &str = "/public/css/harvard-outline.css";

/// First-party script that steps the current unit.
pub const HARVARD_OUTLINE_SCRIPT_HREF: &str = "/public/js/harvard-outline-narrate.js";

/// The pre-rendered stage the portal injects at construction.
#[derive(Clone, Default)]
pub struct InjectedOutlineStage(pub OutlineStageContent);

/// One bundled document ready to narrate.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OutlineStageContent {
    pub title: String,
    /// The `<article data-harvard-outline>` fragment.
    pub stage_html: String,
}

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OutlineStageView {
    pub firm_name: String,
    pub content: OutlineStageContent,
}

/// Resolve the injected stage and the viewer chrome.
#[server]
pub async fn outline_stage_view() -> Result<OutlineStageView, ServerFnError> {
    let _role = crate::admin_listing::require_lawyer().await?;
    let content = consume_context::<InjectedOutlineStage>().0;
    Ok(OutlineStageView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
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
            p { class: "nav-muted",
                "Press H to hide this chrome for a recording. "
                "Offline drafts: "
                code { "navigator template narrate" }
                "."
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
            content: OutlineStageContent {
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
