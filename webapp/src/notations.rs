//! `/notations` — the Notations page, migrated to Dioxus SSR
//! (#956 Phase 4).
//!
//! The successor to the `views::pages::template_tree`. It opens with a
//! services-style neon hero and a short story about what a notation *is*, then
//! renders `templates/README.md` for the tree-organization detail, so the public
//! page stays tied to the repository instructions.
//!
//! Fixed content: the README is baked in at compile time, so the portal router
//! resolves the rendered body once at construction and injects it, rather than
//! per request. The hero owns the page title, so the README's leading
//! `# Notations` heading is stripped before rendering (see
//! `views::notations::readme_html`).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The stylesheet that styles the animated product hero — self-contained on its
/// own `--ph-*` custom properties; hoisted into the head so the hero renders
/// without porting its CSS.
pub const PRODUCT_HERO_STYLESHEET_HREF: &str = "/public/css/product-hero.css";

/// The page's `<meta description>`, as the page set it.
const DESCRIPTION: &str = "Neon Law Navigator notations: the executable markdown form of the \
                           firm's legal work — template, questionnaire, and workflow in one file, \
                           checked live by the LSP.";

/// The rendered README body the portal router resolves at construction.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct NotationsContent {
    /// The rendered README HTML (already link-rewritten; NOT raw markdown).
    pub readme_html: String,
}

