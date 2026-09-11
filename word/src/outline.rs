//! The presentation-neutral Harvard outline model shared by Word import,
//! Markdown narration, and PDF output.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Block, BreakKind, Inline, NumberingDefinition, RevisionKind, Story, StoryKind};

/// Navigator supports the seven established Harvard outline depths.
pub const MAX_DEPTH: u8 = 7;

/// The root numbering scheme used by a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlineScheme {
    Roman,
    Arabic,
}

/// The seven marker groups used by the agreement outline, most significant
/// first. This is the one vocabulary: the Typst pattern below is these
/// groups concatenated, and `pdf` builds its own numbering table from this
/// array rather than restating the literals.
pub const MARKER_GROUPS: [&str; MAX_DEPTH as usize] =
    ["I.", "A.", "1.", "a.", "(1)", "(a)", "(i)"];

/// The seven marker groups as one Typst `numbering()` pattern.
pub const HARVARD_OUTLINE_PATTERN: &str = "I.A.1.a.(1)(a)(i)";

/// The OOXML `w:numFmt` a Harvard level must declare at each depth. A
/// depth-one root is the one choice in the scheme — upper roman for
/// contracts and letters, decimal for motion practice — so it carries two
/// accepted formats and every deeper level carries exactly one.
const LEVEL_FORMATS: [&[&str]; MAX_DEPTH as usize] = [
    &["upperRoman", "upper_roman", "decimal", "decimalZero", "decimal_zero"],
    &["upperLetter", "upper_letter"],
    &["decimal", "decimalZero", "decimal_zero"],
    &["lowerLetter", "lower_letter"],
    &["decimal", "decimalZero", "decimal_zero"],
    &["lowerLetter", "lower_letter"],
    &["lowerRoman", "lower_roman"],
];

/// The OOXML `w:lvlText` a Harvard level must declare at each depth: its
/// own placeholder in its own marker group. A cumulative `%1.%2.` or a
/// `%3)` is a different outline, and rendering it as `1.` would invent a
/// marker the source document never displayed.
fn expected_level_text(depth: u8) -> String {
    let group = MARKER_GROUPS[usize::from(depth) - 1];
    if group.starts_with('(') {
        format!("(%{depth})")
    } else {
        format!("%{depth}.")
    }
}

/// Whether a resolved level can be displayed with this depth's marker
/// group without inventing anything. A level that cannot is an ambiguity
/// for an attorney to resolve, not a marker for Navigator to guess.
fn level_matches_depth(level: &NumberingLevel, depth: u8) -> bool {
    let index = usize::from(depth) - 1;
    LEVEL_FORMATS[index].contains(&level.number_format.as_str())
        && level.level_text == expected_level_text(depth)
}

/// A resolved numbering level from an OOXML abstract numbering definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberingLevel {
    pub level: u8,
    pub number_format: String,
    pub level_text: String,
    pub start: u32,
    pub restart_level: Option<u8>,
    pub style_id: Option<String>,
    pub override_start: Option<u32>,
}

/// The list identity retained on every imported outline unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListIdentity {
    pub numbering_id: String,
    pub abstract_numbering_id: Option<String>,
    pub level: u8,
    pub number_format: String,
    pub level_text: String,
    pub start: u32,
    pub restart_level: Option<u8>,
    pub override_start: Option<u32>,
    pub style_id: Option<String>,
}

/// One numbered outline unit with its source identity and computed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineUnit {
    pub anchor: String,
    pub depth: u8,
    /// The marker displayed for this unit, without a trailing delimiter.
    pub marker: String,
    /// The cumulative path, e.g. `II.B.1`.
    pub path: String,
    pub text: String,
    pub list: ListIdentity,
    /// A marker-like prefix typed into ordinary text rather than supplied by
    /// Word numbering. It is never interpreted as an outline unit.
    pub manual_label: Option<String>,
}

/// A canonical document block. `outline` is populated only for a genuine
/// Word-numbered outline paragraph; all other block kinds remain explicit and
/// ordered instead of being flattened into prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBlock {
    pub anchor: String,
    pub kind: CanonicalBlockKind,
    pub text: String,
    pub outline: Option<OutlineUnit>,
    pub manual_label: Option<String>,
    pub inlines: Vec<CanonicalInline>,
    pub children: Vec<CanonicalBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalBlockKind {
    Outline,
    Paragraph,
    Table,
    Signature,
    SectionBreak,
}

