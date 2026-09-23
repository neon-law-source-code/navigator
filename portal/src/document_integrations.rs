//! Server-side resolution of a document's external ids.
//!
//! `GET /app/api/projects/{id}/documents/integrity` is where a CI session asks
//! whether an external id still resolves. The session never receives Xero or
//! DocuSign credentials; Navigator reads the firm's integration secrets.
//! A document pointer has no external-id field, so nothing here resolves and
//! the list is empty.

use serde::Serialize;
use store::assets::Asset;
use uuid::Uuid;

/// One external-id check for a document on a Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub asset_id: Uuid,
    /// `xero` or `docusign`.
    pub integration: String,
    /// `absent` when the remote record does not resolve, `could_not_read`
    /// when the integration could not be asked.
    pub outcome: String,
    pub detail: String,
}

/// External-id findings for `assets`.
///
/// Empty: a pointer carries no external id for an integration to resolve.
#[must_use]
pub fn findings(assets: &[Asset]) -> Vec<Finding> {
    let _ = assets;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::findings;

    #[test]
    fn a_pointer_with_no_external_id_resolves_nothing() {
        assert!(findings(&[]).is_empty());
    }
}
