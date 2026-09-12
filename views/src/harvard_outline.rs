//! Parse a legal Markdown body into Harvard-outline narration units.
//!
//! Motions and contracts are walked paragraph by paragraph on a stage: each
//! heading, body paragraph, and lettered subsection is one unit a narrator can
//! highlight. Depth-1 headings take **Roman numerals** (`I.`, `II.`) for
//! contracts and engagement letters, or **Arabic numerals** (`1.`, `2.`) for
//! motion practice. Lettered subsections (`A.`, `B.`) live in Markdown block
//! quotes so the PDF conversion actually indents them.
//!
//! Unlabeled paragraphs under a heading inherit that heading's depth, so they
//! share one highlight colour as the narrator steps through them.

use crate::markdown;

/// How depth-1 headings are numbered in this document. This is the shared
/// Word/PDF vocabulary, re-exported here for the narration API.
pub use word::OutlineScheme as DepthOneScheme;

/// What kind of block a unit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Heading,
    Paragraph,
    Subsection,
}

/// One highlightable block on the narration stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub index: usize,
    /// Stable within this Markdown source; imported Word anchors are carried
    /// through the canonical adapter before this narration projection.
    pub anchor: String,
    /// Outline depth, 0 for preamble before the first heading, then 1..=7.
    pub depth: u8,
    /// Displayed marker (`I`, `A`, `1`) or empty for unlabeled prose.
    pub marker: String,
    /// Full path from the root (`I`, `I.A`, `1.B`).
    pub path: String,
    pub kind: UnitKind,
    /// Markdown of this unit (heading text without the `#` marks, or the
    /// paragraph/quote body).
    pub markdown: String,
}

/// A parsed document ready to render as a narration stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineDocument {
    pub title: String,
    pub scheme: Option<DepthOneScheme>,
    pub units: Vec<Unit>,
    /// The raw YAML between the `---` fences, with neither fence line. `None`
    /// for a document with no frontmatter.
    pub frontmatter: Option<String>,
}

impl OutlineDocument {
    /// Convert this parsed Markdown projection to the shared canonical model.
    #[must_use]
    pub fn canonical_model(&self) -> word::CanonicalDocument {
        canonical_model(self)
    }
}