/// Typed inline structure retained by the canonical boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalInline {
    Text {
        text: String,
    },
    Tab,
    Break {
        break_kind: BreakKind,
    },
    BookmarkStart {
        id: String,
        name: Option<String>,
    },
    BookmarkEnd {
        id: String,
    },
    Hyperlink {
        relationship_id: Option<String>,
        anchor: Option<String>,
        children: Vec<CanonicalInline>,
    },
    Field {
        field_kind: crate::FieldKind,
        instruction: Option<String>,
        children: Vec<CanonicalInline>,
    },
    Revision {
        revision_kind: RevisionKind,
        id: Option<String>,
        anchor: String,
        children: Vec<CanonicalInline>,
    },
    CommentReference {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalStory {
    pub kind: StoryKind,
    pub part_uri: String,
    pub blocks: Vec<CanonicalBlock>,
}

/// The complete canonical model returned by the Word import boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalDocument {
    pub scheme: Option<OutlineScheme>,
    pub stories: Vec<CanonicalStory>,
    pub diagnostics: Vec<crate::Diagnostic>,
}

impl CanonicalDocument {
    /// Emit the governed, editable Markdown representation of this import.
    /// The visible prose is ordinary Markdown; the structural identity that
    /// Markdown cannot express rides beside it in `navigator-*` comments.
    /// See [`crate::notation`].
    #[must_use]
    pub fn to_markdown(&self) -> String {
        crate::notation::to_markdown(self)
    }

    /// Read a governed Markdown projection back into the canonical model
    /// through the workspace's one CommonMark grammar.
    #[must_use]
    pub fn from_markdown(source: &str) -> Self {
        crate::notation::from_markdown(source)
    }
}

/// Convert the loss-aware Word model into the canonical outline model.
#[must_use]
pub fn from_stories(
    stories: &[Story],
    numbering: &[NumberingDefinition],
    inherited_diagnostics: &[crate::Diagnostic],
) -> CanonicalDocument {
    let definitions: HashMap<_, _> = numbering
        .iter()
        .map(|definition| (definition.numbering_id.as_str(), definition))
        .collect();
    let mut diagnostics = inherited_diagnostics.to_vec();
    let mut scheme = None;
    let mut canonical_stories = Vec::with_capacity(stories.len());

    for story in stories {
        let mut counters = HashMap::new();
        // One ordinal sequence per part: a table cell restarts its own child
        // index, so a per-container ordinal would hand two distinct blocks
        // the same fallback anchor.
        let mut ordinal = 0_usize;
        let blocks = canonical_blocks(
            &story.blocks,
            &story.part_uri,
            &definitions,
            &mut counters,
            &mut ordinal,
            &mut scheme,
            &mut diagnostics,
        );
        canonical_stories.push(CanonicalStory {
            kind: story.kind.clone(),
            part_uri: story.part_uri.clone(),
            blocks,
        });
    }

    CanonicalDocument {
        scheme,
        stories: canonical_stories,
        diagnostics,
    }
}

