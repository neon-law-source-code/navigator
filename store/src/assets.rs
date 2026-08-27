//! `store::assets` — the content-addressed lane for the `asset` table,
//! and every query against it.
//!
//! # This table lives in SurrealDB
//!
//! `assets` moved with wave five of #1093 (ENG-121), together with
//! [`crate::templates`], because `templates.asset_id` couples them:
//! `save_version` folds that id into the tuple deciding whether a template
//! version changed, so porting one without the other would leave the lane's
//! version identity in one engine and the bytes it names in the other.
//!
//! # Two shapes, one table
//!
//! [`crate::documents::ingest_bytes`] writes a **document asset** — a
//! matter artifact carrying `filename`/`kind`/provenance. This module is
//! the lower-level seam for a **bare content asset** — a template body or
//! any non-document artifact that wants only the byte pointer: sha-dedup
//! the bytes, write them to object storage at `blobs/<sha>` (an opaque,
//! content-addressed key), and insert/reuse a bare `asset` row.
//!
//! # Engine facts this module is shaped around
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! [`writing`] re-runs the statement under the crate's one retry policy,
//! [`crate::surreal::retry`]. The seed and `cli import` both ingest template
//! bodies on every boot, and the cucumber suite shares one engine across
//! concurrent scenarios, so idempotence under contention is the contract.
//!
//! **A multi-statement query is not one transaction.** A dedup probe and a
//! row insert in one query are two statements; see
//! [`crate::documents::ingest_bytes_as`] for what carries that guarantee
//! now.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use cloud::StorageService;
use serde::Serialize;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
pub(crate) const TABLE: &str = "asset";

/// One asset — the byte pointer, plus document metadata when the row is a
/// matter document.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`AssetRow`] is the seam that turns it into (and back out of) what the
/// SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Asset {
    pub id: Uuid,
    /// Object-storage key (`blobs/<sha>`). Not unique: two asset rows may
    /// point at one content-addressed object.
    pub storage_key: String,
    /// A second object-storage key holding the **same bytes**, written
    /// outside content-addressing — a generated PDF's notation key
    /// (`notations/<id>/document.pdf`). Recorded so a governed expunge
    /// deletes every copy; `None` for the common single-copy asset.
    pub secondary_storage_key: Option<String>,
    pub content_type: String,
    pub byte_size: i64,
    /// Lowercase hex SHA-256 of the byte content — the dedup key.
    pub sha256_hex: String,
    /// The matter this document belongs to; `None` for a bare content
    /// asset.
    pub project_id: Option<Uuid>,
    pub filename: Option<String>,
    pub kind: Option<String>,
    /// Inbound channel — `upload`, `email`, `generated`.
    pub source: Option<String>,
    pub received_at: Option<String>,
    pub description: Option<String>,
    /// `internal` (lawyer-only) or `client`. See
    /// [`crate::documents::visibility`].
    pub visibility: String,
    /// The document identity within a Project. `None` is a one-off
    /// artifact; `Some` makes this row one **revision** of a living
    /// document, and `(project_id, slug)` is deliberately non-unique.
    pub slug: Option<String>,
    /// Publication stamp, distinct from `received_at`. Display and sort
    /// metadata only — insertion order, not this field, decides which
    /// revision is current.
    pub published_at: Option<String>,
    /// Free-form JSON carrying per-kind detail. The ported JSONB column;
    /// the schema types it `any`, which is what lets it hold an object, a
    /// nested object, or an array. Validators belong in the `rules` crate
    /// per the S103 discipline, never in a database constraint.
    pub metadata: Option<Json>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from [`Asset`]
