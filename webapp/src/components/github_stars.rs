//! The source-repository link and its star count, as a Dioxus component.
//!
//! Navigator is open source, and the footer says so by naming the repository,
//! linking it, and printing how many people have starred it.
//!
//! Prop-driven like every component beside it: the count arrives as a plain
//! `Option<u64>` and this renders it. Fetching is somebody else's job —
//! `crate::source_repository` keeps a process-wide cache warm from a background
//! task, and `crate::public_chrome` reads that cache into the page's chrome once
//! per request. Nothing here touches the network, which is what lets the same
//! component render a server-only marketing page and the `/design` gallery.
//!
//! **The count is optional and its absence is normal.** The cache is empty
//! until the first fetch lands, stays empty when GitHub is unreachable, and is
//! empty in every test. So `stars: None` renders the link by itself rather than
//! a zero, a dash, or a spinner — the repository is the part worth publishing,
//! and a number we do not have is not worth inventing a placeholder for.

use dioxus::prelude::*;

use crate::components::{ExternalLink, Icon, IconName};

/// Group `count` into thousands, so a five-figure star count reads at a glance
/// instead of being counted digit by digit.
///
/// Comma-grouped, because the site publishes in English only and every other
/// number on it is written the same way.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The repository link, with its star count beside it when one is known.
///
/// - `href`: the repository's web address.
/// - `repo`: how the repository is named to a reader — `owner/name`, which is
///   how GitHub addresses it and how a developer expects to see it written.
/// - `stars`: the star count, or `None` when it is not known. `None` renders
///   the link alone; see the module docs for why that is the normal case
///   rather than an error one.
///
/// The star glyph carries the accessible name, so the count is announced as
/// "GitHub stars, 1,234" rather than as a bare number floating after a link.
/// The GitHub glyph inside the anchor is decorative — the repository name
/// beside it is already the link's accessible text.
#[component]
pub fn GitHubStars(href: String, repo: String, #[props(default)] stars: Option<u64>) -> Element {
    rsx! {
        span { class: "github-stars",
            ExternalLink {
                href: href,
                class: "github-stars__repo".to_string(),
                Icon { name: IconName::Github }
                " {repo}"
            }
            if let Some(count) = stars {
                span { class: "github-stars__count",
                    Icon { name: IconName::StarFill, label: "GitHub stars".to_string() }
                    " {grouped(count)}"
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

    fn starred() -> String {
        fn app() -> Element {
            rsx! {
                GitHubStars {
                    href: "https://github.com/neon-law-source-code/navigator".to_string(),
                    repo: "neon-law-source-code/navigator".to_string(),
                    stars: 1234u64,
                }
            }
        }
        ssr(app)
    }

    /// The repository is named, linked, and left off-site the way every other
    /// outbound link on the page is.
    #[test]
    fn links_the_repository_off_site_with_the_owasp_rel_pair() {
        let out = starred();
        assert!(
            out.contains(r#"href="https://github.com/neon-law-source-code/navigator""#),
            "the repository is linked: {out}"
        );
        assert!(
            out.contains("neon-law-source-code/navigator"),
            "and named, so a reader sees the repository rather than a bare URL: {out}"
        );
        assert!(
            out.contains(r#"target="_blank""#) && out.contains(r#"rel="noopener noreferrer""#),
            "an off-site link is hardened like every other: {out}"
        );
    }

    /// The count renders beside the link, grouped, and announced by name.
    ///
    /// The accessible name is the point of the assertion: a bare "1,234" after
    /// a repository link tells a screen-reader user nothing about what was
    /// counted, so the star glyph carries the label rather than being hidden
    /// like the decorative one inside the anchor.
    #[test]
    fn publishes_the_star_count_under_an_accessible_name() {
        let out = starred();
        assert!(out.contains("1,234"), "the count is grouped: {out}");
        assert!(
            out.contains("<title>GitHub stars</title>"),
            "the count is announced as a star count: {out}"
        );
    }

    /// An unknown count renders the link by itself.
    ///
    /// This is the state every test and every process runs in before the first
    /// fetch lands, so it is the shape that must not degrade: no zero, no
    /// placeholder, and no empty element reserving space for a number that is
    /// not coming.
    #[test]
    fn renders_the_link_alone_when_the_count_is_unknown() {
        fn app() -> Element {
            rsx! {
                GitHubStars {
                    href: "https://github.com/neon-law-source-code/navigator".to_string(),
                    repo: "neon-law-source-code/navigator".to_string(),
                }
            }
        }
        let out = ssr(app);
        assert!(
            out.contains("neon-law-source-code/navigator"),
            "the repository is still published: {out}"
        );
        assert!(
            !out.contains("github-stars__count"),
            "no element is rendered for a count we do not have: {out}"
        );
        // No placeholder stands in for the missing number either — the star
        // glyph is the count's own label, so its absence is the assertion that
        // nothing was rendered in the number's place.
        assert!(
            !out.contains("GitHub stars"),
            "and no labelled-but-empty count: {out}"
        );
    }

    /// Grouping holds at every width, including the boundaries where a naive
    /// "insert a comma every three digits" puts one in front of the number.
    #[test]
    fn groups_thousands_at_every_width() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234), "1,234");
        assert_eq!(grouped(12_345), "12,345");
        assert_eq!(grouped(123_456), "123,456");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