fn canonical_blocks(
    blocks: &[Block],
    part_uri: &str,
    definitions: &HashMap<&str, &NumberingDefinition>,
    counters: &mut HashMap<String, Vec<u32>>,
    ordinals: &mut usize,
    scheme: &mut Option<OutlineScheme>,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Vec<CanonicalBlock> {
    blocks
        .iter()
        .map(|block| {
            let ordinal = *ordinals;
            *ordinals += 1;
            match block {
            Block::Paragraph(paragraph) => canonical_paragraph(
                paragraph,
                part_uri,
                ordinal,
                definitions,
                counters,
                scheme,
                diagnostics,
            ),
            Block::Table(table) => {
                let anchor = if table.anchor.is_empty() {
                    crate::anchor::block_anchor(part_uri, "table", ordinal)
                } else {
                    table.anchor.clone()
                };
                if table.anchor.is_empty() {
                    diagnostics.push(crate::Diagnostic::missing_source_anchor(&anchor));
                }
                let children = table
                    .rows
                    .iter()
                    .flat_map(|row| row.cells.iter())
                    .flat_map(|cell| {
                        canonical_blocks(
                            &cell.blocks,
                            part_uri,
                            definitions,
                            counters,
                            ordinals,
                            scheme,
                            diagnostics,
                        )
                    })
                    .collect();
                CanonicalBlock {
                    anchor,
                    kind: CanonicalBlockKind::Table,
                    text: table_text(table),
                    outline: None,
                    manual_label: None,
                    inlines: Vec::new(),
                    children,
                }
            }
            Block::SectionBreak { break_kind, anchor } => {
                let resolved_anchor = if anchor.is_empty() {
                    crate::anchor::block_anchor(part_uri, "section-break", ordinal)
                } else {
                    anchor.clone()
                };
                if anchor.is_empty() {
                    diagnostics.push(crate::Diagnostic::missing_source_anchor(&resolved_anchor));
                }
                CanonicalBlock {
                    anchor: resolved_anchor,
                    kind: CanonicalBlockKind::SectionBreak,
                    text: String::new(),
                    outline: None,
                    manual_label: None,
                    inlines: vec![CanonicalInline::Break {
                        break_kind: break_kind.clone(),
                    }],
                    children: Vec::new(),
                }
            }
        }})
        .collect()
}

#[allow(clippy::too_many_lines)]
fn canonical_paragraph(
    paragraph: &crate::Paragraph,
    part_uri: &str,
    ordinal: usize,
    definitions: &HashMap<&str, &NumberingDefinition>,
    counters: &mut HashMap<String, Vec<u32>>,
    scheme: &mut Option<OutlineScheme>,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> CanonicalBlock {
    let anchor = if paragraph.anchor.is_empty() {
        crate::anchor::block_anchor(part_uri, "paragraph", ordinal)
    } else {
        paragraph.anchor.clone()
    };
    if paragraph.anchor.is_empty() {
        diagnostics.push(crate::Diagnostic::missing_source_anchor(&anchor));
    }
    let text = inline_text(&paragraph.nodes);
    let inlines = paragraph
        .nodes
        .iter()
        .map(canonical_inline)
        .collect::<Vec<_>>();
    let manual_label = paragraph
        .numbering
        .is_none()
        .then(|| marker_like_prefix(&text))
        .flatten();
    if manual_label.is_some() {
        diagnostics.push(crate::Diagnostic::manual_outline_label(&anchor));
    }

    let Some(identity) = &paragraph.numbering else {
        return CanonicalBlock {
            anchor,
            kind: if is_signature_style(paragraph.style_id.as_deref()) {
                CanonicalBlockKind::Signature
            } else {
                CanonicalBlockKind::Paragraph
            },
            text,
            outline: None,
            manual_label,
            inlines,
            children: Vec::new(),
        };
    };
    let Some(level) = identity
        .level
        .as_deref()
        .and_then(|level| level.parse::<u8>().ok())
    else {
        diagnostics.push(crate::Diagnostic::unsupported_numbering(&anchor));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    };
    let depth = level.saturating_add(1);
    if depth > MAX_DEPTH {
        diagnostics.push(crate::Diagnostic::depth_overflow(&anchor, depth));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    }
    let Some(definition) = definitions.get(identity.numbering_id.as_str()) else {
        diagnostics.push(crate::Diagnostic::unsupported_numbering(&anchor));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    };
    // No fallback to level zero: a level this definition does not declare is
    // an unresolved list, and borrowing another level's format would invent
    // a marker the source document never displayed.
    let Some(level_definition) = definition
        .level_definitions
        .iter()
        .find(|definition| definition.level == level)
    else {
        diagnostics.push(crate::Diagnostic::unsupported_numbering(&anchor));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    };
    if !level_matches_depth(level_definition, depth) {
        diagnostics.push(crate::Diagnostic::unsupported_numbering(&anchor));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    }

    let detected_scheme = match level_definition.number_format.as_str() {
        "upper_roman" | "upperRoman" => Some(OutlineScheme::Roman),
        "decimal" | "decimal_zero" | "decimalZero" => Some(OutlineScheme::Arabic),
        _ => None,
    };
    if depth == 1 {
        if let Some(detected) = detected_scheme {
            if let Some(existing) = *scheme {
                if existing != detected {
                    diagnostics.push(crate::Diagnostic::ambiguous_outline(&anchor));
                }
            } else {
                *scheme = Some(detected);
            }
        } else {
            diagnostics.push(crate::Diagnostic::ambiguous_outline(&anchor));
        }
    }
    let display_scheme = if depth == 1 {
        detected_scheme.or(*scheme)
    } else {
        *scheme
    };
    let counters = counters
        .entry(identity.numbering_id.clone())
        .or_insert_with(|| vec![0_u32; usize::from(MAX_DEPTH)]);
    // A depth reached without its ancestors has no cumulative path, and a
    // path with a hole in it is a guess. Stop at the diagnostic and keep the
    // paragraph as anchored text instead.
    if depth > 1 && counters[..usize::from(depth - 1)].contains(&0) {
        diagnostics.push(crate::Diagnostic::skipped_outline_level(&anchor, depth));
        return paragraph_block(anchor, text, inlines, paragraph.style_id.as_deref());
    }
    if level_definition.override_start.is_some() && counters[usize::from(depth - 1)] == 0 {
        diagnostics.push(crate::Diagnostic::list_restart(&anchor));
    }
    let index = usize::from(depth - 1);
    if counters[index] == 0 {
        counters[index] = level_definition
            .override_start
            .unwrap_or(level_definition.start)
            .saturating_sub(1);
    }
    counters[index] = counters[index].saturating_add(1);
    for count in counters.iter_mut().skip(index + 1) {
        *count = 0;
    }
    let marker = marker_for(display_scheme, depth, counters[index]);
    let path = counters
        .iter()
        .take(index + 1)
        .enumerate()
        .map(|(level, count)| {
            marker_for(
                display_scheme,
                u8::try_from(level + 1).unwrap_or(MAX_DEPTH),
                *count,
            )
        })
        .collect::<Vec<_>>()
        .join(".");
    let list = ListIdentity {
        numbering_id: identity.numbering_id.clone(),
        abstract_numbering_id: definition.abstract_numbering_id.clone(),
        level,
        number_format: level_definition.number_format.clone(),
        level_text: level_definition.level_text.clone(),
        start: level_definition.start,
        restart_level: level_definition.restart_level,
        override_start: level_definition.override_start,
        style_id: level_definition.style_id.clone(),
    };
    let unit = OutlineUnit {
        anchor: anchor.clone(),
        depth,
        marker,
        path,
        text: text.clone(),
        list,
        manual_label: None,
    };
    CanonicalBlock {
        anchor,
        kind: CanonicalBlockKind::Outline,
        text,
        outline: Some(unit),
        manual_label: None,
        inlines,
        children: Vec::new(),
    }
}

fn paragraph_block(
    anchor: String,
    text: String,
    inlines: Vec<CanonicalInline>,
    style_id: Option<&str>,
) -> CanonicalBlock {
    CanonicalBlock {
        anchor,
        kind: if is_signature_style(style_id) {
            CanonicalBlockKind::Signature
        } else {
            CanonicalBlockKind::Paragraph
        },
        text,
        outline: None,
        manual_label: None,
        inlines,
        children: Vec::new(),
    }
}

#[allow(clippy::match_same_arms)]
fn marker_for(scheme: Option<OutlineScheme>, depth: u8, value: u32) -> String {
    match depth {
        1 if scheme == Some(OutlineScheme::Roman) => roman(value),
        1 => value.to_string(),
        3 => value.to_string(),
        2 => alpha(value, false),
        4 => alpha(value, false).to_ascii_lowercase(),
        5 => format!("({value})"),
        6 => alpha(value, true).to_ascii_lowercase(),
        7 => format!("({})", roman(value).to_ascii_lowercase()),
        _ => value.to_string(),
    }
}

fn roman(mut value: u32) -> String {
    let mut output = String::new();
    for (number, glyph) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= number {
            output.push_str(glyph);
            value -= number;
        }
    }
    output
}

fn alpha(mut value: u32, parenthesized: bool) -> String {
    let mut output = String::new();
    while value > 0 {
        value -= 1;
        output.insert(
            0,
            char::from_u32(u32::from(b'A') + value % 26).unwrap_or('A'),
        );
        value /= 26;
    }
    if parenthesized {
        format!("({output})")
    } else {
        output
    }
}

fn inline_text(nodes: &[Inline]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            Inline::Text {
                text: value,
                revision,
                ..
            } => {
                if revision.is_none_or(RevisionKind::is_accepted) {
                    text.push_str(value);
                }
            }
            Inline::Tab { .. } => text.push('\t'),
            Inline::Break { .. } => text.push('\n'),
            Inline::Hyperlink { children, .. } | Inline::Field { children, .. } => {
                text.push_str(&inline_text(children));
            }
            Inline::Revision { revision, children } if revision.kind.is_accepted() => {
                text.push_str(&inline_text(children));
            }
            Inline::BookmarkStart { .. }
            | Inline::BookmarkEnd { .. }
            | Inline::CommentRangeStart { .. }
            | Inline::CommentRangeEnd { .. }
            | Inline::CommentReference { .. }
            | Inline::Revision { .. } => {}
        }
    }
    text.trim().to_string()
}

