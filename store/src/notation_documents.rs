//! `notation_documents` — the durable, append-only Notation Markdown
//! document-source model for one Notation (ENG-578).
//!
//! # Why this is not `templates` or `notation_clauses`
//!
//! An imported Word document is matter work product, not a reusable
//! Git-backed [`crate::templates`] blueprint, and it is a full document, not
//! one custom paragraph a [`crate::notation_clauses`] row splices into
//! `{{custom_clauses}}`. This module owns a third shape: one document
//! identity per Notation (tied to the immutable original Word
//! [`crate::assets`] row it was parsed from), and an append-only chain of
//! immutable Markdown-source versions hanging off that identity. A version
//! is never rewritten — a re-parse or an attorney edit always appends a
//! child.
//!
//! # This table lives in SurrealDB
//!
//! `notation_document` and `notation_document_version` are defined in
//! `store/src/schema/navigator.surql`. See `docs/notation.md` for the
//! document-source vocabulary and the anchor-durability decision this
//! module's `anchor_manifest_asset_id` column exists to store.
//!
//! # Content never enters Git, logs, or the workflow journal
//!
//! `markdown_asset_id` and `anchor_manifest_asset_id` are internal,
//! content-addressed [`crate::assets`] rows (`visibility::INTERNAL`, never
//! `client`) written through [`crate::assets::ingest_content`]. The
//! `notation_event` rows this module journals ([`MACHINE_DOCUMENT`]) carry
//! only identifiers — asset ids, version ids, parser/schema versions — never
//! Markdown bytes or the anchor manifest's contents.
//!
//! # Idempotency and conflict, not a lock
//!
//! [`import_root`] is idempotent: re-parsing the same original asset into
//! byte-identical Markdown with the same parser/schema version is a no-op
//! that returns the existing root version rather than forking the chain.
//! [`append_edit`] requires the caller's `expected_parent_version_id` to
//! still be the document's current version; a caller racing against another
//! writer gets [`NotationDocumentError::Conflict`] rather than silently
//! overwriting or forking. A conflict caught before any write refuses
//! cleanly; a conflict caught only by the compare-and-swap after the version
//! row was already written (a true concurrent race) leaves that row in
//! place, orphaned rather than current — a version is append-only and
//! nothing here ever deletes one.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use cloud::StorageService;
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

pub(crate) const DOCUMENT_TABLE: &str = "notation_document";
pub(crate) const VERSION_TABLE: &str = "notation_document_version";

/// The version that came straight from parsing the original Word package.
pub const STATUS_IMPORTED: &str = "imported";
/// A version an attorney produced by editing a parent version (ENG-579).
pub const STATUS_EDITED: &str = "edited";

/// `notation_events.machine_kind` for a document-version write. A distinct
/// kind so these rows never participate in the workflow / questionnaire
/// state-projection reads.
pub const MACHINE_DOCUMENT: &str = "document";

