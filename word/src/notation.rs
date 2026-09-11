//! The editable Notation Markdown projection of a canonical Word import,
//! and the CommonMark parse that reads it back.
//!
//! # Why the structure rides in comments
//!
//! Word numbering is not text. A displayed `1.` is the product of a list
//! instance, an abstract definition, a level, a restart rule, and a style,
//! and two paragraphs showing the same marker can belong to unrelated
//! lists. Markdown can express none of that, and the visible marker alone
//! is ambiguous even about depth: `(i)` is a depth-six lower letter and a
//! depth-seven lower roman at the same time.
//!
//! So the projection keeps the two concerns apart. The visible Markdown is
//! ordinary prose an attorney edits; every structural fact travels beside
//! it in an HTML comment, which CommonMark hands back verbatim as an
//! [`Event::Html`]. Reading the document back is therefore a lookup rather
//! than a guess, and [`from_markdown`] is the same `pulldown-cmark` grammar
//! the rest of the workspace parses Markdown with — there is no second
//! Markdown parser and no second marker vocabulary.
//!
//! # What survives the round trip
//!
//! Story kind and part, block order and nesting, source anchors, outline
//! depth, displayed marker, cumulative path, the full list identity, manual
//! labels, and the ordered typed inline structure (bookmarks, hyperlinks,
//! fields, revisions, tabs, breaks, comment references). Inline nodes are
//! carried as an ordered, typed sequence attached to their block rather
//! than interleaved into the prose, because the prose is what an editor
//! rewrites and the sequence is what has to stay identifiable.

use pulldown_cmark::{Event, Options, Parser, TagEnd};

/// The CommonMark specification Notation Markdown is written against.
/// `pulldown-cmark` is pinned at 0.13 in the workspace manifest and states
/// conformance with this revision; the constant exists so a dependency bump
/// that moves the grammar has to move a documented number with it.
pub const COMMONMARK_VERSION: &str = "0.31.2";

use crate::outline::{
    CanonicalBlock, CanonicalBlockKind, CanonicalDocument, CanonicalInline, CanonicalStory,
    ListIdentity, OutlineScheme, OutlineUnit,
};
use crate::{BreakKind, FieldKind, RevisionKind, StoryKind};

/// Emit the governed, editable Markdown representation of every canonical
/// story in document order.
#[must_use]
pub fn to_markdown(document: &CanonicalDocument) -> String {
    let mut out = String::new();
    if let Some(scheme) = document.scheme {
        out.push_str(&comment(
            "navigator-document",
            &[("scheme", Some(scheme_name(scheme).to_string()))],
        ));
        out.push('\n');
    }
    for story in &document.stories {
        out.push_str(&comment(
            "navigator-story",
            &[
                ("kind", Some(story_kind_name(&story.kind).to_string())),
                ("part", Some(story.part_uri.clone())),
            ],
        ));
        out.push('\n');
        for block in &story.blocks {
            write_block(&mut out, block, None);
        }
    }
    out
}

fn write_block(out: &mut String, block: &CanonicalBlock, parent: Option<&str>) {
    let mut attributes = vec![
        ("kind", Some(kind_name(block.kind).to_string())),
        ("anchor", Some(block.anchor.clone())),
        ("parent", parent.map(str::to_string)),
        ("manual-label", block.manual_label.clone()),
    ];
    if let Some(unit) = &block.outline {
        attributes.extend(outline_attributes(unit));
    }
    out.push_str(&comment("navigator-block", &attributes));
    out.push('\n');
    for inline in &block.inlines {
        write_inline(out, inline);
    }
    let visible = visible_text(block);
    if !visible.is_empty() {
        out.push_str(&visible);
        out.push_str("\n\n");
    }
    for child in &block.children {
        write_block(out, child, Some(&block.anchor));
    }
}

fn outline_attributes(unit: &OutlineUnit) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("depth", Some(unit.depth.to_string())),
        ("marker", Some(unit.marker.clone())),
        ("path", Some(unit.path.clone())),
        ("num-id", Some(unit.list.numbering_id.clone())),
        ("abstract-num-id", unit.list.abstract_numbering_id.clone()),
        ("level", Some(unit.list.level.to_string())),
        ("num-fmt", Some(unit.list.number_format.clone())),
        ("lvl-text", Some(unit.list.level_text.clone())),
        ("start", Some(unit.list.start.to_string())),
        (
            "restart-level",
            unit.list.restart_level.map(|level| level.to_string()),
        ),
        (
            "override-start",
            unit.list.override_start.map(|start| start.to_string()),
        ),
        ("style-id", unit.list.style_id.clone()),
    ]
}

