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
//!    reference to `/docs/foo` / `/docs/foo#bar`, leaving external URLs,
//!    bare `#anchors`, and `../`-relative links untouched (those still
//!    resolve as repo-relative links on GitHub).
//! 2. Every heading gets a GitHub-style slug `id`, so the in-page
//!    `#anchor` links the rewriter produces actually land.

use include_dir::{include_dir, Dir};
use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};

use super::{Doc, DocsIndex};

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

/// Map a markdown link destination to a site route. Only a
/// same-directory markdown reference is rewritten:
///
/// - `notation.md`        → `/docs/notation`
/// - `glossary.md#asset`  → `/docs/glossary#asset`
///
/// Everything else is returned verbatim so it keeps working as a
/// repo-relative link on GitHub: external URLs (`https://…`,
/// `mailto:…`), bare in-page anchors (`#council`), and any link with a
/// path component (`../store/foo.rs`,
/// `../server/content/marketing/mission.md`, `../server/content/marketing/home.md`).
#[must_use]
pub fn rewrite_link(dest: &str) -> String {
    let (path, anchor) = match dest.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (dest, None),
    };
    // Bare `#anchor` (same-page) or a link carrying any path component
    // is left alone — only a sibling `name.md` in `docs/` maps to a
    // `/docs/name` route.
    if path.is_empty() || path.contains('/') {
        return dest.to_string();
    }
    let Some(stem) = path.strip_suffix(".md") else {
        return dest.to_string();
    };
    if stem.is_empty() {
        return dest.to_string();
    }
    // URLs are kebab-case; the file stem keeps its underscores. The
    // `#anchor` is a heading slug (which may legitimately hold
    // underscores), so it is passed through untouched.
    let stem = views::slug::to_url(stem);
    match anchor {
        Some(a) => format!("/docs/{stem}#{a}"),
        None => format!("/docs/{stem}"),
    }
}

/// GitHub-style heading slug: lowercase, drop punctuation, spaces → `-`,
/// keep existing hyphens and underscores. Matches the anchors our docs
/// already link to (`Engagement / Retainer` → `engagement--retainer`).
#[must_use]
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        } else if c == '-' || c == '_' {
            out.push(c);
        }
    }
    out
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
                    id: Some(slugify(&text).into()),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            // Repoint markdown-relative links/images at site routes.
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => out_events.push(Event::Start(Tag::Link {
                link_type: *link_type,
                dest_url: rewrite_link(dest_url).into(),
                title: title.clone(),
                id: id.clone(),
            })),
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
    use super::{bundled, is_published, rewrite_link, slugify, title_from_markdown};
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
    fn rewrite_link_leaves_everything_else_untouched() {
        // Non-`.md` repo-relative source links.
        assert_eq!(rewrite_link("../store/foo.rs"), "../store/foo.rs");
        // External URLs.
        assert_eq!(rewrite_link("https://example.com"), "https://example.com");
        assert_eq!(
            rewrite_link("mailto:support@neonlaw.com"),
            "mailto:support@neonlaw.com"
        );
        // Bare in-page anchor.
        assert_eq!(rewrite_link("#council"), "#council");
        // `.md` links that escape the docs dir stay repo-relative.
        assert_eq!(
            rewrite_link("../server/content/marketing/mission.md"),
            "../server/content/marketing/mission.md"
        );
        assert_eq!(rewrite_link("../README.md"), "../README.md");
        assert_eq!(
            rewrite_link("../server/content/marketing/home.md"),
            "../server/content/marketing/home.md"
        );
    }

    #[test]
    fn slugify_matches_github_anchor_rules() {
        assert_eq!(slugify("Council"), "council");
        assert_eq!(slugify("Workflow Runtime"), "workflow-runtime");
        // Punctuation drops, the surrounding spaces each become a hyphen
        // — the double hyphen our notation doc links to.
        assert_eq!(slugify("Engagement / Retainer"), "engagement--retainer");
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
            "notation",
            "notation-authoring",
            "oss-install",
            "public-contributor-safety",
            "retainer-intake",
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
        // Cross-doc link rewritten to a site route.
        assert!(
            glossary.body_html.contains("href=\"/docs/notation\""),
            "glossary's notation.md link should point at /docs/notation"
        );
        // A `../`-relative link is left repo-relative. Deliberately a source
        // path rather than a content one: the previous example pointed at the
        // mission letter, which was deleted with that surface, and
        // an assertion anchored to prose outlives the prose.
        assert!(
            glossary
                .body_html
                .contains("href=\"../store/src/schema/navigator.surql\""),
            "../store/src/schema/navigator.surql should stay repo-relative"
        );

        let notation = ix.find("notation").expect("notation published");
        // notation links glossary.md#asset → /docs/glossary#asset.
        assert!(
            notation.body_html.contains("href=\"/docs/glossary#asset\""),
            "notation's glossary anchor link should be rewritten"
        );
    }
}
