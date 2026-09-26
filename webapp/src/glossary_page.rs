//! `/glossary` — the Neon Law Navigator ontology on one page.
//!
//! The glossary is the documentation: every noun the product is built on,
//! one Markdown file per term under `docs/glossary/`. This page lays every
//! term out in one document with a side index of `#<slug>` links, so a
//! reader can jump to any definition and a link to `/glossary#matter`
//! lands on it. The former `/docs` catalog and its per-guide pages redirect
//! here.
//!
//! **Firm-branded on every host.** `portal`'s `glossary_router` mounts this
//! route once, in the composition every brand binary shares, so the page
//! renders on the firm's host and a white-label tenant's alike, wearing the
//! public chrome and the unified footer (which carries the firm's
//! disclaimer, as it does site-wide).
//!
//! Content is compiled in: the portal renders every term to sanitized HTML
//! once at boot and injects the result, so the page carries no store read.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The page heading and `<title>` suffix.
pub const GLOSSARY_TITLE: &str = "Glossary";

/// The `<meta description>` the page carries.
const GLOSSARY_DESCRIPTION: &str =
    "The Neon Law Navigator glossary: every term the product is built on, defined in one place.";

/// One rendered term.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GlossaryEntry {
    /// The in-page anchor (`lawyer-review`), and the term's stable key.
    pub slug: String,
    /// The term as a reader says it (`Lawyer Review`).
    pub title: String,
    /// The one-sentence lede beneath its title.
    pub description: String,
    /// The rendered definition (already sanitized; NOT raw markdown).
    pub body_html: String,
}

/// Everything the page shows besides its chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GlossaryContent {
    /// The rendered preamble above the first term.
    pub preamble_html: String,
    /// Every term, alphabetical by slug.
    pub entries: Vec<GlossaryEntry>,
}

/// The [`GlossaryContent`] the portal route's pre-layer injects, extracted
/// back in [`glossary_page_view`].
#[derive(Clone, Default)]
pub struct InjectedGlossary(pub GlossaryContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GlossaryPageView {
    pub chrome: PublicChrome,
    pub content: GlossaryContent,
}

/// Resolve the glossary from the injected extension and the chrome from the
/// firm brand.
#[server]
pub async fn glossary_page_view() -> Result<GlossaryPageView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedGlossary>, _>()
            .await
            .map_or_else(|_| GlossaryContent::default(), |axum::Extension(g)| g.0);
    Ok(GlossaryPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn GlossaryPageEntry() -> Element {
    let resource = use_server_future(glossary_page_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    glossary_body(&view)
}

/// A title as authored splits into prose and code spans: `` `ctx.run` ``
/// renders as `<code>ctx.run</code>` rather than showing its backticks.
fn title_parts(title: &str) -> Vec<(bool, String)> {
    title
        .split('`')
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(index, part)| (index % 2 == 1, part.to_string()))
        .collect()
}

/// One title, its code spans rendered as `<code>`.
fn title_element(title: &str) -> Element {
    rsx! {
        for (is_code , part) in title_parts(title) {
            if is_code {
                code { "{part}" }
            } else {
                "{part}"
            }
        }
    }
}

/// One letter of the side index and the terms filed under it.
struct IndexGroup<'a> {
    letter: char,
    entries: Vec<&'a GlossaryEntry>,
}

/// Group the entries by the initial of their slug — the letter a reader
/// looks a term up under, even when its title opens with punctuation
/// (`` `ctx.run` `` files under C).
fn index_groups(entries: &[GlossaryEntry]) -> Vec<IndexGroup<'_>> {
    let mut groups: Vec<IndexGroup<'_>> = Vec::new();
    for entry in entries {
        let letter = entry
            .slug
            .chars()
            .next()
            .map_or('#', |c| c.to_ascii_uppercase());
        match groups.last_mut() {
            Some(group) if group.letter == letter => group.entries.push(entry),
            _ => groups.push(IndexGroup {
                letter,
                entries: vec![entry],
            }),
        }
    }
    groups
}