/// The prose a reader sees and edits. The marker is repeated here purely so
/// the document reads correctly on its own; the authoritative depth, marker,
/// and path are the block comment's.
fn visible_text(block: &CanonicalBlock) -> String {
    let text = escape_prose(&block.text);
    match (&block.outline, block.kind) {
        (Some(unit), _) if unit.depth == 1 => format!("# {}{text}", marker_lead(&unit.marker)),
        (Some(unit), _) if unit.depth == 2 => {
            format!("> **{}{text}**", marker_lead(&unit.marker))
        }
        (Some(unit), _) => format!("> {}{text}", marker_lead(&unit.marker)),
        (None, CanonicalBlockKind::SectionBreak | CanonicalBlockKind::Table) => String::new(),
        (None, _) if text.is_empty() => String::new(),
        (None, _) => text,
    }
}

/// `1` is written `1.` and `(1)` is written `(1)`; a parenthesized group
/// already closes itself, and `(1).` is not a Harvard marker.
fn marker_lead(marker: &str) -> String {
    if marker.starts_with('(') {
        format!("{marker} ")
    } else {
        format!("{marker}. ")
    }
}

fn write_inline(out: &mut String, inline: &CanonicalInline) {
    let attributes: Vec<(&'static str, Option<String>)> = match inline {
        CanonicalInline::Text { .. } => return,
        CanonicalInline::Tab => vec![("kind", Some("tab".into()))],
        CanonicalInline::Break { break_kind } => vec![
            ("kind", Some("break".into())),
            ("break", Some(break_name(break_kind).to_string())),
        ],
        CanonicalInline::BookmarkStart { id, name } => vec![
            ("kind", Some("bookmark_start".into())),
            ("id", Some(id.clone())),
            ("name", name.clone()),
        ],
        CanonicalInline::BookmarkEnd { id } => vec![
            ("kind", Some("bookmark_end".into())),
            ("id", Some(id.clone())),
        ],
        CanonicalInline::Hyperlink {
            relationship_id,
            anchor,
            ..
        } => vec![
            ("kind", Some("hyperlink".into())),
            ("rel", relationship_id.clone()),
            ("target", anchor.clone()),
        ],
        CanonicalInline::Field {
            field_kind,
            instruction,
            ..
        } => vec![
            ("kind", Some("field".into())),
            ("field", Some(field_name(field_kind).to_string())),
            ("instruction", instruction.clone()),
        ],
        CanonicalInline::Revision {
            revision_kind,
            id,
            anchor,
            ..
        } => vec![
            ("kind", Some("revision".into())),
            ("revision", Some(revision_name(*revision_kind))),
            ("id", id.clone()),
            ("target", Some(anchor.clone())),
        ],
        CanonicalInline::CommentReference { id } => vec![
            ("kind", Some("comment_reference".into())),
            ("id", Some(id.clone())),
        ],
    };
    out.push_str(&comment("navigator-inline", &attributes));
    out.push('\n');
}

/// Parse the emitted Markdown back into the canonical model through the
/// workspace's one CommonMark grammar.
#[must_use]
pub fn from_markdown(source: &str) -> CanonicalDocument {
    let mut reader = Reader::default();
    for event in Parser::new_ext(source, Options::empty()) {
        match event {
            Event::Html(raw) | Event::InlineHtml(raw) => reader.html(&raw),
            Event::Text(text) | Event::Code(text) => reader.text.push_str(&text),
            Event::SoftBreak | Event::HardBreak => reader.text.push(' '),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) => reader.flush(),
            _ => {}
        }
    }
    reader.finish()
}

#[derive(Default)]
struct Reader {
    scheme: Option<OutlineScheme>,
    stories: Vec<CanonicalStory>,
    pending_story: Option<(StoryKind, String)>,
    pending_block: Option<Vec<(String, String)>>,
    pending_inlines: Vec<CanonicalInline>,
    text: String,
    blocks: Vec<(Option<String>, CanonicalBlock)>,
}