/// Parse Markdown (optional YAML frontmatter) into narration units.
#[must_use]
pub fn parse(src: &str) -> OutlineDocument {
    let (title, frontmatter, body) = title_and_body(src);
    let mut units = Vec::new();
    let mut scheme = None;
    let mut current_depth: u8 = 0;
    let mut current_path = String::new();
    let mut section_path = String::new();

    for block in blocks(body) {
        match block {
            Block::Heading { text, anchor } => {
                let labeled = parse_depth_one_heading(&text);
                let (depth, marker, path, heading_text) = match labeled {
                    Some((DepthOneScheme::Roman, marker, rest)) => {
                        scheme.get_or_insert(DepthOneScheme::Roman);
                        let path = marker.clone();
                        (1_u8, marker, path, rest)
                    }
                    Some((DepthOneScheme::Arabic, marker, rest)) => {
                        // A `1.` heading is depth 1 for motion practice. Under a
                        // Roman contract it is a depth-3 heading (`### 1.`).
                        if scheme == Some(DepthOneScheme::Roman) {
                            let path = join_path(&section_path, &marker);
                            (3, marker, path, rest)
                        } else {
                            scheme.get_or_insert(DepthOneScheme::Arabic);
                            let path = marker.clone();
                            (1, marker, path, rest)
                        }
                    }
                    None => {
                        if let Some((letter, rest)) = parse_capital_letter(&text) {
                            let path = join_path(&section_path, &letter);
                            (2, letter, path, rest)
                        } else {
                            let path = if current_path.is_empty() {
                                String::new()
                            } else {
                                current_path.clone()
                            };
                            let depth = if current_depth == 0 { 1 } else { current_depth };
                            (depth, String::new(), path, text)
                        }
                    }
                };
                current_depth = depth;
                current_path.clone_from(&path);
                if depth == 1 {
                    section_path.clone_from(&path);
                }
                push_unit(
                    &mut units,
                    depth,
                    marker,
                    path,
                    UnitKind::Heading,
                    &heading_text,
                );
                set_last_anchor(&mut units, anchor);
            }
            Block::Quote { paragraphs, anchor } => {
                let mut anchor = anchor;
                for para in paragraphs {
                    if let Some((letter, rest)) = parse_bold_letter_lead(&para) {
                        let path = join_path(&section_path, &letter);
                        current_depth = 2;
                        current_path.clone_from(&path);
                        push_unit(&mut units, 2, letter, path, UnitKind::Subsection, &rest);
                        set_last_anchor(&mut units, anchor.take());
                    } else if let Some((marker, rest, depth)) =
                        parse_deeper_lead(&para, scheme.unwrap_or(DepthOneScheme::Roman))
                    {
                        let path = join_path(&current_path, &marker);
                        current_depth = depth;
                        current_path.clone_from(&path);
                        push_unit(&mut units, depth, marker, path, UnitKind::Subsection, &rest);
                        set_last_anchor(&mut units, anchor.take());
                    } else {
                        let depth = current_depth.max(1);
                        push_paragraph(&mut units, depth, current_path.clone(), &para);
                        set_last_anchor(&mut units, anchor.take());
                    }
                }
            }
            Block::Prose { text, anchor } => {
                // 0 in the preamble, else the enclosing heading's depth.
                push_paragraph(&mut units, current_depth, current_path.clone(), &text);
                set_last_anchor(&mut units, anchor);
            }
        }
    }

    OutlineDocument {
        title,
        scheme,
        units,
        frontmatter: frontmatter.map(str::to_string),
    }
}

/// Project narration units into the canonical model shared with Word import.
/// The narration parser remains intentionally presentation-focused; this
/// projection gives callers a typed, ordered representation without creating
/// a second numbering vocabulary.
#[must_use]
pub fn canonical_model(doc: &OutlineDocument) -> word::CanonicalDocument {
    let blocks = doc
        .units
        .iter()
        .map(|unit| {
            let is_outline = !unit.marker.is_empty();
            let outline = is_outline.then(|| word::OutlineUnit {
                anchor: unit.anchor.clone(),
                depth: unit.depth,
                marker: unit.marker.clone(),
                path: unit.path.clone(),
                text: unit.markdown.clone(),
                // Narration Markdown has no OOXML list behind it. The
                // depth is real and the numbering identity is genuinely
                // absent, so these fields stay empty rather than carrying
                // an invented `w:numFmt` a caller might act on.
                list: word::ListIdentity {
                    numbering_id: String::new(),
                    abstract_numbering_id: None,
                    level: unit.depth.saturating_sub(1),
                    number_format: String::new(),
                    level_text: String::new(),
                    start: 1,
                    restart_level: None,
                    override_start: None,
                    style_id: None,
                },
                manual_label: None,
            });
            word::CanonicalBlock {
                anchor: unit.anchor.clone(),
                kind: if is_outline {
                    word::CanonicalBlockKind::Outline
                } else {
                    word::CanonicalBlockKind::Paragraph
                },
                text: unit.markdown.clone(),
                outline,
                manual_label: None,
                inlines: vec![word::CanonicalInline::Text {
                    text: unit.markdown.clone(),
                }],
                children: Vec::new(),
            }
        })
        .collect();
    word::CanonicalDocument {
        scheme: doc.scheme,
        stories: vec![word::CanonicalStory {
            kind: word::StoryKind::MainDocument,
            part_uri: "markdown".into(),
            blocks,
        }],
        diagnostics: Vec::new(),
    }
}

