//! `store::documents` — write-side primitive for **document assets**.
//!
//! A matter document (a portal upload, an inbound-email attachment, a
//! generated PDF) is one `asset` row carrying both the byte pointer and
//! the document metadata. [`ingest_bytes`] is the one entry point. Given
//! a project, bytes, and provenance, it:
//!
//! 1. Computes a SHA-256 of the bytes.
//! 2. If an `asset` row **on this project** already references that
//!    content, the object is already stored (`blobs/<sha>`,
//!    content-addressed) — the storage write is skipped. Otherwise it
//!    writes the bytes through [`cloud::StorageService`].
//! 3. Inserts one `asset` row carrying the byte pointer plus the
//!    inbound-channel provenance (`source`, `received_at`), the
//!    `filename`/`kind`, and the optional lawyer-view `description`.
//!
//! # Ordering, not a transaction
//!
//! A multi-statement Surreal query is not one transaction, so steps 2–3
//! do not share one. They do not need to. The guarantee to hold is "never
//! leave a row pointing at a storage key
//! that does not exist", and that is a property of the **order**: the
//! object is written before the row that names it, so a failure between
//! them leaves an unreferenced object rather than a dangling row. An
//! unreferenced object at a content-addressed key is inert — the next
//! ingest of the same bytes reuses it.
//!
//! The dedup probe losing a race is equally benign: both writers then
//! `put` identical bytes to the same content-addressed key, and `put` is
//! idempotent on both `FsStorage` and `GcsStorage`. The only observable
//! difference is a redundant write, never a wrong byte.
//!
//! The bare-content lane (template bodies, raw `.eml`) is
//! [`crate::assets::ingest_content`], which writes an `asset` row with
//! the document-metadata fields left unset.

use std::sync::Arc;

use chrono::Utc;
use cloud::{StorageError, StorageService};
use uuid::Uuid;

use crate::assets::{sha256_hex as asset_sha256_hex, SELECT, TABLE};
use crate::surreal::{record_id, SurrealDb};

/// Inbound-channel literals written to `assets.source`. Centralized here
/// so handlers and tests agree on the same strings; mismatches turn into
/// silent dedup-planner bugs.
pub mod source {
    pub const UPLOAD: &str = "upload";
    /// An attachment received on inbound `support@` mail (see
    /// `portal::email_threads`).
    pub const EMAIL: &str = "email";
    /// A document the workflow engine rendered itself — the PDF a
    /// `generate_pdf__*` step produces (retainer, trust, filled
    /// government packet). Not an inbound channel; the bytes originate
    /// in-house from the template + answers.
    pub const GENERATED: &str = "generated";
}

/// Client-portal visibility literals written to `assets.visibility`. A
/// closed vocabulary enforced by the schema ASSERT on `asset.visibility`;
/// centralized here so every caller states its intent explicitly rather
/// than falling back to a silent default (#782).
pub mod visibility {
    /// Lawyer-only — the default. Attorney work product (`review_memo`) and
    /// `unclassified` lawyer/email uploads.
    pub const INTERNAL: &str = "internal";
    /// Listed in the client's "Your documents".
    pub const CLIENT: &str = "client";
}

/// Errors surfaced by [`ingest_bytes`].
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Asset(#[from] crate::assets::AssetError),
    /// The insert reported success but returned no row this module could
    /// read back.
    #[error("writing a document asset returned no usable row")]
    WriteReturnedNothing,
    /// `args.kind` is not a [`rules::kind::Kind`] valid in
    /// [`rules::kind::Lane::Asset`]. The ingest boundary rejects rather than
    /// silently coercing to `unclassified` — a lane whose whole job is
    /// classification should refuse a bad classification loudly, not file it
    /// under a value the caller never chose.
    #[error("`{0}` is not a valid asset-lane kind")]
    InvalidKind(String),
}

