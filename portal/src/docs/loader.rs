//! Bake the workspace docs into the binary and render them to HTML.
//!
//! The workspace `docs/` directory is embedded at compile time, then
//! each top-level Markdown file opts into publishing with leading YAML
//! frontmatter: `publish: true`. The default is deliberately private:
//! adding an unflagged contributor document neither emits a route nor
//! needs a central registration.
//!
//! Two transforms run at render time over the pulldown-cmark event
//! stream:
//!
//! 1. [`rewrite_link`] maps a same-directory `foo.md` / `foo.md#bar`
//!    reference to `/docs/foo` / `/docs/foo#bar`, and a `../` repo path
//!    (from `docs/`) to the matching GitHub blob or tree URL. A browser
//!    at `/docs/glossary` would otherwise resolve `../store/…` against
//!    the site origin and 404. External URLs, `mailto:`, bare `#anchors`,
//!    and already-absolute `/docs/…` paths pass through.
//! 2. Every heading gets a GitHub-style slug `id`, so the in-page
//!    `#anchor` links the rewriter produces actually land. Off-site
//!    anchors open in a new tab with the same up-right arrow the rest
//!    of the site uses.

use include_dir::{include_dir, Dir};
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};

use super::{Doc, DocsIndex};

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

/// `CARGO_MANIFEST_DIR` is `…/portal`; the docs tree is one level up.
static DOCS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../docs");

#[derive(serde::Deserialize)]
struct PublishFlag {
    publish: Option<bool>,
}

/// Build the index of baked docs. Parsed once at boot.
#[must_use]
pub fn bundled() -> DocsIndex {
    let mut docs: Vec<Doc> = DOCS
        .files()
        .filter(|file| file.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .filter_map(|file| {
            let raw = file.contents_utf8()?;
            is_published(raw).then(|| {
                let slug = file.path().file_stem().and_then(|stem| stem.to_str())?;
                let body = markdown_body(raw);
                Some(Doc {
                    slug: views::slug::to_url(slug),
                    title: title_from_markdown(body, slug),
                    body_html: render_markdown(body),
                })
            })?
        })
        .collect();
    docs.sort_by(|a, b| a.slug.cmp(&b.slug));
    DocsIndex::new(docs)
}

/// A document is public only with an explicit `publish: true` in its
/// leading YAML frontmatter. Missing, malformed, or non-boolean flags
/// fail closed.
fn is_published(raw: &str) -> bool {
    let Some((frontmatter, _)) = split_frontmatter(raw) else {
        return false;
    };
    matches!(
        serde_yaml::from_str::<PublishFlag>(frontmatter),
        Ok(PublishFlag {
            publish: Some(true)
        })
    )
}

/// The Markdown body after a leading frontmatter block. Rendering must
/// not expose the publish control itself as a horizontal rule and YAML
/// paragraph.
fn markdown_body(raw: &str) -> &str {
    split_frontmatter(raw).map_or(raw, |(_, body)| body)
}

/// The YAML body and Markdown body from a leading frontmatter block.
///
/// Delegates to `rules::frontmatter::split` so this loader accepts the
/// same delimiters as every other reader of these files. The LF-only
/// probe it replaces treated a CRLF checkout's docs as having no
/// frontmatter at all, which silently dropped the publish control that
/// `markdown_body` exists to hide.
fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    rules::frontmatter::split(raw)
}

/// The page title is the doc's first `# ` heading. Anything else before
/// an H1 means the file leads with content, not a title — fall back to
/// the slug rather than guessing.
fn title_from_markdown(raw: &str, fallback: &str) -> String {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            return heading.trim().to_string();
        }
        if !trimmed.is_empty() {
            break;
        }
    }
    fallback.to_string()
}

/// Map a markdown link destination to a site route or a GitHub source URL.
///
/// - `notation.md`        → `/docs/notation`
/// - `glossary.md#asset`  → `/docs/glossary#asset`
/// - `../store/foo.rs`    → GitHub blob at `store/foo.rs`
/// - `../store/`          → GitHub tree at `store`
///
/// Absolute URLs (`https://…`, `mailto:`), already-absolute site paths
/// (`/docs/…`), and bare in-page anchors (`#council`) pass through.
/// A `../` that would climb out of the repository is left verbatim.
#[must_use]
pub fn rewrite_link(dest: &str) -> String {
    if dest.starts_with("http://")
        || dest.starts_with("https://")
        || dest.starts_with("mailto:")
        || dest.starts_with('/')
    {
        return dest.to_string();
    }
    let (path, anchor) = match dest.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (dest, None),
    };
    if path.is_empty() {
        return dest.to_string();
    }
    if !path.contains('/') {
        let Some(stem) = path.strip_suffix(".md").filter(|stem| !stem.is_empty()) else {
            return dest.to_string();
        };
        // URLs are kebab-case; the file stem keeps its underscores. The
        // `#anchor` is a heading slug (which may legitimately hold
        // underscores), so it is passed through untouched.
        return with_anchor(
            &format!("/docs/{stem}", stem = views::slug::to_url(stem)),
            anchor,
        );
    }
    if let Some(repo_path) = path.strip_prefix("../") {
        if repo_path.is_empty() || repo_path.starts_with("../") || repo_path.starts_with('/') {
            return dest.to_string();
        }
        return with_anchor(&github_source_url(repo_path), anchor);
    }
    dest.to_string()
}