/// The page body. Prop-driven and free of any server future, so it
/// server-renders and unit-tests directly.
pub fn glossary_body(view: &GlossaryPageView) -> Element {
    let chrome = &view.chrome;
    // Brand first ("{site_name} | Glossary"), so a shared link previews the
    // site name ahead of the page name.
    let head_title = format!("{} | {GLOSSARY_TITLE}", chrome.brand_name);
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
    let content = &view.content;
    let groups = index_groups(&content.entries);
    let preamble_html = content.preamble_html.clone();
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: GLOSSARY_DESCRIPTION }
        SocialMeta {
            title: head_title.clone(),
            description: GLOSSARY_DESCRIPTION.to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer,
            div { class: "glossary",
                nav { class: "glossary__index", "aria-label": "Glossary terms",
                    for group in groups {
                        section { class: "glossary__index-group",
                            h2 { class: "glossary__index-letter", "{group.letter}" }
                            ul { class: "glossary__index-list",
                                for entry in group.entries {
                                    li {
                                        a {
                                            class: "glossary__index-link",
                                            href: "#{entry.slug}",
                                            {title_element(&entry.title)}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                article { class: "glossary__terms", "aria-labelledby": "glossary-title",
                    h1 { id: "glossary-title", "{GLOSSARY_TITLE}" }
                    // Baked from `docs/glossary/` and sanitized at boot, so
                    // the HTML is emitted verbatim.
                    div { class: "glossary__preamble", dangerous_inner_html: "{preamble_html}" }
                    for entry in content.entries.iter() {
                        section {
                            class: "glossary__term",
                            id: "{entry.slug}",
                            "aria-labelledby": "{entry.slug}-title",
                            h2 { id: "{entry.slug}-title", class: "glossary__term-title",
                                a { class: "glossary__anchor", href: "#{entry.slug}", {title_element(&entry.title)} }
                            }
                            p { class: "glossary__lede", "{entry.description}" }
                            div {
                                class: "glossary__definition",
                                dangerous_inner_html: "{entry.body_html}",
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The firm chrome this page wears. Hand-built because
    /// `firm_public_chrome` is gated behind the `server` feature, which this
    /// unit build does not carry — so these tests cover the *rendering* half
    /// only. The real route is pinned end-to-end in `server/tests/routes.rs`.
    fn view(entries: &[(&str, &str, &str)]) -> GlossaryPageView {
        GlossaryPageView {
            chrome: PublicChrome {
                brand_name: "Neon Law".to_string(),
                home_href: "/".to_string(),
                logo_href: "/public/logo.svg".to_string(),
                firm_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            },
            content: GlossaryContent {
                preamble_html: "<p>The vocabulary.</p>".to_string(),
                entries: entries
                    .iter()
                    .map(|(slug, title, body_html)| GlossaryEntry {
                        slug: (*slug).to_string(),
                        title: (*title).to_string(),
                        description: "A short definition.".to_string(),
                        body_html: (*body_html).to_string(),
                    })
                    .collect(),
            },
        }
    }

    fn render(view: &GlossaryPageView) -> String {
        dioxus_ssr::render_element(glossary_body(view))
    }

    fn sample() -> GlossaryPageView {
        view(&[
            ("asset", "Asset", "<p>A byte artifact.</p>"),
            ("ctxrun", "`ctx.run`", "<p>The side-effect primitive.</p>"),
            (
                "council",
                "Council",
                "<p>A group. See <a href=\"#asset\">Asset</a>.</p>",
            ),
        ])
    }

    #[test]
    fn every_term_is_a_section_anchored_at_its_slug() {
        let out = render(&sample());
        for slug in ["asset", "ctxrun", "council"] {
            assert!(
                out.contains(&format!("id=\"{slug}\"")),
                "`#{slug}` must land on its term: {out}"
            );
        }
        assert!(
            out.contains("<p>A byte artifact.</p>"),
            "definition verbatim"
        );
        assert!(
            out.contains("<p class=\"glossary__lede\">A short definition.</p>"),
            "the description appears as a lede beneath its term"
        );
        assert!(
            !out.contains("&lt;p"),
            "the body must not be escaped: {out}"
        );
    }

    #[test]
    fn the_side_index_links_every_term_in_page() {
        let out = render(&sample());
        let nav = out
            .split_once("<nav class=\"glossary__index\" aria-label=\"Glossary terms\"")
            .and_then(|(_, rest)| rest.split_once("</nav>"))
            .map(|(nav, _)| nav)
            .expect("a named side index");
        for (slug, title) in [
            ("asset", "Asset"),
            ("ctxrun", "<code>ctx.run</code>"),
            ("council", "Council"),
        ] {
            assert!(
                nav.contains(&format!("href=\"#{slug}\">{title}</a>")),
                "index link for {slug}: {nav}"
            );
        }
    }

    #[test]
    fn the_index_files_terms_under_their_slug_initial() {
        let out = render(&sample());
        let a = out.find(">A</h2>").expect("A group");
        let c = out.find(">C</h2>").expect("C group");
        assert!(a < c, "letters in order: {out}");
        assert_eq!(
            out.matches(">C</h2>").count(),
            1,
            "`ctx.run` and Council share one C group: {out}"
        );
    }

    #[test]
    fn a_code_span_in_a_title_renders_as_code() {
        let out = render(&sample());
        assert!(!out.contains("`ctx.run`"), "no raw backticks: {out}");
        assert!(out.contains("<code>ctx.run</code>"), "{out}");
        assert_eq!(
            title_parts("Restate context (`ctx`)"),
            vec![
                (false, "Restate context (".to_string()),
                (true, "ctx".to_string()),
                (false, ")".to_string())
            ]
        );
    }

    #[test]
    fn the_page_has_one_heading_and_the_preamble() {
        let out = render(&sample());
        assert_eq!(out.matches("<h1").count(), 1, "one page heading: {out}");
        assert!(out.contains(">Glossary</h1>"));
        assert!(out.contains("<p>The vocabulary.</p>"), "preamble: {out}");
    }

    #[test]
    fn the_page_wears_the_firm_chrome_on_every_host() {
        let out = render(&sample());
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "unified footer chrome");
        assert!(out.contains("Neon Law"), "firm wordmark: {out}");
        assert!(out.contains("/public/logo.svg"), "firm mark: {out}");
    }

    #[test]
    fn an_empty_glossary_still_renders_its_shell() {
        let out = render(&GlossaryPageView::default());
        assert!(out.contains("class=\"glossary\""), "frame still rendered");
        assert!(out.contains("site-header"), "chrome still rendered");
    }
}