impl Reader {
    fn html(&mut self, raw: &str) {
        for (name, attributes) in comments(raw) {
            match name.as_str() {
                "navigator-document" => {
                    self.scheme = attribute(&attributes, "scheme").and_then(parse_scheme);
                }
                "navigator-story" => {
                    self.flush();
                    self.close_story();
                    self.pending_story = Some((
                        attribute(&attributes, "kind")
                            .and_then(parse_story_kind)
                            .unwrap_or(StoryKind::MainDocument),
                        attribute(&attributes, "part").unwrap_or_default(),
                    ));
                }
                "navigator-block" => {
                    self.flush();
                    self.pending_block = Some(attributes);
                }
                "navigator-inline" => {
                    if let Some(inline) = parse_inline(&attributes) {
                        self.pending_inlines.push(inline);
                    }
                }
                _ => {}
            }
        }
    }

    fn flush(&mut self) {
        let text = self.text.trim().to_string();
        self.text.clear();
        let Some(attributes) = self.pending_block.take() else {
            self.pending_inlines.clear();
            return;
        };
        let kind = attribute(&attributes, "kind")
            .and_then(|kind| parse_kind(&kind))
            .unwrap_or(CanonicalBlockKind::Paragraph);
        let anchor = attribute(&attributes, "anchor").unwrap_or_default();
        let outline = parse_outline(&attributes, &anchor, &text);
        // The marker is repeated in the prose so the document reads on its
        // own; the model's text is the prose without it.
        let text = outline.as_ref().map_or(text.clone(), |unit| {
            strip_marker(&text, &unit.marker).to_string()
        });
        let outline = outline.map(|mut unit| {
            unit.text.clone_from(&text);
            unit
        });
        let mut inlines = std::mem::take(&mut self.pending_inlines);
        if !text.is_empty() {
            inlines.insert(0, CanonicalInline::Text { text: text.clone() });
        }
        self.blocks.push((
            attribute(&attributes, "parent"),
            CanonicalBlock {
                anchor,
                kind,
                text,
                outline,
                manual_label: attribute(&attributes, "manual-label"),
                inlines,
                children: Vec::new(),
            },
        ));
    }

    fn close_story(&mut self) {
        let Some((kind, part_uri)) = self.pending_story.take() else {
            self.blocks.clear();
            return;
        };
        self.stories.push(CanonicalStory {
            kind,
            part_uri,
            blocks: nest(std::mem::take(&mut self.blocks)),
        });
    }

    fn finish(mut self) -> CanonicalDocument {
        self.flush();
        self.close_story();
        CanonicalDocument {
            scheme: self.scheme,
            stories: self.stories,
            diagnostics: Vec::new(),
        }
    }
}

/// Rebuild the block tree from the flat, `parent`-tagged emission. A child
/// whose parent anchor never appears stays at the top level rather than
/// being dropped: an anchored, mis-ordered block is recoverable and a
/// discarded one is not.
fn nest(flat: Vec<(Option<String>, CanonicalBlock)>) -> Vec<CanonicalBlock> {
    let mut roots: Vec<CanonicalBlock> = Vec::new();
    for (parent, block) in flat {
        match parent.and_then(|anchor| find(&mut roots, &anchor)) {
            Some(target) => target.children.push(block),
            None => roots.push(block),
        }
    }
    roots
}

fn find<'a>(blocks: &'a mut [CanonicalBlock], anchor: &str) -> Option<&'a mut CanonicalBlock> {
    for block in blocks {
        if block.anchor == anchor {
            return Some(block);
        }
        if let Some(found) = find(&mut block.children, anchor) {
            return Some(found);
        }
    }
    None
}