/// because the SDK owns its own `RecordId` and `Datetime`, and the
/// conversion belongs at this seam rather than in every caller.
#[derive(SurrealValue)]
struct AssetRow {
    id: surrealdb::types::RecordId,
    storage_key: String,
    secondary_storage_key: Option<String>,
    content_type: String,
    byte_size: i64,
    sha256_hex: String,
    project_id: Option<surrealdb::types::RecordId>,
    filename: Option<String>,
    kind: Option<String>,
    source: Option<String>,
    received_at: Option<String>,
    description: Option<String>,
    visibility: String,
    slug: Option<String>,
    published_at: Option<String>,
    metadata: Option<Json>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AssetRow {
    /// `None` when the record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_asset(self) -> Option<Asset> {
        Some(Asset {
            id: record_uuid(&self.id)?,
            storage_key: self.storage_key,
            secondary_storage_key: self.secondary_storage_key,
            content_type: self.content_type,
            byte_size: self.byte_size,
            sha256_hex: self.sha256_hex,
            // A link this module could not have written reads back as
            // "unscoped" rather than failing the whole row: the byte
            // pointer is still usable, and the project scope is what the
            // caller's own ACL already decided.
            project_id: self.project_id.as_ref().and_then(record_uuid),
            filename: self.filename,
            kind: self.kind,
            source: self.source,
            received_at: self.received_at,
            description: self.description,
            visibility: self.visibility,
            slug: self.slug,
            published_at: self.published_at,
            metadata: self.metadata,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new field cannot reach [`AssetRow`] from only one query.
pub(crate) const SELECT: &str = "id, storage_key, secondary_storage_key, content_type, byte_size, \
     sha256_hex, project_id, filename, kind, source, received_at, description, visibility, slug, \
     published_at, metadata, inserted_at, updated_at";

/// Errors from [`ingest_content`] / [`fetch`].
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("storage: {0}")]
    Storage(#[from] cloud::StorageError),
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back — see [`AssetRow::into_asset`].
    #[error("writing an asset returned no usable row")]
    WriteReturnedNothing,
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here — for this slice, none at all. How long a
/// lost race is re-run, and which engine conditions count as a lost
/// race, are one policy for the whole crate.
///
/// Still `pub(crate)`: [`crate::documents`] and [`crate::templates`]
/// write these tables too, and reach the policy through this seam.
pub(crate) async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, surrealdb::Error>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await
}

/// Read one asset out of a query response, dropping a row this module
/// could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Asset>, AssetError> {
    let row: Option<AssetRow> = response.take(0)?;
    Ok(row.and_then(AssetRow::into_asset))
}

/// Read every asset out of a query response, in the order the engine
/// returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Asset>, AssetError> {
    let rows: Vec<AssetRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(AssetRow::into_asset).collect())
}

/// The content hash every asset is stored and deduped under.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Resolve an asset by id.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Asset>, AssetError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve many assets by id in one round trip — what a listing does after
/// reading a page of rows that reference them, rather than one lookup per
/// row. Ids that match nothing are simply absent.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn find_by_ids(db: &SurrealDb, ids: &[Uuid]) -> Result<Vec<Asset>, AssetError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<surrealdb::types::RecordId> =
        ids.iter().map(|id| record_id(TABLE, *id)).collect();
    let response = db
        .query(format!("SELECT {SELECT} FROM $ids"))
        .bind(("ids", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every asset on a Project, newest first — the lawyer document listing.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn for_project(db: &SurrealDb, project_id: Uuid) -> Result<Vec<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id = $project ORDER BY id DESC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Whether any asset is scoped to this Project — the matter-delete guard.
///
/// # Errors
///
/// [`AssetError::Db`] if the lookup fails.
pub async fn exists_for_project(db: &SurrealDb, project_id: Uuid) -> Result<bool, AssetError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} WHERE project_id = $project LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let ids: Vec<surrealdb::types::RecordId> = response.take(0)?;
    Ok(!ids.is_empty())
}