/// Inner stage markup: the article a page or standalone file wraps.
#[must_use]
pub fn stage_html(doc: &OutlineDocument) -> String {
    let mut out = String::from(
        "<article class=\"harvard-stage\" data-harvard-outline>\n\
         <header class=\"harvard-stage__chrome\">\n\
         <p class=\"harvard-stage__title\">",
    );
    out.push_str(&escape_text(&doc.title));
    out.push_str("</p>\n<p class=\"harvard-stage__counter\" data-harvard-counter></p>\n");
    out.push_str(
        "<p class=\"harvard-stage__hint\">Arrow keys or Space step through the document. \
         Click a paragraph to highlight it.</p>\n</header>\n<div class=\"harvard-doc\">\n",
    );
    for unit in &doc.units {
        out.push_str(&unit_html(unit));
    }
    out.push_str("</div>\n</article>\n");
    out
}

/// One unit as a focusable section.
#[must_use]
pub fn unit_html(unit: &Unit) -> String {
    let kind = match unit.kind {
        UnitKind::Heading => "heading",
        UnitKind::Paragraph => "paragraph",
        UnitKind::Subsection => "subsection",
    };
    let mut out = format!(
        "<section class=\"harvard-unit harvard-unit--depth-{depth} harvard-unit--{kind}\" \
         id=\"harvard-u-{index}\" tabindex=\"-1\" data-harvard-index=\"{index}\" \
         data-harvard-depth=\"{depth}\" data-harvard-path=\"{path}\" data-harvard-kind=\"{kind}\">\n",
        depth = unit.depth,
        kind = kind,
        index = unit.index,
        path = escape_attr(&unit.path),
    );
    if !unit.marker.is_empty() {
        out.push_str("<span class=\"harvard-unit__marker\" aria-hidden=\"true\">");
        out.push_str(&escape_text(&unit.marker));
        out.push_str("</span>\n");
    }
    if unit.kind == UnitKind::Heading {
        out.push_str("<h2>");
        out.push_str(&escape_text(&unit.markdown));
        out.push_str("</h2>\n");
    } else {
        out.push_str(&markdown::render(&unit.markdown));
    }
    out.push_str("</section>\n");
    out
}

/// A complete HTML document for offline recording (CLI `--out`).
#[must_use]
pub fn standalone_html(doc: &OutlineDocument, css: &str, javascript: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>\n{css}\n</style>\n</head>\n\
         <body class=\"harvard-standalone nav-theme\">\n{stage}\
         <script>\n{javascript}\n</script>\n</body>\n</html>\n",
        title = escape_text(&doc.title),
        css = css,
        stage = stage_html(doc),
        javascript = javascript,
    )
}

fn push_unit(
    units: &mut Vec<Unit>,
    depth: u8,
    marker: String,
    path: String,
    kind: UnitKind,
    markdown: &str,
) {
    let markdown = markdown.trim().to_string();
    if markdown.is_empty() {
        return;
    }
    units.push(Unit {
        index: units.len(),
        anchor: format!("markdown:unit:{}", units.len()),
        depth,
        marker,
        path,
        kind,
        markdown,
    });
}

/// An unlabeled paragraph — no marker, no distinct subsection.
fn push_paragraph(units: &mut Vec<Unit>, depth: u8, path: String, markdown: &str) {
    push_unit(
        units,
        depth,
        String::new(),
        path,
        UnitKind::Paragraph,
        markdown,
    );
}

fn join_path(prefix: &str, marker: &str) -> String {
    if prefix.is_empty() {
        marker.to_string()
    } else if marker.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{marker}")
    }
}

fn title_and_body(src: &str) -> (String, Option<&str>, &str) {
    let Some(after_open) = src.strip_prefix("---\n") else {
        return ("Untitled".to_string(), None, src);
    };
    let Some(end) = after_open.find("\n---\n") else {
        return ("Untitled".to_string(), None, src);
    };
    let yaml = &after_open[..end];
    let body = &after_open[end + "\n---\n".len()..];
    let title = yaml
        .lines()
        .find_map(|line| {
            line.strip_prefix("title:")
                .map(|rest| unquote(rest.trim()))
                .filter(|t| !t.is_empty())
        })
        .unwrap_or_else(|| "Untitled".to_string());
    (title, Some(yaml), body)
}

