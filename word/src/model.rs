use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The parsed package plus its immutable source asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    source: OriginalSource,
    model: DocumentModel,
}

impl Document {
    pub(crate) fn from_source(bytes: &[u8], model: DocumentModel) -> Self {
        Self {
            source: OriginalSource::new(bytes),
            model,
        }
    }

    #[must_use]
    pub fn source(&self) -> &OriginalSource {
        &self.source
    }

    #[must_use]
    pub fn original_bytes(&self) -> &[u8] {
        &self.source.bytes
    }

    #[must_use]
    pub fn model(&self) -> &DocumentModel {
        &self.model
    }

    /// Return the shared canonical outline projection of this immutable
    /// import, including diagnostics for structures needing attorney review.
    #[must_use]
    pub fn canonical_outline(&self) -> crate::outline::CanonicalDocument {
        self.model.canonical_outline()
    }

    /// Emit the editable Notation Markdown projection of the imported main
    /// story. Persistence remains the caller's governed responsibility.
    #[must_use]
    pub fn notation_markdown(&self) -> crate::notation::TrustedNotationMarkdown {
        self.canonical_outline().to_markdown()
    }

    /// The Word accepted view: insertions and move-to content are readable,
    /// deleted and move-from content is not. Revision nodes stay in `model`.
    #[must_use]
    pub fn accepted_view_text(&self) -> String {
        self.model.accepted_view_text()
    }
}

/// The original bytes are private to this value and exposed only by a shared
/// slice. Nothing in the parser mutates or replaces the source asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalSource {
    bytes: Vec<u8>,
    pub sha256: String,
    pub byte_size: u64,
}

impl OriginalSource {
    fn new(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self {
            bytes: bytes.to_vec(),
            sha256: hex_digest(&digest),
            byte_size: bytes.len() as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentModel {
    pub protocol_version: u16,
    pub package: PackageInventory,
    pub stories: Vec<Story>,
    pub styles: Vec<StyleDefinition>,
    pub numbering: Vec<NumberingDefinition>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default)]
    pub revision_nodes: Vec<RevisionNode>,
}

impl DocumentModel {
    /// Resolve the imported Word model into the shared seven-level outline
    /// representation. Diagnostics are returned alongside content so an
    /// attorney-facing caller can refuse ambiguous mappings without losing
    /// the ordered source blocks.
    #[must_use]
    pub fn canonical_outline(&self) -> crate::outline::CanonicalDocument {
        crate::outline::from_stories(&self.stories, &self.numbering, &self.diagnostics)
    }
}

impl DocumentModel {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            protocol_version: crate::PROTOCOL_VERSION,
            package: PackageInventory::default(),
            stories: Vec::new(),
            styles: Vec::new(),
            numbering: Vec::new(),
            comments: Vec::new(),
            diagnostics: Vec::new(),
            revision_nodes: Vec::new(),
        }
    }

    #[must_use]
    pub fn accepted_view_text(&self) -> String {
        let mut text = String::new();
        for story in &self.stories {
            if !text.is_empty() {
                text.push('\n');
            }
            append_blocks(&mut text, &story.blocks);
        }
        text
    }
}

fn append_blocks(out: &mut String, blocks: &[Block]) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                append_inlines(out, &paragraph.nodes);
                out.push('\n');
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for (cell_index, cell) in row.cells.iter().enumerate() {
                        if cell_index > 0 {
                            out.push('\t');
                        }
                        append_blocks(out, &cell.blocks);
                    }
                    out.push('\n');
                }
            }
            Block::SectionBreak { break_kind, .. } => {
                if matches!(break_kind, BreakKind::Page | BreakKind::Column) {
                    out.push('\n');
                }
            }
        }
    }
}