/// One Notation's document-source identity.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`DocumentRow`] is the seam that turns it into (and back out of) what the
/// SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotationDocument {
    pub id: Uuid,
    pub notation_id: Uuid,
    /// The immutable original Word package this chain was parsed from.
    /// Fixed at creation — never rewritten.
    pub original_asset_id: Uuid,
    /// The current-version projection. `None` only before the first
    /// [`import_root`] call succeeds.
    pub current_version_id: Option<Uuid>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct DocumentRow {
    id: surrealdb::types::RecordId,
    notation_id: surrealdb::types::RecordId,
    original_asset_id: surrealdb::types::RecordId,
    current_version_id: Option<surrealdb::types::RecordId>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl DocumentRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_document(self) -> Option<NotationDocument> {
        Some(NotationDocument {
            id: record_uuid(&self.id)?,
            notation_id: record_uuid(&self.notation_id)?,
            original_asset_id: record_uuid(&self.original_asset_id)?,
            current_version_id: self.current_version_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const DOCUMENT_SELECT: &str =
    "id, notation_id, original_asset_id, current_version_id, inserted_at, updated_at";

/// One immutable Markdown-source version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotationDocumentVersion {
    pub id: Uuid,
    pub document_id: Uuid,
    /// `None` only for the root version created by [`import_root`].
    pub parent_version_id: Option<Uuid>,
    /// The internal, content-addressed asset holding this version's
    /// Notation Markdown bytes.
    pub markdown_asset_id: Uuid,
    /// The internal, content-addressed asset holding this version's
    /// block/anchor manifest (JSON) — see the module doc's anchor-durability
    /// note.
    pub anchor_manifest_asset_id: Uuid,
    pub parser_version: String,
    /// The canonical outline schema version the Markdown was projected
    /// against.
    pub schema_version: i64,
    pub authored_by_person_id: Uuid,
    /// [`STATUS_IMPORTED`] or [`STATUS_EDITED`].
    pub status: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct VersionRow {
    id: surrealdb::types::RecordId,
    document_id: surrealdb::types::RecordId,
    parent_version_id: Option<surrealdb::types::RecordId>,
    markdown_asset_id: surrealdb::types::RecordId,
    anchor_manifest_asset_id: surrealdb::types::RecordId,
    parser_version: String,
    schema_version: i64,
    authored_by_person_id: surrealdb::types::RecordId,
    status: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl VersionRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_version(self) -> Option<NotationDocumentVersion> {
        Some(NotationDocumentVersion {
            id: record_uuid(&self.id)?,
            document_id: record_uuid(&self.document_id)?,
            parent_version_id: self.parent_version_id.as_ref().and_then(record_uuid),
            markdown_asset_id: record_uuid(&self.markdown_asset_id)?,
            anchor_manifest_asset_id: record_uuid(&self.anchor_manifest_asset_id)?,
            parser_version: self.parser_version,
            schema_version: self.schema_version,
            authored_by_person_id: record_uuid(&self.authored_by_person_id)?,
            status: self.status,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const VERSION_SELECT: &str = "id, document_id, parent_version_id, markdown_asset_id, \
     anchor_manifest_asset_id, parser_version, schema_version, authored_by_person_id, status, \
     inserted_at, updated_at";

/// Why a `notation_documents` command refused.
#[derive(Debug, thiserror::Error)]
pub enum NotationDocumentError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("notation: {0}")]
    Notation(#[from] crate::notations::NotationError),
    #[error("asset: {0}")]
    Asset(#[from] crate::assets::AssetError),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a notation document returned no usable row")]
    WriteReturnedNothing,
    /// `notation_id` resolves to a different Project than the caller
    /// asserted — an unrelated participant's project id can never reach a
    /// document it does not own.
    #[error("notation {notation_id} does not belong to project {project_id}")]
    ProjectMismatch { notation_id: Uuid, project_id: Uuid },
    /// A second [`import_root`] named a different original asset than the
    /// document's existing baseline. A changed baseline is a different
    /// document; it must not silently rebase the chain.
    #[error(
        "notation {notation_id} already has an original asset {existing}; {attempted} is a \
         different baseline"
    )]
    OriginalAssetMismatch {
        notation_id: Uuid,
        existing: Uuid,
        attempted: Uuid,
    },
    /// The caller's `expected_parent_version_id` (or, for [`import_root`],
    /// the absence of one) no longer matches the document's current
    /// version. Nothing was written to the current-version pointer; the
    /// version row this call may have appended, if any, is preserved but
    /// orphaned rather than lost.
    #[error("expected parent {expected:?} does not match current version {actual:?}")]
    Conflict {
        expected: Option<Uuid>,
        actual: Option<Uuid>,
    },
    #[error("notation document for notation {0} not found")]
    DocumentNotFound(Uuid),
    #[error("notation document version {0} not found")]
    VersionNotFound(Uuid),
    #[error("notation document version body is not valid UTF-8")]
    NotUtf8,
    #[error("notation event: {0}")]
    NotationEvent(#[from] crate::notation_events::NotationEventError),
}

fn one_document(
    mut response: surrealdb::IndexedResults,
) -> Result<Option<NotationDocument>, NotationDocumentError> {
    let row: Option<DocumentRow> = response.take(0)?;
    Ok(row.and_then(DocumentRow::into_document))
}

fn one_version(
    mut response: surrealdb::IndexedResults,
) -> Result<Option<NotationDocumentVersion>, NotationDocumentError> {
    let row: Option<VersionRow> = response.take(0)?;
    Ok(row.and_then(VersionRow::into_version))
}

fn many_versions(
    mut response: surrealdb::IndexedResults,
) -> Result<Vec<NotationDocumentVersion>, NotationDocumentError> {
    let rows: Vec<VersionRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(VersionRow::into_version)
        .collect())
}

/// Resolve `notation_id`, asserting it belongs to `project_id`.
///
/// This is the module's one authorization primitive: every public read and
/// write threads a caller-asserted `project_id` through this check, so an
/// unrelated participant's project id can never reach another Project's
/// document or version — the same shape [`crate::assets::set_visibility`]
/// uses for its Project scope.
async fn resolve_scoped_notation(
    db: &SurrealDb,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<crate::notations::Notation, NotationDocumentError> {
    let notation = crate::notations::find_by_id(db, notation_id)
        .await?
        .ok_or(crate::notations::NotationError::NotFound(notation_id))?;
    if notation.project_id != project_id {
        return Err(NotationDocumentError::ProjectMismatch {
            notation_id,
            project_id,
        });
    }
    Ok(notation)
}

async fn find_document_by_notation(
    db: &SurrealDb,
    notation_id: Uuid,
) -> Result<Option<NotationDocument>, NotationDocumentError> {
    let response = db
        .query(format!(
            "SELECT {DOCUMENT_SELECT} FROM {DOCUMENT_TABLE} WHERE notation_id = $notation LIMIT 1"
        ))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one_document(response)
}

/// The document identity for a Notation, scoped to `project_id`.
///
/// Returns `None` when the Notation has never had a document imported, and
/// [`NotationDocumentError::ProjectMismatch`] when `notation_id` belongs to
/// a different Project.
///
/// # Errors
/// Propagates any database error, or a project-scope mismatch.
pub async fn find_document(
    db: &SurrealDb,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<Option<NotationDocument>, NotationDocumentError> {
    resolve_scoped_notation(db, project_id, notation_id).await?;
    find_document_by_notation(db, notation_id).await
}

async fn find_version_by_id_unscoped(
    db: &SurrealDb,
    version_id: Uuid,
) -> Result<Option<NotationDocumentVersion>, NotationDocumentError> {
    let response = db
        .query(format!("SELECT {VERSION_SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(VERSION_TABLE, version_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one_version(response)
}

/// The current version of a Notation's document, scoped to `project_id`.
///
/// Returns `None` when no document exists yet, or none has been imported.
///
/// # Errors
/// Propagates any database error, or a project-scope mismatch.
pub async fn current_version(
    db: &SurrealDb,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<Option<NotationDocumentVersion>, NotationDocumentError> {
    let Some(document) = find_document(db, project_id, notation_id).await? else {
        return Ok(None);
    };
    let Some(current_id) = document.current_version_id else {
        return Ok(None);
    };
    find_version_by_id_unscoped(db, current_id).await
}

/// Every version of a Notation's document, newest first — both the current
/// version and every version it superseded remain readable and
/// attributable.
///
/// # Errors
/// Propagates any database error, or a project-scope mismatch.
pub async fn history(
    db: &SurrealDb,
    project_id: Uuid,
    notation_id: Uuid,
) -> Result<Vec<NotationDocumentVersion>, NotationDocumentError> {
    let Some(document) = find_document(db, project_id, notation_id).await? else {
        return Ok(Vec::new());
    };
    let response = db
        .query(format!(
            "SELECT {VERSION_SELECT} FROM {VERSION_TABLE} \
             WHERE document_id = $document ORDER BY inserted_at DESC, id DESC"
        ))
        .bind(("document", record_id(DOCUMENT_TABLE, document.id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many_versions(response)
}

/// One specific version, scoped to `project_id` through its document's
/// Notation.
///
/// # Errors
/// Propagates any database error, or a project-scope mismatch.
pub async fn find_version(
    db: &SurrealDb,
    project_id: Uuid,
    notation_id: Uuid,
    version_id: Uuid,
) -> Result<Option<NotationDocumentVersion>, NotationDocumentError> {
    let Some(document) = find_document(db, project_id, notation_id).await? else {
        return Ok(None);
    };
    let Some(version) = find_version_by_id_unscoped(db, version_id).await? else {
        return Ok(None);
    };
    Ok((version.document_id == document.id).then_some(version))
}

/// Read a version's Notation Markdown bytes back from object storage as
/// text.
///
/// # Errors
/// [`NotationDocumentError::Asset`] on a storage or database failure, or
/// [`NotationDocumentError::NotUtf8`] if the stored bytes are not UTF-8.
pub async fn markdown_of(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    version: &NotationDocumentVersion,
) -> Result<String, NotationDocumentError> {
    let bytes = crate::assets::fetch(db, storage, version.markdown_asset_id).await?;
    String::from_utf8(bytes).map_err(|_| NotationDocumentError::NotUtf8)
}

/// Read a version's block/anchor manifest back from object storage as raw
/// bytes (JSON). Left untyped here deliberately: this store module has no
/// dependency on `word`, so the manifest's shape is the producing caller's
/// contract, not this module's.
///
/// # Errors
/// [`NotationDocumentError::Asset`] on a storage or database failure.
pub async fn anchor_manifest_of(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    version: &NotationDocumentVersion,
) -> Result<Vec<u8>, NotationDocumentError> {
    Ok(crate::assets::fetch(db, storage, version.anchor_manifest_asset_id).await?)
}

/// Outcome of [`import_root`].
#[derive(Debug)]
pub enum Saved {
    /// Re-importing identical bytes at the same parser/schema version was a
    /// no-op; this is the existing root version.
    Unchanged(NotationDocumentVersion),
    /// A new document identity, a new root version, or both were written.
    Written(NotationDocumentVersion),
}

impl Saved {
    /// The version, either way.
    #[must_use]
    pub fn into_model(self) -> NotationDocumentVersion {
        match self {
            Saved::Unchanged(v) | Saved::Written(v) => v,
        }
    }

    /// Whether this call wrote a new version row.
    #[must_use]
    pub fn was_written(&self) -> bool {
        matches!(self, Saved::Written(_))
    }
}

/// What [`import_root`] needs.
pub struct ImportRoot<'a> {
    pub project_id: Uuid,
    pub notation_id: Uuid,
    /// The immutable original Word package this import came from.
    pub original_asset_id: Uuid,
    pub markdown: &'a [u8],
    pub anchor_manifest: &'a [u8],
    pub parser_version: &'a str,
    pub schema_version: i64,
    pub authored_by_person_id: Uuid,
    /// RFC 3339 / ISO 8601, threaded to the journaled `notation_event`.
    pub recorded_at: &'a str,
}

/// Create (or reuse) a Notation's document identity and write its root
/// Markdown-source version from a freshly parsed Word package.
///
/// Idempotent: calling this again with the same `original_asset_id` and
/// byte-identical `markdown`/`anchor_manifest` at the same
/// `parser_version`/`schema_version` is a no-op that returns
/// [`Saved::Unchanged`] rather than forking the chain — the same "identical
/// bytes are a no-op" rule [`crate::assets::file_revision`] applies to
/// documents.
///
/// # Errors
/// [`NotationDocumentError::ProjectMismatch`] if `notation_id` is not in
/// `project_id`; [`NotationDocumentError::OriginalAssetMismatch`] if the
/// document already names a different baseline;
/// [`NotationDocumentError::Conflict`] if the document already has edits on
/// top of its root (re-importing cannot rebase a chain that has moved); or
/// a database/asset error.
pub async fn import_root(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    args: ImportRoot<'_>,
) -> Result<Saved, NotationDocumentError> {
    resolve_scoped_notation(db, args.project_id, args.notation_id).await?;

    let markdown_asset_id =
        crate::assets::ingest_content(db, storage, args.markdown, "text/markdown").await?;
    let anchor_manifest_asset_id =
        crate::assets::ingest_content(db, storage, args.anchor_manifest, "application/json")
            .await?;

    let document = match find_document_by_notation(db, args.notation_id).await? {
        Some(existing) => {
            if existing.original_asset_id != args.original_asset_id {
                return Err(NotationDocumentError::OriginalAssetMismatch {
                    notation_id: args.notation_id,
                    existing: existing.original_asset_id,
                    attempted: args.original_asset_id,
                });
            }
            existing
        }
        None => create_document(db, args.notation_id, args.original_asset_id).await?,
    };

    if let Some(current_id) = document.current_version_id {
        let current = find_version_by_id_unscoped(db, current_id)
            .await?
            .ok_or(NotationDocumentError::DocumentNotFound(document.id))?;
        let unchanged = current.parent_version_id.is_none()
            && current.markdown_asset_id == markdown_asset_id
            && current.anchor_manifest_asset_id == anchor_manifest_asset_id
            && current.parser_version == args.parser_version
            && current.schema_version == args.schema_version;
        if unchanged {
            return Ok(Saved::Unchanged(current));
        }
        return Err(NotationDocumentError::Conflict {
            expected: None,
            actual: Some(current_id),
        });
    }

    let version = append_version(
        db,
        &document,
        None,
        markdown_asset_id,
        anchor_manifest_asset_id,
        args.parser_version,
        args.schema_version,
        args.authored_by_person_id,
        STATUS_IMPORTED,
    )
    .await?;

    journal(
        db,
        args.notation_id,
        args.authored_by_person_id,
        &document,
        &version,
        args.recorded_at,
        "imported",
    )
    .await?;

    Ok(Saved::Written(version))
}

/// What [`append_edit`] needs.
pub struct AppendEdit<'a> {
    pub project_id: Uuid,
    pub notation_id: Uuid,
    /// The version this edit was made against. Must still be the
    /// document's current version, or the call refuses with
    /// [`NotationDocumentError::Conflict`].
    pub expected_parent_version_id: Uuid,
    pub markdown: &'a [u8],
    pub anchor_manifest: &'a [u8],
    pub parser_version: &'a str,
    pub schema_version: i64,
    pub authored_by_person_id: Uuid,
    /// RFC 3339 / ISO 8601, threaded to the journaled `notation_event`.
    pub recorded_at: &'a str,
}

/// Append a child version on top of `expected_parent_version_id`.
///
/// # Errors
/// [`NotationDocumentError::ProjectMismatch`] if `notation_id` is not in
/// `project_id`; [`NotationDocumentError::DocumentNotFound`] if the
/// Notation has no document yet; [`NotationDocumentError::Conflict`] if
/// another writer already advanced the document past
/// `expected_parent_version_id`; or a database/asset error.
pub async fn append_edit(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    args: AppendEdit<'_>,
) -> Result<NotationDocumentVersion, NotationDocumentError> {
    resolve_scoped_notation(db, args.project_id, args.notation_id).await?;

    let document = find_document_by_notation(db, args.notation_id)
        .await?
        .ok_or(NotationDocumentError::DocumentNotFound(args.notation_id))?;

    if document.current_version_id != Some(args.expected_parent_version_id) {
        return Err(NotationDocumentError::Conflict {
            expected: Some(args.expected_parent_version_id),
            actual: document.current_version_id,
        });
    }

    let markdown_asset_id =
        crate::assets::ingest_content(db, storage, args.markdown, "text/markdown").await?;
    let anchor_manifest_asset_id =
        crate::assets::ingest_content(db, storage, args.anchor_manifest, "application/json")
            .await?;

    let version = append_version(
        db,
        &document,
        Some(args.expected_parent_version_id),
        markdown_asset_id,
        anchor_manifest_asset_id,
        args.parser_version,
        args.schema_version,
        args.authored_by_person_id,
        STATUS_EDITED,
    )
    .await?;

    journal(
        db,
        args.notation_id,
        args.authored_by_person_id,
        &document,
        &version,
        args.recorded_at,
        "edited",
    )
    .await?;

    Ok(version)
}

async fn create_document(
    db: &SurrealDb,
    notation_id: Uuid,
    original_asset_id: Uuid,
) -> Result<NotationDocument, NotationDocumentError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             notation_id = $notation_id, \
             original_asset_id = $original_asset_id, \
             current_version_id = NONE \
             RETURN {DOCUMENT_SELECT}"
        ))
        .bind(("id", record_id(DOCUMENT_TABLE, id)))
        .bind((
            "notation_id",
            record_id(crate::notations::TABLE, notation_id),
        ))
        .bind((
            "original_asset_id",
            record_id(crate::assets::TABLE, original_asset_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DocumentRow> = response.take(0)?;
    row.and_then(DocumentRow::into_document)
        .ok_or(NotationDocumentError::WriteReturnedNothing)
}

/// Append one immutable version row, then compare-and-swap the document's
/// `current_version_id` from `expected_parent` to the new row.
///
/// The version row is always written first and is never deleted, even when
/// the compare-and-swap below loses a race — an append-only table has
/// nothing to roll back. Losing the swap means only that this version never
/// becomes current; the caller sees [`NotationDocumentError::Conflict`] and
/// the winner's version remains readable through [`history`].
#[allow(clippy::too_many_arguments)]
async fn append_version(
    db: &SurrealDb,
    document: &NotationDocument,
    expected_parent: Option<Uuid>,
    markdown_asset_id: Uuid,
    anchor_manifest_asset_id: Uuid,
    parser_version: &str,
    schema_version: i64,
    authored_by_person_id: Uuid,
    status: &str,
) -> Result<NotationDocumentVersion, NotationDocumentError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             document_id = $document_id, \
             parent_version_id = $parent_version_id, \
             markdown_asset_id = $markdown_asset_id, \
             anchor_manifest_asset_id = $anchor_manifest_asset_id, \
             parser_version = $parser_version, \
             schema_version = $schema_version, \
             authored_by_person_id = $authored_by_person_id, \
             status = $status \
             RETURN {VERSION_SELECT}"
        ))
        .bind(("id", record_id(VERSION_TABLE, id)))
        .bind(("document_id", record_id(DOCUMENT_TABLE, document.id)))
        .bind((
            "parent_version_id",
            expected_parent.map(|p| record_id(VERSION_TABLE, p)),
        ))
        .bind((
            "markdown_asset_id",
            record_id(crate::assets::TABLE, markdown_asset_id),
        ))
        .bind((
            "anchor_manifest_asset_id",
            record_id(crate::assets::TABLE, anchor_manifest_asset_id),
        ))
        .bind(("parser_version", parser_version.to_string()))
        .bind(("schema_version", schema_version))
        .bind((
            "authored_by_person_id",
            record_id("person", authored_by_person_id),
        ))
        .bind(("status", status.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<VersionRow> = response.take(0)?;
    let version = row
        .and_then(VersionRow::into_version)
        .ok_or(NotationDocumentError::WriteReturnedNothing)?;

    let mut cas_response = db
        .query(
            "UPDATE $document_id SET current_version_id = $new, updated_at = time::now() \
             WHERE current_version_id = $expected \
             RETURN VALUE current_version_id",
        )
        .bind(("document_id", record_id(DOCUMENT_TABLE, document.id)))
        .bind(("new", record_id(VERSION_TABLE, version.id)))
        .bind((
            "expected",
            expected_parent.map(|p| record_id(VERSION_TABLE, p)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let swapped: Vec<surrealdb::types::RecordId> = cas_response.take(0)?;
    if swapped.is_empty() {
        let reread = find_document_by_notation(db, document.notation_id)
            .await?
            .ok_or(NotationDocumentError::DocumentNotFound(document.id))?;
        return Err(NotationDocumentError::Conflict {
            expected: expected_parent,
            actual: reread.current_version_id,
        });
    }

    Ok(version)
}

/// Journal identifiers only — never Markdown bytes, never the anchor
/// manifest's contents. `from_state`/`to_state` carry version ids (or the
/// literal `"NONE"` for a root's absent parent) purely as opaque tokens for
/// the shared `notation_events` shape; the payload below is the structured
/// read.
async fn journal(
    db: &SurrealDb,
    notation_id: Uuid,
    acting_person_id: Uuid,
    document: &NotationDocument,
    version: &NotationDocumentVersion,
    recorded_at: &str,
    condition: &str,
) -> Result<(), NotationDocumentError> {
    let from_state = version
        .parent_version_id
        .map_or_else(|| "NONE".to_string(), |id| id.to_string());
    let payload = serde_json::json!({
        "document_id": document.id,
        "version_id": version.id,
        "parent_version_id": version.parent_version_id,
        "original_asset_id": document.original_asset_id,
        "markdown_asset_id": version.markdown_asset_id,
        "anchor_manifest_asset_id": version.anchor_manifest_asset_id,
        "parser_version": version.parser_version,
        "schema_version": version.schema_version,
        "status": version.status,
    })
    .to_string();
    crate::notation_events::append_event(
        db,
        crate::notation_events::TransitionRecord {
            notation_id,
            acting_person_id: Some(acting_person_id),
            machine_kind: MACHINE_DOCUMENT,
            from_state: &from_state,
            to_state: &version.id.to_string(),
            condition,
            payload_json: Some(payload),
            recorded_at,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        anchor_manifest_of, append_edit, current_version, find_document, history, import_root,
        markdown_of, AppendEdit, ImportRoot, NotationDocumentError, STATUS_EDITED, STATUS_IMPORTED,
    };
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    async fn fs_storage() -> std::sync::Arc<dyn cloud::StorageService> {
        let dir =
            std::env::temp_dir().join(format!("navigator-notation-documents-{}", Uuid::now_v7()));
        std::sync::Arc::new(cloud::FsStorage::new(dir).await.unwrap())
    }

    /// One notation, its project, and an "original Word asset" bare content
    /// asset standing in for a parsed `.docx` package (this module never
    /// inspects Word bytes, so any content-addressed asset does).
    async fn fixture(
        surreal: &SurrealDb,
        storage: &std::sync::Arc<dyn cloud::StorageService>,
    ) -> (Uuid, Uuid, Uuid, Uuid) {
        let notation_id = crate::test_support::seed_notation(surreal).await;
        let notation = crate::notations::find_by_id(surreal, notation_id)
            .await
            .unwrap()
            .unwrap();
        let original_asset_id = crate::assets::ingest_content(
            surreal,
            storage,
            b"PK\x03\x04 fake docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
        .unwrap();
        (
            notation_id,
            notation.project_id,
            notation.person_id,
            original_asset_id,
        )
    }

    fn import_args(
        project_id: Uuid,
        notation_id: Uuid,
        original_asset_id: Uuid,
        person_id: Uuid,
        markdown: &[u8],
    ) -> ImportRoot<'_> {
        ImportRoot {
            project_id,
            notation_id,
            original_asset_id,
            markdown,
            anchor_manifest: b"{\"blocks\":[]}",
            parser_version: "word-crate-1",
            schema_version: 1,
            authored_by_person_id: person_id,
            recorded_at: "2026-09-19T10:00:00+00:00",
        }
    }

    #[tokio::test]
    async fn import_root_creates_a_document_and_its_first_version() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;

        let saved = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap();
        assert!(saved.was_written());
        let version = saved.into_model();
        assert!(version.parent_version_id.is_none());
        assert_eq!(version.status, STATUS_IMPORTED);

        let document = find_document(&surreal, project_id, notation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(document.original_asset_id, original_asset_id);
        assert_eq!(document.current_version_id, Some(version.id));

        assert_eq!(
            markdown_of(&surreal, &storage, &version).await.unwrap(),
            "# Hello"
        );
        assert_eq!(
            anchor_manifest_of(&surreal, &storage, &version)
                .await
                .unwrap(),
            b"{\"blocks\":[]}"
        );
    }

    #[tokio::test]
    async fn reimporting_identical_bytes_is_idempotent() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;

        let first = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();

        let again = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap();
        assert!(
            !again.was_written(),
            "an identical re-import must not fork the chain"
        );
        assert_eq!(again.into_model().id, first.id);

        assert_eq!(
            history(&surreal, project_id, notation_id)
                .await
                .unwrap()
                .len(),
            1,
            "one version, not two"
        );
    }

    #[tokio::test]
    async fn a_different_original_asset_is_rejected_as_a_different_baseline() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap();

        let other_asset = crate::assets::ingest_content(
            &surreal,
            &storage,
            b"a different package",
            "application/octet-stream",
        )
        .await
        .unwrap();
        let err = import_root(
            &surreal,
            &storage,
            import_args(project_id, notation_id, other_asset, person_id, b"# Hello"),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            NotationDocumentError::OriginalAssetMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn editing_appends_a_child_version_and_both_remain_readable() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let root = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();

        let child = append_edit(
            &surreal,
            &storage,
            AppendEdit {
                project_id,
                notation_id,
                expected_parent_version_id: root.id,
                markdown: b"# Hello, edited",
                anchor_manifest: b"{\"blocks\":[]}",
                parser_version: "word-crate-1",
                schema_version: 1,
                authored_by_person_id: person_id,
                recorded_at: "2026-09-19T11:00:00+00:00",
            },
        )
        .await
        .unwrap();
        assert_eq!(child.parent_version_id, Some(root.id));
        assert_eq!(child.status, STATUS_EDITED);

        let current = current_version(&surreal, project_id, notation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.id, child.id);

        let versions = history(&surreal, project_id, notation_id).await.unwrap();
        assert_eq!(versions.len(), 2, "both versions remain readable");
        assert_eq!(versions[0].id, child.id, "newest first");
        assert_eq!(versions[1].id, root.id);

        assert_eq!(
            markdown_of(&surreal, &storage, &root).await.unwrap(),
            "# Hello",
            "the parent version's exact bytes are still readable"
        );
    }

    #[tokio::test]
    async fn a_stale_expected_parent_is_refused_as_a_conflict_not_an_overwrite() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let root = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();
        let winner = append_edit(
            &surreal,
            &storage,
            AppendEdit {
                project_id,
                notation_id,
                expected_parent_version_id: root.id,
                markdown: b"# Winner",
                anchor_manifest: b"{\"blocks\":[]}",
                parser_version: "word-crate-1",
                schema_version: 1,
                authored_by_person_id: person_id,
                recorded_at: "2026-09-19T11:00:00+00:00",
            },
        )
        .await
        .unwrap();

        // A second editor who also started from `root` (now stale) must not
        // silently overwrite the winner.
        let err = append_edit(
            &surreal,
            &storage,
            AppendEdit {
                project_id,
                notation_id,
                expected_parent_version_id: root.id,
                markdown: b"# Loser",
                anchor_manifest: b"{\"blocks\":[]}",
                parser_version: "word-crate-1",
                schema_version: 1,
                authored_by_person_id: person_id,
                recorded_at: "2026-09-19T11:05:00+00:00",
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            NotationDocumentError::Conflict {
                expected: Some(expected),
                actual: Some(actual),
            } if expected == root.id && actual == winner.id
        ));

        // The current version is still the winner's — the conflict did not
        // overwrite it.
        assert_eq!(
            current_version(&surreal, project_id, notation_id)
                .await
                .unwrap()
                .unwrap()
                .id,
            winner.id
        );
        // The loser's write is refused before any version row is created —
        // the fast-fail check in `append_edit` runs before `append_version`
        // — so the chain holds only the root and the winner's edit.
        assert_eq!(
            history(&surreal, project_id, notation_id)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// A true concurrent race — both writers pass the fast-fail check
    /// before either's compare-and-swap runs — settles on exactly one
    /// winner via the CAS in `append_version`, without panicking or
    /// forking the current pointer. The loser's version row still exists
    /// (append-only), just never becomes current.
    #[tokio::test]
    async fn concurrent_edits_against_the_same_parent_settle_on_one_winner() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let root = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();

        let racers: Vec<_> = (0..6)
            .map(|n| {
                let surreal = surreal.clone();
                let storage = storage.clone();
                let markdown = format!("# Racer {n}").into_bytes();
                tokio::spawn(async move {
                    append_edit(
                        &surreal,
                        &storage,
                        AppendEdit {
                            project_id,
                            notation_id,
                            expected_parent_version_id: root.id,
                            markdown: &markdown,
                            anchor_manifest: b"{\"blocks\":[]}",
                            parser_version: "word-crate-1",
                            schema_version: 1,
                            authored_by_person_id: person_id,
                            recorded_at: "2026-09-19T11:00:00+00:00",
                        },
                    )
                    .await
                })
            })
            .collect();

        let mut winners = 0;
        let mut conflicts = 0;
        for racer in racers {
            match racer.await.expect("task must not panic") {
                Ok(_) => winners += 1,
                Err(NotationDocumentError::Conflict { .. }) => conflicts += 1,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert_eq!(winners, 1, "exactly one racer must win the current pointer");
        assert_eq!(conflicts, 5);

        let current = current_version(&surreal, project_id, notation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.parent_version_id, Some(root.id));
    }

    #[tokio::test]
    async fn an_unrelated_project_cannot_read_or_write_the_source() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap();

        let other_project = crate::test_support::seed_project_surreal(&surreal, "unrelated").await;

        assert!(matches!(
            find_document(&surreal, other_project, notation_id).await,
            Err(NotationDocumentError::ProjectMismatch { .. })
        ));
        assert!(matches!(
            import_root(
                &surreal,
                &storage,
                import_args(
                    other_project,
                    notation_id,
                    original_asset_id,
                    person_id,
                    b"# Hi"
                ),
            )
            .await,
            Err(NotationDocumentError::ProjectMismatch { .. })
        ));
        assert!(matches!(
            append_edit(
                &surreal,
                &storage,
                AppendEdit {
                    project_id: other_project,
                    notation_id,
                    expected_parent_version_id: Uuid::now_v7(),
                    markdown: b"# Hi",
                    anchor_manifest: b"{}",
                    parser_version: "word-crate-1",
                    schema_version: 1,
                    authored_by_person_id: person_id,
                    recorded_at: "2026-09-19T11:00:00+00:00",
                },
            )
            .await,
            Err(NotationDocumentError::ProjectMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn document_source_assets_are_internal_and_content_addressed() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let version = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();

        let markdown_asset = crate::assets::find_by_id(&surreal, version.markdown_asset_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            markdown_asset.visibility,
            crate::documents::visibility::INTERNAL
        );
        let manifest_asset = crate::assets::find_by_id(&surreal, version.anchor_manifest_asset_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            manifest_asset.visibility,
            crate::documents::visibility::INTERNAL
        );

        // Re-importing the same Markdown bytes for a different notation
        // dedupes onto the same content-addressed asset.
        let (other_notation, other_project, other_person, other_original) = {
            let notation_id = crate::test_support::seed_notation(&surreal).await;
            let notation = crate::notations::find_by_id(&surreal, notation_id)
                .await
                .unwrap()
                .unwrap();
            let asset = crate::assets::ingest_content(
                &surreal,
                &storage,
                b"a second fake docx",
                "application/octet-stream",
            )
            .await
            .unwrap();
            (notation_id, notation.project_id, notation.person_id, asset)
        };
        let other_version = import_root(
            &surreal,
            &storage,
            import_args(
                other_project,
                other_notation,
                other_original,
                other_person,
                b"# Hello",
            ),
        )
        .await
        .unwrap()
        .into_model();
        assert_eq!(other_version.markdown_asset_id, version.markdown_asset_id);
    }

    #[tokio::test]
    async fn notation_events_carry_only_identifiers_never_document_content() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let root = import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"the secret clause text",
            ),
        )
        .await
        .unwrap()
        .into_model();
        append_edit(
            &surreal,
            &storage,
            AppendEdit {
                project_id,
                notation_id,
                expected_parent_version_id: root.id,
                markdown: b"the edited secret clause text",
                anchor_manifest: b"{\"blocks\":[]}",
                parser_version: "word-crate-1",
                schema_version: 1,
                authored_by_person_id: person_id,
                recorded_at: "2026-09-19T11:00:00+00:00",
            },
        )
        .await
        .unwrap();

        let events: Vec<_> = crate::notation_events::for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.machine_kind == super::MACHINE_DOCUMENT)
            .collect();
        assert_eq!(events.len(), 2);
        for event in &events {
            let payload = event.payload.as_deref().unwrap();
            assert!(!payload.contains("secret clause"));
            let value: serde_json::Value = serde_json::from_str(payload).unwrap();
            let keys: std::collections::BTreeSet<_> =
                value.as_object().unwrap().keys().cloned().collect();
            assert_eq!(
                keys,
                [
                    "document_id",
                    "version_id",
                    "parent_version_id",
                    "original_asset_id",
                    "markdown_asset_id",
                    "anchor_manifest_asset_id",
                    "parser_version",
                    "schema_version",
                    "status",
                ]
                .into_iter()
                .map(str::to_string)
                .collect()
            );
        }
    }

    #[tokio::test]
    async fn document_source_does_not_disturb_template_pinning_or_custom_clauses() {
        let surreal = mem().await;
        let storage = fs_storage().await;
        let (notation_id, project_id, person_id, original_asset_id) =
            fixture(&surreal, &storage).await;
        let notation = crate::notations::find_by_id(&surreal, notation_id)
            .await
            .unwrap()
            .unwrap();
        let pinned_template = crate::templates::find_by_id(&surreal, notation.template_id)
            .await
            .unwrap()
            .unwrap();

        crate::notation_clauses::append(&surreal, notation_id, "a custom clause", Some(person_id))
            .await
            .unwrap();

        import_root(
            &surreal,
            &storage,
            import_args(
                project_id,
                notation_id,
                original_asset_id,
                person_id,
                b"# Hello",
            ),
        )
        .await
        .unwrap();

        let clauses = crate::notation_clauses::for_notation(&surreal, notation_id)
            .await
            .unwrap();
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].body_markdown, "a custom clause");

        let template_again = crate::templates::find_by_id(&surreal, notation.template_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(template_again, pinned_template, "template row untouched");
    }
}