/// Read one flat top-level scalar key (`origin_url:`, `jurisdiction:`, …) out
/// of a template's raw frontmatter YAML, the same way [`title_and_body`]
/// reads `title:`. `None` when the key is absent or its value is empty.
#[must_use]
pub fn frontmatter_field(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    yaml.lines().find_map(|line| {
        line.strip_prefix(prefix.as_str())
            .map(|rest| unquote(rest.trim()))
            .filter(|value| !value.is_empty())
    })
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

enum Block {
    Heading {
        text: String,
        anchor: Option<String>,
    },
    Quote {
        paragraphs: Vec<String>,
        anchor: Option<String>,
    },
    Prose {
        text: String,
        anchor: Option<String>,
    },
}

fn blocks(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    let mut pending_anchor = None;
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        // `word::notation` carries a canonical import's structural identity
        // in `navigator-*` comments. Narration needs only the source anchor
        // out of them; the rest is invisible to this presentation parser.
        if let Some(directive) = navigator_directive(line) {
            if let Some(anchor) = comment_attribute(directive, "anchor") {
                pending_anchor = Some(anchor);
            }
            continue;
        }
        if line.trim().eq_ignore_ascii_case("<!-- pagebreak -->") {
            pending_anchor = None;
            continue;
        }
        if let Some(heading) = heading_text(line) {
            out.push(Block::Heading {
                text: heading,
                anchor: pending_anchor.take(),
            });
            continue;
        }
        if line.starts_with('>') {
            let mut quote_lines = vec![strip_quote(line)];
            while matches!(lines.peek(), Some(next) if next.starts_with('>') || next.trim().is_empty())
            {
                let next = lines.next().expect("peeked line");
                if next.trim().is_empty() {
                    // A blank inside a quote run ends this quote paragraph group
                    // only when the following line is not a continuation quote.
                    if matches!(lines.peek(), Some(peek) if peek.starts_with('>')) {
                        quote_lines.push(String::new());
                    } else {
                        break;
                    }
                } else {
                    quote_lines.push(strip_quote(next));
                }
            }
            let paragraphs = quote_lines
                .split(String::is_empty)
                .map(|para| para.join("\n"))
                .filter(|p| !p.trim().is_empty())
                .collect();
            out.push(Block::Quote {
                paragraphs,
                anchor: pending_anchor.take(),
            });
            continue;
        }
        let mut prose = vec![line.to_string()];
        while matches!(lines.peek(), Some(next) if !next.trim().is_empty() && !next.starts_with('>') && heading_text(next).is_none())
        {
            prose.push(lines.next().expect("peeked line").to_string());
        }
        out.push(Block::Prose {
            text: prose.join("\n"),
            anchor: pending_anchor.take(),
        });
    }
    out
}

/// The body of a `<!-- navigator-… -->` comment occupying a whole line.
fn navigator_directive(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("<!-- navigator-")?
        .strip_suffix("-->")
        .map(str::trim_end)
}

fn comment_attribute(directive: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = directive.find(&needle)? + needle.len();
    let rest = &directive[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn set_last_anchor(units: &mut [Unit], anchor: Option<String>) {
    if let (Some(unit), Some(anchor)) = (units.last_mut(), anchor) {
        unit.anchor = anchor;
    }
}

fn heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?;
    if !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim().to_string())
}

fn strip_quote(line: &str) -> String {
    line.trim_start()
        .strip_prefix('>')
        .map_or(line, str::trim_start)
        .to_string()
}

