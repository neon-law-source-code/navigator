//! The one funnel for filing a document into a matter.
//!
//! Legal-document bytes live only in object storage. A Project repository may
//! carry a YAML pointer to an asset revision, but this server seam never writes
//! the bytes to Git.

use std::sync::Arc;

use cloud::StorageService;
use repos::Author;
use store::documents::{self, IngestArgs, IngestedDocument};

/// File a document into authoritative object storage and its `assets` row.
///
/// `author` remains part of the shared browser/email call contract because
/// those callers already resolve the acting person. It is intentionally not
/// used for Git attribution: raw legal-document bytes must never enter a
/// Project repository.
///
/// # Errors
/// Returns the durable object-storage or database failure.
pub async fn record_document(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn StorageService>,
    _author: Author<'_>,
    args: &IngestArgs<'_>,
    bytes: &[u8],
) -> Result<IngestedDocument, documents::IngestError> {
    documents::ingest_bytes(surreal, storage, args, bytes).await
}
