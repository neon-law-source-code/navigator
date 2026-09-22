//! `store::notation_drafts` — a stored, addressable notation **draft**
//! (LAW-29): a preview artifact, not an executed instrument.
//!
//! `navigator notations preview` used to serve its own local imitation of
//! the notation show page. This is the other half: a template pushed to
//! the Project it belongs to, so an author can open the *real* portal —
//! production chrome, production questionnaire engine — against something
//! that has never been run.
//!
//! Creating a draft **deliberately never**:
//! * creates a [`crate::notations::Notation`] row,
//! * starts a [`workflows`] state-machine instance, or
//! * journals a [`crate::notation_events`] event (`intake_submitted`
//!   included).
//!
//! A draft is a preview, not a filing: no workflow instance, no PDF, no
//! signature. It carries a short [`TTL`] rather than accumulating.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

pub const TABLE: &str = "notation_draft";

/// How long a draft stays reachable. Drafts are a preview artifact with a
/// short life, not a place to accumulate — a reaper can delete rows past
/// this age, and [`find_live`] already treats them as gone.
pub const TTL: Duration = Duration::hours(24);

/// A stored, addressable, unrun notation preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotationDraft {
    pub id: Uuid,
    pub project_id: Uuid,
    /// The `{slug}` the draft is addressed at — the same kebab-case a
    /// published notation's URL would use.
    pub slug: String,
    pub title: String,
    /// The template's raw Markdown source (frontmatter and body), exactly
    /// as [`webapp::notation_preview::PreviewDoc`] parses a preview from.
    pub source: String,
    pub inserted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct NotationDraftRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    slug: String,
    title: String,
    source: String,
    inserted_at: surrealdb::types::Datetime,
    expires_at: surrealdb::types::Datetime,
}

impl NotationDraftRow {
    fn into_draft(self) -> Option<NotationDraft> {
        Some(NotationDraft {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            slug: self.slug,
            title: self.title,
            source: self.source,
            inserted_at: self.inserted_at.into(),
            expires_at: self.expires_at.into(),
        })
    }
}

const SELECT: &str = "id, project_id, slug, title, source, inserted_at, expires_at";

/// Errors from the notation-draft commands.
#[derive(Debug, thiserror::Error)]
pub enum NotationDraftError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row this module could read
    /// back.
    #[error("writing a notation draft returned no usable row")]
    WriteReturnedNothing,
}

/// What a new [`NotationDraft`] needs.
#[derive(Debug, Clone)]
pub struct NewNotationDraft<'a> {
    pub project_id: Uuid,
    pub slug: &'a str,
    pub title: &'a str,
    pub source: &'a str,
}

/// Store a notation draft.
///
/// # Errors
/// Propagates any database error.
pub async fn create(
    db: &SurrealDb,
    new: &NewNotationDraft<'_>,
) -> Result<NotationDraft, NotationDraftError> {
    let id = Uuid::now_v7();
    let inserted_at = Utc::now();
    let expires_at = inserted_at + TTL;
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             project_id = $project_id, slug = $slug, title = $title, source = $source, \
             inserted_at = $inserted_at, expires_at = $expires_at \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, new.project_id),
        ))
        .bind(("slug", new.slug.to_string()))
        .bind(("title", new.title.to_string()))
        .bind(("source", new.source.to_string()))
        .bind(("inserted_at", surrealdb::types::Datetime::from(inserted_at)))
        .bind(("expires_at", surrealdb::types::Datetime::from(expires_at)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<NotationDraftRow> = response.take(0)?;
    row.and_then(NotationDraftRow::into_draft)
        .ok_or(NotationDraftError::WriteReturnedNothing)
}

/// Fetch a draft by id — `None` when it does not exist *or has expired*,
/// so an expired draft's door 404s exactly like an unknown one rather than
/// serving stale content past its stated life. The row itself is left
/// alone; expiry is a read-time filter, not a delete, so a reaper (or an
/// operator) can still find and clean up what expired.
///
/// # Errors
/// Propagates any database error.
pub async fn find_live(
    db: &SurrealDb,
    id: Uuid,
) -> Result<Option<NotationDraft>, NotationDraftError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<NotationDraftRow> = response.take(0)?;
    Ok(row
        .and_then(NotationDraftRow::into_draft)
        .filter(|draft| draft.expires_at > Utc::now()))
}