fn parse_depth_one_heading(text: &str) -> Option<(DepthOneScheme, String, String)> {
    let (marker, rest) = split_marker(text)?;
    if is_roman(&marker) {
        Some((DepthOneScheme::Roman, marker, rest))
    } else if marker.chars().all(|c| c.is_ascii_digit()) {
        Some((DepthOneScheme::Arabic, marker, rest))
    } else {
        None
    }
}

fn parse_capital_letter(text: &str) -> Option<(String, String)> {
    let (marker, rest) = split_marker(text)?;
    let mut chars = marker.chars();
    let c = chars.next()?;
    if chars.next().is_none() && c.is_ascii_uppercase() {
        Some((marker, rest))
    } else {
        None
    }
}

/// `**A. Label.** body` — the engagement-letter subsection form.
fn parse_bold_letter_lead(para: &str) -> Option<(String, String)> {
    let trimmed = para.trim();
    let rest = trimmed.strip_prefix("**")?;
    let closing = rest.find("**")?;
    let bold = &rest[..closing];
    let (marker, label) = split_marker(bold)?;
    let mut chars = marker.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_uppercase() {
        return None;
    }
    let body = rest[closing + 2..].trim();
    let text = if body.is_empty() {
        label
    } else {
        format!("{label} {body}")
    };
    Some((marker, text))
}

fn parse_deeper_lead(para: &str, scheme: DepthOneScheme) -> Option<(String, String, u8)> {
    let trimmed = para.trim().trim_start_matches('*').trim_start();
    if let Some(rest) = trimmed.strip_prefix('(') {
        let close = rest.find(')')?;
        let inner = &rest[..close];
        let after = rest[close + 1..].trim().to_string();
        if inner.chars().all(|c| c.is_ascii_digit()) {
            let depth = if scheme == DepthOneScheme::Arabic {
                3
            } else {
                5
            };
            return Some((format!("({inner})"), after, depth));
        }
        if inner.chars().all(|c| c.is_ascii_lowercase()) {
            return Some((format!("({inner})"), after, 6));
        }
        if is_roman(&inner.to_ascii_uppercase()) {
            return Some((format!("({inner})"), after, 7));
        }
        return None;
    }
    let (marker, rest) = split_marker(trimmed)?;
    if marker.chars().all(|c| c.is_ascii_lowercase()) && marker.len() == 1 {
        return Some((marker, rest, 4));
    }
    if marker.chars().all(|c| c.is_ascii_digit()) && scheme == DepthOneScheme::Roman {
        return Some((marker, rest, 3));
    }
    None
}

fn split_marker(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim();
    let dot = trimmed.find('.')?;
    if dot == 0 {
        return None;
    }
    let marker = trimmed[..dot].to_string();
    if marker.chars().any(|c| !(c.is_ascii_alphanumeric())) {
        return None;
    }
    let rest = trimmed[dot + 1..].trim().to_string();
    Some((marker, rest))
}

fn is_roman(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'))
}

fn escape_text(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(raw: &str) -> String {
    escape_text(raw)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Synthetic motion-practice body shared by the lawyer stage and CLI tests.
pub const SAMPLE_MOTION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/sample_motion.md"
));

#[cfg(test)]
mod tests {
    use super::{canonical_model, frontmatter_field, parse, stage_html, DepthOneScheme, UnitKind};

