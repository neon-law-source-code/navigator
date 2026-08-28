//! `review_documents` — the attorney-reviewed drafts a client reads and
//! comments on before signing, and every query against the table.
//!
//! # This table lives in SurrealDB
//!
//! `review_documents` moved with wave five of #1093 (ENG-121), in the
//! satellite-ring slice.
//!
//! The generation workflow inserts a draft (`status = draft`); an attorney
//! advances it to `pending_review`; the client's sign-off advances it to
//! `approved`. Kept beside the other orchestration helpers so `web` and the
//! generation workflow reach them without re-importing the entity.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
pub(crate) const TABLE: &str = "review_document";

/// Hidden from the client; the generation workflow parks a freshly
/// generated draft here until an attorney approves it.
pub const STATUS_DRAFT: &str = "draft";
/// Visible to the scoped client, who may read and comment.
pub const STATUS_PENDING_REVIEW: &str = "pending_review";
/// The client has signed off; the draft is ready for signature.
pub const STATUS_APPROVED: &str = "approved";

/// One attorney-reviewed draft.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`ReviewDocumentRow`] is the seam that turns it into (and back out of)
/// what the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewDocument {
    pub id: Uuid,
    /// The matter this draft belongs to. Unique with [`Self::kind`] — one
    /// row per instrument per matter, which is what [`upsert_draft`] writes
    /// through.
    pub notation_id: Uuid,
    /// Document kind within the matter: `will`, `trust`,
    /// `directive_health`, `directive_financial`, … Unique with
    /// [`Self::notation_id`].
    pub kind: String,
    /// Human-readable title shown to the client.
    pub title: String,
    /// Attorney-reviewed draft body as sanitized HTML.
    pub body_html: String,
    /// `draft`, `pending_review`, or `approved` — see the `STATUS_*`
    /// constants.
    pub status: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct ReviewDocumentRow {
    id: surrealdb::types::RecordId,
    notation_id: surrealdb::types::RecordId,
    kind: String,
    title: String,
    body_html: String,
    status: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl ReviewDocumentRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_document(self) -> Option<ReviewDocument> {
        Some(ReviewDocument {
            id: record_uuid(&self.id)?,
            notation_id: record_uuid(&self.notation_id)?,
            kind: self.kind,
            title: self.title,
            body_html: self.body_html,
            status: self.status,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`ReviewDocumentRow`] from only one query.
const SELECT: &str = "id, notation_id, kind, title, body_html, status, inserted_at, updated_at";

/// What to record for one reviewable draft. `status` defaults to
/// [`STATUS_DRAFT`] via [`create`] — a freshly generated draft is never
/// visible to the client until an attorney advances it.
#[derive(Debug, Clone)]
pub struct NewReviewDocument<'a> {
    pub notation_id: Uuid,
    /// Document kind within the matter (`will`, `trust`, …).
    pub kind: &'a str,
    /// Human-readable title shown to the client.
    pub title: &'a str,
    /// Attorney-reviewed draft body as sanitized HTML.
    pub body_html: &'a str,
}

/// Errors reading or writing a review document.
#[derive(Debug, thiserror::Error)]
pub enum ReviewDocumentError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("notation: {0}")]
    Notation(#[from] crate::notations::NotationError),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a review document returned no usable row")]
    WriteReturnedNothing,
}

/// A unique violation on `review_document_notation_kind` — the other
/// writer's insert won the `(notation_id, kind)` slot.
fn is_notation_kind_conflict(error: &surrealdb::Error) -> bool {
    error.to_string().contains("review_document_notation_kind")
}

/// How many times [`upsert_draft`] re-reads after losing the
/// `review_document_notation_kind` slot, and the first backoff window.
///
/// Not the transaction-conflict retry — that is one policy for the whole
/// crate, in [`crate::surreal::retry`], and this module's [`writing`]
/// defers to it. This bound is around a *unique index* violation, where
/// re-running is pointless and the loop exists to re-read the winner's
/// row instead. A loser that still finds nothing after these attempts
/// has hit the same lost-create window ENG-114 tracks in
/// [`crate::persons::find_or_create`], not a budget that wants raising.
const WRITE_ATTEMPTS: usize = 5;
const WRITE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, surrealdb::Error>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await
}

fn one(
    mut response: surrealdb::IndexedResults,
) -> Result<Option<ReviewDocument>, ReviewDocumentError> {
    let row: Option<ReviewDocumentRow> = response.take(0)?;
    Ok(row.and_then(ReviewDocumentRow::into_document))
}

