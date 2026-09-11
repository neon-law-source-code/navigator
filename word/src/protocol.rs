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