fn parse_outline(
    attributes: &[(String, String)],
    anchor: &str,
    text: &str,
) -> Option<OutlineUnit> {
    let depth = attribute(attributes, "depth")?.parse().ok()?;
    Some(OutlineUnit {
        anchor: anchor.to_string(),
        depth,
        marker: attribute(attributes, "marker").unwrap_or_default(),
        path: attribute(attributes, "path").unwrap_or_default(),
        text: text.to_string(),
        list: ListIdentity {
            numbering_id: attribute(attributes, "num-id").unwrap_or_default(),
            abstract_numbering_id: attribute(attributes, "abstract-num-id"),
            level: attribute(attributes, "level")
                .and_then(|level| level.parse().ok())
                .unwrap_or_default(),
            number_format: attribute(attributes, "num-fmt").unwrap_or_default(),
            level_text: attribute(attributes, "lvl-text").unwrap_or_default(),
            start: attribute(attributes, "start")
                .and_then(|start| start.parse().ok())
                .unwrap_or(1),
            restart_level: attribute(attributes, "restart-level")
                .and_then(|level| level.parse().ok()),
            override_start: attribute(attributes, "override-start")
                .and_then(|start| start.parse().ok()),
            style_id: attribute(attributes, "style-id"),
        },
        manual_label: None,
    })
}

fn parse_inline(attributes: &[(String, String)]) -> Option<CanonicalInline> {
    let id = || attribute(attributes, "id").unwrap_or_default();
    Some(match attribute(attributes, "kind")?.as_str() {
        "tab" => CanonicalInline::Tab,
        "break" => CanonicalInline::Break {
            break_kind: attribute(attributes, "break")
                .and_then(|kind| parse_break(&kind))
                .unwrap_or(BreakKind::Line),
        },
        "bookmark_start" => CanonicalInline::BookmarkStart {
            id: id(),
            name: attribute(attributes, "name"),
        },
        "bookmark_end" => CanonicalInline::BookmarkEnd { id: id() },
        "hyperlink" => CanonicalInline::Hyperlink {
            relationship_id: attribute(attributes, "rel"),
            anchor: attribute(attributes, "target"),
            children: Vec::new(),
        },
        "field" => CanonicalInline::Field {
            field_kind: attribute(attributes, "field")
                .and_then(|kind| parse_field(&kind))
                .unwrap_or(FieldKind::Simple),
            instruction: attribute(attributes, "instruction"),
            children: Vec::new(),
        },
        "revision" => CanonicalInline::Revision {
            revision_kind: attribute(attributes, "revision")
                .and_then(|kind| parse_revision(&kind))
                .unwrap_or(RevisionKind::Insertion),
            id: attribute(attributes, "id"),
            anchor: attribute(attributes, "target").unwrap_or_default(),
            children: Vec::new(),
        },
        "comment_reference" => CanonicalInline::CommentReference { id: id() },
        _ => return None,
    })
}

fn strip_marker<'a>(text: &'a str, marker: &str) -> &'a str {
    if marker.is_empty() {
        return text;
    }
    let lead = marker_lead(marker);
    text.strip_prefix(lead.trim_end())
        .map_or(text, |rest| rest.trim_start())
}

// -- comment encoding ------------------------------------------------------

fn comment(name: &str, attributes: &[(&str, Option<String>)]) -> String {
    let mut out = format!("<!-- {name}");
    for (key, value) in attributes {
        if let Some(value) = value {
            out.push_str(&format!(" {key}=\"{}\"", escape_attribute(value)));
        }
    }
    out.push_str(" -->");
    out
}

/// Every comment in one raw HTML run, in order. Consecutive comment lines
/// are a single CommonMark HTML block, so a run routinely carries several.
fn comments(raw: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let Some(end) = after.find("-->") else { break };
        let body = after[..end].trim();
        rest = &after[end + 3..];
        let mut parts = body.splitn(2, char::is_whitespace);
        let Some(name) = parts.next() else { continue };
        if !name.starts_with("navigator-") {
            continue;
        }
        out.push((
            name.to_string(),
            parse_attributes(parts.next().unwrap_or_default()),
        ));
    }
    out
}

fn parse_attributes(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(equals) = rest.find("=\"") {
        let key = rest[..equals].trim().to_string();
        let after = &rest[equals + 2..];
        let Some(end) = after.find('"') else { break };
        out.push((key, unescape_attribute(&after[..end])));
        rest = &after[end + 1..];
    }
    out
}

fn attribute(attributes: &[(String, String)], key: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

fn escape_attribute(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            other => out.push(other),
        }
    }
    out
}

fn unescape_attribute(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#10;", "\n")
        .replace("&#9;", "\t")
        .replace("&amp;", "&")
}