/// The document asset on `project_id` carrying `filename`, if one exists.
///
/// The seeders' natural key. They deliberately dedup on `(project,
/// filename)` rather than the content hash: keying on the hash would
/// insert a second asset whenever a bundled document is re-authored,
/// leaving the matter with duplicate visible versions and an orphaned
/// blob. One filename is one seeded document.
///
/// Newest-first, so a matter that somehow holds two rows under one
/// filename resolves to the one a reader would see rather than an
/// arbitrary one.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn find_by_project_and_filename(
    db: &SurrealDb,
    project_id: Uuid,
    filename: &str,
) -> Result<Option<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE project_id = $project AND filename = $filename ORDER BY id DESC LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("filename", filename.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// The asset on `project_id` matching `(filename, sha256_hex)` — the
/// re-upload dedup probe. Deliberately narrower than the content hash
/// alone: the same bytes under a different name are a different document.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn find_filed_copy(
    db: &SurrealDb,
    project_id: Uuid,
    filename: &str,
    sha256_hex: &str,
) -> Result<Option<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id = $project \
             AND filename = $filename AND sha256_hex = $sha LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("filename", filename.to_string()))
        .bind(("sha", sha256_hex.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Set `visibility` on **every** row this matter holds for
/// `(filename, sha256_hex)`, and report how many rows changed.
///
/// Every row, not the one a dedup lookup happened to return: a matter can
/// hold several duplicates of one file (uploads filed before dedup
/// existed, concurrent submissions, other ingest paths), and updating only
/// one would leave a duplicate at its old visibility — a re-upload as
/// `internal` could otherwise leave a duplicate still client-visible and
/// its bytes reachable through the client listing and the ZIP export.
///
/// The `visibility != $target` predicate touches only the divergent rows,
/// so this is a no-op when they already agree.
///
/// # Errors
/// [`AssetError::Db`] if the update fails.
pub async fn sync_visibility(
    db: &SurrealDb,
    project_id: Uuid,
    filename: &str,
    sha256_hex: &str,
    visibility: &str,
) -> Result<usize, AssetError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE {TABLE} SET visibility = $visibility, updated_at = time::now() \
             WHERE project_id = $project AND filename = $filename \
             AND sha256_hex = $sha AND visibility != $visibility \
             RETURN id"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("filename", filename.to_string()))
        .bind(("sha", sha256_hex.to_string()))
        .bind(("visibility", visibility.to_string()))
    })
    .await?;
    let changed: Vec<surrealdb::types::RecordId> = response.take((0, "id"))?;
    Ok(changed.len())
}

/// The newest asset on `project_id` carrying `kind`, or `None`.
///
/// Newest by insertion order (`id` descending, UUIDv7), the same ordering
/// [`current`] uses — not `inserted_at`, so the two cannot disagree.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn latest_of_kind(
    db: &SurrealDb,
    project_id: Uuid,
    kind: &str,
) -> Result<Option<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id = $project AND kind = $kind \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("kind", kind.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Whether any asset on a matter **other than** `project_id` points at
/// `storage_key` — through either the primary or the secondary key.
///
/// The governed-expunge guard. A row with no `project_id` deliberately
/// does **not** count: it is unattached, not another matter, and treating
/// it as a referent would let a stray row defeat a privilege clawback,
/// which is the failure this primitive exists to prevent.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn referenced_by_another_project(
    db: &SurrealDb,
    project_id: Uuid,
    storage_key: &str,
) -> Result<bool, AssetError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} \
             WHERE (storage_key = $key OR secondary_storage_key = $key) \
             AND project_id != NONE AND project_id != $project LIMIT 1"
        ))
        .bind(("key", storage_key.to_string()))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let found: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(found.is_some())
}