/// Inputs to [`ingest_bytes`]. Held in a struct (rather than a long
/// positional list) so future fields don't break callers.
#[derive(Debug, Clone)]
pub struct IngestArgs<'a> {
    /// Project the document belongs to.
    pub project_id: Uuid,
    /// Inbound channel name — `upload`, `email`, `generated`. Goes into
    /// `assets.source`.
    pub source: &'a str,
    /// Caller-visible filename. Goes into `assets.filename`.
    pub filename: &'a str,
    /// Document classification — one of [`rules::kind::Kind::valid_for`]
    /// [`rules::kind::Lane::Asset`] (`onboarding`, `offboarding`,
    /// `unclassified`, …). Goes into `assets.kind`; [`ingest_bytes`] rejects
    /// any other value.
    pub kind: &'a str,
    /// MIME content type of `bytes`.
    pub content_type: &'a str,
    /// Optional lawyer-view caption; goes into `assets.description`.
    pub description: Option<&'a str>,
    /// Client-portal visibility — `visibility::INTERNAL` or
    /// `visibility::CLIENT`. Every caller states this explicitly (no silent
    /// default at the Rust layer) even though the column itself defaults to
    /// `internal`, so a call site that should be client-visible can't
    /// silently forget to say so.
    pub visibility: &'a str,
    /// A second object-storage key holding the **same bytes**, written
    /// outside content-addressing — a generated PDF's notation key
    /// (`notations/<id>/document.pdf`). Stamped onto the asset row in the
    /// **same transaction** as the insert so every asset row for the bytes
    /// carries it, and a governed expunge deletes every copy (#470). `None`
    /// for the common single-copy ingest.
    pub secondary_storage_key: Option<&'a str>,
}

/// The **document identity** an ingest optionally carries — the asset
/// lane's slug, publication stamp, and free-form detail.
///
/// Separate from [`IngestArgs`] rather than three more fields on it
/// because the two answer different questions. `IngestArgs` describes
/// *these bytes*: where they came from, what they are, who may see them.
/// This describes *which document they are a revision of*, and almost
/// every caller has no answer — an inbound email attachment or a
/// generated PDF is a one-off artifact, not a revision of anything. Those
/// callers use [`ingest_bytes`] and get [`DocumentIdentity::default`],
/// a `NULL` slug.
///
/// A caller that *does* know the document identity goes through
/// [`crate::assets::file_revision`], which enforces the asset lane's
/// write rules rather than writing a row directly.
#[derive(Debug, Clone, Default)]
pub struct DocumentIdentity<'a> {
    /// The lawyer-chosen document identity within the Project, or `None`
    /// for a one-off artifact. Never derived from the filename: a
    /// re-upload named `captable_final_v2.pdf` must not fork a chain, and
    /// two unrelated `agreement.pdf`s must not merge into one.
    pub slug: Option<&'a str>,
    /// RFC 3339 publication stamp, back-datable to a court's file stamp.
    /// Distinct from `received_at`, which every ingest stamps with the
    /// moment the bytes arrived.
    pub published_at: Option<&'a str>,
    /// Free-form per-kind detail — a back-dating reason, a court's entry
    /// sequence. Validators belong in the `rules` crate, never a database
    /// CHECK.
    pub metadata: Option<serde_json::Value>,
}

/// What [`ingest_bytes`] writes, returned for the caller to log /
/// reference / show in a UI.
#[derive(Debug, Clone)]
pub struct IngestedDocument {
    /// The `assets` row id for this document.
    pub asset_id: Uuid,
    pub sha256_hex: String,
    pub byte_size: i64,
    /// `true` when the bytes were already stored under another asset —
    /// no new storage write happened (the content-addressed object was
    /// reused).
    pub reused: bool,
}

/// Ingest one artifact: write the bytes (if new), insert the document
/// `asset` row, return the id.
///
/// # Errors
/// [`IngestError`] on a storage or database failure.
pub async fn ingest_bytes(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    args: &IngestArgs<'_>,
    bytes: &[u8],
) -> Result<IngestedDocument, IngestError> {
    ingest_bytes_as(db, storage, args, &DocumentIdentity::default(), bytes).await
}