/// Backslash-escape the CommonMark punctuation that would otherwise turn
/// legal prose into structure — including the `<` that would open a comment
/// this parser then read as Navigator metadata.
fn escape_prose(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (index, character) in raw.char_indices() {
        let structural_lead = index == 0 && matches!(character, '#' | '>' | '|' | '-' | '+' | '=');
        if structural_lead || matches!(character, '\\' | '`' | '*' | '_' | '[' | ']' | '<') {
            out.push('\\');
        }
        out.push(character);
    }
    out.replace('\n', "  \n")
}

// -- enum names ------------------------------------------------------------

fn scheme_name(scheme: OutlineScheme) -> &'static str {
    match scheme {
        OutlineScheme::Roman => "roman",
        OutlineScheme::Arabic => "arabic",
    }
}

fn parse_scheme(raw: String) -> Option<OutlineScheme> {
    match raw.as_str() {
        "roman" => Some(OutlineScheme::Roman),
        "arabic" => Some(OutlineScheme::Arabic),
        _ => None,
    }
}

fn kind_name(kind: CanonicalBlockKind) -> &'static str {
    match kind {
        CanonicalBlockKind::Outline => "outline",
        CanonicalBlockKind::Paragraph => "paragraph",
        CanonicalBlockKind::Table => "table",
        CanonicalBlockKind::Signature => "signature",
        CanonicalBlockKind::SectionBreak => "section_break",
    }
}

fn parse_kind(raw: &str) -> Option<CanonicalBlockKind> {
    match raw {
        "outline" => Some(CanonicalBlockKind::Outline),
        "paragraph" => Some(CanonicalBlockKind::Paragraph),
        "table" => Some(CanonicalBlockKind::Table),
        "signature" => Some(CanonicalBlockKind::Signature),
        "section_break" => Some(CanonicalBlockKind::SectionBreak),
        _ => None,
    }
}

fn story_kind_name(kind: &StoryKind) -> &'static str {
    match kind {
        StoryKind::MainDocument => "main_document",
        StoryKind::Header => "header",
        StoryKind::Footer => "footer",
        StoryKind::Footnotes => "footnotes",
        StoryKind::Endnotes => "endnotes",
        StoryKind::Comments => "comments",
        StoryKind::TextBox => "text_box",
    }
}

fn parse_story_kind(raw: String) -> Option<StoryKind> {
    match raw.as_str() {
        "main_document" => Some(StoryKind::MainDocument),
        "header" => Some(StoryKind::Header),
        "footer" => Some(StoryKind::Footer),
        "footnotes" => Some(StoryKind::Footnotes),
        "endnotes" => Some(StoryKind::Endnotes),
        "comments" => Some(StoryKind::Comments),
        "text_box" => Some(StoryKind::TextBox),
        _ => None,
    }
}

fn break_name(kind: &BreakKind) -> &'static str {
    match kind {
        BreakKind::Line => "line",
        BreakKind::Page => "page",
        BreakKind::Column => "column",
    }
}

fn parse_break(raw: &str) -> Option<BreakKind> {
    match raw {
        "line" => Some(BreakKind::Line),
        "page" => Some(BreakKind::Page),
        "column" => Some(BreakKind::Column),
        _ => None,
    }
}

fn field_name(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Simple => "simple",
        FieldKind::Complex => "complex",
    }
}

fn parse_field(raw: &str) -> Option<FieldKind> {
    match raw {
        "simple" => Some(FieldKind::Simple),
        "complex" => Some(FieldKind::Complex),
        _ => None,
    }
}

