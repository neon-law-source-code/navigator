//! Rust-owned boundary for parsing loss-aware Office Open XML Word packages.
//!
//! The managed adapter is deliberately narrow: it validates and reads a local
//! `.docx` package with Microsoft's Open XML SDK, then returns the versioned
//! [`protocol`] shape. Rust owns the immutable source bytes, safety preflight,
//! orchestration, diagnostics contract, and the typed document model.

mod adapter;
pub mod anchor;
mod model;
pub mod notation;
pub mod outline;
mod preflight;
pub mod protocol;

pub use adapter::{AdapterError, ManagedAdapter, WordAdapter};
pub use model::{
    Block, BreakKind, Comment, Diagnostic, DiagnosticCode, DiagnosticSeverity, Document,
    DocumentModel, FieldKind, Inline, NumberingDefinition, NumberingIdentity, OriginalSource,
    PackageInventory, PackagePart, PackageRelationship, Paragraph, RevisionKind, RevisionNode,
    Story, StoryKind, StyleDefinition, Table, TableCell, TableRow,
};
pub use notation::{to_markdown, TrustedNotationMarkdown};
pub use outline::{
    CanonicalBlock, CanonicalBlockKind, CanonicalDocument, CanonicalInline, CanonicalStory,
    ListIdentity, NumberingLevel, OutlineScheme, OutlineUnit, HARVARD_OUTLINE_PATTERN,
    MARKER_GROUPS, MAX_DEPTH,
};
pub use preflight::is_docx_filename;

/// The only protocol version currently understood by both sides of the local
/// boundary. A version mismatch is terminal rather than best-effort: a lossy
/// interpretation of a legal document is worse than a visible refusal.
pub const PROTOCOL_VERSION: u16 = 1;

/// Parse one uploaded Word package through the configured local managed
/// adapter.
///
/// The filename is used only to classify the accepted `.docx` family. It is
/// never sent to the adapter and never included in a diagnostic.
pub async fn parse(filename: &str, bytes: &[u8]) -> Result<Document, WordError> {
    let adapter = ManagedAdapter::from_env();
    parse_with_adapter(&adapter, filename, bytes).await
}

/// Parse through a caller-supplied adapter. This is the seam used by focused
/// tests and by a future in-process managed host; the production constructor
/// is [`parse`].
pub async fn parse_with_adapter<A: WordAdapter + ?Sized>(
    adapter: &A,
    filename: &str,
    bytes: &[u8],
) -> Result<Document, WordError> {
    preflight::validate_filename(filename)?;
    preflight::validate_zip(bytes)?;

    let request = protocol::AdapterRequest::new(bytes);
    let model = adapter.parse(request).await.map_err(WordError::Adapter)?;
    if model.protocol_version != PROTOCOL_VERSION {
        return Err(WordError::ProtocolVersion {
            expected: PROTOCOL_VERSION,
            received: model.protocol_version,
        });
    }
    if let Some(diagnostic) = model.diagnostic {
        return Err(WordError::Rejected(diagnostic));
    }
    let document = model.document.ok_or(WordError::MissingDocument)?;
    Ok(Document::from_source(bytes, document))
}

