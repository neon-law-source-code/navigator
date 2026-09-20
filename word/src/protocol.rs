use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::{Diagnostic, DocumentModel, PROTOCOL_VERSION};

/// Rust-to-managed request. The source filename never crosses this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRequest {
    pub protocol_version: u16,
    pub bytes_base64: String,
}

impl AdapterRequest {
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// Managed-to-Rust response. A rejected package carries only a stable code and
/// structural anchor, never content or caller-supplied names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterReply {
    pub protocol_version: u16,
    pub ok: bool,
    pub document: Option<DocumentModel>,
    pub diagnostic: Option<Diagnostic>,
}

/// One attorney-approved text replacement sent to the managed native Word
/// exporter. The anchor is the embedded paragraph identity, never a guessed
/// text match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportChange {
    pub anchor: String,
    pub replacement_text: String,
    pub author: String,
}

/// Rust-to-managed request for a baseline-preserving native Word export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRequest {
    pub protocol_version: u16,
    pub original_bytes_base64: String,
    pub changes: Vec<ExportChange>,
}

impl ExportRequest {
    #[must_use]
    pub fn new(bytes: &[u8], changes: Vec<ExportChange>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            original_bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            changes,
        }
    }
}

/// Managed-to-Rust export response. The returned package is still an internal
/// asset; diagnostics contain only stable structural codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReply {
    pub protocol_version: u16,
    pub ok: bool,
    pub bytes_base64: Option<String>,
    pub diagnostic: Option<Diagnostic>,
}

/// Rust-to-managed request for independent package validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub protocol_version: u16,
    pub bytes_base64: String,
}

impl VerifyRequest {
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// Managed-to-Rust verification response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReply {
    pub protocol_version: u16,
    pub ok: bool,
    pub diagnostic: Option<Diagnostic>,
}

impl AdapterReply {
    #[must_use]
    pub fn success(document: DocumentModel) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            ok: true,
            document: Some(document),
            diagnostic: None,
        }
    }

    #[must_use]
    pub fn rejected(diagnostic: Diagnostic) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            ok: false,
            document: None,
            diagnostic: Some(diagnostic),
        }
    }
}
