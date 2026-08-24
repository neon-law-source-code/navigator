//! `/docs` and `/docs/{slug}` — the workspace documentation, migrated to Dioxus
//! SSR (#956 Phase 4).
//!
//! The successor to the `views::pages::docs`. One doc is a title over a
//! rendered `CommonMark` body baked from the single-source-of-truth `docs/`
//! tree — these are workspace/reference docs, not an offer of representation.
//! The unified footer carries the firm's disclaimer, as it does site-wide.
//!
//! **Firm-branded on every host.** `portal`'s `docs_router` mounts these routes
//! once, in the composition every brand binary shares, so the page renders on
//! the firm's host and a white-label tenant's. The
//! documentation is the Firm's own operating material, so it wears the firm
//! chrome throughout: a retired second brand's wordmark and home link have no
//! business on the firm's host or a tenant's.
//!
//! Per-request content: the doc is selected by the `{slug}` path parameter, so
//! the portal route's pre-layer resolves it from the compiled-in `DocsIndex`
//! and injects it. That layer also owns every non-render outcome on the path —
//! the kebab-case redirect, the `/docs/index` → `/docs` redirect, and the
//! unknown-slug 404 — because axum cannot register a second `GET` handler where
//! the render sits.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The `<meta description>` the doc page carried.
const DOC_DESCRIPTION: &str = "Neon Law Navigator workspace documentation.";

/// One resolved workspace doc — the portal pre-layer builds it from the
/// compiled-in `DocsIndex`; the wasm-safe carrier across the server-function
/// boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DocContent {
    /// The page heading and `<title>` suffix ("Glossary").
    pub title: String,
    /// The rendered HTML body (already sanitized; NOT raw markdown).
    pub body_html: String,
}

/// The [`DocContent`] the portal route's pre-layer injects for the matched slug,
/// extracted back in [`docs_page_view`].
#[derive(Clone, Default)]
pub struct InjectedDoc(pub DocContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DocsPageView {
    pub chrome: PublicChrome,
    pub content: DocContent,
}

/// Resolve the doc from the injected extension and the chrome from the firm
/// brand.
#[server]
pub async fn docs_page_view() -> Result<DocsPageView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedDoc>, _>()
            .await
            .map_or_else(|_| DocContent::default(), |axum::Extension(doc)| doc.0);
    Ok(DocsPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn DocsPageEntry() -> Element {
    let resource = use_server_future(docs_page_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    docs_body(&view)
}

/// The doc page body. Prop-driven and free of any server future, so it
/// server-renders and unit-tests directly.
pub fn docs_body(view: &DocsPageView) -> Element {
    let chrome = &view.chrome;
    // The layout prefixed the brand ("{site_name} | {title}") on every
    // page, so a shared link previews the site name ahead of the doc name.
    let head_title = format!("{} | {}", chrome.brand_name, view.content.title);
    let header = rsx! {
        SiteHeader {
            brand_name: chrome.brand_name.clone(),
            home_href: chrome.home_href.clone(),
            logo_href: chrome.logo_href.clone(),
            destinations: chrome
                .destinations
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
            utility: chrome
                .utility
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
        }
    };
    let footer = rsx! {
        PublicFooter { chrome: chrome.clone() }
    };
    let body_html = view.content.body_html.clone();
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: DOC_DESCRIPTION }
        SocialMeta {
            title: head_title.clone(),
            description: DOC_DESCRIPTION.to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer,
            // The body is already-rendered, already-sanitized HTML baked from
            // the `docs/` tree, so it is emitted verbatim — the page used
            // `PreEscaped` for the same reason.
            article { class: "docs-article", dangerous_inner_html: "{body_html}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The firm chrome this page now wears. Hand-built because
    /// `firm_public_chrome` is gated behind the `server` feature, which this
    /// unit build does not carry — so these tests cover the *rendering* half
    /// only. That the `/docs` route actually resolves the firm's brand is
    /// pinned end-to-end by `docs_glossary_renders_headings` in
    /// `server/tests/routes.rs`, which drives the real router.
    fn view(title: &str, body_html: &str) -> DocsPageView {
        DocsPageView {
            chrome: PublicChrome {
                brand_name: "Neon Law".to_string(),
                home_href: "/".to_string(),
                logo_href: "/public/logo.svg".to_string(),
                firm_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            },
            content: DocContent {
                title: title.to_string(),
                body_html: body_html.to_string(),
            },
        }
    }

    fn render(view: &DocsPageView) -> String {
        dioxus_ssr::render_element(docs_body(view))
    }

    #[test]
    fn the_rendered_markdown_body_is_emitted_as_html_not_escaped() {
        let out = render(&view(
            "Glossary",
            "<h2 id=\"council\">Council</h2><p>A group.</p>",
        ));
        assert!(
            out.contains("<h2 id=\"council\">Council</h2>"),
            "baked heading survives verbatim, anchor id included: {out}"
        );
        assert!(out.contains("<p>A group.</p>"), "baked body paragraph");
        // Escaped markup would show up as `&lt;h2`.
        assert!(
            !out.contains("&lt;h2"),
            "the body must not be escaped: {out}"
        );
    }

    #[test]
    fn the_doc_sits_in_its_own_article() {
        let out = render(&view("Glossary", "<p>Body.</p>"));
        assert!(
            out.contains(r#"class="docs-article""#),
            "article class: {out}"
        );
    }

    #[test]
    fn the_page_wears_the_firm_chrome_on_every_host() {
        let out = render(&view("Glossary", "<p>Body.</p>"));
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "unified footer chrome");
        // The firm's wordmark and mark, because one mount serves the firm's
        // host and every tenant's, and these are the Firm's
        // own docs. Both brands share `home_href = "/"`, so the wordmark and the
        // logo are the discriminators — a home-link assertion would pass either
        // way.
        assert!(out.contains("Neon Law"), "firm wordmark: {out}");
        assert!(
            out.contains("/public/logo.svg"),
            "firm mark, not the nonprofit's: {out}"
        );
    }

    #[test]
    fn an_empty_doc_still_renders_its_shell() {
        // The default view is what a missing injection would produce; it must
        // not panic or emit a bare body.
        let out = render(&view("", ""));
        assert!(
            out.contains(r#"class="docs-article""#),
            "article still framed"
        );
        assert!(out.contains("site-header"), "chrome still rendered");
    }
}
