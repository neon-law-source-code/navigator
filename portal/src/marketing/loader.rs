//! Read one `.md` content file into a [`MarketingDoc`].
//!
//! Front-matter with `title`, `slug`, `description`; everything
//! after the closing `---` is rendered through pulldown-cmark at
//! load time so handlers can ship the HTML verbatim.

use pulldown_cmark::{html, Event, Options, Parser, Tag};
// Shared with the workshop/presentation loader so a blog post and a slide
// resolve an author-written picture identically.
use views::assets::rewrite_image_src;

use super::MarketingDoc;

/// Parse a single doc. `fallback_slug` is used when no `slug:`
/// is set in front-matter (typical case — file stem matches).
#[must_use]
pub fn parse(raw: &str, fallback_slug: &str) -> Option<MarketingDoc> {
    // Delimiter handling lives in `rules::frontmatter` rather than here,
    // so this loader cannot drift from the other readers of the same
    // files. The local probe it replaces was LF-only, and every `.md`
    // under `server/content` opens `---\r\n` on a checkout with
    // `core.autocrlf=true` — so `parse` returned `None` for all of them
    // and `web` died at boot on `loading marketing content`.
    let (frontmatter, body) = rules::frontmatter::split(raw)?;
    let body = body.trim_start_matches(['\n', '\r']);
    let fields = parse_frontmatter(frontmatter);

    let title = fields
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Untitled".into());
    let slug = fields
        .get("slug")
        .cloned()
        .unwrap_or_else(|| fallback_slug.to_string());
    let description = fields.get("description").cloned().unwrap_or_default();
    let body_html = render_markdown(body);
    let metadata = fields
        .into_iter()
        .filter(|(k, _)| !matches!(k.as_str(), "title" | "slug" | "description"))
        .collect();

    Some(MarketingDoc {
        slug,
        title,
        description,
        body_html,
        metadata,
    })
}

fn parse_frontmatter(source: &str) -> std::collections::HashMap<String, String> {
    if let Ok(serde_yaml::Value::Mapping(mapping)) = serde_yaml::from_str(source) {
        return mapping
            .into_iter()
            .filter_map(|(key, value)| {
                let serde_yaml::Value::String(key) = key else {
                    return None;
                };
                let value = match value {
                    serde_yaml::Value::String(s) => s.trim().to_string(),
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    _ => return None,
                };
                Some((key, value))
            })
            .collect();
    }

    let mut out = std::collections::HashMap::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Indented lines belong to the previous top-level key: either a
        // nested structure (`pricing:`) or a folded scalar handled below.
        if line.starts_with([' ', '\t']) {
            i += 1;
            continue;
        }
        let trimmed = line.trim();
        let Some(colon) = trimmed.find(':') else {
            i += 1;
            continue;
        };
        let key = trimmed[..colon].trim().to_string();
        let value = unwrap_quotes(trimmed[colon + 1..].trim());
        if matches!(value.as_str(), ">" | "|") {
            let folded = value == ">";
            let mut parts = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].starts_with([' ', '\t']) {
                parts.push(lines[i].trim());
                i += 1;
            }
            let separator = if folded { " " } else { "\n" };
            out.insert(key, parts.join(separator).trim().to_string());
            continue;
        }
        out.insert(key, value);
        i += 1;
    }
    out
}

fn unwrap_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2 {
        let first = chars[0];
        let last = chars[chars.len() - 1];
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return chars[1..chars.len() - 1].iter().collect();
        }
    }
    s.to_string()
}