/// How many assets are filed across `project_ids`, in one round trip —
/// the client portal's KPI tile, which needs the number and none of the
/// rows. An empty slice never reaches the engine.
///
/// # Errors
/// [`AssetError::Db`] if the count fails.
pub async fn count_for_projects(db: &SurrealDb, project_ids: &[Uuid]) -> Result<usize, AssetError> {
    if project_ids.is_empty() {
        return Ok(0);
    }
    let keys: Vec<surrealdb::types::RecordId> = project_ids
        .iter()
        .map(|id| record_id(crate::projects::PROJECT_TABLE, *id))
        .collect();
    let mut response = db
        .query(format!(
            "SELECT VALUE count() FROM {TABLE} WHERE project_id IN $projects GROUP ALL"
        ))
        .bind(("projects", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    // `GROUP ALL` yields no row at all when nothing matched, which is a
    // count of zero rather than an error.
    let counted: Option<i64> = response.take(0)?;
    Ok(usize::try_from(counted.unwrap_or(0)).unwrap_or(0))
}

/// `(project_id, kind)` for every asset filed across `project_ids`, in one
/// round trip — what the matter-lifecycle fold needs and nothing else, so it
/// reads a narrow projection rather than every [`Asset`] field. An empty
/// slice never reaches the engine.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn kinds_by_projects(
    db: &SurrealDb,
    project_ids: &[Uuid],
) -> Result<Vec<(Uuid, Option<String>)>, AssetError> {
    #[derive(SurrealValue)]
    struct KindRow {
        project_id: Option<surrealdb::types::RecordId>,
        kind: Option<String>,
    }

    if project_ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<surrealdb::types::RecordId> = project_ids
        .iter()
        .map(|id| record_id(crate::projects::PROJECT_TABLE, *id))
        .collect();
    let mut response = db
        .query(format!(
            "SELECT project_id, kind FROM {TABLE} WHERE project_id IN $projects"
        ))
        .bind(("projects", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<KindRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((record_uuid(r.project_id.as_ref()?)?, r.kind)))
        .collect())
}

/// Every asset, newest first — the `/lawyer/assets` transparency listing.
///
/// Unbounded. That is a real
/// cost on a large deployment and the reason this is not a hot path: it is
/// a lawyer directory, not something a matter surface reads.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Asset>, AssetError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY id DESC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Delete the asset `id`, returning the row that was removed so a caller
/// can decide what to do with its storage object. `None` when no such row
/// existed.
///
/// Deliberately does **not** touch storage: `storage_key` is not unique,
/// so whether the bytes may go is a question about the *other* rows, which
/// only the caller's own bookkeeping can answer. See
/// [`storage_key_is_referenced`].
///
/// # Errors
/// [`AssetError::Db`] if the delete fails.
pub async fn delete(db: &SurrealDb, id: Uuid) -> Result<Option<Asset>, AssetError> {
    let response = writing(|| {
        db.query("DELETE $id RETURN BEFORE")
            .bind(("id", record_id(TABLE, id)))
    })
    .await?;
    one(response)
}

/// Whether any asset row still points at `storage_key`.
///
/// The guard before deleting a content-addressed object: identical bytes
/// are stored once and may be referenced by several rows, so the object
/// outlives any one of them.
///
/// # Errors
/// [`AssetError::Db`] if the lookup fails.
pub async fn storage_key_is_referenced(
    db: &SurrealDb,
    storage_key: &str,
) -> Result<bool, AssetError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} WHERE storage_key = $key LIMIT 1"
        ))
        .bind(("key", storage_key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let found: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(found.is_some())
}

/// Ingest `bytes` as a bare content asset: dedup by SHA-256, write to
/// storage at `blobs/<sha>` when new, insert/reuse a **bare** `asset` row
/// (no document metadata), and return its id. Idempotent — re-ingesting
/// identical bare content reuses the existing bare asset, which is what
/// keeps a re-read of an unchanged template body from churning a new row.
///
/// The dedup lookup only ever reuses a *bare* asset (`filename` unset), so
/// this lane never hands back a document asset that merely shares bytes.
///
/// # Errors
/// [`AssetError`] on a storage or database failure.
pub async fn ingest_content(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    bytes: &[u8],
    content_type: &str,
) -> Result<Uuid, AssetError> {
    let sha_hex = sha256_hex(bytes);
    if let Some(existing) = find_bare_by_sha(db, &sha_hex).await? {
        return Ok(existing.id);
    }
    let storage_key = format!("blobs/{sha_hex}");
    storage.put(&storage_key, bytes, content_type).await?;
    let byte_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);

    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             storage_key = $storage_key, \
             content_type = $content_type, \
             byte_size = $byte_size, \
             sha256_hex = $sha256_hex, \
             visibility = $visibility, \
             metadata = NONE \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("storage_key", storage_key.clone()))
        .bind(("content_type", content_type.to_string()))
        .bind(("byte_size", byte_size))
        .bind(("sha256_hex", sha_hex.clone()))
        .bind((
            "visibility",
            crate::documents::visibility::INTERNAL.to_string(),
        ))
    })
    .await?;

    let row: Option<AssetRow> = response.take(0)?;
    row.and_then(AssetRow::into_asset)
        .map(|a| a.id)
        .ok_or(AssetError::WriteReturnedNothing)
}