/// The [`NotationsContent`] the portal router injects for this route.
#[derive(Clone, Default)]
pub struct InjectedNotations(pub NotationsContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct NotationsView {
    pub chrome: PublicChrome,
    pub content: NotationsContent,
}

/// Resolve the page: the injected README body plus the Foundation chrome.
#[server]
pub async fn notations_view() -> Result<NotationsView, ServerFnError> {
    let content = consume_context::<InjectedNotations>().0;
    Ok(NotationsView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn NotationsEntry() -> Element {
    let resource = use_server_future(notations_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    notations_body(&view)
}

/// The page body. Prop-driven and free of any server future, so it
/// server-renders and unit-tests directly.
pub fn notations_body(view: &NotationsView) -> Element {
    let chrome = &view.chrome;
    let head_title = format!("{} | Notations", chrome.brand_name);
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
    let readme_html = view.content.readme_html.clone();
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: DESCRIPTION }
        SocialMeta {
            title: head_title.clone(),
            description: DESCRIPTION.to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        // `product-hero.css` is a separate stylesheet the layout linked on
        // every page; a Dioxus page loads only what it names, so the hero band
        // hoists it here — the same way the service pages do.
        document::Stylesheet { href: PRODUCT_HERO_STYLESHEET_HREF }
        PublicShell { header, footer,
            NotationsHero {}
            article { class: "docs-article",
                NotationsStory {}
                div { dangerous_inner_html: "{readme_html}" }
            }
        }
    }
}

/// The services-style neon hero band. It owns the page title, which is why the
/// README's own `# Notations` heading is stripped from the body.
#[component]
fn NotationsHero() -> Element {
    rsx! {
        section { class: "product-hero",
            div { class: "product-hero__bg", "aria-hidden": "true",
                div { class: "product-hero__glow" }
                div { class: "product-hero__grid" }
                div { class: "product-hero__horizon" }
                div { class: "product-hero__sweep" }
            }
            div { class: "product-hero__content",
                h1 { class: "product-hero__title", "Notations" }
                p { class: "product-hero__tagline",
                    "Every Markdown file we publish is checked the moment we type it. A notation is what that "
                    "same checked Markdown becomes when it carries the questions and the workflow of real legal work."
                }
            }
        }
    }
}

/// The story arc: ordinary checked Markdown → add a questionnaire and a workflow
/// → an executable, attorney-gated legal instrument.
#[component]
fn NotationsStory() -> Element {
    rsx! {
        section { class: "notations-story",
            p {
                "We hold every Markdown file in Neon Law Navigator to the same standard — the language server "
                "checks each one as it is written, and underlines what is wrong in red before it is ever "
                "saved. Our READMEs, our docs, our blog posts: all of them, the same way."
            }
            p {
                "A "
                strong { "notation template" }
                " starts life as one more Markdown file held to that standard. What sets it apart is its "
                "frontmatter: declare a "
                code { "questionnaire" }
                " (the questions a client answers) and a "
                code { "workflow" }
                " (the path the document walks, with a mandatory attorney-review step), and the file stops "
                "being a document about the law and becomes an instrument that "
                em { "runs" }
                " it."
            }
            p {
                "That is the whole idea of a "
                strong { "notation" }
                ": the executable form of legal work. The template is the prose a client signs, the "
                "questionnaire fills it in, and the workflow carries it from intake to attorney review to "
                "signature — three faces of one checked file. Plain documentation, elevated, and verified the "
                "entire way down. The pages below show how the tree is organized; the keys are explained, in "
                "plain English, in "
                a { href: "/docs/frontmatter", "the frontmatter guide" }
                "."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(readme_html: &str) -> NotationsView {
        NotationsView {
            chrome: PublicChrome {
                // The shared header, which is the firm's: a Foundation page
                // wears the same wordmark, mark, and home link as every other
                // page. A fixture naming the retired Foundation header would
                // describe chrome no route produces.
                brand_name: "Neon Law".to_string(),
                home_href: "/".to_string(),
                logo_href: "/public/logo-firm.svg".to_string(),
                // The shared header's real destinations, so the Foundation
                // link the assertions below look for is the one the live
                // chrome supplies — a nav entry now, not the brand href.
                destinations: vec![crate::public_chrome::ChromeNavLink {
                    label: "Foundation".to_string(),
                    href: "/foundation".to_string(),
                }],
                firm_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            },
            content: NotationsContent {
                readme_html: readme_html.to_string(),
            },
        }
    }

    fn render(readme_html: &str) -> String {
        dioxus_ssr::render_element(notations_body(&view(readme_html)))
    }

    #[test]
    fn the_page_opens_with_the_neon_hero_band() {
        let out = render("<p>Body.</p>");
        assert!(out.contains("product-hero__title"), "hero band: {out}");
        assert!(out.contains("product-hero__glow"), "hero motif layers");
        // `document::Stylesheet` is a head element the fullstack head collector
        // emits, not body markup, so `product-hero.css` cannot be asserted here.
        // The real route carries it — see
        // `server/tests/routes.rs::notations_serve_the_tree_readme_under_foundation_brand`.
    }

    #[test]
    fn the_story_explains_a_notation_and_links_the_frontmatter_guide() {
        let out = render("<p>Body.</p>");
        assert!(out.contains("notations-story"), "story section: {out}");
        assert!(out.contains("executable form of legal work"));
        assert!(
            out.contains(r#"href="/docs/frontmatter""#),
            "links the frontmatter guide: {out}"
        );
    }

    #[test]
    fn the_readme_body_is_emitted_as_html_not_escaped() {
        let out = render("<h2 id=\"naming\">Naming convention</h2>");
        assert!(
            out.contains("<h2 id=\"naming\">Naming convention</h2>"),
            "README heading verbatim, anchor id included: {out}"
        );
        assert!(
            !out.contains("&lt;h2"),
            "the body must not be escaped: {out}"
        );
    }

    /// The hero owns the title, so the page carries exactly one `Notations`
    /// `<h1>` even when the injected body is non-empty.
    #[test]
    fn exactly_one_notations_heading() {
        let out = render("<p>Body.</p>");
        assert_eq!(
            out.matches(">Notations</h1>").count(),
            1,
            "one Notations <h1> (the hero), not a duplicate from the README: {out}"
        );
    }

    /// The page wears the site's one shared chrome.
    ///
    /// The mark opens the site root, not `/foundation` — the nonprofit's own
    /// header is retired — and the nonprofit is reached from a destination in
    /// that header's row instead. Both halves are asserted: a brand href of
    /// `/foundation` would mean the retired header came back, and a missing
    /// `/foundation` anywhere would mean the page lost its way to the
    /// organization it belongs to.
    #[test]
    fn the_page_wears_the_shared_chrome() {
        let out = render("<p>Body.</p>");
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "unified footer chrome");
        assert!(
            out.contains(r#"class="site-header__brand" href="/""#),
            "the mark opens the site root: {out}"
        );
        assert!(
            out.contains(r#"href="/foundation""#),
            "and the header row links the nonprofit: {out}"
        );
    }
}