/// Errors at the Rust-owned document boundary. These variants intentionally
/// carry no filename, extracted text, package URI, or party data.
#[derive(Debug, thiserror::Error)]
pub enum WordError {
    #[error("unsupported Word package format")]
    UnsupportedFormat,
    #[error("legacy Word package rejected")]
    LegacyPackage,
    #[error("macro-enabled Word package rejected")]
    MacroEnabledPackage,
    #[error("encrypted Word package rejected")]
    EncryptedPackage,
    #[error("Word package is corrupt")]
    CorruptPackage,
    #[error("Word package path escapes its container")]
    EscapingPackage,
    #[error("Word package ZIP entry count {actual} exceeds maximum {maximum}")]
    ZipEntryCountExceeded { actual: usize, maximum: usize },
    #[error(
        "Word package ZIP entry uncompressed size {actual} bytes exceeds maximum {maximum} bytes"
    )]
    ZipEntryUncompressedSizeExceeded { actual: u64, maximum: u64 },
    #[error(
        "Word package total ZIP uncompressed size {actual} bytes exceeds maximum {maximum} bytes"
    )]
    ZipTotalUncompressedSizeExceeded { actual: u64, maximum: u64 },
    #[error("Word package ZIP entry inflated size {actual} bytes exceeds maximum {maximum} bytes")]
    ZipEntryInflatedSizeExceeded { actual: u64, maximum: u64 },
    #[error("managed Word adapter: {0}")]
    Adapter(#[from] AdapterError),
    #[error("Word adapter protocol version {received} is not supported (expected {expected})")]
    ProtocolVersion { expected: u16, received: u16 },
    #[error("Word adapter returned no document")]
    MissingDocument,
    #[error("Word package rejected: {0:?}")]
    Rejected(Diagnostic),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        parse_with_adapter, protocol, Block, DiagnosticCode, DiagnosticSeverity, Document,
        DocumentModel, Inline, PackageInventory, Paragraph, RevisionKind, RevisionNode, Story,
        StoryKind, WordAdapter, WordError,
    };

    struct FixtureAdapter {
        reply: protocol::AdapterReply,
    }

    #[async_trait::async_trait]
    impl WordAdapter for FixtureAdapter {
        async fn parse(
            &self,
            _request: protocol::AdapterRequest,
        ) -> Result<protocol::AdapterReply, super::AdapterError> {
            Ok(self.reply.clone())
        }
    }

    fn empty_model() -> DocumentModel {
        DocumentModel::empty()
    }

    #[tokio::test]
    async fn parser_attaches_immutable_source_without_sending_filename() {
        let reply = protocol::AdapterReply::success(empty_model());
        let adapter = FixtureAdapter { reply };
        let document = parse_with_adapter(&adapter, "synthetic.docx", &valid_zip()).await;
        let document = document.expect("synthetic adapter reply parses");
        assert_eq!(document.original_bytes(), valid_zip());
        assert_eq!(document.source().sha256.len(), 64);
        assert_eq!(
            document.source().byte_size,
            document.original_bytes().len() as u64
        );
    }

    #[tokio::test]
    async fn parser_returns_structural_diagnostic_without_document_content() {
        let diagnostic = super::Diagnostic::unsupported_revision("word/document.xml", "insert");
        let adapter = FixtureAdapter {
            reply: protocol::AdapterReply::rejected(diagnostic.clone()),
        };
        let error = parse_with_adapter(&adapter, "synthetic.docx", &valid_zip())
            .await
            .expect_err("rejected adapter response");
        assert!(matches!(&error, WordError::Rejected(found) if found == &diagnostic));
        assert!(!error.to_string().contains("synthetic"));
        assert!(!error.to_string().contains("body"));
    }

    #[tokio::test]
    async fn parser_surfaces_protocol_version_diagnostic() {
        let diagnostic = super::Diagnostic {
            code: DiagnosticCode::ProtocolVersion,
            severity: DiagnosticSeverity::Error,
            anchor: "request".into(),
        };
        let adapter = FixtureAdapter {
            reply: protocol::AdapterReply::rejected(diagnostic.clone()),
        };

        let error = parse_with_adapter(&adapter, "synthetic.docx", &valid_zip())
            .await
            .expect_err("protocol diagnostic is surfaced");

        assert!(matches!(&error, WordError::Rejected(found) if found == &diagnostic));
    }

    /// Read every diagnostic code string out of the adapter source. Both
    /// `Diagnostic` factories and every `Diagnostic.Rejected` call site pass
    /// the code as a string literal, so a refusal added to `Program.cs` reaches
    /// the pin below without anyone having to remember it.
    fn adapter_diagnostic_codes() -> BTreeSet<String> {
        const SOURCE: &str = include_str!("../adapter/Program.cs");

        fn literals_after(source: &str, marker: &str) -> Vec<String> {
            let mut found = Vec::new();
            let mut rest = source;
            while let Some(offset) = rest.find(marker) {
                rest = &rest[offset + marker.len()..];
                if let Some((literal, _)) = rest
                    .trim_start()
                    .strip_prefix('"')
                    .and_then(|tail| tail.split_once('"'))
                {
                    found.push(literal.to_owned());
                }
            }
            found
        }

        // The two factories that mint a code of their own. The third `new(`
        // in the record forwards a caller's code and yields no literal.
        let record = SOURCE
            .split_once("internal sealed record Diagnostic(")
            .expect("the adapter declares a Diagnostic record")
            .1;
        let record = record
            .split_once("\n}\n")
            .expect("the Diagnostic record is closed")
            .0;
        let mut codes: BTreeSet<String> = literals_after(record, "new(").into_iter().collect();

        // Every other refusal names its code at the call site.
        let rejections = literals_after(SOURCE, "Diagnostic.Rejected(");
        assert_eq!(
            rejections.len(),
            SOURCE.matches("Diagnostic.Rejected(").count(),
            "a Diagnostic.Rejected call in Program.cs no longer passes its code as a string \
             literal, so this test can no longer enumerate the wire contract"
        );
        codes.extend(rejections);
        codes
    }

    /// Every diagnostic string emitted by `word/adapter/Program.cs` is pinned
    /// here against the Rust wire enum. A rename on either side turns a clean
    /// refusal into an opaque protocol failure at the boundary.
    #[test]
    fn every_adapter_diagnostic_deserialises_into_the_rust_wire_contract() {
        let pinned = [
            ("protocol_version", DiagnosticCode::ProtocolVersion),
            ("corrupt_package", DiagnosticCode::CorruptPackage),
            ("missing_main_document", DiagnosticCode::MissingMainDocument),
            (
                "external_relationship",
                DiagnosticCode::ExternalRelationship,
            ),
            ("unsupported_revision", DiagnosticCode::UnsupportedRevision),
            (
                "zip_entry_count_exceeded",
                DiagnosticCode::ZipEntryCountExceeded,
            ),
            (
                "zip_entry_uncompressed_size_exceeded",
                DiagnosticCode::ZipEntryUncompressedSizeExceeded,
            ),
            (
                "zip_total_uncompressed_size_exceeded",
                DiagnosticCode::ZipTotalUncompressedSizeExceeded,
            ),
            ("macro_enabled_package", DiagnosticCode::MacroEnabledPackage),
            ("encrypted_package", DiagnosticCode::EncryptedPackage),
            ("escaping_package", DiagnosticCode::EscapingPackage),
        ];

        // The adapter is the wire's source of truth, so the table is checked
        // against the codes actually written there rather than against a
        // reviewer's memory of them.
        assert_eq!(
            adapter_diagnostic_codes(),
            pinned
                .iter()
                .map(|(code, _)| (*code).to_owned())
                .collect::<BTreeSet<String>>()
        );

        for (code, expected) in pinned {
            let version = super::PROTOCOL_VERSION;
            let json = format!(
                "{{\"protocol_version\":{version},\"ok\":false,\"document\":null,\
                 \"diagnostic\":{{\"code\":\"{code}\",\"severity\":\"error\",\
                 \"anchor\":\"package\"}}}}"
            );

            let reply: protocol::AdapterReply =
                serde_json::from_str(&json).expect("adapter refusal deserialises");
            let diagnostic = reply.diagnostic.expect("refusal carries a diagnostic");

            assert_eq!(diagnostic.code, expected);
            assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
            assert_eq!(diagnostic.anchor, "package");
        }
    }

    #[test]
    fn accepted_view_includes_insertions_and_preserves_revision_nodes() {
        let model = DocumentModel {
            protocol_version: super::PROTOCOL_VERSION,
            package: PackageInventory::default(),
            stories: vec![Story {
                kind: StoryKind::MainDocument,
                part_uri: "/word/document.xml".into(),
                blocks: vec![Block::Paragraph(Paragraph {
                    anchor: "document:paragraph:1".into(),
                    style_id: Some("BodyText".into()),
                    numbering: None,
                    nodes: vec![
                        Inline::Text {
                            text: "kept".into(),
                            style_id: Some("BodyText".into()),
                            revision: None,
                        },
                        Inline::Revision {
                            revision: RevisionNode {
                                kind: RevisionKind::Insertion,
                                id: Some("1".into()),
                                anchor: "document:ins".into(),
                            },
                            children: vec![Inline::Text {
                                text: " added".into(),
                                style_id: None,
                                revision: None,
                            }],
                        },
                        Inline::Revision {
                            revision: RevisionNode {
                                kind: RevisionKind::Deletion,
                                id: Some("2".into()),
                                anchor: "document:del".into(),
                            },
                            children: vec![Inline::Text {
                                text: " removed".into(),
                                style_id: None,
                                revision: None,
                            }],
                        },
                    ],
                    revisions: Vec::new(),
                })],
            }],
            styles: Vec::new(),
            numbering: Vec::new(),
            comments: Vec::new(),
            diagnostics: Vec::new(),
            revision_nodes: vec![RevisionNode {
                kind: RevisionKind::Insertion,
                id: Some("1".into()),
                anchor: "document:ins".into(),
            }],
        };
        let document = Document::from_source(b"immutable", model);

        assert_eq!(document.accepted_view_text(), "kept added\n");
        assert_eq!(document.model().revision_nodes.len(), 1);
        assert_eq!(document.original_bytes(), b"immutable");
    }

    fn valid_zip() -> Vec<u8> {
        use std::io::Write as _;
        let mut bytes = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(b"<Types/>").unwrap();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<document/>").unwrap();
        zip.finish().unwrap();
        bytes.into_inner()
    }
}