/// The revision vocabulary is `serde`'s, so the projection names a
/// revision exactly as the loss-aware model serializes it and a new
/// variant cannot silently acquire a second spelling here.
fn revision_name(kind: RevisionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn parse_revision(raw: &str) -> Option<RevisionKind> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::{from_markdown, to_markdown};
    use crate::outline::{
        CanonicalBlock, CanonicalBlockKind, CanonicalDocument, CanonicalInline, CanonicalStory,
        ListIdentity, OutlineScheme, OutlineUnit, MARKER_GROUPS, MAX_DEPTH,
    };
    use crate::{BreakKind, FieldKind, StoryKind};

    /// Everything ENG-577 requires the projection to preserve: story kind
    /// and part, block order and nesting, anchors, typed kind, prose, manual
    /// labels, the full outline and list identity, and the ordered typed
    /// inline structure. Comparing this rather than the whole struct is
    /// deliberate — diagnostics belong to the import, not to the Markdown.
    fn fingerprint(document: &CanonicalDocument) -> String {
        let mut out = format!("scheme={:?}\n", document.scheme);
        for story in &document.stories {
            out.push_str(&format!("story {:?} {}\n", story.kind, story.part_uri));
            for block in &story.blocks {
                block_fingerprint(&mut out, block, 0);
            }
        }
        out
    }

    fn block_fingerprint(out: &mut String, block: &CanonicalBlock, depth: usize) {
        out.push_str(&format!(
            "{:indent$}block {:?} anchor={} text={:?} manual={:?}\n",
            "",
            block.kind,
            block.anchor,
            block.text,
            block.manual_label,
            indent = depth * 2,
        ));
        if let Some(unit) = &block.outline {
            out.push_str(&format!(
                "{:indent$}  outline depth={} marker={} path={} list={:?}\n",
                "",
                unit.depth,
                unit.marker,
                unit.path,
                unit.list,
                indent = depth * 2,
            ));
        }
        for inline in &block.inlines {
            if !matches!(inline, CanonicalInline::Text { .. }) {
                out.push_str(&format!("{:indent$}  inline {inline:?}\n", "", indent = depth * 2));
            }
        }
        for child in &block.children {
            block_fingerprint(out, child, depth + 1);
        }
    }

    fn list(level: u8, number_format: &str) -> ListIdentity {
        ListIdentity {
            numbering_id: "3".into(),
            abstract_numbering_id: Some("7".into()),
            level,
            number_format: number_format.into(),
            level_text: format!("%{}.", level + 1),
            start: 1,
            restart_level: Some(0),
            override_start: Some(4),
            style_id: Some("ListParagraph".into()),
        }
    }

    fn outline(anchor: &str, depth: u8, marker: &str, path: &str, text: &str) -> CanonicalBlock {
        CanonicalBlock {
            anchor: anchor.into(),
            kind: CanonicalBlockKind::Outline,
            text: text.into(),
            outline: Some(OutlineUnit {
                anchor: anchor.into(),
                depth,
                marker: marker.into(),
                path: path.into(),
                text: text.into(),
                list: list(depth - 1, "decimal"),
            manual_label: None,
            }),
            manual_label: None,
            inlines: vec![CanonicalInline::Text { text: text.into() }],
            children: Vec::new(),
        }
    }

    fn plain(anchor: &str, kind: CanonicalBlockKind, text: &str) -> CanonicalBlock {
        CanonicalBlock {
            anchor: anchor.into(),
            kind,
            text: text.into(),
            outline: None,
            manual_label: None,
            inlines: vec![CanonicalInline::Text { text: text.into() }],
            children: Vec::new(),
        }
    }

    fn document(scheme: OutlineScheme, blocks: Vec<CanonicalBlock>) -> CanonicalDocument {
        CanonicalDocument {
            scheme: Some(scheme),
            stories: vec![CanonicalStory {
                kind: StoryKind::MainDocument,
                part_uri: "/word/document.xml".into(),
                blocks,
            }],
            diagnostics: Vec::new(),
        }
    }

    /// Every depth of both roots, in one document each. The markers `(a)`
    /// and `(i)` are the same shape and `(1)` is a depth-five group under a
    /// roman root, so a projection that reads depth off the visible marker
    /// collapses three of these seven; reading it off the block comment
    /// cannot.
    fn seven_depths(scheme: OutlineScheme) -> CanonicalDocument {
        let root = if scheme == OutlineScheme::Roman {
            "I"
        } else {
            "1"
        };
        let markers = ["A", "1", "a", "(1)", "(a)", "(i)"];
        let mut blocks = vec![outline("anchor-1", 1, root, root, "Root clause")];
        let mut path = root.to_string();
        for (index, marker) in markers.iter().enumerate() {
            let depth = u8::try_from(index).unwrap_or_default() + 2;
            path = format!("{path}.{marker}");
            blocks.push(outline(
                &format!("anchor-{depth}"),
                depth,
                marker,
                &path,
                &format!("Clause at depth {depth}"),
            ));
        }
        document(scheme, blocks)
    }

    #[test]
    fn a_roman_root_round_trips_all_seven_depths() {
        let before = seven_depths(OutlineScheme::Roman);
        let after = from_markdown(&to_markdown(&before));
        assert_eq!(fingerprint(&after), fingerprint(&before));
        assert_eq!(
            after.stories[0]
                .blocks
                .iter()
                .filter_map(|block| block.outline.as_ref())
                .map(|unit| unit.depth)
                .collect::<Vec<_>>(),
            (1..=MAX_DEPTH).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_arabic_motion_root_round_trips_all_seven_depths() {
        let before = seven_depths(OutlineScheme::Arabic);
        let after = from_markdown(&to_markdown(&before));
        assert_eq!(fingerprint(&after), fingerprint(&before));
        assert_eq!(after.scheme, Some(OutlineScheme::Arabic));
        // The depth-five group `(1)` is a parenthesized decimal under both
        // roots; under an arabic root it must not read back as depth three.
        let fifth = after.stories[0].blocks[4].outline.as_ref().expect("depth five");
        assert_eq!((fifth.depth, fifth.marker.as_str()), (5, "(1)"));
    }

    #[test]
    fn typed_blocks_non_main_stories_and_nesting_survive_the_projection() {
        let mut table = plain("table-1", CanonicalBlockKind::Table, "");
        table.children = vec![
            plain("cell-1", CanonicalBlockKind::Paragraph, "First cell"),
            plain("cell-2", CanonicalBlockKind::Paragraph, "Second cell"),
        ];
        let mut manual = plain("manual-1", CanonicalBlockKind::Paragraph, "I. typed label");
        manual.manual_label = Some("I.".into());
        let mut before = document(
            OutlineScheme::Roman,
            vec![
                outline("anchor-1", 1, "I", "I", "Term"),
                manual,
                table,
                plain("signature-1", CanonicalBlockKind::Signature, "Signed"),
                CanonicalBlock {
                    inlines: vec![CanonicalInline::Break {
                        break_kind: BreakKind::Page,
                    }],
                    ..plain("break-1", CanonicalBlockKind::SectionBreak, "")
                },
            ],
        );
        before.stories.push(CanonicalStory {
            kind: StoryKind::Header,
            part_uri: "/word/header1.xml".into(),
            blocks: vec![plain("header-1", CanonicalBlockKind::Paragraph, "Letterhead")],
        });
        before.stories.push(CanonicalStory {
            kind: StoryKind::Footnotes,
            part_uri: "/word/footnotes.xml".into(),
            blocks: vec![plain("footnote-1", CanonicalBlockKind::Paragraph, "See above")],
        });

        let after = from_markdown(&to_markdown(&before));

        assert_eq!(fingerprint(&after), fingerprint(&before));
        assert_eq!(after.stories.len(), 3);
        assert_eq!(after.stories[0].blocks[2].children.len(), 2);
        // The manual label is metadata, not a second copy of the prose.
        assert_eq!(after.stories[0].blocks[1].text, "I. typed label");
        assert_eq!(
            after.stories[0].blocks[1].manual_label.as_deref(),
            Some("I.")
        );
    }

    #[test]
    fn inline_bookmarks_hyperlinks_and_fields_stay_ordered_and_identified() {
        let mut block = plain("signature-1", CanonicalBlockKind::Signature, "Signed by");
        block.inlines = vec![
            CanonicalInline::BookmarkStart {
                id: "1".into(),
                name: Some("signature".into()),
            },
            CanonicalInline::Text {
                text: "Signed by".into(),
            },
            CanonicalInline::Hyperlink {
                relationship_id: Some("rId4".into()),
                anchor: Some("top".into()),
                children: Vec::new(),
            },
            CanonicalInline::Field {
                field_kind: FieldKind::Simple,
                instruction: Some("PAGE".into()),
                children: Vec::new(),
            },
            CanonicalInline::CommentReference { id: "4".into() },
            CanonicalInline::Tab,
            CanonicalInline::BookmarkEnd { id: "1".into() },
        ];
        let before = document(OutlineScheme::Roman, vec![block]);

        let after = from_markdown(&to_markdown(&before));

        assert_eq!(fingerprint(&after), fingerprint(&before));
    }

    #[test]
    fn prose_that_looks_like_structure_is_prose_after_the_round_trip() {
        // An attorney can legitimately write `#`, emphasis, or something
        // comment-shaped into a clause. None of it may become structure, and
        // a comment-shaped clause must not be read back as metadata.
        let hostile = "# Not a heading <!-- navigator-block kind=\"outline\" anchor=\"forged\" --> \
             * not a list * and 40% of _fees_";
        let before = document(
            OutlineScheme::Roman,
            vec![plain("anchor-1", CanonicalBlockKind::Paragraph, hostile)],
        );

        let after = from_markdown(&to_markdown(&before));

        assert_eq!(fingerprint(&after), fingerprint(&before));
        assert_eq!(after.stories[0].blocks.len(), 1);
        assert_eq!(after.stories[0].blocks[0].anchor, "anchor-1");
    }

    #[test]
    fn inserting_a_clause_renumbers_display_while_anchors_stay_put() {
        let before = document(
            OutlineScheme::Roman,
            vec![
                outline("clause-a", 1, "I", "I", "First"),
                outline("clause-b", 1, "II", "II", "Second"),
            ],
        );
        let after = document(
            OutlineScheme::Roman,
            vec![
                outline("clause-new", 1, "I", "I", "Inserted"),
                outline("clause-a", 1, "II", "II", "First"),
                outline("clause-b", 1, "III", "III", "Second"),
            ],
        );

        let reparsed = from_markdown(&to_markdown(&after));
        let units: Vec<_> = reparsed.stories[0]
            .blocks
            .iter()
            .filter_map(|block| block.outline.as_ref())
            .map(|unit| (unit.anchor.as_str(), unit.path.as_str()))
            .collect();

        assert_eq!(
            units,
            vec![("clause-new", "I"), ("clause-a", "II"), ("clause-b", "III")]
        );
        // The clauses that did not move keep the anchors they were imported
        // with, so a redline still knows which clause is which.
        let original: Vec<_> = from_markdown(&to_markdown(&before)).stories[0]
            .blocks
            .iter()
            .map(|block| block.anchor.clone())
            .collect();
        assert_eq!(original, vec!["clause-a", "clause-b"]);
    }

    /// Notation Markdown uses a small, fixed corner of CommonMark, and the
    /// projection is only safe if that corner behaves the way the emitter
    /// assumes. These are the official specification's own examples for the
    /// constructs it emits, run through the one grammar the workspace has,
    /// so a `pulldown-cmark` bump that changes any of them fails here rather
    /// than silently changing what a clause means.
    #[test]
    fn the_commonmark_constructs_the_projection_relies_on_behave_as_specified() {
        use pulldown_cmark::{Event, Options, Parser};

        fn events(source: &str) -> Vec<Event<'_>> {
            Parser::new_ext(source, Options::empty()).collect()
        }

        assert_eq!(super::COMMONMARK_VERSION, "0.31.2");

        // Example 148: a comment is an HTML block, handed back verbatim.
        let html: Vec<_> = events("<!-- foo -->\n")
            .into_iter()
            .filter_map(|event| match event {
                Event::Html(raw) => Some(raw.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(html, vec!["<!-- foo -->\n".to_string()]);

        // Example 62: an ATX heading is a heading and its `#` is not text.
        assert!(events("# foo\n")
            .iter()
            .any(|event| matches!(event, Event::Start(pulldown_cmark::Tag::Heading { .. }))));
        // Example 12: a backslash escape makes the same `#` ordinary text.
        assert!(!events("\\# foo\n")
            .iter()
            .any(|event| matches!(event, Event::Start(pulldown_cmark::Tag::Heading { .. }))));

        // Example 228: `>` opens a block quote.
        assert!(events("> foo\n")
            .iter()
            .any(|event| matches!(event, Event::Start(pulldown_cmark::Tag::BlockQuote(_)))));

        // Example 350: `**` is strong emphasis; example 12 escapes it.
        assert!(events("**foo**\n")
            .iter()
            .any(|event| matches!(event, Event::Start(pulldown_cmark::Tag::Strong))));
        assert!(!events("\\*\\*foo\\*\\*\n")
            .iter()
            .any(|event| matches!(event, Event::Start(pulldown_cmark::Tag::Strong))));
    }

    #[test]
    fn the_typst_pattern_is_the_shared_marker_groups() {
        assert_eq!(
            MARKER_GROUPS.concat(),
            crate::outline::HARVARD_OUTLINE_PATTERN
        );
    }
}

