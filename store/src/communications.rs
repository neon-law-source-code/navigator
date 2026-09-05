//! `store::communications` — write + read side of the project-scoped,
//! attorney-client privileged conversation log.
//!
//! [`ingest`] is the one write entry point, the analogue of
//! [`crate::documents::ingest_bytes`]: it maps a message from any channel
//! into a [`Communication`] spine row. It is idempotent on
//! `(channel, source_ref)` — a re-delivered email or re-ingested source
//! returns the existing row instead of duplicating, so callers can replay
//! safely.
//!
//! [`for_project`] is the read side: the whole thread for one matter,
//! oldest→newest, the way the conversation view renders it.
//!
//! Channel-specific fidelity (a comment's anchor, an email's headers) lives
//! in satellites that FK back to the row this returns. Privilege is enforced
//! one layer up (`store::access::can_see_project`); this module never widens a
//! query past `project_id`.
//!
//! # This table lives in SurrealDB
//!
//! `communications` moved with wave six of #1093 (ENG-160), in the
//! communications slice alongside [`crate::email_conversations`].

use std::sync::Arc;

use chrono::{DateTime, Months, Utc};
use cloud::{StorageError, StorageService};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "communication";
const PERSON_TABLE: &str = "person";

/// How long a matter's privileged conversation log is kept after the matter
/// closes, then securely destroyed. Firm policy — the client consents to it
/// in the retainer ("Your file, kept for ten years"). Exceeds the NV RPC
/// file-retention floor, so it is the controlling number.
pub const RETENTION_YEARS: u32 = 10;

/// Channel literals written to `communications.channel`. Centralized so the
/// ingest seams, the thread view, and tests agree on the same strings — a
/// mismatch is a silent "message vanished from the thread" bug. SMS is here
/// already so wiring it up later is a caller change, not a schema change.
pub mod channel {
    pub const DOCUMENT_COMMENT: &str = "document_comment";
    pub const EMAIL_INBOUND: &str = "email_inbound";
    pub const EMAIL_OUTBOUND: &str = "email_outbound";
    pub const PORTAL_MESSAGE: &str = "portal_message";
    pub const SMS_INBOUND: &str = "sms_inbound";
    pub const SMS_OUTBOUND: &str = "sms_outbound";
}

/// Direction literals written to `communications.direction`.
pub mod direction {
    /// From the client to the firm.
    pub const INBOUND: &str = "inbound";
    /// From the firm to the client.
    pub const OUTBOUND: &str = "outbound";
    /// A firm-internal note — never shown to the client.
    pub const INTERNAL: &str = "internal";
}

