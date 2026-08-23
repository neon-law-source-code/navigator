//! `/{category}/{slug}` — a workshop or presentation's hub,
//! migrated to Dioxus SSR (#956 Phase 4).
//!
//! This is the
//! bookmarkable page a returning learner lands on: an orientation lede, a
//! numbered outline grouped by chapter, a "start" button, and a link to the
//! material's raw-Markdown twin.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, CATALOG_STYLESHEET_HREF};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// One entry in the outline.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MaterialStep {
    /// 1-based position across the whole material.
    pub number: usize,
    pub title: String,
    pub href: String,
}

/// One chapter of the outline, with the steps it contains.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MaterialChapter {
    pub number: usize,
    pub title: String,
    /// Rendered prose introducing the chapter. Empty renders nothing.
    pub preamble_html: String,
    pub steps: Vec<MaterialStep>,
}

/// The material hub's resolved content.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MaterialContent {
    pub title: String,
    pub description: String,
    /// Rendered HTML for the pre-heading orientation lede. Empty renders nothing.
    pub intro_html: String,
    /// Full rendered body, used only when the material has no chapters to step
    /// through.
    pub body_html: String,
    pub chapters: Vec<MaterialChapter>,
    /// Where "Start →" goes: the first step, when there is one.
    pub start_href: Option<String>,
    /// The light-table grid of every slide.
    pub slides_href: String,
    /// This material's raw-Markdown twin. The page links to it and the head
    /// advertises it as `rel="alternate"` — one source, two uses.
    pub md_href: String,
}

/// The [`MaterialContent`] the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedMaterial(pub MaterialContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogMaterialView {
    pub chrome: PublicChrome,
    pub content: MaterialContent,
}