fn append_inlines(out: &mut String, nodes: &[Inline]) {
    for node in nodes {
        match node {
            Inline::Text { text, revision, .. } => {
                if revision.is_none_or(RevisionKind::is_accepted) {
                    out.push_str(text);
                }
            }
            Inline::Tab { .. } => out.push('\t'),
            Inline::Break { break_kind } => match break_kind {
                BreakKind::Line | BreakKind::Page | BreakKind::Column => out.push('\n'),
            },
            Inline::BookmarkStart { .. }
            | Inline::BookmarkEnd { .. }
            | Inline::CommentRangeStart { .. }
            | Inline::CommentRangeEnd { .. }
            | Inline::CommentReference { .. } => {}
            Inline::Hyperlink { children, .. } | Inline::Field { children, .. } => {
                append_inlines(out, children);
            }
            Inline::Revision { revision, children } => {
                if revision.kind.is_accepted() {
                    append_inlines(out, children);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PackageInventory {
    pub parts: Vec<PackagePart>,
    pub relationships: Vec<PackageRelationship>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePart {
    pub uri: String,
    pub content_type: String,
    pub relationship_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRelationship {
    pub source_uri: String,
    pub relationship_id: String,
    pub target_uri: String,
    pub relationship_type: String,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Story {
    pub kind: StoryKind,
    pub part_uri: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoryKind {
    MainDocument,
    Header,
    Footer,
    Footnotes,
    Endnotes,
    Comments,
    TextBox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    SectionBreak {
        break_kind: BreakKind,
        #[serde(default)]
        anchor: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paragraph {
    #[serde(default)]
    pub anchor: String,
    pub style_id: Option<String>,
    pub numbering: Option<NumberingIdentity>,
    pub nodes: Vec<Inline>,
    pub revisions: Vec<RevisionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    #[serde(default)]
    pub anchor: String,
    pub style_id: Option<String>,
    pub rows: Vec<TableRow>,
    pub revisions: Vec<RevisionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableCell {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Inline {
    Text {
        text: String,
        style_id: Option<String>,
        revision: Option<RevisionKind>,
    },
    Tab {
        style_id: Option<String>,
    },
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
        children: Vec<Inline>,
    },
    Field {
        field_kind: FieldKind,
        instruction: Option<String>,
        children: Vec<Inline>,
    },
    CommentRangeStart {
        id: String,
    },
    CommentRangeEnd {
        id: String,
    },
    CommentReference {
        id: String,
    },
    Revision {
        revision: RevisionNode,
        children: Vec<Inline>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakKind {
    Line,
    Page,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Simple,
    Complex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberingIdentity {
    pub numbering_id: String,
    pub level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDefinition {
    pub id: String,
    pub style_type: Option<String>,
    pub based_on: Option<String>,
    pub next_style: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberingDefinition {
    pub numbering_id: String,
    pub abstract_numbering_id: Option<String>,
    pub levels: Vec<String>,
    #[serde(default)]
    pub level_definitions: Vec<crate::outline::NumberingLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionNode {
    pub kind: RevisionKind,
    pub id: Option<String>,
    pub anchor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
    Insertion,
    Deletion,
    MoveFrom,
    MoveTo,
    ParagraphProperties,
    RunProperties,
    TableProperties,
    TableGrid,
    TableRowProperties,
    TableCellProperties,
    Numbering,
    SectionProperties,
}

impl RevisionKind {
    #[must_use]
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Insertion | Self::MoveTo)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub anchor: String,
}

impl Diagnostic {
    #[must_use]
    pub fn unsupported_revision(anchor: &str, kind: &str) -> Self {
        Self {
            code: DiagnosticCode::UnsupportedRevision,
            severity: DiagnosticSeverity::Error,
            anchor: format!("{anchor}:{kind}"),
        }
    }

    #[must_use]
    pub fn unsupported_numbering(anchor: &str) -> Self {
        Self::outline(DiagnosticCode::UnsupportedNumbering, anchor)
    }

    #[must_use]
    pub fn ambiguous_outline(anchor: &str) -> Self {
        Self::outline(DiagnosticCode::AmbiguousOutline, anchor)
    }

    #[must_use]
    pub fn depth_overflow(anchor: &str, depth: u8) -> Self {
        Self {
            code: DiagnosticCode::DepthOverflow,
            severity: DiagnosticSeverity::Error,
            anchor: format!("{anchor}:depth-{depth}"),
        }
    }

    #[must_use]
    pub fn skipped_outline_level(anchor: &str, depth: u8) -> Self {
        Self {
            code: DiagnosticCode::SkippedOutlineLevel,
            severity: DiagnosticSeverity::Error,
            anchor: format!("{anchor}:depth-{depth}"),
        }
    }

    #[must_use]
    pub fn manual_outline_label(anchor: &str) -> Self {
        Self::outline(DiagnosticCode::ManualOutlineLabel, anchor)
    }

    #[must_use]
    pub fn list_restart(anchor: &str) -> Self {
        Self::outline(DiagnosticCode::ListRestart, anchor)
    }

    #[must_use]
    pub fn missing_source_anchor(anchor: &str) -> Self {
        Self::outline(DiagnosticCode::MissingSourceAnchor, anchor)
    }

    fn outline(code: DiagnosticCode, anchor: &str) -> Self {
        Self {
            code,
            severity: DiagnosticSeverity::Warning,
            anchor: anchor.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    ProtocolVersion,
    UnsupportedRevision,
    MissingMainDocument,
    ExternalRelationship,
    CorruptPackage,
    EscapingPackage,
    MacroEnabledPackage,
    EncryptedPackage,
    ZipEntryCountExceeded,
    ZipEntryUncompressedSizeExceeded,
    ZipTotalUncompressedSizeExceeded,
    UnsupportedNumbering,
    AmbiguousOutline,
    DepthOverflow,
    SkippedOutlineLevel,
    ManualOutlineLabel,
    ListRestart,
    MissingSourceAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
