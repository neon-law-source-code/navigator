#![allow(clippy::doc_markdown)]
//! Open Graph + Twitter Card meta tags, as a Dioxus component (issue #641,
//! Phase 2). The product names in these docs (iMessage, LinkedIn, …) are not
//! code, so the module opts out of the backtick lint.
//!
//! The successor to the `views::components::social`. The social-share
//! "preview card" partial: when a link to the site is pasted into iMessage,
//! Slack, X, LinkedIn, or Discord, those clients read these `<meta>` tags to
//! render a rich preview (the brand logo, the page title, a one-line message).
//! The tags live in `<head>`; [`document::Meta`] hoists them there during SSR.
//!
//! `image` is the **absolute** logo URL — scrapers drop relative `og:image`
//! URLs, so the caller resolves it against the site origin server-side (the same
//! `absolute_url` boundary the version used) and passes the result in.

use dioxus::prelude::*;

/// Render the Open Graph + Twitter Card `<meta>` block for one page.
#[component]
pub fn SocialMeta(title: String, description: String, site_name: String, image: String) -> Element {
    let image_alt = format!("{site_name} logo");
    rsx! {
        // Open Graph — Facebook, iMessage, Slack, LinkedIn, Discord.
        document::Meta { property: "og:type", content: "website" }
        document::Meta { property: "og:site_name", content: "{site_name}" }
        document::Meta { property: "og:title", content: "{title}" }
        document::Meta { property: "og:description", content: "{description}" }
        if !image.is_empty() {
            document::Meta { property: "og:image", content: "{image}" }
            document::Meta { property: "og:image:alt", content: "{image_alt}" }
        }
        // Twitter / X. `summary` renders the square logo as a small thumbnail;
        // the wide card expects a 1.91:1 banner, which a square mark would
        // letterbox.
        document::Meta { name: "twitter:card", content: "summary" }
        document::Meta { name: "twitter:title", content: "{title}" }
        document::Meta { name: "twitter:description", content: "{description}" }
        if !image.is_empty() {
            document::Meta { name: "twitter:image", content: "{image}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_wordmark_does_not_emit_social_image_metadata() {
        fn app() -> Element {
            rsx! {
                SocialMeta {
                    title: "Lawyer Shook".to_string(),
                    description: "A Shook Law PLLC practice.".to_string(),
                    site_name: "Lawyer Shook".to_string(),
                    image: String::new(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);
        assert!(
            !out.contains("og:image"),
            "no empty Open Graph image: {out}"
        );
        assert!(
            !out.contains("twitter:image"),
            "no empty Twitter image: {out}"
        );
    }
}