/// One message in a matter's privileged conversation log.
///
/// The application-facing shape: plain Rust types, no engine handles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Communication {
    pub id: Uuid,
    /// The matter this message belongs to.
    pub project_id: Uuid,
    /// Channel discriminator — one of [`channel`].
    pub channel: String,
    /// `inbound`, `outbound`, or `internal` — one of [`direction`].
    pub direction: String,
    /// The [`crate::persons`] row that authored it; `None` for a system or
    /// unknown sender.
    pub author_person_id: Option<Uuid>,
    /// Email/name of the other party when there is no `persons` row.
    pub counterparty: Option<String>,
    /// Optional subject line (email subject; `None` for comments).
    pub subject: Option<String>,
    /// Normalized message text — the only body representation.
    pub body: String,
    /// External id for idempotent ingest: email `Message-ID`, the comment
    /// id, the SMS provider id.
    pub source_ref: Option<String>,
    /// Raw payload (verbatim `.eml`, …); `None` for comments.
    pub asset_id: Option<Uuid>,
    /// RFC 3339 timestamp of when the message actually happened.
    pub occurred_at: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct CommunicationRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    channel: String,
    direction: String,
    author_person_id: Option<surrealdb::types::RecordId>,
    counterparty: Option<String>,
    subject: Option<String>,
    body: String,
    source_ref: Option<String>,
    asset_id: Option<surrealdb::types::RecordId>,
    occurred_at: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl CommunicationRow {
    fn into_communication(self) -> Option<Communication> {
        Some(Communication {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            channel: self.channel,
            direction: self.direction,
            author_person_id: self.author_person_id.as_ref().and_then(record_uuid),
            counterparty: self.counterparty,
            subject: self.subject,
            body: self.body,
            source_ref: self.source_ref,
            asset_id: self.asset_id.as_ref().and_then(record_uuid),
            occurred_at: self.occurred_at,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares.
const SELECT: &str = "id, project_id, channel, direction, author_person_id, counterparty, \
                      subject, body, source_ref, asset_id, occurred_at, inserted_at, updated_at";

/// What can go wrong reading or writing the conversation log.
#[derive(Debug, thiserror::Error)]
pub enum CommunicationError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
}

/// Inputs to [`ingest`]. Held in a struct (not a positional list) so future
/// fields don't break callers, mirroring [`crate::documents::IngestArgs`].
#[derive(Debug, Clone)]
pub struct IngestArgs<'a> {
    /// Matter this message belongs to.
    pub project_id: Uuid,
    /// Channel literal — one of [`channel`].
    pub channel: &'a str,
    /// Direction literal — one of [`direction`].
    pub direction: &'a str,
    /// Author, when we have a `persons` row for them.
    pub author_person_id: Option<Uuid>,
    /// Email/name of the other party when there is no `persons` row.
    pub counterparty: Option<&'a str>,
    /// Optional subject line.
    pub subject: Option<&'a str>,
    /// Normalized message text.
    pub body: &'a str,
    /// External id for idempotent ingest (`Message-ID`, comment id, SMS id).
    /// When `Some`, a second ingest with the same `(channel, source_ref)`
    /// returns the existing row.
    pub source_ref: Option<&'a str>,
    /// Raw payload asset, when one was archived (verbatim `.eml`, …).
    pub asset_id: Option<Uuid>,
    /// When the message actually happened (RFC 3339). Distinct from the
    /// insert time, which the row stamps itself.
    pub occurred_at: &'a str,
}

/// What [`ingest`] resolved to.
#[derive(Debug, Clone, Copy)]
pub struct Ingested {
    pub communication_id: Uuid,
    /// `true` when an existing row with the same `(channel, source_ref)`
    /// was returned instead of inserting — the caller replayed a source.
    pub deduped: bool,
}

/// Ingest one message into the conversation log. Idempotent on
/// `(channel, source_ref)` when `source_ref` is `Some`.
///
/// Two halves, and both are load-bearing. The lookup below is the common
/// path — a replayed source returns the existing row as `deduped` rather
/// than an error. The `communication_channel_source_ref` unique index is
/// the guarantee: two racers past the lookup at once cannot both insert,
/// and the loser re-reads the winner's row instead of duplicating a
/// client's message in their own privileged thread.
///
/// An absent `source_ref` does not collide — many rows may carry `NONE`
/// under one unique index — so a document comment, which has no external
/// id, is unrestricted.
///
/// # Errors
///
/// Propagates any database error.
pub async fn ingest(db: &SurrealDb, args: &IngestArgs<'_>) -> Result<Ingested, CommunicationError> {
    if let Some(source_ref) = args.source_ref {
        if let Some(row) = find_by_source(db, args.channel, source_ref).await? {
            return Ok(Ingested {
                communication_id: row.id,
                deduped: true,
            });
        }
    }

    let id = Uuid::now_v7();
    let created = db
        .query(format!(
            "CREATE $id SET \
             project_id = $project_id, channel = $channel, direction = $direction, \
             author_person_id = $author, counterparty = $counterparty, subject = $subject, \
             body = $body, source_ref = $source_ref, asset_id = $asset_id, \
             occurred_at = $occurred_at \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, args.project_id),
        ))
        .bind(("channel", args.channel.to_string()))
        .bind(("direction", args.direction.to_string()))
        .bind((
            "author",
            args.author_person_id.map(|p| record_id(PERSON_TABLE, p)),
        ))
        .bind(("counterparty", args.counterparty.map(str::to_string)))
        .bind(("subject", args.subject.map(str::to_string)))
        .bind(("body", args.body.to_string()))
        .bind(("source_ref", args.source_ref.map(str::to_string)))
        .bind((
            "asset_id",
            args.asset_id.map(|a| record_id(crate::assets::TABLE, a)),
        ))
        .bind(("occurred_at", args.occurred_at.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check);

    let mut response = match created {
        Ok(response) => response,
        Err(error) if is_duplicate_source_ref(&error) => {
            // A racer won between our lookup and this insert. Return their
            // row: the whole point of the key is that one source produces
            // one message.
            let Some(source_ref) = args.source_ref else {
                return Err(CommunicationError::Db(error));
            };
            let existing = find_by_source(db, args.channel, source_ref).await?;
            return existing
                .map(|row| Ingested {
                    communication_id: row.id,
                    deduped: true,
                })
                .ok_or(CommunicationError::Db(error));
        }
        Err(error) => return Err(CommunicationError::Db(error)),
    };
    let row: Option<CommunicationRow> = response.take(0)?;

    Ok(Ingested {
        communication_id: row
            .and_then(CommunicationRow::into_communication)
            .map_or(id, |c| c.id),
        deduped: false,
    })
}

/// Whether this failure is the `(channel, source_ref)` key rejecting a
/// duplicate.
///
/// Discriminated on the index name rather than a typed detail: a unique
/// violation arrives as the untyped `Internal` catch-all every
/// unclassified failure also uses, so the index name — which the workspace
/// owns and no other failure mentions — is the only reliable signal.
fn is_duplicate_source_ref(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("communication_channel_source_ref")
}

/// The row a `(channel, source_ref)` pair already resolves to, if any.
async fn find_by_source(
    db: &SurrealDb,
    channel: &str,
    source_ref: &str,
) -> Result<Option<Communication>, CommunicationError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE channel = $channel AND source_ref = $source_ref LIMIT 1"
        ))
        .bind(("channel", channel.to_string()))
        .bind(("source_ref", source_ref.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<CommunicationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .find_map(CommunicationRow::into_communication))
}

/// The whole conversation for one matter, oldest→newest — the order the
/// thread view renders. This is the **firm** view: every row, internal notes
/// included. Whether the caller may read this `project_id` at all is the
/// access layer's job (`store::access::can_see_project`); a client gets
/// [`for_project_client_visible`] instead.
///
/// # Errors
///
/// Propagates any database error.
pub async fn for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Communication>, CommunicationError> {
    read_thread(db, project_id, None).await
}

/// Whether the matter's thread holds any message at all — including a
/// firm-internal note, which [`for_project_client_visible`] hides but which
/// is still a reference. The matter-delete guard.
///
/// # Errors
///
/// Propagates any database error.
pub async fn exists_for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<bool, CommunicationError> {
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

/// The conversation as a **client** may see it: every row except firm-internal
/// notes (`direction = internal`). Internal notes are firm work product; a
/// client must never read one, so the exclusion is enforced in the query, not
/// left to the template. The firm sees [`for_project`].
///
/// # Errors
///
/// Propagates any database error.
pub async fn for_project_client_visible(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Communication>, CommunicationError> {
    read_thread(db, project_id, Some(direction::INTERNAL)).await
}

/// The body both thread reads share, so the ordering — and the
/// `project_id` scope neither may widen — is written once.
///
/// `occurred_at` then `id`: the timestamp is when the message happened,
/// and the UUIDv7 id breaks ties in insertion order, so two messages
/// stamped the same second still render in the order they arrived.
async fn read_thread(
    db: &SurrealDb,
    project_id: Uuid,
    exclude_direction: Option<&str>,
) -> Result<Vec<Communication>, CommunicationError> {
    let filter = if exclude_direction.is_some() {
        "AND direction != $excluded "
    } else {
        ""
    };
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE project_id = $project {filter}ORDER BY occurred_at ASC, id ASC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind((
            "excluded",
            exclude_direction.unwrap_or_default().to_string(),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<CommunicationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(CommunicationRow::into_communication)
        .collect())
}

/// Errors from a retention purge — the DB deletes and the storage-object
/// deletes can each fail.
#[derive(Debug, thiserror::Error)]
pub enum PurgeError {
    #[error("database: {0}")]
    Db(#[from] CommunicationError),
    #[error("matters: {0}")]
    Projects(#[from] crate::projects::ProjectStoreError),
    #[error("asset: {0}")]
    Asset(#[from] crate::assets::AssetError),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("document comment: {0}")]
    DocumentComment(#[from] crate::document_comments::DocumentCommentError),
}

/// What a purge removed — returned for the caller to log / report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub communications_deleted: u64,
    pub assets_deleted: u64,
}

/// Permanently delete one matter's privileged conversation log and the raw
/// payloads only it referenced — the end-of-retention destruction the
/// retainer promises. Matter-scoped and logged.
///
/// The comment satellite's `communication_id` link is cleared first (the
/// Phase A review-surface row itself is governed by the separate expunge
/// machinery, not this sweep): the comment must not outlive the spine row
/// it names.
///
/// The order of the three deletes is the guarantee, and it survived the
/// port unchanged. Communications go first, so a crash part-way leaves an
/// asset row whose last referent is gone — an orphan a later sweep
/// reclaims — rather than a communication pointing at bytes that are
/// already deleted. Storage objects go last for the same reason, and only
/// once no remaining asset row references the content-addressed key.
///
/// # Errors
///
/// [`PurgeError`] on a database or storage failure.
pub async fn purge_for_project(
    surreal: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    project_id: Uuid,
) -> Result<PurgeReport, PurgeError> {
    let comms = for_project(surreal, project_id).await?;
    if comms.is_empty() {
        return Ok(PurgeReport::default());
    }
    let comm_ids: Vec<Uuid> = comms.iter().map(|c| c.id).collect();
    let mut asset_ids: Vec<Uuid> = comms.iter().filter_map(|c| c.asset_id).collect();
    asset_ids.sort();
    asset_ids.dedup();

    // Clear the comment satellite's link first: nothing enforces the
    // reference, so the ordering is what keeps a comment from outliving the
    // spine row it names.
    crate::document_comments::clear_communication_links(surreal, &comm_ids).await?;

    let deleted = u64::try_from(comm_ids.len()).unwrap_or(u64::MAX);
    surreal
        .query(format!("DELETE {TABLE} WHERE project_id = $project"))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(CommunicationError::Db)?;

    // Which assets lost their last communication referent — read after the
    // delete, so the answer reflects it. A raw payload referenced by a
    // communication on *another* matter is retained: this sweep destroys one
    // matter's file, and an asset another matter still points at is not part
    // of it.
    let mut orphaned: Vec<Uuid> = Vec::new();
    for aid in asset_ids {
        if !asset_is_referenced(surreal, aid).await? {
            orphaned.push(aid);
        }
    }

    // Now the other engine. Delete each orphaned asset row, then keep its
    // storage key as a candidate only if no other row still references that
    // content-addressed object — `storage_key` is not unique, because
    // identical bytes are stored once and may be referenced several times.
    let mut storage_keys: Vec<String> = Vec::new();
    for aid in orphaned {
        let Some(deleted) = crate::assets::delete(surreal, aid).await? else {
            continue;
        };
        if !crate::assets::storage_key_is_referenced(surreal, &deleted.storage_key).await?
            && !storage_keys.contains(&deleted.storage_key)
        {
            storage_keys.push(deleted.storage_key);
        }
    }

    // Storage objects last — orphaned bytes are recoverable; a dangling row is
    // not.
    for key in &storage_keys {
        storage.delete(key).await?;
    }

    let report = PurgeReport {
        communications_deleted: deleted,
        assets_deleted: storage_keys.len() as u64,
    };
    tracing::info!(
        %project_id,
        communications = report.communications_deleted,
        assets = report.assets_deleted,
        "purged matter conversation log at end of retention",
    );
    Ok(report)
}

/// Whether any communication still points at this raw-payload asset.
async fn asset_is_referenced(db: &SurrealDb, asset_id: Uuid) -> Result<bool, CommunicationError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} WHERE asset_id = $asset LIMIT 1"
        ))
        .bind(("asset", record_id(crate::assets::TABLE, asset_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let ids: Vec<surrealdb::types::RecordId> = response.take(0)?;
    Ok(!ids.is_empty())
}

/// Purge every matter whose retention window has elapsed: a closed matter
/// (`projects.closed_at` set) is destroyed once `closed_at + retention_years`
/// has passed `now`. `now` is passed in so the sweep is deterministic in
/// tests and replayable in a durable workflow. `retention_years` is normally
/// [`RETENTION_YEARS`].
///
/// # Errors
///
/// [`PurgeError`] on the first database or storage failure.
pub async fn purge_expired_matters(
    surreal: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    now: DateTime<Utc>,
    retention_years: u32,
) -> Result<PurgeReport, PurgeError> {
    let closed_matters = crate::projects::all(surreal).await?;

    let mut total = PurgeReport::default();
    for p in closed_matters {
        let Some(stamp) = p.closed_at.as_deref() else {
            continue;
        };
        let Ok(closed_time) = DateTime::parse_from_rfc3339(stamp) else {
            tracing::warn!(project_id = %p.id, closed_at = stamp, "unparseable closed_at; skipping retention");
            continue;
        };
        let due = closed_time
            .with_timezone(&Utc)
            .checked_add_months(Months::new(retention_years * 12));
        if due.is_some_and(|due| now >= due) {
            let r = purge_for_project(surreal, storage, p.id).await?;
            total.communications_deleted += r.communications_deleted;
            total.assets_deleted += r.assets_deleted;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::{
        channel, direction, for_project, for_project_client_visible, ingest, purge_expired_matters,
        purge_for_project, IngestArgs, RETENTION_YEARS,
    };
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use uuid::Uuid;

    async fn seed_project(surreal: &crate::surreal::SurrealDb) -> Uuid {
        let id = Uuid::now_v7();
        let __dri = crate::test_support::dri_person(surreal).await;
        crate::projects::create(
            surreal,
            &crate::projects::NewProject {
                code: format!("communication-{id}"),
                name: "Test Matter".into(),
                status: "open".into(),
                entity_id: crate::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    fn args<'a>(project_id: Uuid, ch: &'a str, dir: &'a str, body: &'a str) -> IngestArgs<'a> {
        IngestArgs {
            project_id,
            channel: ch,
            direction: dir,
            author_person_id: None,
            counterparty: None,
            subject: None,
            body,
            source_ref: None,
            asset_id: None,
            occurred_at: "2026-06-08T10:00:00Z",
        }
    }

    #[tokio::test]
    async fn ingest_inserts_a_spine_row() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        let out = ingest(
            &surreal,
            &args(
                project_id,
                channel::DOCUMENT_COMMENT,
                direction::INBOUND,
                "Should this be my full legal name?",
            ),
        )
        .await
        .unwrap();
        assert!(!out.deduped);

        let rows = for_project(&surreal, project_id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, out.communication_id);
        assert_eq!(rows[0].channel, channel::DOCUMENT_COMMENT);
        assert_eq!(rows[0].direction, direction::INBOUND);
        assert_eq!(rows[0].body, "Should this be my full legal name?");
        assert!(rows[0].source_ref.is_none());
    }

    #[tokio::test]
    async fn ingest_dedupes_on_channel_and_source_ref() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        let mut a = args(project_id, channel::EMAIL_INBOUND, direction::INBOUND, "hi");
        a.source_ref = Some("msg-abc@mail.example.com");

        let first = ingest(&surreal, &a).await.unwrap();
        assert!(!first.deduped);

        // Same Message-ID re-delivered (SendGrid retry): no duplicate row.
        let second = ingest(&surreal, &a).await.unwrap();
        assert!(second.deduped);
        assert_eq!(second.communication_id, first.communication_id);

        assert_eq!(for_project(&surreal, project_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn same_source_ref_different_channel_is_distinct() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        let mut inbound = args(project_id, channel::EMAIL_INBOUND, direction::INBOUND, "q");
        inbound.source_ref = Some("ref-1");
        let mut outbound = args(
            project_id,
            channel::EMAIL_OUTBOUND,
            direction::OUTBOUND,
            "a",
        );
        outbound.source_ref = Some("ref-1");

        let a = ingest(&surreal, &inbound).await.unwrap();
        let b = ingest(&surreal, &outbound).await.unwrap();
        assert_ne!(a.communication_id, b.communication_id);
        assert_eq!(for_project(&surreal, project_id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_source_ref_key_is_enforced_by_the_database_not_only_the_lookup() {
        // The read-then-write in `ingest` is the common path; the unique
        // index is the guarantee. Written around the command — as a racer
        // that got past the lookup would be — the second insert must still
        // be refused, or one client message can land twice in their own
        // privileged thread.
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;
        let mut first = args(
            project_id,
            channel::EMAIL_INBOUND,
            direction::INBOUND,
            "the only copy",
        );
        first.source_ref = Some("<dupe@mail.example.com>");
        ingest(&surreal, &first).await.unwrap();

        let raw = surreal
            .query(
                "CREATE type::record('communication', rand::uuid::v7()) SET \
                 project_id = $project, channel = $channel, direction = 'inbound', \
                 body = 'a second copy', source_ref = $source_ref, \
                 occurred_at = '2026-06-08T10:00:00Z'",
            )
            .bind((
                "project",
                crate::surreal::record_id(crate::projects::PROJECT_TABLE, project_id),
            ))
            .bind(("channel", channel::EMAIL_INBOUND.to_string()))
            .bind(("source_ref", "<dupe@mail.example.com>".to_string()))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("the unique index must refuse a duplicate source_ref");
        assert!(
            super::is_duplicate_source_ref(&raw),
            "the refusal must be recognizable as the source-ref key: {raw}"
        );
        assert_eq!(for_project(&surreal, project_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn null_source_ref_never_dedupes() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        // Two comments with no external id are distinct events. This also
        // holds the `(channel, source_ref)` unique index honest: an absent
        // `source_ref` must not collide, or the first comment on a matter
        // would block every later one.
        ingest(
            &surreal,
            &args(
                project_id,
                channel::DOCUMENT_COMMENT,
                direction::INBOUND,
                "one",
            ),
        )
        .await
        .unwrap();
        ingest(
            &surreal,
            &args(
                project_id,
                channel::DOCUMENT_COMMENT,
                direction::INBOUND,
                "two",
            ),
        )
        .await
        .unwrap();

        assert_eq!(for_project(&surreal, project_id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn client_visible_excludes_internal_firm_notes() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        // A client-facing inbound message and a firm-internal note.
        ingest(
            &surreal,
            &args(
                project_id,
                channel::EMAIL_INBOUND,
                direction::INBOUND,
                "client question",
            ),
        )
        .await
        .unwrap();
        ingest(
            &surreal,
            &args(
                project_id,
                channel::PORTAL_MESSAGE,
                direction::INTERNAL,
                "FIRM EYES ONLY — strategy note",
            ),
        )
        .await
        .unwrap();

        // The firm sees both; the client sees only the non-internal row, and
        // never the internal note's body.
        assert_eq!(for_project(&surreal, project_id).await.unwrap().len(), 2);
        let client_view = for_project_client_visible(&surreal, project_id)
            .await
            .unwrap();
        assert_eq!(client_view.len(), 1);
        assert_eq!(client_view[0].body, "client question");
        assert!(
            client_view
                .iter()
                .all(|c| c.direction != direction::INTERNAL),
            "a client must never read a firm-internal note"
        );
    }

    #[tokio::test]
    async fn for_project_returns_thread_oldest_first() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal).await;

        let mut later = args(
            project_id,
            channel::EMAIL_OUTBOUND,
            direction::OUTBOUND,
            "reply",
        );
        later.occurred_at = "2026-06-08T12:00:00Z";
        let mut earlier = args(
            project_id,
            channel::EMAIL_INBOUND,
            direction::INBOUND,
            "question",
        );
        earlier.occurred_at = "2026-06-08T09:00:00Z";

        ingest(&surreal, &later).await.unwrap();
        ingest(&surreal, &earlier).await.unwrap();

        let rows = for_project(&surreal, project_id).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].body, "question");
        assert_eq!(rows[1].body, "reply");
    }

    async fn fs_storage() -> Arc<dyn cloud::StorageService> {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        Arc::new(cloud::FsStorage::new(root).await.unwrap())
    }

    #[tokio::test]
    async fn purge_for_project_deletes_the_log_and_its_raw_payload() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project_id = seed_project(&surreal).await;

        // A raw .eml payload stored as a content-addressed asset, referenced by
        // an inbound-email communication.
        let asset_id = crate::assets::ingest_content(
            &surreal,
            &storage,
            b"raw rfc5322 bytes",
            "message/rfc822",
        )
        .await
        .unwrap();
        let storage_key = crate::assets::find_by_id(&surreal, asset_id)
            .await
            .unwrap()
            .unwrap()
            .storage_key;

        let mut with_asset = args(
            project_id,
            channel::EMAIL_INBOUND,
            direction::INBOUND,
            "raw email body",
        );
        with_asset.asset_id = Some(asset_id);
        ingest(&surreal, &with_asset).await.unwrap();
        ingest(
            &surreal,
            &args(
                project_id,
                channel::PORTAL_MESSAGE,
                direction::INTERNAL,
                "a note",
            ),
        )
        .await
        .unwrap();

        let report = purge_for_project(&surreal, &storage, project_id)
            .await
            .unwrap();
        assert_eq!(report.communications_deleted, 2);
        assert_eq!(report.assets_deleted, 1);

        // The conversation log is gone, the asset row is gone, and the raw
        // payload is gone from storage — the row and its bytes share a fate.
        assert!(for_project(&surreal, project_id).await.unwrap().is_empty());
        assert!(crate::assets::find_by_id(&surreal, asset_id)
            .await
            .unwrap()
            .is_none());
        assert!(storage.get(&storage_key).await.is_err());
    }

    #[tokio::test]
    async fn purge_expired_matters_only_purges_past_the_retention_window() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;

        // now, an old close (11 years ago → expired), a recent close (kept).
        let now: DateTime<Utc> = "2026-06-08T00:00:00Z".parse().unwrap();
        let old_project = seed_project(&surreal).await;
        let recent_project = seed_project(&surreal).await;
        set_closed_at(&surreal, old_project, "2015-01-01T00:00:00Z").await;
        set_closed_at(&surreal, recent_project, "2025-12-01T00:00:00Z").await;

        ingest(
            &surreal,
            &args(
                old_project,
                channel::EMAIL_INBOUND,
                direction::INBOUND,
                "old",
            ),
        )
        .await
        .unwrap();
        ingest(
            &surreal,
            &args(
                recent_project,
                channel::EMAIL_INBOUND,
                direction::INBOUND,
                "recent",
            ),
        )
        .await
        .unwrap();

        let report = purge_expired_matters(&surreal, &storage, now, RETENTION_YEARS)
            .await
            .unwrap();
        assert_eq!(report.communications_deleted, 1, "only the expired matter");

        // The 11-year-old matter's log is destroyed; the recent one is kept.
        assert!(for_project(&surreal, old_project).await.unwrap().is_empty());
        assert_eq!(
            for_project(&surreal, recent_project).await.unwrap().len(),
            1
        );
    }

    async fn set_closed_at(surreal: &crate::surreal::SurrealDb, project_id: Uuid, when: &str) {
        surreal
            .query("UPDATE $id SET status = 'closed', closed_at = $closed_at")
            .bind(("id", crate::surreal::record_id("project", project_id)))
            .bind(("closed_at", when.to_string()))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .unwrap();
    }
}
