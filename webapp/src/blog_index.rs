//! The `/blog` index, migrated to Dioxus SSR (issue #641 / #730 PR6 — the first
//! content-backed page port).
//!
//! Unlike the brand-only team pages, the blog reads request state: the portal
//! router builds the wasm-safe [`BlogPosts`] list from its `BlogIndex` and
//! injects it into the render context (the same `context_providers` seam the
//! lawyer pages use for the database), and [`blog_index_view`] reads it back.
//! Two layers like the team pages: the pure [`BlogIndex`] component and the
//! [`BlogIndexPage`] server wrapper. English-only (the firm blog has no `/es`
//! twin), so it renders in the default locale.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// One post's index-card summary — the pre-formatted date comes from the portal
/// side so the wasm DTO carries no date-formatting logic.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BlogPostSummary {
    pub slug: String,
    pub date: String,
    pub title: String,
    pub description: String,
}

/// The blog index content injected into the render context by the portal router
/// (built from its `BlogIndex`), read back by [`blog_index_view`]. The wasm-safe
/// carrier across the server-function boundary — `portal`'s `BlogIndex` cannot
/// cross it.
#[derive(Clone, Default)]
pub struct BlogPosts(pub Vec<BlogPostSummary>);

/// Everything the page renders: the resolved chrome and the post summaries.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BlogIndexView {
    pub chrome: PublicChrome,
    pub posts: Vec<BlogPostSummary>,
}

/// Resolve the page chrome from the process brand and the post list from the
/// injected [`BlogPosts`] context (the portal router provides it through
/// `ServeConfig::context_providers`, the same seam the lawyer pages use for the
/// database).
#[server]
pub async fn blog_index_view() -> Result<BlogIndexView, ServerFnError> {
    let posts = consume_context::<BlogPosts>().0;
    Ok(BlogIndexView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        posts,
    })
}

/// The page's route entry: resolve on the server, then render the pure page.
#[component]
pub fn BlogIndexPage() -> Element {
    let resource = use_server_future(blog_index_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        BlogIndex { chrome: view.chrome, posts: view.posts }
    }
}

/// The pure blog index: the public shell wrapping the post list. Every field is
/// a prop, so it server-renders and unit-tests without a server future.
#[component]
pub fn BlogIndex(chrome: PublicChrome, posts: Vec<BlogPostSummary>) -> Element {
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
        document::Title { "{chrome.brand_name} | Blog" }
        PublicShell { header, footer,
            section { class: "blog-index",
                h1 { "Blog" }
                ul { class: "blog-index__posts",
                    for post in posts.iter() {
                        li { class: "blog-index__post",
                            a { class: "blog-index__link", href: "/blog/{post.slug}",
                                h2 { class: "blog-index__title", "{post.title}" }
                            }
                            p { class: "blog-index__date", "{post.date}" }
                            p { class: "blog-index__description", "{post.description}" }
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

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn html() -> String {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let posts = vec![BlogPostSummary {
                slug: "thanks-apple".to_string(),
                date: "June 19, 2026".to_string(),
                title: "Thanks, Apple".to_string(),
                description: "A short note of thanks.".to_string(),
            }];
            rsx! { BlogIndex { chrome, posts } }
        }
        ssr(app)
    }

    #[test]
    fn lists_each_post_linked_to_its_slug() {
        let out = html();
        assert!(out.contains("Thanks, Apple"), "post title: {out}");
        assert!(
            out.contains(r#"href="/blog/thanks-apple""#),
            "post links to its slug"
        );
        assert!(out.contains("June 19, 2026"), "formatted date");
        assert!(out.contains("A short note of thanks."), "description");
    }

    #[test]
    fn wraps_the_list_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
    }
}
