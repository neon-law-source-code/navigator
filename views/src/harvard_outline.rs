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

/// How depth-1 headings are numbered in this document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthOneScheme {
    /// `I.`, `II.`, `III.` — contracts and engagement letters.
    Roman,
    /// `1.`, `2.`, `3.` — motion practice.
    Arabic,
}

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
}

/// Parse Markdown (optional YAML frontmatter) into narration units.
#[must_use]
pub fn parse(src: &str) -> OutlineDocument {
    let (title, body) = title_and_body(src);
    let mut units = Vec::new();
    let mut scheme = None;
    let mut current_depth: u8 = 0;
    let mut current_path = String::new();
    let mut section_path = String::new();

    for block in blocks(body) {
        match block {
            Block::Heading { text, .. } => {
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
            }
            Block::Quote(paragraphs) => {
                for para in paragraphs {
                    if let Some((letter, rest)) = parse_bold_letter_lead(&para) {
                        let path = join_path(&section_path, &letter);
                        current_depth = 2;
                        current_path.clone_from(&path);
                        push_unit(&mut units, 2, letter, path, UnitKind::Subsection, &rest);
                    } else if let Some((marker, rest, depth)) =
                        parse_deeper_lead(&para, scheme.unwrap_or(DepthOneScheme::Roman))
                    {
                        let path = join_path(&current_path, &marker);
                        current_depth = depth;
                        current_path.clone_from(&path);
                        push_unit(&mut units, depth, marker, path, UnitKind::Subsection, &rest);
                    } else {
                        let depth = current_depth.max(1);
                        push_unit(
                            &mut units,
                            depth,
                            String::new(),
                            current_path.clone(),
                            UnitKind::Paragraph,
                            &para,
                        );
                    }
                }
            }
            Block::Prose(text) => {
                let depth = current_depth; // 0 in the preamble, else the heading
                push_unit(
                    &mut units,
                    depth,
                    String::new(),
                    current_path.clone(),
                    UnitKind::Paragraph,
                    &text,
                );
            }
        }
    }

    OutlineDocument {
        title,
        scheme,
        units,
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
        "<p class=\"harvard-stage__hint\">Arrow keys, J and K, or Space step through the \
         document. Click a paragraph to highlight it.</p>\n</header>\n<div class=\"harvard-doc\">\n",
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
        depth,
        marker,
        path,
        kind,
        markdown,
    });
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

fn title_and_body(src: &str) -> (String, &str) {
    let Some(after_open) = src.strip_prefix("---\n") else {
        return ("Untitled".to_string(), src);
    };
    let Some(end) = after_open.find("\n---\n") else {
        return ("Untitled".to_string(), src);
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
    (title, body)
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
    Heading { text: String },
    Quote(Vec<String>),
    Prose(String),
}

fn blocks(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(heading) = heading_text(line) {
            out.push(Block::Heading { text: heading });
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
            out.push(Block::Quote(paragraphs));
            continue;
        }
        let mut prose = vec![line.to_string()];
        while matches!(lines.peek(), Some(next) if !next.trim().is_empty() && !next.starts_with('>') && heading_text(next).is_none())
        {
            prose.push(lines.next().expect("peeked line").to_string());
        }
        out.push(Block::Prose(prose.join("\n")));
    }
    out
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
    let (marker, _) = split_marker(rest)?;
    let mut chars = marker.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_uppercase() {
        return None;
    }
    Some((marker, trimmed.to_string()))
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
    use super::{parse, stage_html, DepthOneScheme, UnitKind};

    const RETAINER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../templates/neon_law/shared/letter.md"
    ));
    const OFFBOARDING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../templates/neon_law/shared/offboarding_letter.md"
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
    }

    #[test]
    fn the_bundled_retainer_is_a_roman_outline() {
        let doc = parse(RETAINER);
        assert_eq!(doc.title, "Retainer Agreement");
        assert_eq!(doc.scheme, Some(DepthOneScheme::Roman));
        let markers: Vec<_> = doc
            .units
            .iter()
            .filter(|u| u.kind == UnitKind::Heading)
            .map(|u| u.marker.as_str())
            .collect();
        assert_eq!(
            markers,
            vec!["I", "II", "III", "IV", "V", "VI", "VII", "VIII"]
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