/// Resolve the Foundation chrome and this material's content.
#[server]
pub async fn catalog_material_view() -> Result<CatalogMaterialView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedMaterial>, _>()
            .await
            .map(|axum::Extension(c)| c.0)
            .unwrap_or_default();
    Ok(CatalogMaterialView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn CatalogMaterialEntry() -> Element {
    let resource = use_server_future(catalog_material_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        CatalogMaterialPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure material hub.
#[component]
pub fn CatalogMaterialPage(chrome: PublicChrome, content: MaterialContent) -> Element {
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
    rsx! {
        document::Title { "{chrome.brand_name} | {content.title}" }
        document::Meta { name: "description", content: "{content.description}" }
        // The raw-Markdown twin, advertised to anything that prefers source over
        // rendered HTML. Head content, so no component test sees it.
        document::Link {
            rel: "alternate",
            r#type: "text/markdown",
            href: "{content.md_href}",
        }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        PublicShell { header, footer,
            article { class: "material-page",
                // The description stays metadata — the `<meta>` tag above and
                // the index card — rather than repeating on the page, where
                // the material's own intro already opens it.
                header { class: "material-header",
                    h1 { "{content.title}" }
                }
                if !content.intro_html.is_empty() {
                    div { class: "material-intro", dangerous_inner_html: "{content.intro_html}" }
                }
                div { class: "material-actions",
                    if let Some(start) = content.start_href.clone() {
                        a { class: "nav-btn nav-btn--primary", href: "{start}", "Start →" }
                    }
                    if !content.chapters.is_empty() {
                        a { class: "nav-btn nav-btn--secondary", href: "{content.slides_href}",
                            "View all slides"
                        }
                    }
                    a { class: "nav-btn nav-btn--secondary", href: "{content.md_href}",
                        "View as Markdown"
                    }
                }
                if content.chapters.is_empty() {
                    div { class: "material-body", dangerous_inner_html: "{content.body_html}" }
                } else {
                    nav { "aria-label": "Workshop outline",
                        div { class: "material-outline",
                            for chapter in content.chapters.iter() {
                                OutlineChapter { chapter: chapter.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One chapter of the outline: a numbered heading over its ordered steps.
#[component]
fn OutlineChapter(chapter: MaterialChapter) -> Element {
    rsx! {
        section {
            class: "workshop-chapter",
            "data-workshop-chapter": "{chapter.title}",
            header { class: "workshop-chapter__header",
                span { class: "catalog-badge", "{chapter.number}" }
                div {
                    p { class: "catalog-eyebrow", "Chapter {chapter.number}" }
                    h2 { class: "workshop-chapter__title", "{chapter.title}" }
                    if !chapter.preamble_html.is_empty() {
                        div {
                            class: "workshop-chapter__preamble",
                            dangerous_inner_html: "{chapter.preamble_html}",
                        }
                    }
                }
            }
            ol { class: "workshop-steps",
                for step in chapter.steps.iter() {
                    li { class: "workshop-steps__item",
                        span { class: "workshop-steps__number", "{step.number}" }
                        a { href: "{step.href}", "{step.title}" }
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

    fn stepped() -> MaterialContent {
        MaterialContent {
            title: "Using Neon Law Navigator".to_string(),
            description: "You walk out with a notation you built yourself.".to_string(),
            intro_html: "<p>Read this first.</p>".to_string(),
            body_html: String::new(),
            chapters: vec![
                MaterialChapter {
                    number: 1,
                    title: "Intro".to_string(),
                    preamble_html: "<p>Where we start.</p>".to_string(),
                    steps: vec![
                        MaterialStep {
                            number: 1,
                            title: "Install".to_string(),
                            href: "/workshops/use-the-navigator/step/1".to_string(),
                        },
                        MaterialStep {
                            number: 2,
                            title: "Build the template".to_string(),
                            href: "/workshops/use-the-navigator/step/2".to_string(),
                        },
                    ],
                },
                MaterialChapter {
                    number: 2,
                    title: "Wrap Up".to_string(),
                    preamble_html: String::new(),
                    steps: vec![MaterialStep {
                        number: 3,
                        title: "Notarize".to_string(),
                        href: "/workshops/use-the-navigator/step/3".to_string(),
                    }],
                },
            ],
            start_href: Some("/workshops/use-the-navigator/step/1".to_string()),
            slides_href: "/workshops/use-the-navigator/slides".to_string(),
            md_href: "/workshops/use-the-navigator.md".to_string(),
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CatalogMaterialPage { chrome: PublicChrome::default(), content: stepped() }
            }
        }
        ssr(app)
    }

    #[test]
    fn the_outline_groups_every_step_under_its_chapter() {
        let out = html();
        assert!(out.contains("Intro"), "first chapter: {out}");
        assert!(out.contains("Wrap Up"), "second chapter: {out}");
        for (n, title) in [(1, "Install"), (2, "Build the template"), (3, "Notarize")] {
            assert!(out.contains(title), "step {n} title: {out}");
            assert!(
                out.contains(&format!(r#"href="/workshops/use-the-navigator/step/{n}""#)),
                "step {n} links to its own URL: {out}"
            );
        }
        assert_eq!(
            out.matches("workshop-chapter\"").count(),
            2,
            "one section per chapter: {out}"
        );
    }

    #[test]
    fn the_chapter_keeps_its_progress_hook() {
        // `workshop-progress.js` reads `data-workshop-chapter` to group the
        // slide-seen state; dropping it silently breaks progress grouping.
        let out = html();
        assert!(
            out.contains(r#"data-workshop-chapter="Intro""#),
            "chapter hook: {out}"
        );
    }

    #[test]
    fn a_chapter_preamble_renders_only_when_present() {
        let out = html();
        assert!(out.contains("Where we start."), "chapter 1 preamble: {out}");
        assert_eq!(
            out.matches("workshop-chapter__preamble").count(),
            1,
            "chapter 2 has no preamble, so renders none: {out}"
        );
    }

    #[test]
    fn the_page_offers_start_slides_and_the_markdown_link() {
        let out = html();
        assert!(out.contains("Start →"), "start button: {out}");
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator/slides""#),
            "light-table link: {out}"
        );
        assert!(out.contains("View as Markdown"), "visible md link: {out}");
    }

    #[test]
    fn the_description_stays_metadata_and_never_renders_as_a_lede() {
        // It is still the `<meta name="description">` and the index card's
        // summary; it just no longer repeats above the material's own intro.
        // `document::Meta` is head content, so its absence here is the page
        // body's contract rather than a claim the metadata went away.
        let out = html();
        assert!(
            !out.contains("You walk out with a notation you built yourself."),
            "description must not render as an on-page lede: {out}"
        );
        assert!(
            out.contains("Read this first."),
            "the intro still opens the page: {out}"
        );
    }

    #[test]
    fn the_page_offers_no_copy_to_clipboard_affordance() {
        // The clipboard button was removed; the `.md` twin is reached through
        // the visible link and the `rel="alternate"` head tag instead. Asserted
        // on the hooks too, because a button whose label changed but whose
        // markup returned would be the same regression wearing a new name.
        let out = html();
        assert!(
            !out.contains("Copy as Markdown"),
            "copy button label: {out}"
        );
        assert!(
            !out.contains("data-copy-markdown"),
            "copy button hook: {out}"
        );
    }

    #[test]
    fn a_material_with_no_chapters_renders_its_whole_body_instead() {
        fn app() -> Element {
            let content = MaterialContent {
                title: "One-pager".to_string(),
                description: "No steps.".to_string(),
                body_html: "<p>The whole thing.</p>".to_string(),
                md_href: "/x.md".to_string(),
                ..MaterialContent::default()
            };
            rsx! { CatalogMaterialPage { chrome: PublicChrome::default(), content } }
        }
        let out = ssr(app);
        assert!(out.contains("The whole thing."), "full body: {out}");
        assert!(
            !out.contains("material-outline"),
            "no outline without chapters: {out}"
        );
        assert!(
            !out.contains("Start →"),
            "nothing to start without steps: {out}"
        );
        assert!(
            !out.contains("View all slides"),
            "no light table without slides: {out}"
        );
    }

    #[test]
    fn the_title_is_the_pages_single_h1() {
        let out = html();
        assert_eq!(out.matches("<h1").count(), 1, "exactly one h1: {out}");
        assert!(
            out.contains("Using Neon Law Navigator"),
            "the material title: {out}"
        );
    }
}