fn canonical_inline(node: &Inline) -> CanonicalInline {
    match node {
        Inline::Text { text, .. } => CanonicalInline::Text { text: text.clone() },
        Inline::Tab { .. } => CanonicalInline::Tab,
        Inline::Break { break_kind } => CanonicalInline::Break {
            break_kind: break_kind.clone(),
        },
        Inline::BookmarkStart { id, name } => CanonicalInline::BookmarkStart {
            id: id.clone(),
            name: name.clone(),
        },
        Inline::BookmarkEnd { id } => CanonicalInline::BookmarkEnd { id: id.clone() },
        Inline::Hyperlink {
            relationship_id,
            anchor,
            children,
        } => CanonicalInline::Hyperlink {
            relationship_id: relationship_id.clone(),
            anchor: anchor.clone(),
            children: children.iter().map(canonical_inline).collect(),
        },
        Inline::Field {
            field_kind,
            instruction,
            children,
        } => CanonicalInline::Field {
            field_kind: field_kind.clone(),
            instruction: instruction.clone(),
            children: children.iter().map(canonical_inline).collect(),
        },
        Inline::Revision { revision, children } => CanonicalInline::Revision {
            revision_kind: revision.kind,
            id: revision.id.clone(),
            anchor: revision.anchor.clone(),
            children: children.iter().map(canonical_inline).collect(),
        },
        Inline::CommentRangeStart { id }
        | Inline::CommentRangeEnd { id }
        | Inline::CommentReference { id } => CanonicalInline::CommentReference { id: id.clone() },
    }
}