fn many(
    mut response: surrealdb::IndexedResults,
) -> Result<Vec<ReviewDocument>, ReviewDocumentError> {
    let rows: Vec<ReviewDocumentRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(ReviewDocumentRow::into_document)
        .collect())
}

/// Insert one `review_documents` row at `status = draft`, returning its id.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the insert fails (including a
/// `(notation_id, kind)` conflict — use [`upsert_draft`] when a sibling
/// row for the same instrument may already exist).
pub async fn create(
    db: &SurrealDb,
    new: &NewReviewDocument<'_>,
) -> Result<Uuid, ReviewDocumentError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             notation_id = $notation_id, kind = $kind, title = $title, \
             body_html = $body_html, status = $status \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "notation_id",
            record_id(crate::notations::TABLE, new.notation_id),
        ))
        .bind(("kind", new.kind.to_string()))
        .bind(("title", new.title.to_string()))
        .bind(("body_html", new.body_html.to_string()))
        .bind(("status", STATUS_DRAFT.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ReviewDocumentRow> = response.take(0)?;
    row.and_then(ReviewDocumentRow::into_document)
        .map(|d| d.id)
        .ok_or(ReviewDocumentError::WriteReturnedNothing)
}

/// One review document for `(notation_id, kind)`, if any.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup fails.
pub async fn find_by_notation_kind(
    db: &SurrealDb,
    notation_id: Uuid,
    kind: &str,
) -> Result<Option<ReviewDocument>, ReviewDocumentError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE notation_id = $notation AND kind = $kind LIMIT 1"
        ))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .bind(("kind", kind.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Insert one `review_documents` row for `(notation_id, kind)`, or write in
/// place if a row already exists — the generation pipeline's per-instrument
/// write, so re-rendering a notation (a corrected transcript, a re-answered
/// question) never inserts a sibling row.
///
/// A row still at `draft` is refreshed: nobody has seen it yet, so the
/// newest render wins. A row that has already left `draft` (an attorney
/// released it, or the client signed off) is left untouched — the
/// generation step never overwrites reviewed content, since that would
/// bypass the human-in-the-loop gate `draft` exists to enforce. Returns the
/// row's id either way.
///
/// Race-safe without a lock, the way `crate::templates::save_version` is:
/// two concurrent renders both reading no existing row race to create one,
/// and the `review_document_notation_kind` unique index refuses the loser's
/// insert. The loser re-reads and, if the winner wrote the same instrument
/// (which it always does — both draws from the same notation/kind), treats
/// the winner's fresh draft as its own outcome and refreshes it, rather
/// than erroring on a no-op race.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup or write fails.
pub async fn upsert_draft(
    db: &SurrealDb,
    new: &NewReviewDocument<'_>,
) -> Result<Uuid, ReviewDocumentError> {
    let mut backoff = WRITE_BACKOFF;
    for remaining in (0..WRITE_ATTEMPTS).rev() {
        if let Some(existing) = find_by_notation_kind(db, new.notation_id, new.kind).await? {
            if existing.status == STATUS_DRAFT {
                update_content(db, existing.id, new.title, new.body_html).await?;
            }
            return Ok(existing.id);
        }
        match create(db, new).await {
            Ok(id) => return Ok(id),
            Err(ReviewDocumentError::Db(error)) if is_notation_kind_conflict(&error) => {
                if remaining == 0 {
                    // One more read: the winner's row is there now.
                    if let Some(existing) =
                        find_by_notation_kind(db, new.notation_id, new.kind).await?
                    {
                        return Ok(existing.id);
                    }
                    return Err(ReviewDocumentError::Db(error));
                }
                tokio::time::sleep(rand::random_range(std::time::Duration::ZERO..=backoff)).await;
                backoff *= 2;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the last attempt returns rather than falling out of the loop")
}

async fn update_content(
    db: &SurrealDb,
    id: Uuid,
    title: &str,
    body_html: &str,
) -> Result<(), ReviewDocumentError> {
    writing(|| {
        db.query(format!(
            "UPDATE $id SET title = $title, body_html = $body_html, updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("title", title.to_string()))
        .bind(("body_html", body_html.to_string()))
    })
    .await?;
    Ok(())
}

/// Load one review document by id.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup fails.
pub async fn by_id(
    db: &SurrealDb,
    id: Uuid,
) -> Result<Option<ReviewDocument>, ReviewDocumentError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// All review documents for a notation, oldest first.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup fails.
pub async fn for_notation(
    db: &SurrealDb,
    notation_id: Uuid,
) -> Result<Vec<ReviewDocument>, ReviewDocumentError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE notation_id = $notation ORDER BY id ASC"
        ))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Which of `notation_ids` have at least one drafted instrument — the
/// batched, **content-free** existence probe the matter-lifecycle
/// indicator reads.
///
/// One round trip for the whole set, never one per notation, because the
/// caller ([`crate::projects::matter_lifecycle_sets`]) renders a list and
/// its query budget is per-table.
///
/// Projects `notation_id` alone and returns bare ids. That is deliberate,
/// not an optimization: [`ReviewDocument::body_html`] is a drafted legal
/// instrument — a will, a trust, a health-care directive — and the caller
/// needs to know only *whether* one exists. Selecting the body would pull
/// the most sensitive text the firm holds into a list-page request that
/// has no use for it.
///
/// Every status counts, `draft` included. A draft sitting in attorney
/// review is an instrument the walk actually produced; whether the
/// attorney has approved it yet is a different question from whether it
/// exists.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup fails.
pub async fn notations_with_drafts(
    db: &SurrealDb,
    notation_ids: &[Uuid],
) -> Result<std::collections::HashSet<Uuid>, ReviewDocumentError> {
    #[derive(SurrealValue)]
    struct NotationIdRow {
        notation_id: surrealdb::types::RecordId,
    }

    if notation_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let keys: Vec<surrealdb::types::RecordId> = notation_ids
        .iter()
        .map(|id| record_id(crate::notations::TABLE, *id))
        .collect();
    let mut response = db
        .query(format!(
            "SELECT notation_id FROM {TABLE} WHERE notation_id IN $notations"
        ))
        .bind(("notations", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<NotationIdRow> = response.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| record_uuid(&r.notation_id))
        .collect())
}

/// All client-visible review documents for a project — those whose
/// notation belongs to the project and whose status has been advanced past
/// `draft`.
///
/// Resolves the project's notation ids first, then filters by that set —
/// two round trips rather than a join, the way `notation` (a different
/// table) never joins across engines any more.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the lookup fails.
pub async fn client_visible_for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<ReviewDocument>, ReviewDocumentError> {
    let notation_ids: Vec<Uuid> = crate::notations::list_by_project(db, project_id)
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect();
    if notation_ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<surrealdb::types::RecordId> = notation_ids
        .iter()
        .map(|id| record_id(crate::notations::TABLE, *id))
        .collect();
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE notation_id IN $notations AND status != $draft \
             ORDER BY id ASC"
        ))
        .bind(("notations", keys))
        .bind(("draft", STATUS_DRAFT.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Move a review document to a new `status`. Returns the updated row, or
/// `Ok(None)` if no row matched.
///
/// # Errors
///
/// [`ReviewDocumentError::Db`] if the write fails.
pub async fn set_status(
    db: &SurrealDb,
    id: Uuid,
    status: &str,
) -> Result<Option<ReviewDocument>, ReviewDocumentError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE $id SET status = $status, updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("status", status.to_string()))
    })
    .await?;
    let row: Option<ReviewDocumentRow> = response.take(0)?;
    Ok(row.and_then(ReviewDocumentRow::into_document))
}

#[cfg(test)]
mod tests {
    use super::{
        by_id, client_visible_for_project, create, for_notation, notations_with_drafts, set_status,
        upsert_draft, NewReviewDocument, STATUS_DRAFT, STATUS_PENDING_REVIEW,
    };
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_notation;

    /// The matter-lifecycle indicator's probe: exactly the notations that
    /// produced an instrument, in one round trip, and nothing about the
    /// notations that did not.
    #[tokio::test]
    async fn notations_with_drafts_returns_only_the_notations_carrying_one() {
        let surreal = mem().await;
        let produced = seed_notation(&surreal).await;
        let abandoned = seed_notation(&surreal).await;

        upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id: produced,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<p>Synthetic draft body.</p>",
            },
        )
        .await
        .unwrap();

        let found = notations_with_drafts(&surreal, &[produced, abandoned])
            .await
            .unwrap();
        assert!(
            found.contains(&produced),
            "a notation whose walk drafted an instrument must be reported"
        );
        assert!(
            !found.contains(&abandoned),
            "a notation with no drafted instrument must not be reported — this is what \
             keeps an abandoned walk from clearing a lifecycle flag"
        );
        assert_eq!(found.len(), 1);
    }

    /// A draft still awaiting attorney approval counts: the walk produced the
    /// instrument, which is a different question from whether it was approved.
    #[tokio::test]
    async fn notations_with_drafts_counts_a_draft_still_in_attorney_review() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let id = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "trust",
                title: "Revocable Trust",
                body_html: "<p>Synthetic draft body.</p>",
            },
        )
        .await
        .unwrap();
        assert_eq!(
            by_id(&surreal, id).await.unwrap().unwrap().status,
            STATUS_DRAFT
        );

        assert!(notations_with_drafts(&surreal, &[notation_id])
            .await
            .unwrap()
            .contains(&notation_id));
    }

    /// No notations to ask about is not a query.
    #[tokio::test]
    async fn notations_with_drafts_on_an_empty_set_asks_nothing() {
        let surreal = mem().await;
        assert!(notations_with_drafts(&surreal, &[])
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn create_defaults_to_draft_and_is_readable_by_notation() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;

        let id = create(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<h1>Will</h1><p>I, Libra…</p>",
            },
        )
        .await
        .unwrap();

        let row = by_id(&surreal, id).await.unwrap().unwrap();
        assert_eq!(row.kind, "will");
        assert_eq!(row.status, STATUS_DRAFT);
        assert!(row.body_html.contains("Libra"));

        let all = for_notation(&surreal, notation_id).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
    }

    #[tokio::test]
    async fn client_visible_for_project_hides_drafts() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let project_id = crate::notations::find_by_id(&surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;

        let hidden = create(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "trust",
                title: "Trust (draft)",
                body_html: "<p>x</p>",
            },
        )
        .await
        .unwrap();
        let shown = create(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Will (ready)",
                body_html: "<p>y</p>",
            },
        )
        .await
        .unwrap();
        set_status(&surreal, shown, STATUS_PENDING_REVIEW)
            .await
            .unwrap();

        let visible = client_visible_for_project(&surreal, project_id)
            .await
            .unwrap();
        let ids: Vec<_> = visible.iter().map(|d| d.id).collect();
        assert!(ids.contains(&shown));
        assert!(!ids.contains(&hidden));
    }

    #[tokio::test]
    async fn set_status_advances_the_draft() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let id = create(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Will",
                body_html: "<p>x</p>",
            },
        )
        .await
        .unwrap();
        let updated = set_status(&surreal, id, STATUS_PENDING_REVIEW)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, STATUS_PENDING_REVIEW);
    }

    #[tokio::test]
    async fn upsert_draft_inserts_the_first_version() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let id = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<p>Executor: Nobody yet</p>",
            },
        )
        .await
        .unwrap();
        let all = for_notation(&surreal, notation_id).await.unwrap();
        assert_eq!(all.len(), 1, "first render creates exactly one row");
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].status, STATUS_DRAFT);
    }

    #[tokio::test]
    async fn upsert_draft_refreshes_a_still_draft_row_in_place() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let first = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<p>Executor: Nobody yet</p>",
            },
        )
        .await
        .unwrap();
        let second = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<p>Executor: Aries</p>",
            },
        )
        .await
        .unwrap();
        assert_eq!(first, second, "re-render refreshes the same row");

        let all = for_notation(&surreal, notation_id).await.unwrap();
        assert_eq!(all.len(), 1, "no sibling row for the same (notation, kind)");
        assert_eq!(all[0].status, STATUS_DRAFT);
        assert!(
            all[0].body_html.contains("Aries"),
            "a still-draft row refreshes to the newest render: {}",
            all[0].body_html
        );
    }

    #[tokio::test]
    async fn upsert_draft_leaves_a_released_document_untouched() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;

        let id = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament",
                body_html: "<p>Executor: Aries</p>",
            },
        )
        .await
        .unwrap();
        set_status(&surreal, id, STATUS_PENDING_REVIEW)
            .await
            .unwrap();

        let again = upsert_draft(
            &surreal,
            &NewReviewDocument {
                notation_id,
                kind: "will",
                title: "Last Will and Testament (v2)",
                body_html: "<p>Executor: Taurus</p>",
            },
        )
        .await
        .unwrap();
        assert_eq!(id, again);

        let all = for_notation(&surreal, notation_id).await.unwrap();
        assert_eq!(all.len(), 1, "no sibling row once released");
        assert_eq!(all[0].status, STATUS_PENDING_REVIEW, "status is not reset");
        assert!(
            all[0].body_html.contains("Aries"),
            "released body is not silently overwritten: {}",
            all[0].body_html
        );

        let project_id = crate::notations::find_by_id(&surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;
        let visible = client_visible_for_project(&surreal, project_id)
            .await
            .unwrap();
        assert_eq!(
            visible.iter().filter(|d| d.kind == "will").count(),
            1,
            "client sees exactly one released copy of the instrument"
        );
    }
}