/// The bare asset holding `sha_hex`, if one exists. Bare means
/// `filename` unset — a document asset sharing the bytes is a different
/// row with different metadata and must never be handed to this lane.
async fn find_bare_by_sha(db: &SurrealDb, sha_hex: &str) -> Result<Option<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE sha256_hex = $sha AND filename IS NONE LIMIT 1"
        ))
        .bind(("sha", sha_hex.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Fetch an asset's bytes from storage by asset id.
///
/// # Errors
/// [`AssetError`] when the row is missing or storage fails.
pub async fn fetch(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    asset_id: Uuid,
) -> Result<Vec<u8>, AssetError> {
    let row = find_by_id(db, asset_id)
        .await?
        .ok_or_else(|| AssetError::Storage(cloud::StorageError::NotFound(asset_id.to_string())))?;
    Ok(storage.get(&row.storage_key).await?.bytes)
}

/// Which revision of a slugged document a given reader is looking at.
///
/// Not a role and not an ACL — the project ACL has already decided the
/// reader may see the matter at all. This picks *which revision* of a
/// document that reader's view is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lens {
    /// Lawyers see the latest revision, published or not — the redline they
    /// are still working on.
    Lawyer,
    /// A client sees the latest revision that is both published and
    /// marked client-visible.
    Client,
}

/// The **operative version** of the document `(project_id, slug)` under
/// `lens`, or `None` when the chain is empty for that reader.
///
/// One rule gives both behaviours the asset lane needs:
///
/// - *Official ≠ latest.* An unpublished redline sitting above the
///   executed agreement changes nothing for the client — the
///   NetDocuments "Official Version" semantics without an `is_current`
///   flag to keep in sync.
/// - *Latest visible to you.* The client keeps seeing v2 while lawyer
///   iterate on v3.
///
/// **Insertion order decides current**, not `published_at`. Ids are
/// UUIDv7, so `id` descending is newest-first insertion order, and a
/// back-dated `published_at` (a court's file stamp) therefore cannot
/// reorder a chain — it stays display and sort metadata. That ordering
/// survived the port unchanged: it is a property of how the ids are
/// minted, not of the engine underneath. History remains reachable
/// through [`revisions`].
///
/// Expressed here once so no call site reimplements it and drifts.
///
/// # Errors
/// Propagates any database error.
pub async fn current(
    db: &SurrealDb,
    project_id: Uuid,
    slug: &str,
    lens: Lens,
) -> Result<Option<Asset>, AssetError> {
    let lens_filter = match lens {
        Lens::Lawyer => "",
        Lens::Client => " AND published_at IS NOT NONE AND visibility = $visible",
    };
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE project_id = $project AND slug = $slug{lens_filter} \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("slug", slug.to_string()))
        .bind(("visible", crate::documents::visibility::CLIENT.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every revision of `(project_id, slug)`, newest first.
///
/// The "dig deep and see earlier versions" path. Unfiltered by lens on
/// purpose — callers that show history to a client filter it themselves,
/// because "which revisions existed" is a different question from "which
/// one is operative".
///
/// # Errors
/// Propagates any database error.
pub async fn revisions(
    db: &SurrealDb,
    project_id: Uuid,
    slug: &str,
) -> Result<Vec<Asset>, AssetError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE project_id = $project AND slug = $slug ORDER BY id DESC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("slug", slug.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Why a [`file_revision`] call wrote nothing.
#[derive(Debug, thiserror::Error)]
pub enum RevisionError {
    /// The kind names a Markdown lane value (`workshop`, `post`) or is
    /// not vocabulary at all. The asset lane is closed: see
    /// [`rules::kind::Kind::valid_for`].
    #[error("`{0}` is not a document kind that can be filed on a matter")]
    KindNotFilable(String),
    /// The chain already exists under a different kind. A changed kind is
    /// a *different document*, not a new revision of this one.
    #[error("document `{slug}` is a `{existing}`; a `{attempted}` is a different document")]
    KindChanged {
        slug: String,
        existing: String,
        attempted: String,
    },
    #[error(transparent)]
    Ingest(#[from] crate::documents::IngestError),
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Asset(#[from] AssetError),
}

/// What [`file_revision`] did.
#[derive(Debug, Clone)]
pub enum Filed {
    /// A new revision was appended; the chain grew by one row.
    Revision(crate::documents::IngestedDocument),
    /// The bytes were identical to the chain's newest revision, so
    /// nothing was written. Carries that row's id so a caller can link to
    /// the document it already has.
    Unchanged { asset_id: Uuid },
}

/// File `bytes` as the next revision of the document `(project_id,
/// identity.slug)` — the **asset lane's write boundary**.
///
/// Three rules live here and nowhere else, so no call site reimplements
/// one and drifts:
///
/// 1. **The lane is closed.** `args.kind` must be
///    [`rules::kind::Lane::Asset`]-valid. A teaching page or a dashboard
///    skeleton is never a byte artifact filed on a matter.
/// 2. **Kind is immutable across a chain.** A slug that already holds a
///    `retainer` does not accept an `agreement` revision — that is a
///    different document and belongs under its own slug.
/// 3. **Identical bytes are a no-op**, scoped to this one slug on this
///    one project. Never a global "we already have these bytes" probe:
///    that is an existence oracle, and legal documents are boilerplate
///    plus a few low-entropy fields — exactly the shape Harnik et al.
///    (IEEE S&P 2010) showed lets an outsider confirm a document's
///    contents by uploading a guess. The same bytes under a *different*
///    slug are a genuinely different document and do create a revision.
///
/// # Errors
/// [`RevisionError`] when a rule rejects the write, or on a storage or
/// database failure.
pub async fn file_revision(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    args: &crate::documents::IngestArgs<'_>,
    identity: &crate::documents::DocumentIdentity<'_>,
    bytes: &[u8],
) -> Result<Filed, RevisionError> {
    // Rule 1 — the lane is closed. An unrecognized string is rejected by
    // the same arm as a valid-but-wrong-lane one: neither classifies a
    // document that can be filed on a matter.
    if !rules::kind::Kind::parse(args.kind).is_some_and(|k| k.valid_for(rules::kind::Lane::Asset)) {
        return Err(RevisionError::KindNotFilable(args.kind.to_string()));
    }

    // An unslugged artifact is a one-off: it is a revision of nothing, so
    // rules 2 and 3 have no chain to consult.
    let Some(slug) = identity.slug else {
        let doc = crate::documents::ingest_bytes_as(db, storage, args, identity, bytes).await?;
        return Ok(Filed::Revision(doc));
    };

    let latest = current(db, args.project_id, slug, Lens::Lawyer).await?;

    if let Some(head) = latest {
        // Rule 2 — kind is immutable across the chain.
        if let Some(existing) = head.kind.as_deref() {
            if existing != args.kind {
                return Err(RevisionError::KindChanged {
                    slug: slug.to_string(),
                    existing: existing.to_string(),
                    attempted: args.kind.to_string(),
                });
            }
        }
        // Rule 3 — identical bytes change nothing.
        if head.sha256_hex == sha256_hex(bytes) {
            return Ok(Filed::Unchanged { asset_id: head.id });
        }
    }

    let doc = crate::documents::ingest_bytes_as(db, storage, args, identity, bytes).await?;
    Ok(Filed::Revision(doc))
}

/// Every asset in the database, for tests that count rows without a
/// project scope.
#[cfg(test)]
async fn for_project_free(db: &SurrealDb) -> Vec<Asset> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE}"))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .unwrap();
    many(response).unwrap()
}

#[cfg(test)]
mod tests {
    use super::{fetch, find_by_id, ingest_content, sha256_hex, Asset};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use cloud::{FsStorage, StorageService};
    use serde_json::json;
    use std::sync::Arc;

    async fn fixtures() -> (SurrealDb, Arc<dyn StorageService>, tempfile::TempDir) {
        let db = mem().await;
        let tmp = tempfile::tempdir().unwrap();
        let storage: Arc<dyn StorageService> =
            Arc::new(FsStorage::new(tmp.path().to_path_buf()).await.unwrap());
        (db, storage, tmp)
    }

    #[tokio::test]
    async fn ingest_content_writes_a_bare_asset_and_fetch_round_trips() {
        let (db, storage, _tmp) = fixtures().await;
        let id = ingest_content(&db, &storage, b"# a template body", "text/markdown")
            .await
            .unwrap();

        let row = find_by_id(&db, id).await.unwrap().unwrap();
        assert!(row.filename.is_none(), "bare content asset has no filename");
        assert!(row.project_id.is_none());
        assert!(row.source.is_none());
        assert_eq!(row.content_type, "text/markdown");
        assert!(row.storage_key.starts_with("blobs/"));

        let bytes = fetch(&db, &storage, id).await.unwrap();
        assert_eq!(bytes, b"# a template body");
    }

    #[tokio::test]
    async fn ingest_content_dedupes_identical_bare_content() {
        let (db, storage, _tmp) = fixtures().await;
        let a = ingest_content(&db, &storage, b"same", "text/markdown")
            .await
            .unwrap();
        let b = ingest_content(&db, &storage, b"same", "text/markdown")
            .await
            .unwrap();
        assert_eq!(a, b, "identical bare content reuses the same asset row");
        assert_eq!(super::for_project_free(&db).await.len(), 1);
    }

    /// The bare-content lane must never hand back a *document* asset that
    /// merely shares bytes — the two carry different metadata and a
    /// different expunge blast radius.
    #[tokio::test]
    async fn ingest_content_does_not_reuse_a_document_asset() {
        let (db, storage, _tmp) = fixtures().await;
        let bytes = b"shared bytes";
        let project = crate::test_support::seed_project_surreal(&db, "matter").await;
        crate::documents::ingest_bytes(
            &db,
            &storage,
            &crate::documents::IngestArgs {
                project_id: project,
                source: "upload",
                filename: "doc.txt",
                kind: "unclassified",
                content_type: "text/plain",
                description: None,
                secondary_storage_key: None,
                visibility: crate::documents::visibility::INTERNAL,
            },
            bytes,
        )
        .await
        .unwrap();

        let bare = ingest_content(&db, &storage, bytes, "text/plain")
            .await
            .unwrap();
        let row = find_by_id(&db, bare).await.unwrap().unwrap();
        assert!(
            row.filename.is_none(),
            "bare-content ingest must create a fresh bare asset, not reuse the document asset"
        );
        assert_eq!(super::for_project_free(&db).await.len(), 2);
    }

    /// The headline risk of moving this table: `assets.metadata` is the
    /// ported JSONB column, and on a SCHEMAFULL table only `TYPE any`
    /// accepts every shape it carries. Each near miss fails differently
    /// and none of them fail at DEFINE time, so the round trip is pinned
    /// here rather than inferred from the schema file.
    #[tokio::test]
    async fn metadata_round_trips_every_json_shape() {
        let (db, storage, _tmp) = fixtures().await;
        let project = crate::test_support::seed_project_surreal(&db, "matter").await;

        let shapes = [
            (
                "flat object",
                json!({ "backdated_reason": "court file stamp" }),
            ),
            (
                "nested object",
                json!({ "entry": { "sequence": 12, "court": "NV-8th" }, "sealed": false }),
            ),
            (
                "array",
                json!([{ "exhibit": "A" }, { "exhibit": "B", "pages": 4 }]),
            ),
            ("bare scalar", json!(42)),
        ];

        for (shape, metadata) in shapes {
            let doc = crate::documents::ingest_bytes_as(
                &db,
                &storage,
                &crate::documents::IngestArgs {
                    project_id: project,
                    source: "upload",
                    filename: "exhibit.pdf",
                    kind: "unclassified",
                    content_type: "application/pdf",
                    description: None,
                    secondary_storage_key: None,
                    visibility: crate::documents::visibility::INTERNAL,
                },
                &crate::documents::DocumentIdentity {
                    slug: None,
                    published_at: None,
                    metadata: Some(metadata.clone()),
                },
                format!("bytes for {shape}").as_bytes(),
            )
            .await
            .unwrap();

            let read_back = find_by_id(&db, doc.asset_id).await.unwrap().unwrap();
            assert_eq!(
                read_back.metadata.as_ref(),
                Some(&metadata),
                "the {shape} shape drifted through the engine"
            );
        }
    }

    /// A row with no metadata reads back as `None`, not as a JSON null —
    /// `TYPE any` carries no NOT NULL, so the absence has
    /// to stay distinguishable from a stored null.
    #[tokio::test]
    async fn absent_metadata_reads_back_as_none() {
        let (db, storage, _tmp) = fixtures().await;
        let id = ingest_content(&db, &storage, b"no metadata here", "text/plain")
            .await
            .unwrap();
        assert_eq!(find_by_id(&db, id).await.unwrap().unwrap().metadata, None);
    }

    #[tokio::test]
    async fn a_missing_asset_reads_back_as_none() {
        let db = mem().await;
        assert!(find_by_id(&db, uuid::Uuid::now_v7())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn sha256_hex_matches_the_known_digest() {
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    /// Insertion order decides which revision is operative, and a
    /// back-dated `published_at` must not reorder the chain.
    #[tokio::test]
    async fn insertion_order_decides_current_not_published_at() {
        let (db, storage, _tmp) = fixtures().await;
        let project = crate::test_support::seed_project_surreal(&db, "matter").await;
        let file = |bytes: &'static [u8], published: &'static str| {
            let db = db.clone();
            let storage = storage.clone();
            async move {
                crate::documents::ingest_bytes_as(
                    &db,
                    &storage,
                    &crate::documents::IngestArgs {
                        project_id: project,
                        source: "upload",
                        filename: "agreement.pdf",
                        kind: "agreement",
                        content_type: "application/pdf",
                        description: None,
                        secondary_storage_key: None,
                        visibility: crate::documents::visibility::CLIENT,
                    },
                    &crate::documents::DocumentIdentity {
                        slug: Some("agreement"),
                        published_at: Some(published),
                        metadata: None,
                    },
                    bytes,
                )
                .await
                .unwrap()
            }
        };

        let first = file(b"v1", "2026-06-01T00:00:00Z").await;
        // The second revision is *back-dated* below the first.
        let second = file(b"v2", "2020-01-01T00:00:00Z").await;

        let operative = super::current(&db, project, "agreement", super::Lens::Lawyer)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            operative.id, second.asset_id,
            "the later insertion is current even though it is back-dated"
        );

        let chain: Vec<_> = super::revisions(&db, project, "agreement")
            .await
            .unwrap()
            .into_iter()
            .map(|a: Asset| a.id)
            .collect();
        assert_eq!(chain, vec![second.asset_id, first.asset_id]);
    }

    /// The client lens skips an unpublished or lawyer-only revision sitting
    /// above the one they executed.
    #[tokio::test]
    async fn the_client_lens_stops_at_the_latest_published_client_revision() {
        let (db, storage, _tmp) = fixtures().await;
        let project = crate::test_support::seed_project_surreal(&db, "matter").await;
        let file = |bytes: &'static [u8], published: Option<&'static str>, vis: &'static str| {
            let db = db.clone();
            let storage = storage.clone();
            async move {
                crate::documents::ingest_bytes_as(
                    &db,
                    &storage,
                    &crate::documents::IngestArgs {
                        project_id: project,
                        source: "upload",
                        filename: "agreement.pdf",
                        kind: "agreement",
                        content_type: "application/pdf",
                        description: None,
                        secondary_storage_key: None,
                        visibility: vis,
                    },
                    &crate::documents::DocumentIdentity {
                        slug: Some("agreement"),
                        published_at: published,
                        metadata: None,
                    },
                    bytes,
                )
                .await
                .unwrap()
            }
        };

        let executed = file(
            b"executed",
            Some("2026-06-01T00:00:00Z"),
            crate::documents::visibility::CLIENT,
        )
        .await;
        // Lawyers keep working above it: an unpublished redline…
        let redline = file(b"redline", None, crate::documents::visibility::CLIENT).await;

        assert_eq!(
            super::current(&db, project, "agreement", super::Lens::Lawyer)
                .await
                .unwrap()
                .unwrap()
                .id,
            redline.asset_id,
            "lawyers see the redline they are still working on"
        );
        assert_eq!(
            super::current(&db, project, "agreement", super::Lens::Client)
                .await
                .unwrap()
                .unwrap()
                .id,
            executed.asset_id,
            "the client stays anchored to the executed revision"
        );
    }
}
