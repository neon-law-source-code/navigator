//! Conservative safety checks for portal-facing uploaded PDFs.
//!
//! This is not a PDF sanitizer. It is an admission gate for files the app
//! will later serve back to browsers: reject files that are not PDFs, are too
//! large for the upload surface, or declare active PDF features.

use lopdf::{Document, Object};
use thiserror::Error;

/// Initial upload cap for a portal-facing PDF: 25 MiB.
pub const DEFAULT_MAX_PDF_BYTES: usize = 25 * 1024 * 1024;

const ACTIVE_NAMES: &[&str] = &[
    "javascript",
    "js",
    "openaction",
    "aa",
    "launch",
    "submitform",
    "importdata",
    "richmedia",
    "embeddedfile",
    "filespec",
    "encrypt",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PdfSafetyError {
    #[error("file is empty")]
    Empty,
    #[error("file is too large ({actual} bytes > {limit} bytes)")]
    TooLarge { actual: usize, limit: usize },
    #[error("file does not start with %PDF-")]
    MissingHeader,
    #[error("file does not contain %%EOF")]
    MissingEof,
    #[error("file is not a parseable PDF: {0}")]
    Parse(String),
    #[error("PDF uses active feature /{0}")]
    ActiveFeature(String),
}

/// Validate a PDF with the default size limit.
///
/// # Errors
/// Returns [`PdfSafetyError`] when bytes are not a PDF or when active PDF
/// features are visible in raw bytes, PDF names with hex escapes decoded, or
/// decoded stream content.
pub fn validate_pdf(bytes: &[u8]) -> Result<(), PdfSafetyError> {
    validate_pdf_with_limit(bytes, DEFAULT_MAX_PDF_BYTES)
}

/// Validate a PDF with an explicit size limit.
///
/// # Errors
/// Returns [`PdfSafetyError`] on unsafe or malformed input.
pub fn validate_pdf_with_limit(bytes: &[u8], max_bytes: usize) -> Result<(), PdfSafetyError> {
    if bytes.is_empty() {
        return Err(PdfSafetyError::Empty);
    }
    if bytes.len() > max_bytes {
        return Err(PdfSafetyError::TooLarge {
            actual: bytes.len(),
            limit: max_bytes,
        });
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(PdfSafetyError::MissingHeader);
    }
    if !bytes.windows(b"%%EOF".len()).any(|w| w == b"%%EOF") {
        return Err(PdfSafetyError::MissingEof);
    }
    scan_pdf_names(bytes)?;

    let doc = Document::load_mem(bytes).map_err(|e| PdfSafetyError::Parse(e.to_string()))?;
    for object in doc.objects.values() {
        if let Object::Stream(stream) = object {
            let decoded = stream
                .decompressed_content()
                .map_err(|e| PdfSafetyError::Parse(e.to_string()))?;
            scan_pdf_names(&decoded)?;
        }
    }

    Ok(())
}

fn scan_pdf_names(bytes: &[u8]) -> Result<(), PdfSafetyError> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'/' {
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && !is_delimiter_or_whitespace(bytes[i]) {
            i += 1;
        }
        if start == i {
            continue;
        }
        let decoded = decode_name(&bytes[start..i]);
        let lower = decoded.to_ascii_lowercase();
        if ACTIVE_NAMES.contains(&lower.as_str()) {
            return Err(PdfSafetyError::ActiveFeature(decoded));
        }
    }
    Ok(())
}

fn decode_name(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() {
            if let (Some(hi), Some(lo)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_delimiter_or_whitespace(b: u8) -> bool {
    matches!(
        b,
        0x00 | b'\t'
            | b'\n'
            | b'\x0c'
            | b'\r'
            | b' '
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

#[cfg(test)]
mod tests {
    use super::{validate_pdf, validate_pdf_with_limit, PdfSafetyError};

    fn minimal_pdf(extra: &str) -> Vec<u8> {
        format!("%PDF-1.7\n1 0 obj\n<< {extra} >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n")
            .into_bytes()
    }

    #[test]
    fn accepts_minimal_pdf() {
        let pdf = crate::render("Homer v. Flanders status memo.").unwrap();
        validate_pdf(&pdf).unwrap();
    }

    #[test]
    fn rejects_escaped_active_names() {
        let err = validate_pdf(&minimal_pdf("/Open#41ction 2 0 R")).unwrap_err();
        assert_eq!(err, PdfSafetyError::ActiveFeature("OpenAction".into()));
    }

    #[test]
    fn rejects_raw_active_names() {
        let err = validate_pdf(&minimal_pdf("/JavaScript 2 0 R")).unwrap_err();
        assert_eq!(err, PdfSafetyError::ActiveFeature("JavaScript".into()));
    }

    #[test]
    fn rejects_missing_eof() {
        let err = validate_pdf(b"%PDF-1.7\n").unwrap_err();
        assert_eq!(err, PdfSafetyError::MissingEof);
    }

    #[test]
    fn rejects_oversized_pdf() {
        let pdf = minimal_pdf("/Type /Catalog");
        let err = validate_pdf_with_limit(&pdf, pdf.len() - 1).unwrap_err();
        assert!(matches!(err, PdfSafetyError::TooLarge { .. }));
    }

    #[test]
    fn rejects_streams_that_cannot_be_decompressed() {
        // lopdf's FlateDecode path recovers corrupt streams leniently (falling
        // back to raw deflate, then to an empty payload) rather than erroring,
        // so this exercises ASCIIHexDecode, which still hard-fails on content
        // that isn't valid hex.
        let pdf = b"%PDF-1.7
1 0 obj
<< /Length 4 /Filter /ASCIIHexDecode >>
stream
zzzz
endstream
endobj
trailer
<< /Root 1 0 R >>
%%EOF
";

        let err = validate_pdf(pdf).unwrap_err();

        assert!(matches!(err, PdfSafetyError::Parse(_)));
    }
}