    const ONBOARDING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../templates/notations/neon_law/shared/onboarding_letter.md"
    ));
    const OFFBOARDING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../templates/notations/neon_law/shared/offboarding_letter.md"
    ));

    #[test]
    fn roman_headings_and_lettered_quotes() {
        let doc = parse(
            "---\ntitle: Sample\n---\n\nDear client:\n\n## I. Scope\n\nThe firm acts.\n\n\
             ## II. Fees\n\n> **A. Invoices.** Read them.\n>\n> **B. Costs.** Passed through.\n",
        );
        assert_eq!(doc.title, "Sample");
        assert_eq!(doc.scheme, Some(DepthOneScheme::Roman));
        let headings: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Heading)
            .map(|u| (u.marker.as_str(), u.depth, u.markdown.as_str()))
            .collect();
        assert_eq!(headings, vec![("I", 1, "Scope"), ("II", 1, "Fees")]);
        let subs: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Subsection)
            .map(|u| (u.path.as_str(), u.marker.as_str(), u.depth))
            .collect();
        assert_eq!(subs, vec![("II.A", "A", 2), ("II.B", "B", 2)]);
        let preamble = doc
            .units
            .iter()
            .find(|u| u.markdown.contains("Dear client"))
            .expect("preamble");
        assert_eq!(preamble.depth, 0);
        let under_scope = doc
            .units
            .iter()
            .find(|u| u.markdown.contains("The firm acts"))
            .expect("body under I");
        assert_eq!(under_scope.depth, 1);
        assert_eq!(under_scope.path, "I");
    }

    #[test]
    fn parse_extracts_the_frontmatter_yaml_verbatim() {
        let doc = parse("---\ntitle: Sample\ncode: sample__doc\n---\n\nBody.\n");
        assert_eq!(
            doc.frontmatter.as_deref(),
            Some("title: Sample\ncode: sample__doc")
        );
    }

    #[test]
    fn a_document_with_no_frontmatter_has_none() {
        let doc = parse("Just a body, no frontmatter fences.\n");
        assert_eq!(doc.frontmatter, None);
    }

    #[test]
    fn frontmatter_field_reads_a_flat_scalar_key() {
        let yaml = "kind: filing\ntitle: Nevada LLC Formation\norigin_url: https://example.gov/forms\njurisdiction: NV";
        assert_eq!(
            frontmatter_field(yaml, "origin_url").as_deref(),
            Some("https://example.gov/forms")
        );
        assert_eq!(
            frontmatter_field(yaml, "jurisdiction").as_deref(),
            Some("NV")
        );
        assert_eq!(frontmatter_field(yaml, "no_such_key"), None);
    }

    #[test]
    fn motion_practice_uses_arabic_depth_one() {
        let doc = parse(
            "## 1. Introduction\n\nFacts follow.\n\n## 2. Argument\n\n\
             > **A. Standard.** De novo.\n",
        );
        assert_eq!(doc.scheme, Some(DepthOneScheme::Arabic));
        assert_eq!(doc.units[0].marker, "1");
        assert_eq!(doc.units[0].depth, 1);
        assert_eq!(doc.units[1].depth, 1);
        assert_eq!(doc.units[1].path, "1");
        let sub = doc
            .units
            .iter()
            .find(|u| u.kind == UnitKind::Subsection)
            .expect("A");
        assert_eq!(sub.path, "2.A");
        assert_eq!(sub.depth, 2);
        assert_eq!(doc.canonical_model().scheme, Some(DepthOneScheme::Arabic));
    }

    #[test]
    fn the_bundled_onboarding_letter_is_a_roman_outline() {
        let doc = parse(ONBOARDING);
        assert_eq!(doc.title, "Onboarding Letter");
        assert_eq!(doc.scheme, Some(DepthOneScheme::Roman));
        let markers: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Heading)
            .map(|u| u.marker.as_str())
            .collect();
        assert_eq!(
            markers,
            vec!["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"]
        );
        assert!(doc.units.iter().any(|u| u.path == "II.A" && u.depth == 2));
        assert!(doc.units.iter().any(|u| u.path == "III.B" && u.depth == 2));
        let html = stage_html(&doc);
        assert!(html.contains("data-harvard-outline"));
        assert!(html.contains("data-harvard-path=\"I\""));
        assert!(html.contains("data-harvard-path=\"II.A\""));
        assert!(html.contains("harvard-unit--depth-1"));
        assert!(html.contains("harvard-unit--depth-2"));
    }

    #[test]
    fn narration_carries_the_source_anchors_of_a_canonical_word_import() {
        // The canonical projection is `word::notation`; narration is the
        // presentation stage downstream of it. What narration owes the
        // import is the source anchor, so an edit made here still names the
        // same clause in the imported document.
        let document = word::CanonicalDocument {
            scheme: Some(DepthOneScheme::Roman),
            stories: vec![word::CanonicalStory {
                kind: word::StoryKind::MainDocument,
                part_uri: "/word/document.xml".into(),
                blocks: vec![outline_block(
                    "/word/document.xml:paragraph:7A",
                    1,
                    "I",
                    "Term",
                )],
            }],
            diagnostics: Vec::new(),
        };

        let narrated = parse(&document.to_markdown());

        assert_eq!(narrated.scheme, Some(DepthOneScheme::Roman));
        assert_eq!(
            narrated
                .units
                .iter()
                .map(|unit| unit.anchor.as_str())
                .collect::<Vec<_>>(),
            vec!["/word/document.xml:paragraph:7A"]
        );
        assert_eq!(narrated.units[0].path, "I");
    }

    #[test]
    fn narration_units_that_never_saw_word_still_carry_a_stable_anchor() {
        let doc = parse("# I. First\n\n# II. Second\n");
        assert_eq!(doc.units[0].anchor, "markdown:unit:0");
        assert_eq!(doc.units[1].anchor, "markdown:unit:1");
        assert_eq!(
            canonical_model(&doc).stories[0].blocks[1].anchor,
            "markdown:unit:1"
        );
    }

    fn outline_block(anchor: &str, depth: u8, marker: &str, text: &str) -> word::CanonicalBlock {
        word::CanonicalBlock {
            anchor: anchor.into(),
            kind: word::CanonicalBlockKind::Outline,
            text: text.into(),
            outline: Some(word::OutlineUnit {
                anchor: anchor.into(),
                depth,
                marker: marker.into(),
                path: marker.into(),
                text: text.into(),
                list: word::ListIdentity {
                    numbering_id: "1".into(),
                    abstract_numbering_id: None,
                    level: depth - 1,
                    number_format: "upperRoman".into(),
                    level_text: format!("%{depth}."),
                    start: 1,
                    restart_level: None,
                    override_start: None,
                    style_id: None,
                },
                manual_label: None,
            }),
            manual_label: None,
            inlines: vec![word::CanonicalInline::Text { text: text.into() }],
            children: Vec::new(),
        }
    }

    #[test]
    fn the_bundled_offboarding_letter_is_a_roman_outline() {
        let doc = parse(OFFBOARDING);
        assert_eq!(doc.title, "Closing Letter");
        assert_eq!(doc.scheme, Some(DepthOneScheme::Roman));
        let markers: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Heading)
            .map(|u| u.marker.as_str())
            .collect();
        assert_eq!(markers, vec!["I", "II", "III", "IV", "V", "VI"]);
        let html = stage_html(&doc);
        assert!(html.contains("data-harvard-outline"));
        assert!(html.contains("data-harvard-path=\"I\""));
        assert!(html.contains("harvard-unit--depth-1"));
        assert!(html.contains("Representation concluded"));
    }

    #[test]
    fn unlabeled_paragraphs_share_the_section_highlight_depth() {
        let doc = parse("## I. One\n\nFirst.\n\nSecond.\n");
        assert_eq!(doc.units.len(), 3);
        assert!(doc.units[1..].iter().all(|u| u.depth == 1 && u.path == "I"));
    }

    #[test]
    fn the_sample_motion_is_an_arabic_outline() {
        let doc = parse(super::SAMPLE_MOTION);
        assert_eq!(doc.title, "Sample Motion");
        assert_eq!(doc.scheme, Some(DepthOneScheme::Arabic));
        let headings: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Heading)
            .map(|u| u.path.as_str())
            .collect();
        assert_eq!(headings, vec!["1", "2", "3"]);
        assert!(doc.units.iter().any(|u| u.path == "2.A" && u.depth == 2));
        assert!(doc.units.iter().any(|u| u.path == "2.B" && u.depth == 2));
    }
}
