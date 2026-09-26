//! Render the glossary — `docs/glossary/`, one Markdown file per term —
//! to the HTML the one-page `/glossary` shows.
//!
//! The terms come from [`store::glossary::terms`], embedded at compile
//! time, so the page, the CLI, and the `glossary_term` rows read one
//! source. Rendering runs once, on first use, and the result is shared.
//!
//! Two transforms run at render time over the pulldown-cmark event
//! stream:
//!
//! 1. [`rewrite_link`] resolves each destination from the entries'
//!    directory: a sibling term (`matter.md`) becomes the in-page anchor
//!    `#matter`, and any other repository path (`../../store/`,
//!    `../notation.md`) becomes the matching GitHub blob or tree URL — a
//!    browser at `/glossary` would otherwise resolve it against the site
//!    origin and 404.
//! 2. Off-site anchors open in a new tab with the same up-right arrow the
//!    rest of the site uses, and a `.md` link label loses its extension.

use std::sync::LazyLock;

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};

use store::glossary::{link_target, LinkTarget};
use webapp::glossary_page::{GlossaryContent, GlossaryEntry};

/// Navigator's own repository — the target for a `../` source link the
/// docs renderer cannot serve as a site route.
const REPO: &str = cloud::workspace::NAVIGATOR_REPOSITORY_URL;

/// The box-arrow-up-right glyph [`webapp::components::IconName::BoxArrowUpRight`]
/// draws. Inlined here because docs bodies are baked HTML, not Dioxus
/// components, and the reader still needs the same off-site cue.
const OFFSITE_ARROW: &str = concat!(
    "<svg class=\"nav-icon\" xmlns=\"http://www.w3.org/2000/svg\" ",
    "viewBox=\"0 0 16 16\" width=\"1em\" height=\"1em\" fill=\"currentColor\" ",
    "role=\"img\" aria-hidden=\"true\">",
    "<path fill-rule=\"evenodd\" d=\"M8.636 3.5a.5.5 0 0 0-.5-.5H1.5A1.5 1.5 0 0 0 0 4.5v10A1.5 1.5 0 0 0 1.5 16h10a1.5 1.5 0 0 0 1.5-1.5V7.864a.5.5 0 0 0-1 0V14.5a.5.5 0 0 1-.5.5h-10a.5.5 0 0 1-.5-.5v-10a.5.5 0 0 1 .5-.5h6.636a.5.5 0 0 0 .5-.5\"/>",
    "<path fill-rule=\"evenodd\" d=\"M16 .5a.5.5 0 0 0-.5-.5h-5a.5.5 0 0 0 0 1h3.793L6.146 9.146a.5.5 0 1 0 .708.708L15 1.707V5.5a.5.5 0 0 0 1 0z\"/>",
    "</svg>",
);

/// The rendered glossary, built once from the embedded terms.
#[must_use]
pub fn content() -> &'static GlossaryContent {
    static CONTENT: LazyLock<GlossaryContent> = LazyLock::new(|| GlossaryContent {
        preamble_html: render_markdown(store::glossary::preamble()),
        entries: store::glossary::terms()
            .iter()
            .map(|term| GlossaryEntry {
                slug: term.slug.clone(),
                title: term.title.clone(),
                description: term.description.clone(),
                body_html: render_markdown(&term.body),
            })
            .collect(),
    });
    &CONTENT
}

/// Map a link destination in an entry to an in-page anchor or a GitHub
/// source URL.
///
/// - `matter.md`              → `#matter`
/// - `matter.md#x`            → `#matter` (a term is one section)
/// - `../../store/foo.rs`     → GitHub blob at `store/foo.rs`
/// - `../../store/`           → GitHub tree at `store`
/// - `../notation.md#template` → GitHub blob at `docs/notation.md#template`
///
/// Absolute URLs (`https://…`, `mailto:`), site paths (`/glossary`), and
/// bare in-page anchors (`#council`) pass through, as does a `../` that
/// would climb out of the repository.
#[must_use]
pub fn rewrite_link(dest: &str) -> String {
    match link_target(dest) {
        LinkTarget::Term(slug) => format!("#{slug}"),
        LinkTarget::Repository { path, anchor } => {
            let url = github_source_url(&path);
            anchor.map_or_else(|| url.clone(), |a| format!("{url}#{a}"))
        }
        LinkTarget::Verbatim => dest.to_string(),
    }
}

/// A trailing slash names a directory (`../store/` → tree); anything else
/// is a file (`../LICENSE` → blob), including extensionless files.
fn github_source_url(repo_path: &str) -> String {
    let kind = if repo_path.ends_with('/') {
        "tree"
    } else {
        "blob"
    };
    format!(
        "{REPO}/{kind}/main/{path}",
        path = repo_path.trim_end_matches('/')
    )
}