fn table_text(table: &crate::Table) -> String {
    table
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .flat_map(|cell| cell.blocks.iter())
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(inline_text(&paragraph.nodes)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn is_signature_style(style_id: Option<&str>) -> bool {
    style_id.is_some_and(|style| {
        let style = style.to_ascii_lowercase();
        style.contains("signature") || style.contains("signer")
    })
}

fn marker_like_prefix(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let end = trimmed.find(char::is_whitespace)?;
    let prefix = trimmed[..end].trim_end_matches('.');
    let marker = prefix.trim_matches(['(', ')']);
    if marker.is_empty()
        || !(marker.chars().all(|c| c.is_ascii_digit())
            || marker
                .chars()
                .all(|c| c.is_ascii_uppercase() && c.is_ascii_alphabetic())
            || marker
                .chars()
                .all(|c| c.is_ascii_lowercase() && c.is_ascii_alphabetic()))
    {
        return None;
    }
    Some(trimmed[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::{CanonicalBlockKind, NumberingLevel, OutlineScheme};
    use crate::{
        Block, DocumentModel, Inline, NumberingDefinition, NumberingIdentity, Paragraph, Story,
        StoryKind, Table, TableCell, TableRow,
    };

    fn level(level: u8, number_format: &str, level_text: &str) -> NumberingLevel {
        NumberingLevel {
            level,
            number_format: number_format.into(),
            level_text: level_text.into(),
            start: 1,
            restart_level: None,
            style_id: None,
            override_start: None,
        }
    }

    fn paragraph(anchor: &str, level: Option<u8>, text: &str) -> Block {
        Block::Paragraph(Paragraph {
            anchor: anchor.into(),
            style_id: None,
            numbering: level.map(|level| NumberingIdentity {
                numbering_id: "contract".into(),
                level: Some(level.to_string()),
            }),
            nodes: vec![Inline::Text {
                text: text.into(),
                style_id: None,
                revision: None,
            }],
            revisions: Vec::new(),
        })
    }

    fn model(blocks: Vec<Block>, levels: Vec<NumberingLevel>) -> DocumentModel {
        DocumentModel {
            protocol_version: crate::PROTOCOL_VERSION,
            package: crate::PackageInventory::default(),
            stories: vec![Story {
                kind: StoryKind::MainDocument,
                part_uri: "/word/document.xml".into(),
                blocks,
            }],
            styles: Vec::new(),
            numbering: vec![NumberingDefinition {
                numbering_id: "contract".into(),
                abstract_numbering_id: Some("7".into()),
                levels: Vec::new(),
                level_definitions: levels,
            }],
            comments: Vec::new(),
            diagnostics: Vec::new(),
            revision_nodes: Vec::new(),
        }
    }

    #[test]
    fn resolves_roman_root_and_all_seven_depths_to_one_vocabulary() {
        let formats = [
            ("upperRoman", "%1."),
            ("upperLetter", "%2."),
            ("decimal", "%3."),
            ("lowerLetter", "%4."),
            ("decimal", "(%5)"),
            ("lowerLetter", "(%6)"),
            ("lowerRoman", "(%7)"),
        ];
        let blocks = (0_u8..7)
            .map(|depth| paragraph(&format!("p{depth}"), Some(depth), "clause"))
            .collect();
        let document = model(
            blocks,
            formats
                .into_iter()
                .enumerate()
                .map(|(depth, (format, text))| {
                    level(u8::try_from(depth).unwrap_or_default(), format, text)
                })
                .collect(),
        );

        let canonical = document.canonical_outline();
        let story = &canonical.stories[0];
        let units: Vec<_> = story
            .blocks
            .iter()
            .filter_map(|block| block.outline.as_ref())
            .collect();
        assert_eq!(canonical.scheme, Some(OutlineScheme::Roman));
        assert_eq!(units.len(), 7);
        assert_eq!(
            units.iter().map(|unit| unit.depth).collect::<Vec<_>>(),
            (1_u8..=7).collect::<Vec<_>>()
        );
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.marker.as_str())
                .collect::<Vec<_>>(),
            vec!["I", "A", "1", "a", "(1)", "(a)", "(i)"]
        );
        assert_eq!(units[6].path, "I.A.1.a.(1).(a).(i)");
    }

    #[test]
    fn retains_typed_blocks_and_inline_anchors_in_document_order() {
        let mut table = Table {
            anchor: "table-1".into(),
            style_id: None,
            rows: vec![TableRow {
                cells: vec![TableCell {
                    blocks: vec![paragraph("cell-1", None, "cell")],
                }],
            }],
            revisions: Vec::new(),
        };
        table.rows[0].cells[0]
            .blocks
            .push(paragraph("cell-2", None, "cell two"));
        let mut signature = paragraph("signature-1", None, "Signed by the firm");
        if let Block::Paragraph(paragraph) = &mut signature {
            paragraph.style_id = Some("SignatureBlock".into());
            paragraph.nodes.push(Inline::BookmarkStart {
                id: "bookmark-1".into(),
                name: Some("signature".into()),
            });
            paragraph.nodes.push(Inline::Hyperlink {
                relationship_id: None,
                anchor: Some("signature".into()),
                children: vec![Inline::Field {
                    field_kind: crate::FieldKind::Simple,
                    instruction: Some("PAGE".into()),
                    children: Vec::new(),
                }],
            });
        }
        let document = model(
            vec![
                paragraph("preamble", None, "ordinary"),
                Block::Table(table),
                signature,
            ],
            vec![level(0, "upperRoman", "%1.")],
        );
        let blocks = &document.canonical_outline().stories[0].blocks;
        assert_eq!(blocks[0].kind, CanonicalBlockKind::Paragraph);
        assert_eq!(blocks[1].kind, CanonicalBlockKind::Table);
        assert_eq!(blocks[1].children.len(), 2);
        assert_eq!(blocks[2].kind, CanonicalBlockKind::Signature);
        assert!(blocks[2]
            .inlines
            .iter()
            .any(|inline| matches!(inline, super::CanonicalInline::BookmarkStart { .. })));
    }

    #[test]
    fn restarts_skipped_levels_manual_labels_and_overflow_are_diagnostics() {
        let mut restart = level(0, "decimal", "%1.");
        restart.override_start = Some(4);
        let mut overflow = paragraph("overflow", Some(7), "too deep");
        if let Block::Paragraph(paragraph) = &mut overflow {
            paragraph.numbering = Some(NumberingIdentity {
                numbering_id: "contract".into(),
                level: Some("7".into()),
            });
        }
        let document = model(
            vec![
                paragraph("manual", None, "I. typed label"),
                paragraph("skipped", Some(2), "skipped"),
                paragraph("restart", Some(0), "restart"),
                overflow,
            ],
            vec![restart, level(2, "decimal", "%3.")],
        );
        let canonical = document.canonical_outline();
        assert!(canonical.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            crate::DiagnosticCode::ManualOutlineLabel
        )));
        assert!(canonical.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            crate::DiagnosticCode::SkippedOutlineLevel
        )));
        assert!(canonical
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.code, crate::DiagnosticCode::ListRestart)));
        assert!(canonical
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.code, crate::DiagnosticCode::DepthOverflow)));
        assert_eq!(
            canonical.stories[0].blocks[0].manual_label.as_deref(),
            Some("I.")
        );
        assert_eq!(
            canonical.stories[0].blocks[3].kind,
            CanonicalBlockKind::Paragraph
        );
    }

    /// The seven Harvard levels an OOXML definition must declare for a
    /// document to import as an outline at all.
    fn harvard_levels(root: &str) -> Vec<NumberingLevel> {
        let formats = [
            root,
            "upperLetter",
            "decimal",
            "lowerLetter",
            "decimal",
            "lowerLetter",
            "lowerRoman",
        ];
        formats
            .into_iter()
            .enumerate()
            .map(|(index, format)| {
                let depth = index + 1;
                let text = if depth >= 5 {
                    format!("(%{depth})")
                } else {
                    format!("%{depth}.")
                };
                level(u8::try_from(index).unwrap_or_default(), format, &text)
            })
            .collect()
    }

    fn seven_deep(root: &str) -> crate::outline::CanonicalDocument {
        let blocks = (0_u8..7)
            .map(|depth| paragraph(&format!("p{depth}"), Some(depth), "clause"))
            .collect();
        model(blocks, harvard_levels(root)).canonical_outline()
    }

    #[test]
    fn a_roman_import_round_trips_through_notation_markdown() {
        let imported = seven_deep("upperRoman");
        let reparsed = crate::outline::CanonicalDocument::from_markdown(&imported.to_markdown());

        assert!(imported.diagnostics.is_empty(), "{:?}", imported.diagnostics);
        assert_eq!(reparsed.scheme, Some(OutlineScheme::Roman));
        assert_eq!(units(&reparsed), units(&imported));
        assert_eq!(
            units(&imported),
            vec![
                (1, "I".into(), "I".into()),
                (2, "A".into(), "I.A".into()),
                (3, "1".into(), "I.A.1".into()),
                (4, "a".into(), "I.A.1.a".into()),
                (5, "(1)".into(), "I.A.1.a.(1)".into()),
                (6, "(a)".into(), "I.A.1.a.(1).(a)".into()),
                (7, "(i)".into(), "I.A.1.a.(1).(a).(i)".into()),
            ]
        );
    }

    #[test]
    fn an_arabic_motion_import_round_trips_through_notation_markdown() {
        let imported = seven_deep("decimal");
        let reparsed = crate::outline::CanonicalDocument::from_markdown(&imported.to_markdown());

        assert_eq!(imported.scheme, Some(OutlineScheme::Arabic));
        assert_eq!(reparsed.scheme, Some(OutlineScheme::Arabic));
        assert_eq!(units(&reparsed), units(&imported));
        assert_eq!(units(&imported)[0], (1, "1".into(), "1".into()));
        // `(1)` is depth five under both roots. Reading depth off the marker
        // would call this one depth three.
        assert_eq!(units(&imported)[4], (5, "(1)".into(), "1.A.1.a.(1)".into()));
    }

    #[test]
    fn source_anchors_survive_the_markdown_round_trip_and_table_nesting() {
        let table = Table {
            anchor: String::new(),
            style_id: None,
            rows: vec![TableRow {
                cells: vec![
                    TableCell {
                        blocks: vec![paragraph("", None, "first cell")],
                    },
                    TableCell {
                        blocks: vec![paragraph("", None, "second cell")],
                    },
                ],
            }],
            revisions: Vec::new(),
        };
        let imported = model(
            vec![paragraph("", None, "preamble"), Block::Table(table)],
            harvard_levels("upperRoman"),
        )
        .canonical_outline();

        let anchors = flat_anchors(&imported);
        let reparsed = crate::outline::CanonicalDocument::from_markdown(&imported.to_markdown());

        // A cell restarts its own child index, so a per-container ordinal
        // would give two distinct blocks one anchor.
        assert_eq!(anchors.len(), 4);
        assert_eq!(
            anchors.iter().collect::<std::collections::HashSet<_>>().len(),
            4
        );
        assert_eq!(flat_anchors(&reparsed), anchors);
        assert_eq!(reparsed.stories[0].blocks[1].children.len(), 2);
    }

    #[test]
    fn an_unsupported_or_mixed_level_is_diagnosed_rather_than_given_a_marker() {
        // `%1)` is expressible OOXML and is not this outline's depth-one
        // group; `lowerLetter` at depth one is not a Harvard root at all.
        let document = model(
            vec![
                paragraph("wrong-text", Some(0), "one"),
                paragraph("wrong-format", Some(1), "two"),
                paragraph("undeclared", Some(2), "three"),
            ],
            vec![
                level(0, "upperRoman", "%1)"),
                level(1, "decimal", "%2."),
            ],
        );

        let canonical = document.canonical_outline();

        assert!(canonical.stories[0]
            .blocks
            .iter()
            .all(|block| block.outline.is_none()));
        assert_eq!(
            canonical
                .diagnostics
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic.code,
                    crate::DiagnosticCode::UnsupportedNumbering
                ))
                .count(),
            3
        );
    }

    #[test]
    fn a_skipped_level_stops_instead_of_inventing_a_path() {
        let document = model(
            vec![
                paragraph("root", Some(0), "one"),
                paragraph("skipped", Some(3), "four"),
            ],
            harvard_levels("upperRoman"),
        );

        let canonical = document.canonical_outline();

        assert_eq!(canonical.stories[0].blocks[1].outline, None);
        assert!(canonical.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            crate::DiagnosticCode::SkippedOutlineLevel
        )));
        // No path anywhere in the document is a guess.
        assert!(units(&canonical)
            .iter()
            .all(|(_, _, path)| !path.contains('?')));
    }

    fn units(document: &crate::outline::CanonicalDocument) -> Vec<(u8, String, String)> {
        document
            .stories
            .iter()
            .flat_map(|story| story.blocks.iter())
            .filter_map(|block| block.outline.as_ref())
            .map(|unit| (unit.depth, unit.marker.clone(), unit.path.clone()))
            .collect()
    }

    fn flat_anchors(document: &crate::outline::CanonicalDocument) -> Vec<String> {
        fn walk(blocks: &[crate::outline::CanonicalBlock], out: &mut Vec<String>) {
            for block in blocks {
                out.push(block.anchor.clone());
                walk(&block.children, out);
            }
        }
        let mut out = Vec::new();
        for story in &document.stories {
            walk(&story.blocks, &mut out);
        }
        out
    }

    #[test]
    fn distinct_numbering_instances_do_not_share_counters() {
        let mut document = model(
            vec![
                paragraph("roman", Some(0), "roman"),
                paragraph("new-list", Some(0), "new"),
            ],
            vec![level(0, "upperRoman", "%1.")],
        );
        if let Block::Paragraph(paragraph) = &mut document.stories[0].blocks[1] {
            paragraph.numbering.as_mut().unwrap().numbering_id = "new-list".into();
        }
        document.numbering.push(NumberingDefinition {
            numbering_id: "new-list".into(),
            abstract_numbering_id: Some("8".into()),
            levels: Vec::new(),
            level_definitions: vec![level(0, "decimal", "%1.")],
        });
        let canonical = document.canonical_outline();
        let units: Vec<_> = canonical.stories[0]
            .blocks
            .iter()
            .filter_map(|block| block.outline.as_ref())
            .collect();
        assert_eq!(units[0].marker, "I");
        assert_eq!(units[1].marker, "1");
        assert_eq!(units[1].list.numbering_id, "new-list");
    }
}
