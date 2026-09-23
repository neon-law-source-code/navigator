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
    /// Present only on an Authority capture routed through `site authorities
    /// create` (`documents/cases/**` or `documents/rules/**`): the global
    /// Authority — no `project_id` — this capture's bytes were archived
    /// under. Absent for every other document kind, which stays a plain
    /// Project document with no Authority of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_id: Option<Uuid>,
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
    /// Where this revision was captured from, and when — carried only by an
    /// Authority capture's pointer (see [`DocumentPointer::authority_id`]),
    /// so the portal can render a real citation/link instead of an opaque
    /// file. Absent for every other document kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_on: Option<String>,
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
                canonical_url: None,
                checked_on: None,
            },
            previous_version: None,
            authority_id: None,
        }
    }

    fn evidence_pointer() -> DocumentPointer {
        let mut pointer = pointer();
        pointer.kind = "exhibit".into();
        pointer.authority_id = Some(Uuid::now_v7());
        pointer.current_version.canonical_url = Some("https://example.test/opinion".into());
        pointer.current_version.checked_on = Some("2026-09-05".into());
        pointer
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

    /// `current_version.created_at` is required, not merely
    /// validated-when-present: a committed pointer YAML that omits the key
    /// entirely fails to parse at all, so `project gate --check` (which
    /// calls `from_yaml` on every committed pointer) fails it too.
    #[test]
    fn a_pointer_missing_created_at_entirely_fails_to_parse() {
        let yaml = "kind: agreement\n\
                     visibility: internal\n\
                     current_version:\n  \
                       version: 1\n  \
                       asset_id: 018f3b1a-0000-7000-8000-000000000000\n  \
                       sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n  \
                       size_bytes: 42\n";
        let error = DocumentPointer::from_yaml(yaml).unwrap_err();
        assert!(
            error.to_string().contains("created_at"),
            "the parse failure should name the missing field: {error}"
        );
    }

    /// An evidence capture's pointer (LAW-50) reuses this same struct rather
    /// than a parallel shape: `authority_id` on the identity, `canonical_url`
    /// and `checked_on` on the revision. It round-trips and validates like
    /// any other pointer — the new fields are additive, not a second schema.
    #[test]
    fn an_authority_backed_pointer_round_trips_and_validates() {
        let expected = evidence_pointer();
        assert!(expected.validate().is_ok());
        let yaml = expected.to_yaml().unwrap();
        assert!(yaml.contains("authority_id"));
        assert!(yaml.contains("canonical_url"));
        assert!(yaml.contains("checked_on"));
        assert_eq!(DocumentPointer::from_yaml(&yaml).unwrap(), expected);
    }

    /// A pointer with no Authority — every non-evidence document kind —
    /// serializes none of the three new fields, so an older reader (or a
    /// byte-for-byte diff) never sees noise it does not understand.
    #[test]
    fn a_plain_document_pointer_serializes_no_authority_fields() {
        let plain = pointer();
        let yaml = plain.to_yaml().unwrap();
        assert!(!yaml.contains("authority_id"));
        assert!(!yaml.contains("canonical_url"));
        assert!(!yaml.contains("checked_on"));
    }
}