fn with_anchor(base: &str, anchor: Option<&str>) -> String {
    match anchor {
        Some(a) => format!("{base}#{a}"),
        None => base.to_string(),
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

/// Render markdown to HTML, rewriting `.md` links to `/docs/*` routes
/// and stamping a slug `id` on every heading so in-page anchors resolve.
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
    out
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
    use super::{
        bundled, github_source_url, is_off_site, is_published, rewrite_link, strip_md_label,
        title_from_markdown, REPO,
    };
    use std::collections::BTreeSet;

    #[test]
    fn rewrite_link_maps_sibling_md_to_route() {
        assert_eq!(rewrite_link("notation.md#x"), "/docs/notation#x");
        assert_eq!(rewrite_link("glossary.md"), "/docs/glossary");
        assert_eq!(rewrite_link("access-model.md"), "/docs/access-model");
        // An underscore filename is rewritten to its kebab-case URL,
        // while a heading anchor (which may carry underscores) is left as
        // authored.
        assert_eq!(rewrite_link("retainer_intake.md"), "/docs/retainer-intake");
        assert_eq!(
            rewrite_link("retainer_intake.md#step_one"),
            "/docs/retainer-intake#step_one"
        );
    }

    #[test]
    fn rewrite_link_maps_repo_relative_paths_to_github() {
        assert_eq!(
            rewrite_link("../store/src/schema/navigator.surql"),
            format!("{REPO}/blob/main/store/src/schema/navigator.surql")
        );
        assert_eq!(rewrite_link("../store/"), format!("{REPO}/tree/main/store"));
        assert_eq!(
            rewrite_link("../LICENSE"),
            format!("{REPO}/blob/main/LICENSE")
        );
        assert_eq!(
            rewrite_link("../README.md#trademarks"),
            format!("{REPO}/blob/main/README.md#trademarks")
        );
        assert_eq!(
            rewrite_link("../server/content/marketing/mission.md"),
            format!("{REPO}/blob/main/server/content/marketing/mission.md")
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
        assert_eq!(rewrite_link("/docs/glossary"), "/docs/glossary");
        // Climbing out of the repository is not a GitHub path we can name.
        assert_eq!(rewrite_link("../../outside.rs"), "../../outside.rs");
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
        assert!(!is_off_site("https://www.neonlaw.com/docs/glossary"));
        assert!(!is_off_site("https://staging.neonlaw.com/docs"));
        assert!(!is_off_site("/docs/notation"));
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
    fn heading_slug_matches_the_store_glossary_shape() {
        assert_eq!(store::glossary::slugify("Council"), "council");
        assert_eq!(
            store::glossary::slugify("Workflow Runtime"),
            "workflow-runtime"
        );
        assert_eq!(
            store::glossary::slugify("Engagement / Retainer"),
            "engagement--retainer"
        );
    }

    #[test]
    fn title_comes_from_leading_h1() {
        assert_eq!(title_from_markdown("# Glossary\n\nbody", "x"), "Glossary");
        assert_eq!(
            title_from_markdown("\n\n# Notation vocabulary\n", "x"),
            "Notation vocabulary"
        );
        // Content before any H1 → fall back to the slug.
        assert_eq!(
            title_from_markdown("lead paragraph\n# Late", "fallback"),
            "fallback"
        );
    }

    #[test]
    fn publish_flag_fails_closed_and_is_removed_from_rendered_markdown() {
        assert!(is_published("---\npublish: true\n---\n\n# Public\n"));
        assert!(!is_published("# Contributor notes\n"));
        assert!(!is_published("---\npublish: false\n---\n\n# Private\n"));
        assert!(!is_published("---\npublish: yes\n---\n\n# Private\n"));

        let index = bundled();
        let glossary = index.find("glossary").expect("glossary published");
        assert_eq!(glossary.title, "Glossary");
        assert!(
            !glossary.body_html.contains("publish: true"),
            "frontmatter is a build control, not page content"
        );
    }

    #[test]
    fn only_the_documented_opt_in_set_publishes() {
        let index = bundled();
        let actual: BTreeSet<&str> = index.docs().iter().map(|doc| doc.slug.as_str()).collect();
        let expected = BTreeSet::from([
            "aida-a2a-interaction",
            "bulk-contact-import",
            "command-boundary",
            "durable-workflows",
            "editing-workflows",
            "frontmatter",
            "glossary",
            "gov-forms",
            // The hub `/docs` itself resolves. It is the documentation front
            // door and the footer links it, so leaving it opt-out made the one
            // page every reader lands on first a 404.
            "index",
            "marketing-copy",
            "notation",
            "notation-authoring",
            "oss-install",
            "public-contributor-safety",
            "retainer-intake",
            "validate",
        ]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn sensitive_infrastructure_docs_are_not_published() {
        let index = bundled();
        for slug in [
            "deployment-secrets",
            "gke-prod",
            "dns",
            "cloud-operations",
            "multi-cloud",
            "rego-policy",
        ] {
            assert!(
                index.find(slug).is_none(),
                "sensitive infrastructure doc {slug} must stay unpublished"
            );
        }
    }

    #[test]
    fn doc_route_slugs_are_unique_after_kebab() {
        // `_`→`-` is lossy, so two manifest stems differing only by `_`
        // vs `-` would publish at one `/docs` URL and `DocsIndex::find`
        // would silently return the first. Fail the build if that ever
        // happens instead of shadowing a doc in production.
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for doc in bundled().docs() {
            assert!(
                seen.insert(doc.slug.clone()),
                "two docs map to the kebab route slug `{}` — rename one",
                doc.slug
            );
        }
    }

    #[test]
    fn underscore_doc_publishes_at_its_kebab_slug() {
        // `docs/retainer_intake.md` is the only underscore doc; its route
        // slug is the kebab-case form, even though the manifest key (and
        // the file on disk) keep the underscore.
        let ix = bundled();
        assert!(
            ix.find("retainer-intake").is_some(),
            "retainer_intake.md should publish at /docs/retainer-intake"
        );
        assert!(
            ix.find("retainer_intake").is_none(),
            "the underscore slug is not a valid route — the handler redirects it"
        );
    }

    #[test]
    fn bundled_renders_glossary_and_notation_with_anchors() {
        let ix = bundled();
        let glossary = ix.find("glossary").expect("glossary published");
        // Heading rendered as <h2> with a slug id so `#council` lands.
        assert!(
            glossary
                .body_html
                .contains("<h2 id=\"council\">Council</h2>"),
            "missing slugged Council heading"
        );
        // Cross-doc link rewritten to a site route without the `.md` suffix.
        assert!(
            glossary.body_html.contains("href=\"/docs/notation\""),
            "glossary's notation.md link should point at /docs/notation"
        );
        assert!(
            !glossary.body_html.contains("href=\"/docs/notation.md"),
            "published docs routes must not keep the .md extension: {}",
            glossary.body_html
        );
        // A `../` source link becomes a GitHub blob so a browser at
        // `/docs/glossary` does not resolve it as `/store/…` on neonlaw.com.
        let surql = format!("{REPO}/blob/main/store/src/schema/navigator.surql");
        assert!(
            glossary.body_html.contains(&format!("href=\"{surql}\"")),
            "../store/src/schema/navigator.surql should point at GitHub: {}",
            glossary.body_html
        );
        assert!(
            !glossary.body_html.contains("href=\"../"),
            "no leftover repo-relative href should survive rendering: {}",
            glossary.body_html
        );

        let notation = ix.find("notation").expect("notation published");
        // notation links glossary.md#asset → /docs/glossary#asset.
        assert!(
            notation.body_html.contains("href=\"/docs/glossary#asset\""),
            "notation's glossary anchor link should be rewritten"
        );
    }

    #[test]
    fn glossary_off_site_links_carry_the_up_right_arrow() {
        let ix = bundled();
        let glossary = ix.find("glossary").expect("glossary published");
        let html = &glossary.body_html;
        let surql = format!("{REPO}/blob/main/store/src/schema/navigator.surql");
        assert!(
            html.contains(&format!(
                "href=\"{surql}\" target=\"_blank\" rel=\"noopener noreferrer\""
            )),
            "GitHub source links open off-site: {html}"
        );
        assert!(
            html.contains(
                "href=\"https://restate.dev\" target=\"_blank\" rel=\"noopener noreferrer\""
            ),
            "https://restate.dev must carry the off-site treatment: {html}"
        );
        assert!(
            html.contains("M8.636 3.5"),
            "off-site links include the box-arrow-up-right glyph: {html}"
        );
        // On-site docs routes stay in this tab and do not get the glyph on
        // the anchor itself — the glyph is only emitted for off-site tags.
        assert!(
            html.contains("href=\"/docs/notation\""),
            "internal docs routes remain ordinary anchors"
        );
        assert!(
            !html.contains("href=\"/docs/notation\" target=\"_blank\""),
            "an on-site docs route must not open a new tab: {html}"
        );
    }

    #[test]
    fn glossary_headings_match_the_store_parser() {
        let terms = store::glossary::parse(store::glossary::GLOSSARY_MD);
        let ix = bundled();
        let glossary = ix.find("glossary").expect("glossary published");
        let mut html_ids = Vec::new();
        let mut rest = glossary.body_html.as_str();
        while let Some(at) = rest.find("<h2 id=\"") {
            rest = &rest[at + 8..];
            let Some(end) = rest.find('"') else {
                break;
            };
            html_ids.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        let slugs: Vec<String> = terms.iter().map(|term| term.slug.clone()).collect();
        assert_eq!(
            html_ids, slugs,
            "published /docs/glossary headings drifted from store::glossary::parse"
        );
    }
}