fn render_markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    // Route every image `src` through the asset seam so content authors
    // write a repo-relative path (`img/thanks-apple/foo.jpg`) that
    // resolves to the `/public` mount in dev and the photo CDN bucket
    // (`NAVIGATOR_ASSET_BASE_URL`) in production. Image bytes live in
    // GCS, never in the repo (`/server/public/img/` is gitignored).
    let parser = Parser::new_ext(src, opts).map(|event| match event {
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: rewrite_image_src(&dest_url).into(),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

#[cfg(test)]
mod tests {
    use super::{parse, rewrite_image_src};
    use std::fs;

    const LF_DOC: &str = "---\ntitle: Mission\nslug: mission\ndescription: Why we exist\nweight: 2\n---\n# Heading\n\nA paragraph.\n";

    fn crlf(s: &str) -> String {
        s.replace('\n', "\r\n")
    }

    #[test]
    fn parse_reads_the_same_values_from_lf_and_crlf() {
        // Values, not an error count. `parse` returning `None` on CRLF is
        // what stopped `web` booting, and a count-based assertion would
        // have passed on Linux throughout.
        let lf = parse(LF_DOC, "fallback").expect("lf parses");
        let crlf_doc = crlf(LF_DOC);
        let from_crlf = parse(&crlf_doc, "fallback").expect("crlf parses");

        assert_eq!(from_crlf.title, lf.title);
        assert_eq!(from_crlf.slug, lf.slug);
        assert_eq!(from_crlf.description, lf.description);
        assert_eq!(from_crlf.metadata, lf.metadata);
        assert_eq!(from_crlf.body_html, lf.body_html);

        // Spot-check the absolute values so the equality above cannot be
        // satisfied by both sides being empty.
        assert_eq!(from_crlf.title, "Mission");
        assert_eq!(from_crlf.slug, "mission");
        assert_eq!(from_crlf.description, "Why we exist");
        assert_eq!(
            from_crlf.metadata.get("weight").map(String::as_str),
            Some("2")
        );
        assert!(
            from_crlf.body_html.contains("<h1>Heading</h1>"),
            "body rendered from a CRLF document: {}",
            from_crlf.body_html
        );
        // The closing delimiter must not survive into the rendered body.
        assert!(!from_crlf.body_html.contains("---"));
        assert!(!from_crlf.body_html.contains("title: Mission"));
    }

    #[test]
    fn parse_handles_a_crlf_closer_at_eof() {
        let doc = "---\r\ntitle: T\r\nslug: s\r\n---";
        let parsed = parse(doc, "fallback").expect("closer at eof parses");
        assert_eq!(parsed.title, "T");
        assert_eq!(parsed.slug, "s");
        assert_eq!(parsed.body_html.trim(), "");
    }

    #[test]
    fn every_tracked_content_file_parses_on_this_checkout() {
        // Covers the strictness change: `parse` used to probe `find("\n---")`,
        // which also matched a non-delimiter line like `----`. Routing
        // through `rules::frontmatter::split` requires a real delimiter, so
        // assert the shipped files still parse and still carry a title.
        //
        // This reads the real tree, so it fails on whichever platform is
        // actually broken rather than on a synthetic fixture.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/content");
        let mut seen = 0;
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|x| x.to_str()) == Some("md") {
                    let raw = fs::read_to_string(&path).expect("read content file");
                    if !raw.starts_with("---") {
                        continue; // not a frontmatter-bearing fragment
                    }
                    let parsed = parse(&raw, "fallback")
                        .unwrap_or_else(|| panic!("{}: frontmatter not found", path.display()));
                    assert!(!parsed.title.is_empty(), "{}: empty title", path.display());
                    seen += 1;
                }
            }
        }
        // A floor, not a count. It dropped from nine when the nonprofit's
        // mission letter and its three governance documents were retired with
        // the rest of that surface; what it still catches is a walk that
        // silently stopped finding files.
        assert!(
            seen >= 6,
            "expected the tracked content fragments to be covered, saw {seen}"
        );
    }

    #[test]
    fn relative_image_src_routes_through_the_asset_seam() {
        // A repo-relative markdown image resolves against the asset base
        // (default `/public` in tests, the photo CDN bucket in prod), so
        // image bytes can live in GCS instead of the repo.
        assert_eq!(
            rewrite_image_src("img/thanks-apple/team-lunch.jpg"),
            "/public/img/thanks-apple/team-lunch.jpg"
        );
        // Already-absolute sources are left untouched.
        assert_eq!(
            rewrite_image_src("https://example.com/x.jpg"),
            "https://example.com/x.jpg"
        );
        assert_eq!(rewrite_image_src("/public/img/x.jpg"), "/public/img/x.jpg");
        assert_eq!(
            rewrite_image_src("data:image/png;base64,AA"),
            "data:image/png;base64,AA"
        );
    }

    #[test]
    fn markdown_image_renders_with_resolved_asset_src() {
        let raw = "---\n\
                   title: \"Post\"\n\
                   description: \"d\"\n\
                   ---\n\n\
                   ![a teammate](img/thanks-apple/team-lunch.jpg)";
        let doc = parse(raw, "post").expect("parses");
        assert!(
            doc.body_html
                .contains("src=\"/public/img/thanks-apple/team-lunch.jpg\""),
            "image src must route through the asset seam, got: {}",
            doc.body_html
        );
    }

    #[test]
    fn parse_extracts_fields_and_renders_body() {
        let raw = "---\n\
                   title: \"Flat-fee legal services\"\n\
                   slug: home\n\
                   description: \"Estate and corporate, no litigation.\"\n\
                   ---\n\n\
                   ## Lead\n\nFlat-fee.";
        let doc = parse(raw, "fallback").expect("parses");
        assert_eq!(doc.title, "Flat-fee legal services");
        assert_eq!(doc.slug, "home");
        assert_eq!(doc.description, "Estate and corporate, no litigation.");
        assert!(doc.body_html.contains("<h2>Lead</h2>"));
        assert!(doc.body_html.contains("<p>Flat-fee.</p>"));
    }

    #[test]
    fn parse_extracts_folded_description_scalar() {
        let raw = "---\n\
                   title: \"Mission\"\n\
                   slug: mission\n\
                   description: >\n\
                   \x20\x20A short folded description that spans two source\n\
                   \x20\x20lines in the frontmatter block.\n\
                   ---\n\nBody.";
        let doc = parse(raw, "fallback").expect("parses");
        assert_eq!(
            doc.description,
            "A short folded description that spans two source lines in the frontmatter block."
        );
    }

    #[test]
    fn parse_passes_inline_html_icons_through_verbatim() {
        // The services index denotes each product with a Bootstrap
        // Icon (`<i class="bi …">`) authored inline in the markdown.
        // pulldown-cmark must emit that raw inline HTML unescaped, or
        // the icons render as literal angle-bracket text.
        let raw = "---\n\
                   title: \"Services\"\n\
                   slug: services\n\
                   description: \"d\"\n\
                   ---\n\n\
                   - <i class=\"bi bi-star-fill\" aria-hidden=\"true\"></i> **Services**";
        let doc = parse(raw, "services").expect("parses");
        assert!(
            doc.body_html
                .contains("<i class=\"bi bi-star-fill\" aria-hidden=\"true\"></i>"),
            "icon markup must survive rendering, got: {}",
            doc.body_html
        );
        // And not be HTML-escaped into visible text.
        assert!(!doc.body_html.contains("&lt;i class"));
    }

    #[test]
    fn parse_uses_fallback_slug_when_omitted() {
        let raw = "---\ntitle: T\n---\nbody";
        let doc = parse(raw, "from-filename").expect("parses");
        assert_eq!(doc.slug, "from-filename");
    }

    #[test]
    fn parse_returns_none_without_frontmatter() {
        assert!(parse("just body, no frontmatter", "x").is_none());
    }

    #[test]
    fn parse_preserves_unknown_frontmatter_keys_into_metadata() {
        let raw = "---\n\
                   title: \"Partner\"\n\
                   slug: partner\n\
                   description: \"d\"\n\
                   topic: immigration\n\
                   org_name: Partner X\n\
                   phone: 1-800-555-0199\n\
                   ---\nbody";
        let doc = parse(raw, "x").expect("parses");
        assert_eq!(
            doc.metadata.get("topic").map(String::as_str),
            Some("immigration"),
        );
        assert_eq!(
            doc.metadata.get("org_name").map(String::as_str),
            Some("Partner X"),
        );
        assert_eq!(
            doc.metadata.get("phone").map(String::as_str),
            Some("1-800-555-0199"),
        );
    }

    #[test]
    fn parse_does_not_duplicate_well_known_keys_into_metadata() {
        // title/slug/description are first-class fields on MarketingDoc;
        // they must NOT leak back into the metadata map or callers will
        // have two sources of truth for the same value.
        let raw = "---\ntitle: T\nslug: s\ndescription: D\n---\nbody";
        let doc = parse(raw, "x").expect("parses");
        assert!(doc.metadata.is_empty(), "got: {:?}", doc.metadata);
    }
}