/// True when `href` leaves neonlaw.com. Relative routes, `mailto:`, and
/// the firm's own hosts stay on-site and do not get the off-site cue.
#[must_use]
pub fn is_off_site(href: &str) -> bool {
    let Some(rest) = href
        .strip_prefix("https://")
        .or_else(|| href.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host != "neonlaw.com" && !host.ends_with(".neonlaw.com")
}

/// Drop a `.md` file-extension from a link label so the published page
/// does not show the source filename.
fn strip_md_label(text: &str) -> String {
    if let Some(stem) = text.strip_suffix(".md") {
        return stem.to_string();
    }
    if let Some(stem) = text.strip_suffix(".md`") {
        return format!("{stem}`");
    }
    text.to_string()
}

fn escape_attribute(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn off_site_open_tag(href: &str, title: &str) -> String {
    let href = escape_attribute(href);
    if title.is_empty() {
        format!("<a href=\"{href}\" target=\"_blank\" rel=\"noopener noreferrer\">")
    } else {
        format!(
            "<a href=\"{href}\" title=\"{title}\" target=\"_blank\" rel=\"noopener noreferrer\">",
            title = escape_attribute(title)
        )
    }
}

/// Render one entry's Markdown to HTML, rewriting its links and stamping a
/// slug `id` on any heading so an in-page anchor to it resolves.
#[must_use]
fn render_markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let events: Vec<Event> = Parser::new_ext(src, opts).collect();
    let mut out_events: Vec<Event> = Vec::with_capacity(events.len());
    let mut in_markdown_link = false;
    let mut in_off_site_link = false;

    for i in 0..events.len() {
        match &events[i] {
            // Stamp a slug id on headings that don't already declare one.
            Event::Start(Tag::Heading {
                level,
                id: None,
                classes,
                attrs,
            }) => {
                let text = heading_text(&events[i + 1..]);
                out_events.push(Event::Start(Tag::Heading {
                    level: *level,
                    id: Some(store::glossary::slugify(&text).into()),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let href = rewrite_link(dest_url);
                in_markdown_link = markdown_file_dest(dest_url);
                if is_off_site(&href) {
                    in_off_site_link = true;
                    out_events.push(Event::InlineHtml(CowStr::from(off_site_open_tag(
                        &href, title,
                    ))));
                } else {
                    in_off_site_link = false;
                    out_events.push(Event::Start(Tag::Link {
                        link_type: *link_type,
                        dest_url: href.into(),
                        title: title.clone(),
                        id: id.clone(),
                    }));
                }
            }
            Event::End(TagEnd::Link) if in_off_site_link => {
                in_off_site_link = false;
                in_markdown_link = false;
                out_events.push(Event::InlineHtml(CowStr::from(format!(
                    " {OFFSITE_ARROW}</a>"
                ))));
            }
            Event::End(TagEnd::Link) => {
                in_markdown_link = false;
                out_events.push(Event::End(TagEnd::Link));
            }
            Event::Text(text) if in_markdown_link => {
                out_events.push(Event::Text(CowStr::from(strip_md_label(text))));
            }
            Event::Code(text) if in_markdown_link => {
                out_events.push(Event::Code(CowStr::from(strip_md_label(text))));
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => out_events.push(Event::Start(Tag::Image {
                link_type: *link_type,
                dest_url: rewrite_link(dest_url).into(),
                title: title.clone(),
                id: id.clone(),
            })),
            other => out_events.push(other.clone()),
        }
    }

    let mut out = String::new();
    html::push_html(&mut out, out_events.into_iter());
    views::components::code::decorate_copy_buttons(&out)
}

fn markdown_file_dest(dest: &str) -> bool {
    let path = dest.split_once('#').map_or(dest, |(path, _)| path);
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Concatenate the text of a heading from the events that follow its
/// `Start(Heading)` up to the matching `End`. `Code` spans count as
/// text so `## `code`` headings still slug sensibly.
fn heading_text(rest: &[Event]) -> String {
    let mut text = String::new();
    for ev in rest {
        match ev {
            Event::End(TagEnd::Heading(_)) => break,
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{content, github_source_url, is_off_site, rewrite_link, strip_md_label, REPO};

    #[test]
    fn rewrite_link_maps_a_sibling_term_to_its_anchor() {
        assert_eq!(rewrite_link("matter.md"), "#matter");
        assert_eq!(
            rewrite_link("engagement--retainer.md"),
            "#engagement--retainer"
        );
    }

    #[test]
    fn rewrite_link_maps_repo_relative_paths_to_github() {
        assert_eq!(
            rewrite_link("../../store/src/schema/navigator.surql"),
            format!("{REPO}/blob/main/store/src/schema/navigator.surql")
        );
        assert_eq!(
            rewrite_link("../../store/"),
            format!("{REPO}/tree/main/store")
        );
        assert_eq!(
            rewrite_link("../../README.md#trademark"),
            format!("{REPO}/blob/main/README.md#trademark")
        );
        assert_eq!(
            rewrite_link("../notation.md#template"),
            format!("{REPO}/blob/main/docs/notation.md#template")
        );
    }

    #[test]
    fn rewrite_link_leaves_absolute_and_in_page_destinations() {
        assert_eq!(rewrite_link("https://example.com"), "https://example.com");
        assert_eq!(
            rewrite_link("mailto:support@neonlaw.com"),
            "mailto:support@neonlaw.com"
        );
        assert_eq!(rewrite_link("#council"), "#council");
        assert_eq!(rewrite_link("/glossary"), "/glossary");
        // Climbing out of the repository is not a GitHub path we can name.
        assert_eq!(rewrite_link("../../../outside.rs"), "../../../outside.rs");
    }

    #[test]
    fn github_source_url_uses_tree_only_for_trailing_slash() {
        assert_eq!(
            github_source_url("store/"),
            format!("{REPO}/tree/main/store")
        );
        assert_eq!(
            github_source_url("LICENSE"),
            format!("{REPO}/blob/main/LICENSE")
        );
    }

    #[test]
    fn off_site_is_anything_outside_neonlaw_hosts() {
        assert!(is_off_site(
            "https://github.com/neon-law-source-code/navigator"
        ));
        assert!(is_off_site("https://restate.dev"));
        assert!(!is_off_site("https://www.neonlaw.com/glossary"));
        assert!(!is_off_site("https://staging.neonlaw.com/glossary"));
        assert!(!is_off_site("/glossary"));
        assert!(!is_off_site("#council"));
        assert!(!is_off_site("mailto:support@neonlaw.com"));
    }

    #[test]
    fn strip_md_label_drops_the_file_extension() {
        assert_eq!(strip_md_label("notation.md"), "notation");
        assert_eq!(strip_md_label("docs/frontmatter.md"), "docs/frontmatter");
        assert_eq!(strip_md_label("README.md`"), "README`");
        assert_eq!(strip_md_label("Notation"), "Notation");
    }

    #[test]
    fn every_term_renders_in_slug_order() {
        let slugs: Vec<&str> = content().entries.iter().map(|e| e.slug.as_str()).collect();
        let terms: Vec<&str> = store::glossary::terms()
            .iter()
            .map(|t| t.slug.as_str())
            .collect();
        assert_eq!(slugs, terms, "the page drifted from store::glossary::terms");
        assert!(!content().preamble_html.is_empty(), "the preamble renders");
    }

    #[test]
    fn links_between_terms_stay_on_the_page() {
        let workshop = content()
            .entries
            .iter()
            .find(|e| e.slug == "workshop")
            .expect("Workshop is a term");
        assert!(
            workshop.body_html.contains("href=\"#matter\""),
            "a sibling term link is an in-page anchor: {}",
            workshop.body_html
        );
        for entry in &content().entries {
            for (at, _) in entry.body_html.match_indices("href=\"") {
                let href = &entry.body_html[at + 6..];
                assert!(
                    ["#", "https://", "http://", "mailto:", "/"]
                        .iter()
                        .any(|prefix| href.starts_with(prefix)),
                    "`{}` renders a relative link that 404s on the site: {}",
                    entry.slug,
                    &href[..href.len().min(80)]
                );
            }
        }
    }

    #[test]
    fn off_site_links_carry_the_up_right_arrow() {
        let html: String = content()
            .entries
            .iter()
            .map(|e| e.body_html.as_str())
            .collect();
        let surql = format!("{REPO}/blob/main/store/src/schema/navigator.surql");
        assert!(
            html.contains(&format!(
                "href=\"{surql}\" target=\"_blank\" rel=\"noopener noreferrer\""
            )),
            "GitHub source links open off-site"
        );
        assert!(
            html.contains(
                "href=\"https://restate.dev\" target=\"_blank\" rel=\"noopener noreferrer\""
            ),
            "https://restate.dev must carry the off-site treatment"
        );
        assert!(
            html.contains("M8.636 3.5"),
            "off-site links include the box-arrow-up-right glyph"
        );
        assert!(
            !html.contains("href=\"#matter\" target=\"_blank\""),
            "an in-page anchor must not open a new tab"
        );
    }
}
