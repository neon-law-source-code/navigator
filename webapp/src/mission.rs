//! The Foundation's mission letter — its home at `/`, migrated to Dioxus SSR
//! (#956 Phase 4).
//!
//! The successor to the `views::pages::mission`. The letter body comes from
//! `server/content/marketing/mission.md`, so this component never parses
//! markdown; the portal router resolves the rendered body at construction and
//! injects it.
//!
//! **Bare chrome.** Unlike every other ported public page, the mission renders
//! no site header and no footer. That is deliberate and load-bearing: the
//! Foundation's training-only mission must not inherit the firm's product
//! navigation or its legal/service footer. Do not wrap this page in
//! `PublicShell` — the page reads start-to-finish as one unbroken letter, and
//! the covering tests assert the absence of that chrome.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Freshness, SocialMeta, THEME_STYLESHEET_HREF};

/// The mission letter, resolved by the portal router at construction.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MissionContent {
    /// The page title.
    pub title: String,
    /// The `<meta name="description">`.
    pub description: String,
    /// The rendered HTML letter (already sanitized; NOT raw markdown).
    pub body_html: String,
    /// Pre-formatted "last edited in main" date, or `None` in production (the
    /// distroless image has no git binary), where the line is simply omitted.
    pub last_edited: Option<String>,
}

/// The [`MissionContent`] the portal router injects for this route.
#[derive(Clone, Default)]
pub struct InjectedMission(pub MissionContent);

/// Everything the page renders. The mission draws no site chrome, but it still
/// needs the brand *name* for its document title, and that must come from the
/// request task where the brand `task_local` is live — a white-label deploy
/// titles the letter with its own identity, not the default.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MissionView {
    pub content: MissionContent,
    pub brand_name: String,
    /// Absolute URL of the brand's raster mark, for the share card's `og:image`.
    pub social_image: String,
}

/// Resolve the letter from the injected content, plus the brand name from the
/// chrome the portal pre-layer resolved on the request task.
#[server]
pub async fn mission_view() -> Result<MissionView, ServerFnError> {
    let chrome = crate::public_chrome::firm_public_chrome_from_context().await;
    Ok(MissionView {
        content: consume_context::<InjectedMission>().0,
        brand_name: chrome.brand_name,
        social_image: chrome.social_image,
    })
}

/// The page's route entry.
#[component]
pub fn MissionEntry() -> Element {
    let resource = use_server_future(mission_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    mission_body(&view)
}

/// The letter. Prop-driven and free of any server future, so it server-renders
/// and unit-tests directly.
pub fn mission_body(view: &MissionView) -> Element {
    let content = &view.content;
    let body_html = content.body_html.clone();
    // The layout prefixed the brand on every page title.
    let head_title = format!("{} | {}", view.brand_name, content.title);
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: "{content.description}" }
        // Bare chrome drops the navbar and the footer, NOT the head: the
        // layout emitted the Open Graph / Twitter share card on every page, and
        // the brand-routing feature asserts `og:site_name` here.
        SocialMeta {
            title: head_title.clone(),
            description: content.description.clone(),
            site_name: view.brand_name.clone(),
            image: view.social_image.clone(),
        }
        document::Stylesheet { href: THEME_STYLESHEET_HREF }
        // No `PublicShell`: the training-only mission carries neither the site
        // header nor the legal/service footer. See the module docs.
        main {
            // The letter reads at a ~65-character measure and is centered, so it
            // stays readable on a phone without sprawling across a wide desktop.
            // `ch` tracks the body font, so the cap holds as the type scales.
            article {
                class: "mission-letter",
                style: "max-width: 65ch; margin-inline: auto;",
                div { dangerous_inner_html: "{body_html}" }
                Freshness { last_edited: content.last_edited.clone() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english() -> MissionContent {
        MissionContent {
            title: "Mission".to_string(),
            description: "Why we exist.".to_string(),
            body_html: "<h2>Why Rust</h2><p>Type-safe workflows.</p>".to_string(),
            last_edited: None,
        }
    }

    fn render(content: MissionContent) -> String {
        dioxus_ssr::render_element(mission_body(&MissionView {
            content,
            brand_name: "Neon Law".to_string(),
            social_image: "https://example.test/logo-firm.png".to_string(),
        }))
    }

    #[test]
    fn the_letter_body_is_emitted_as_html_not_escaped() {
        let out = render(english());
        assert!(out.contains("<h2>Why Rust</h2>"), "letter body: {out}");
        assert!(out.contains("<p>Type-safe workflows.</p>"));
        assert!(
            !out.contains("&lt;h2"),
            "the body must not be escaped: {out}"
        );
    }

    /// The training-only mission must not inherit the firm's product navigation
    /// or its legal/service footer — the reason the page asked for bare
    /// chrome. Wrapping this page in `PublicShell` would silently reintroduce
    /// both.
    #[test]
    fn the_letter_carries_no_site_header_or_footer() {
        let out = render(english());
        assert!(!out.contains("site-header"), "no site header: {out}");
        assert!(!out.contains("site-footer"), "no site footer: {out}");
        assert!(!out.contains("<footer"), "no footer element: {out}");
    }

    #[test]
    fn the_freshness_line_appears_only_when_the_date_is_known() {
        let dated = render(MissionContent {
            last_edited: Some("May 22, 2026".to_string()),
            ..english()
        });
        assert!(
            dated.contains("Last edited in main May 22, 2026"),
            "freshness line: {dated}"
        );
        // Production has no git binary, so the date is absent and the line is
        // simply omitted rather than rendered empty.
        let undated = render(english());
        assert!(!undated.contains("Last edited"), "no line: {undated}");
    }

    #[test]
    fn the_letter_is_capped_at_a_readable_measure() {
        let out = render(english());
        assert!(
            out.contains(r#"class="mission-letter""#),
            "letter class: {out}"
        );
        assert!(out.contains("max-width: 65ch"), "65ch measure: {out}");
    }
}
