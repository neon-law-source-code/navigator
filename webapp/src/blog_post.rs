//! A firm `/blog/{slug}` post page, migrated to Dioxus SSR (#641 / #730 PR6 —
//! content-backed, per #811).
//!
//! Per-request content: unlike the doc-only service pages (one fixed value per
//! route), a post is selected by the `{slug}` path parameter, so the portal
//! route's pre-layer resolves the matching post from the compiled-in blog and
//! injects it as a per-request `axum::Extension`; [`blog_post_view`] extracts it
//! back over the server-function boundary. The pre-layer also owns the legacy
//! underscore→hyphen redirect and the unknown-slug 404, matching the
//! handler's control flow. The blog is English-only — there is no `/es` twin, so
//! this page carries no language switcher or hreflang alternates.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// First-party progressive enhancement for photo collages in rendered posts.
/// It turns the existing, correctly labelled images into keyboard-operable
/// dialog triggers without requiring hydration.
pub const COLLAGE_LIGHTBOX_SCRIPT_HREF: &str = "/public/js/collage-lightbox.js";

/// One resolved blog post — the portal pre-layer builds it from the compiled-in
/// blog content and injects it per request; the wasm-safe carrier across the
/// server-function boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BlogPostContent {
    /// Pre-formatted publish date ("June 19, 2026"), formatted portal-side to
    /// match the post.
    pub date: String,
    pub title: String,
    /// The rendered HTML body (already sanitized; NOT raw markdown).
    pub body_html: String,
}

/// The [`BlogPostContent`] the portal route's pre-layer injects for the matched
/// slug, extracted back in [`blog_post_view`].
#[derive(Clone, Default)]
pub struct InjectedBlogPost(pub BlogPostContent);

/// The compiled-in posts keyed by slug — the portal route's pre-layer state,
/// which it consults to redirect legacy slugs, 404 unknown ones, and inject the
/// matched post. `Arc`-shared so the layer state is cheap to clone per request.
#[derive(Clone, Default)]
pub struct BlogPostSet(pub std::sync::Arc<std::collections::HashMap<String, BlogPostContent>>);

impl BlogPostSet {
    /// Look up the post for a canonical (kebab-case) slug.
    #[must_use]
    pub fn get(&self, slug: &str) -> Option<&BlogPostContent> {
        self.0.get(slug)
    }
}

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BlogPostView {
    pub chrome: PublicChrome,
    pub content: BlogPostContent,
    /// The full document `<title>` ("Neon Law | Going all in on Rust"),
    /// assembled from the brand and the post title.
    pub head_title: String,
}

/// The post's fallback `<meta description>` when a post has no title.
const EMPTY_POST_DESCRIPTION: &str = "A post from the firm blog.";

/// Resolve the chrome from the process brand and the post from the injected
/// per-request [`InjectedBlogPost`] extension.
#[server]
pub async fn blog_post_view() -> Result<BlogPostView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedBlogPost>, _>()
            .await
            .map_or_else(
                |_| BlogPostContent::default(),
                |axum::Extension(post)| post.0,
            );
    let chrome = crate::public_chrome::firm_public_chrome_from_context().await;
    // The head prefixed the brand ("{site_name} | {title}") so a shared
    // link previews the firm ahead of the post name.
    let head_title = if content.title.is_empty() {
        chrome.brand_name.clone()
    } else {
        format!("{} | {}", chrome.brand_name, content.title)
    };
    Ok(BlogPostView {
        chrome,
        content,
        head_title,
    })
}

/// The page's route entry.
#[component]
pub fn BlogPostEntry() -> Element {
    let resource = use_server_future(blog_post_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        BlogPostPage {
            chrome: view.chrome,
            content: view.content,
            head_title: view.head_title,
        }
    }
}

/// The pure post page: the post as a centered letter inside the public shell.
/// Prop-driven, so it server-renders and unit-tests without a server future.
#[component]
pub fn BlogPostPage(chrome: PublicChrome, content: BlogPostContent, head_title: String) -> Element {
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
    let description = if content.title.is_empty() {
        EMPTY_POST_DESCRIPTION.to_string()
    } else {
        content.title.clone()
    };
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: "{description}" }
        // The Open Graph / Twitter share card the post head emitted.
        SocialMeta {
            title: head_title.clone(),
            description: description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        document::Script { src: COLLAGE_LIGHTBOX_SCRIPT_HREF, defer: true }
        PublicShell { header, footer,
            // A post reads as a letter: the view capped the measure at
            // ~65ch and centered it. `ch` tracks the body font so the cap holds
            // as the type scales.
            article { class: "blog-post", style: "max-width: 65ch; margin-inline: auto;",
                p {
                    a { href: "/blog", "← All posts" }
                }
                h1 { "{content.title}" }
                p { class: "blog-date",
                    small { "{content.date}" }
                }
                div { dangerous_inner_html: "{content.body_html}" }
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
            let content = BlogPostContent {
                date: "June 25, 2026".to_string(),
                title: "Going all in on Rust".to_string(),
                body_html: "<p>We rewrote everything in Rust.</p>".to_string(),
            };
            rsx! {
                BlogPostPage {
                    chrome,
                    content,
                    head_title: "Neon Law | Going all in on Rust".to_string(),
                }
            }
        }
        ssr(app)
    }

    #[test]
    fn renders_the_post_title_date_and_body() {
        let out = html();
        assert!(out.contains("Going all in on Rust"), "post title: {out}");
        assert!(out.contains("June 25, 2026"), "publish date");
        assert!(
            out.contains("We rewrote everything in Rust."),
            "rendered body html"
        );
    }

    #[test]
    fn links_back_to_the_blog_index() {
        let out = html();
        assert!(out.contains(r#"href="/blog""#), "back-to-index link: {out}");
        assert!(out.contains("← All posts"), "back-link label");
    }

    #[test]
    fn wraps_the_post_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
        assert!(out.contains(r#"class="blog-post""#), "post article class");
    }
}