/// [`ingest_bytes`], filing the bytes as a revision of the document named
/// by `identity`.
///
/// The raw write. It does **not** enforce the asset lane's rules — that
/// is [`crate::assets::file_revision`]'s job, and slugged callers should
/// go through it so no call site reimplements the no-op and immutability
/// checks and drifts.
///
/// # Errors
/// Propagates database and storage errors.
pub async fn ingest_bytes_as(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    args: &IngestArgs<'_>,
    identity: &DocumentIdentity<'_>,
    bytes: &[u8],
) -> Result<IngestedDocument, IngestError> {
    if !rules::kind::Kind::parse(args.kind).is_some_and(|k| k.valid_for(rules::kind::Lane::Asset)) {
        return Err(IngestError::InvalidKind(args.kind.to_string()));
    }

    let sha_hex = sha256_hex(bytes);
    let byte_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let storage_key = format!("blobs/{sha_hex}");

    // Storage dedup, scoped to this matter: if an asset **on this project**
    // already references this content, the object is stored; otherwise write
    // it once. Write before inserting the row so a crash mid-ingest never
    // leaves an `asset` row pointing at a key that doesn't exist — the
    // ordering is what carries that, not a transaction (see the module doc).
    //
    // The `project_id` filter is the whole point: deduping across matters
    // made one matter's governed expunge destroy another's document, because
    // `portal::expunge` deletes the asset's `blobs/<sha>` and a second
    // matter's row pointed at that same object. Two matters holding the same
    // exhibit is ordinary; a sealing order on one silently emptying the other
    // is not. Bytes are cheap, cross-matter coupling is not.
    let reused = project_holds_content(db, args.project_id, &sha_hex).await?;
    if !reused {
        storage.put(&storage_key, bytes, args.content_type).await?;
    }

    let asset_id = insert_asset_row(db, args, identity, &storage_key, &sha_hex, byte_size).await?;

    Ok(IngestedDocument {
        asset_id,
        sha256_hex: sha_hex,
        byte_size,
        reused,
    })
}

/// Whether this matter already holds an asset referencing `sha_hex`.
async fn project_holds_content(
    db: &SurrealDb,
    project_id: Uuid,
    sha_hex: &str,
) -> Result<bool, IngestError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} \
             WHERE sha256_hex = $sha AND project_id = $project LIMIT 1"
        ))
        .bind(("sha", sha_hex.to_string()))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let found: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(found.is_some())
}

/// The content hash every document is stored and deduped under.
///
/// Public so a caller that needs to ask "does this project already hold
/// these bytes?" hashes them exactly the way ingest does — a second
/// implementation would drift and silently stop matching. Delegates to
/// [`crate::assets::sha256_hex`] so both lanes share one digest.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    asset_sha256_hex(bytes)
}

