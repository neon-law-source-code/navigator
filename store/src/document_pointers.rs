//! The source-only YAML pointer to one Project document revision chain.
//!
//! A pointer is safe to commit because it contains metadata and an `assets`
//! row id, never an object-storage coordinate or legal-document bytes. The
//! repository path below `documents/` is the document slug.

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One committed declaration of a Project document's current revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPointer {
    pub kind: String,
    pub visibility: String,
    pub current_version: PointerVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<Uuid>,
}

/// The immutable facts copied from the current `assets` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerVersion {
    pub version: usize,
    pub asset_id: Uuid,
    pub created_at: String,
    pub sha256: String,
    pub size_bytes: i64,
}

/// Why a pointer cannot name a valid document revision.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PointerError {
    #[error("kind `{0}` is not an accepted document kind")]
    Kind(String),
    #[error("visibility must be `internal` or `client`")]
    Visibility,
    #[error("current_version.version must be a positive integer")]
    Version,
    #[error("current_version.created_at must be an RFC 3339 timestamp in UTC")]
    CreatedAt,
    #[error("current_version.sha256 must be exactly 64 lowercase hexadecimal characters")]
    Sha256,
    #[error("current_version.size_bytes must be a positive integer")]
    Size,
    #[error("previous_version must be absent at version 1 and present after version 1")]
    PreviousVersion,
}

impl DocumentPointer {
    /// Parse YAML and enforce the document-pointer contract.
    ///
    /// # Errors
    /// A YAML shape error or a semantic [`PointerError`].
    pub fn from_yaml(raw: &str) -> anyhow::Result<Self> {
        let pointer: Self = serde_yaml::from_str(raw)?;
        pointer.validate()?;
        Ok(pointer)
    }

    /// Serialize the stable committed representation.
    ///
    /// # Errors
    /// A YAML serialization failure.
    pub fn to_yaml(&self) -> anyhow::Result<String> {
        Ok(serde_yaml::to_string(self)?)
    }

    /// Enforce constraints serde's field types cannot express.
    ///
    /// # Errors
    /// The first semantic pointer violation.
    pub fn validate(&self) -> Result<(), PointerError> {
        if !rules::kind::Kind::parse(&self.kind)
            .is_some_and(|kind| kind.valid_for(rules::kind::Lane::Asset))
        {
            return Err(PointerError::Kind(self.kind.clone()));
        }
        if !matches!(self.visibility.as_str(), "internal" | "client") {
            return Err(PointerError::Visibility);
        }
        if self.current_version.version == 0 {
            return Err(PointerError::Version);
        }
        let created_at = DateTime::parse_from_rfc3339(&self.current_version.created_at)
            .map_err(|_| PointerError::CreatedAt)?;
        if created_at.offset() != &FixedOffset::east_opt(0).ok_or(PointerError::CreatedAt)? {
            return Err(PointerError::CreatedAt);
        }
        let sha = self.current_version.sha256.as_bytes();
        if sha.len() != 64
            || !sha
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(PointerError::Sha256);
        }
        if self.current_version.size_bytes <= 0 {
            return Err(PointerError::Size);
        }
        if (self.current_version.version == 1) != self.previous_version.is_none() {
            return Err(PointerError::PreviousVersion);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer() -> DocumentPointer {
        DocumentPointer {
            kind: "agreement".into(),
            visibility: "internal".into(),
            current_version: PointerVersion {
                version: 1,
                asset_id: Uuid::now_v7(),
                created_at: "2026-09-05T12:00:00Z".into(),
                sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                size_bytes: 42,
            },
            previous_version: None,
        }
    }

    #[test]
    fn round_trips_the_committed_yaml_shape() {
        let expected = pointer();
        let yaml = expected.to_yaml().unwrap();
        assert_eq!(DocumentPointer::from_yaml(&yaml).unwrap(), expected);
    }

    #[test]
    fn rejects_a_non_utc_timestamp_and_a_broken_chain() {
        let mut invalid = pointer();
        invalid.current_version.created_at = "2026-09-05T08:00:00-04:00".into();
        assert_eq!(invalid.validate(), Err(PointerError::CreatedAt));
        invalid.current_version.created_at = "2026-09-05T12:00:00Z".into();
        invalid.current_version.version = 2;
        assert_eq!(invalid.validate(), Err(PointerError::PreviousVersion));
    }
}