async fn insert_asset_row(
    db: &SurrealDb,
    args: &IngestArgs<'_>,
    identity: &DocumentIdentity<'_>,
    storage_key: &str,
    sha_hex: &str,
    byte_size: i64,
) -> Result<Uuid, IngestError> {
    let id = Uuid::now_v7();
    let mut response = crate::assets::writing(|| {
        db.query(format!(
            "CREATE $id SET \
             storage_key = $storage_key, \
             secondary_storage_key = $secondary_storage_key, \
             content_type = $content_type, \
             byte_size = $byte_size, \
             sha256_hex = $sha256_hex, \
             project_id = $project_id, \
             filename = $filename, \
             kind = $kind, \
             source = $source, \
             received_at = $received_at, \
             description = $description, \
             visibility = $visibility, \
             slug = $slug, \
             published_at = $published_at, \
             metadata = $metadata \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("storage_key", storage_key.to_string()))
        .bind((
            "secondary_storage_key",
            args.secondary_storage_key.map(String::from),
        ))
        .bind(("content_type", args.content_type.to_string()))
        .bind(("byte_size", byte_size))
        .bind(("sha256_hex", sha_hex.to_string()))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, args.project_id),
        ))
        .bind(("filename", Some(args.filename.to_string())))
        .bind(("kind", Some(args.kind.to_string())))
        .bind(("source", Some(args.source.to_string())))
        .bind(("received_at", Some(Utc::now().to_rfc3339())))
        .bind(("description", args.description.map(String::from)))
        .bind(("visibility", args.visibility.to_string()))
        .bind(("slug", identity.slug.map(String::from)))
        .bind(("published_at", identity.published_at.map(String::from)))
        .bind(("metadata", identity.metadata.clone()))
    })
    .await?;

    let written: Option<surrealdb::types::RecordId> = response.take((0, "id"))?;
    written
        .as_ref()
        .and_then(crate::surreal::record_uuid)
        .ok_or(IngestError::WriteReturnedNothing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::find_by_id;
    use crate::surreal::test_support::mem;
    use cloud::FsStorage;

    async fn fixtures() -> (SurrealDb, Arc<dyn StorageService>, tempfile::TempDir, Uuid) {
        let db = mem().await;
        let tmp = tempfile::tempdir().unwrap();
        let storage: Arc<dyn StorageService> =
            Arc::new(FsStorage::new(tmp.path().to_path_buf()).await.unwrap());
        let project_id = crate::test_support::seed_project_surreal(&db, "Test Matter").await;
        (db, storage, tmp, project_id)
    }

    #[tokio::test]
    async fn ingest_writes_a_document_asset_with_provenance() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "retainer.pdf",
            kind: "onboarding",
            content_type: "application/pdf",
            description: Some("client-signed retainer"),
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };

        let out = ingest_bytes(&db, &storage, &args, b"hello world")
            .await
            .unwrap();

        assert!(!out.reused);
        assert_eq!(out.byte_size, 11);
        // sha256("hello world") =
        //   b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert_eq!(
            out.sha256_hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        let a = find_by_id(&db, out.asset_id)
            .await
            .unwrap()
            .expect("asset row");
        assert_eq!(a.content_type, "application/pdf");
        assert_eq!(a.byte_size, 11);
        assert_eq!(a.storage_key, format!("blobs/{}", out.sha256_hex));
        assert_eq!(a.filename.as_deref(), Some("retainer.pdf"));
        assert_eq!(a.kind.as_deref(), Some("onboarding"));
        assert_eq!(a.project_id, Some(project_id));
        assert_eq!(a.source.as_deref(), Some("upload"));
        assert_eq!(a.description.as_deref(), Some("client-signed retainer"));
        assert!(
            a.received_at.is_some(),
            "received_at must be stamped on insert"
        );
        assert_eq!(a.visibility, visibility::INTERNAL);

        let stored = storage.get(&a.storage_key).await.unwrap();
        assert_eq!(stored.bytes, b"hello world");
    }

    #[tokio::test]
    async fn ingest_dedupes_storage_but_keeps_distinct_asset_rows() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let bytes = b"same bytes";
        let mk = |fname: &'static str| IngestArgs {
            project_id,
            source: "upload",
            filename: fname,
            kind: "unclassified",
            content_type: "text/plain",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };

        let first = ingest_bytes(&db, &storage, &mk("a.txt"), bytes)
            .await
            .unwrap();
        assert!(!first.reused);

        let second = ingest_bytes(&db, &storage, &mk("b.txt"), bytes)
            .await
            .unwrap();
        assert!(second.reused, "the content-addressed object is reused");
        assert_ne!(
            second.asset_id, first.asset_id,
            "each document ingest is a distinct asset row, even sharing bytes"
        );

        // Two asset rows on this matter, one shared storage key. Scoped to
        // the project rather than counted table-wide: the cucumber suite
        // shares one engine across concurrent scenarios.
        let assets = crate::assets::for_project(&db, project_id).await.unwrap();
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].storage_key, assets[1].storage_key);
    }

    #[tokio::test]
    async fn ingest_does_not_dedupe_across_matters() {
        // Dedup is scoped to one matter. Two matters that happen to receive
        // identical bytes each get their own storage write.
        //
        // Global dedup made one matter's governed expunge destroy another's
        // document: `portal::expunge` deletes the asset's `blobs/<sha>`, and a
        // second matter's row pointed at that same object. Two clients sharing
        // a common exhibit is ordinary; one client's sealing order silently
        // emptying an unrelated matter is not.
        let (db, storage, _tmp, first) = fixtures().await;
        let second = crate::test_support::seed_project_surreal(&db, "Second Matter").await;
        let bytes = b"an exhibit filed on two matters";
        let mk = |project_id: Uuid| IngestArgs {
            project_id,
            source: "upload",
            filename: "exhibit.pdf",
            kind: "unclassified",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };

        let a = ingest_bytes(&db, &storage, &mk(first), bytes)
            .await
            .unwrap();
        assert!(!a.reused);

        let b = ingest_bytes(&db, &storage, &mk(second), bytes)
            .await
            .unwrap();
        assert!(
            !b.reused,
            "a second matter must own its copy, not reuse the first matter's object"
        );

        // Same content hash, so the same content-addressed key — but the write
        // happened once per matter, which is what `reused: false` records.
        assert_eq!(a.sha256_hex, b.sha256_hex);
        assert_ne!(a.asset_id, b.asset_id);
    }

    #[tokio::test]
    async fn ingest_different_bytes_produces_different_assets() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let mk = |fname: &'static str| IngestArgs {
            project_id,
            source: "upload",
            filename: fname,
            kind: "unclassified",
            content_type: "text/plain",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };
        let a = ingest_bytes(&db, &storage, &mk("a.txt"), b"alpha")
            .await
            .unwrap();
        let b = ingest_bytes(&db, &storage, &mk("b.txt"), b"bravo")
            .await
            .unwrap();
        assert_ne!(a.asset_id, b.asset_id);
        assert_ne!(a.sha256_hex, b.sha256_hex);
    }

    #[tokio::test]
    async fn ingest_stamps_the_secondary_key_with_the_row() {
        // #470: a generated PDF's notation-key copy is recorded on the asset
        // row by the SAME statement that creates it, so every asset row for
        // the bytes carries it and a governed expunge can delete every copy —
        // no window where a committed row is missing its second key.
        let (db, storage, _tmp, project_id) = fixtures().await;
        let notation_key = "notations/abc/document.pdf";
        let args = IngestArgs {
            project_id,
            source: source::GENERATED,
            filename: "document.pdf",
            kind: "onboarding",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: Some(notation_key),
            visibility: visibility::INTERNAL,
        };
        let ingested = ingest_bytes(&db, &storage, &args, b"%PDF-1.7 body")
            .await
            .unwrap();

        let row = find_by_id(&db, ingested.asset_id).await.unwrap().unwrap();
        assert_eq!(row.secondary_storage_key.as_deref(), Some(notation_key));
        // The canonical content-addressed key is unchanged.
        assert_eq!(row.storage_key, format!("blobs/{}", ingested.sha256_hex));
    }

    #[tokio::test]
    async fn ingest_leaves_the_secondary_key_unset_by_default() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "a.txt",
            kind: "unclassified",
            content_type: "text/plain",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };
        let ingested = ingest_bytes(&db, &storage, &args, b"plain").await.unwrap();
        let row = find_by_id(&db, ingested.asset_id).await.unwrap().unwrap();
        assert_eq!(row.secondary_storage_key, None);
    }

    #[tokio::test]
    async fn ingest_stamps_the_caller_stated_visibility() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "client-visible.pdf",
            kind: "unclassified",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::CLIENT,
        };
        let ingested = ingest_bytes(&db, &storage, &args, b"client bytes")
            .await
            .unwrap();
        let row = find_by_id(&db, ingested.asset_id).await.unwrap().unwrap();
        assert_eq!(row.visibility, visibility::CLIENT);
    }

    /// The schema ASSERT is what closes the visibility vocabulary.
    /// A typo'd visibility would drop a document out of the client's "Your
    /// documents" silently rather than fail, so it must fail at write time.
    #[tokio::test]
    async fn an_unknown_visibility_is_refused_by_the_schema() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "typo.pdf",
            kind: "unclassified",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: "clientt",
        };
        assert!(
            ingest_bytes(&db, &storage, &args, b"bytes").await.is_err(),
            "the engine accepted visibility `clientt`"
        );
    }

    /// The ingest boundary is the asset lane's write gate: a `kind` outside
    /// `rules::kind::Kind::valid_for(Lane::Asset)` must be refused before any
    /// bytes are written or any row is inserted, not silently coerced to
    /// `unclassified`.
    #[tokio::test]
    async fn ingest_bytes_rejects_a_kind_outside_the_asset_lane() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "typo.pdf",
            kind: "bogus",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };
        assert!(
            matches!(
                ingest_bytes(&db, &storage, &args, b"bytes").await,
                Err(IngestError::InvalidKind(k)) if k == "bogus"
            ),
            "the engine accepted kind `bogus`"
        );
    }

    /// A template-lane-only kind (a matter dashboard, a content page) is not
    /// a document classification either — the same lane split
    /// `rules::kind::Kind::valid_for` enforces on the template side must hold
    /// on the asset side.
    #[tokio::test]
    async fn ingest_bytes_rejects_a_template_lane_only_kind() {
        let (db, storage, _tmp, project_id) = fixtures().await;
        let args = IngestArgs {
            project_id,
            source: "upload",
            filename: "not-a-workshop.pdf",
            kind: "workshop",
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: visibility::INTERNAL,
        };
        assert!(
            ingest_bytes(&db, &storage, &args, b"bytes").await.is_err(),
            "the engine accepted the template-lane-only kind `workshop`"
        );
    }
}
