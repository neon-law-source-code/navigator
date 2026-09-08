//! Physical mail, and every query against a `letter` row.
//!
//! # This table lives in SurrealDB
//!
//! `letters` moved with wave two of the flat-table ports (#1093;
//! ENG-20) — the last hop of the `letter -> mailroom -> address` chain,
//! which is why all three moved in one PR: split across waves, each link
//! would have spent a release as a uuid Rust had to resolve.
//!
//! **The engine does not validate a link.** `mailroom_id` is a
//! `record<mailroom>`, but a link to a mailroom that was never written
//! is accepted. The read-back in [`record`] is the check.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::mailrooms::{self, MailroomError};
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "letter";
/// The table `mailroom_id` links into.
const MAILROOM_TABLE: &str = "mailroom";

/// Mail arriving at a mailroom.
pub const DIRECTION_INCOMING: &str = "incoming";
/// Mail leaving one.
pub const DIRECTION_OUTGOING: &str = "outgoing";

/// One piece of physical mail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Letter {
    pub id: Uuid,
    pub mailroom_id: Uuid,
    /// The matter this letter belongs to, when one is known. `None` for
    /// every historical row and for mail not yet linked to a matter — see
    /// the schema note on `letter.project_id`.
    pub project_id: Option<Uuid>,
    /// [`DIRECTION_INCOMING`] or [`DIRECTION_OUTGOING`].
    pub direction: String,
    pub sender: String,
    pub recipient: String,
    pub summary: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Everything [`record`] needs to log one piece of mail.
#[derive(Debug, Clone)]
pub struct NewLetter {
    pub mailroom_id: Uuid,
    pub project_id: Option<Uuid>,
    pub direction: String,
    pub sender: String,
    pub recipient: String,
    pub summary: String,
}

/// The row as the engine reads and writes it — the seam between
/// [`Letter`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct LetterRow {
    id: surrealdb::types::RecordId,
    mailroom_id: surrealdb::types::RecordId,
    project_id: Option<surrealdb::types::RecordId>,
    direction: String,
    sender: String,
    recipient: String,
    summary: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl LetterRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_letter(self) -> Option<Letter> {
        Some(Letter {
            id: record_uuid(&self.id)?,
            mailroom_id: record_uuid(&self.mailroom_id)?,
            project_id: self.project_id.as_ref().and_then(record_uuid),
            direction: self.direction,
            sender: self.sender,
            recipient: self.recipient,
            summary: self.summary,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`LetterRow`] from only one query.
const SELECT: &str = "id, mailroom_id, project_id, direction, sender, recipient, summary, \
                      inserted_at, updated_at";

/// Errors reading or writing a letter.
#[derive(Debug, thiserror::Error)]
pub enum LetterError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the mailroom a write links to failed.
    #[error(transparent)]
    Mailroom(#[from] MailroomError),
    /// The mailroom named by a write does not exist. The engine would
    /// have accepted the dangling link; this is the check that does not.
    #[error("no mailroom {0}")]
    NoSuchMailroom(Uuid),
    /// A write reported success but returned no row, or returned one
    /// this module could not read back.
    #[error("writing a letter returned no usable row")]
    WriteReturnedNothing,
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, LetterError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(LetterError::Db)
}

/// Log one piece of mail at its mailroom.
///
/// # Errors
///
/// [`LetterError::NoSuchMailroom`] when the mailroom does not exist —
/// checked here because the engine would accept the dangling link — and
/// [`LetterError::Db`] if the insert fails, including the schema
/// `ASSERT` when `direction` is neither `incoming` nor `outgoing`.
pub async fn record(db: &SurrealDb, new: &NewLetter) -> Result<Letter, LetterError> {
    if mailrooms::find_by_id(db, new.mailroom_id).await?.is_none() {
        return Err(LetterError::NoSuchMailroom(new.mailroom_id));
    }

    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             mailroom_id = $mailroom_id, \
             project_id = $project_id, \
             direction = $direction, \
             sender = $sender, \
             recipient = $recipient, \
             summary = $summary \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("mailroom_id", record_id(MAILROOM_TABLE, new.mailroom_id)))
        .bind((
            "project_id",
            new.project_id
                .map(|p| record_id(crate::projects::PROJECT_TABLE, p)),
        ))
        .bind(("direction", new.direction.clone()))
        .bind(("sender", new.sender.clone()))
        .bind(("recipient", new.recipient.clone()))
        .bind(("summary", new.summary.clone()))
    })
    .await?;

    let row: Option<LetterRow> = response.take(0)?;
    row.and_then(LetterRow::into_letter)
        .ok_or(LetterError::WriteReturnedNothing)
}

/// One letter by id.
///
/// # Errors
///
/// [`LetterError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Letter>, LetterError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<LetterRow> = response.take(0)?;
    Ok(row.and_then(LetterRow::into_letter))
}

/// The letter at this mailroom with this sender and summary, if any —
/// the natural key the canonical seeder matches on.
///
/// # Errors
///
/// [`LetterError::Db`] if the lookup fails.
pub async fn find_by_mailroom_sender_summary(
    db: &SurrealDb,
    mailroom_id: Uuid,
    sender: &str,
    summary: &str,
) -> Result<Option<Letter>, LetterError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE mailroom_id = $mailroom_id AND sender = $sender AND summary = $summary \
             LIMIT 1"
        ))
        .bind(("mailroom_id", record_id(MAILROOM_TABLE, mailroom_id)))
        .bind(("sender", sender.to_string()))
        .bind(("summary", summary.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<LetterRow> = response.take(0)?;
    Ok(row.and_then(LetterRow::into_letter))
}

/// Every letter, oldest first — the lawyer listing's order.
///
/// # Errors
///
/// [`LetterError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Letter>, LetterError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC, id ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<LetterRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(LetterRow::into_letter)
        .collect())
}

/// How many letters exist. The production-emptiness gate asks this of
/// the engine that holds the table.
///
/// # Errors
///
/// [`LetterError::Db`] if the count fails.
pub async fn count(db: &SurrealDb) -> Result<i64, LetterError> {
    let mut response = db
        .query(format!("SELECT count() FROM {TABLE} GROUP ALL"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let counts: Vec<CountRow> = response.take(0)?;
    Ok(counts.first().map_or(0, |c| c.count))
}

/// The one-field shape `SELECT count() ... GROUP ALL` returns.
#[derive(SurrealValue)]
struct CountRow {
    count: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        count, find_by_id, find_by_mailroom_sender_summary, list_all, record, LetterError,
        NewLetter, DIRECTION_INCOMING, DIRECTION_OUTGOING,
    };
    use crate::addresses::{self, NewAddress};
    use crate::mailrooms;
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use crate::test_support::seed_project_surreal;
    use uuid::Uuid;

    async fn a_mailroom(db: &SurrealDb, name: &str) -> Uuid {
        let address = addresses::create(
            db,
            &NewAddress {
                line1: "123 Main St".into(),
                city: "Reno".into(),
                region: "NV".into(),
                postal_code: "89501".into(),
                country: "US".into(),
                ..NewAddress::default()
            },
        )
        .await
        .unwrap();
        mailrooms::create(db, name, address.id).await.unwrap().id
    }

    fn a_letter(mailroom_id: Uuid, summary: &str) -> NewLetter {
        NewLetter {
            mailroom_id,
            project_id: None,
            direction: DIRECTION_INCOMING.to_string(),
            sender: "IRS".into(),
            recipient: "Acme".into(),
            summary: summary.to_string(),
        }
    }

    #[tokio::test]
    async fn a_recorded_letter_reads_back_by_id() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;

        let written = record(&db, &a_letter(mailroom_id, "Form 990 reminder"))
            .await
            .unwrap();
        assert_eq!(written.mailroom_id, mailroom_id);
        assert_eq!(written.project_id, None);
        assert_eq!(written.summary, "Form 990 reminder");
        assert_eq!(written.direction, DIRECTION_INCOMING);
        assert_eq!(find_by_id(&db, written.id).await.unwrap(), Some(written));
    }

    /// ENG-310: a letter recorded for a known matter carries it through the
    /// round trip untouched.
    #[tokio::test]
    async fn a_letter_scoped_to_a_project_reads_back_with_it() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;
        let project_id = seed_project_surreal(&db, "matter").await;

        let mut letter = a_letter(mailroom_id, "Engagement notice");
        letter.project_id = Some(project_id);
        let written = record(&db, &letter).await.unwrap();

        assert_eq!(written.project_id, Some(project_id));
        assert_eq!(
            find_by_id(&db, written.id)
                .await
                .unwrap()
                .unwrap()
                .project_id,
            Some(project_id)
        );
    }

    /// ENG-310: a row written before this field existed has no value for it
    /// at all — not the engine's default, an absent field entirely. The
    /// reader must tolerate that rather than fail the whole row.
    #[tokio::test]
    async fn a_historical_letter_with_no_project_id_reads_as_none() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;
        let id = Uuid::now_v7();
        db.query(
            "CREATE $id SET mailroom_id = $mailroom_id, direction = $direction, \
                   sender = $sender, recipient = $recipient, summary = $summary",
        )
        .bind(("id", crate::surreal::record_id(super::TABLE, id)))
        .bind((
            "mailroom_id",
            crate::surreal::record_id(super::MAILROOM_TABLE, mailroom_id),
        ))
        .bind(("direction", DIRECTION_INCOMING.to_string()))
        .bind(("sender", "IRS".to_string()))
        .bind(("recipient", "Acme".to_string()))
        .bind(("summary", "Pre-ENG-310 mail".to_string()))
        .await
        .unwrap();

        let read = find_by_id(&db, id).await.unwrap().expect("row reads back");
        assert_eq!(read.project_id, None);
    }

    #[tokio::test]
    async fn a_letter_at_a_mailroom_that_does_not_exist_is_refused() {
        let db = mem().await;
        let nowhere = Uuid::now_v7();

        // The engine would accept this link — `record<mailroom>` is not
        // validated — so the read-back is what refuses it.
        let refused = record(&db, &a_letter(nowhere, "Ghost mail")).await;
        assert!(matches!(
            refused,
            Err(LetterError::NoSuchMailroom(id)) if id == nowhere
        ));
    }

    #[tokio::test]
    async fn an_unknown_direction_is_refused_by_the_schema() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;
        let mut sideways = a_letter(mailroom_id, "Sideways mail");
        sideways.direction = "sideways".into();

        // The ASSERT makes a typo a write-time error instead of a row
        // that vanishes from every surface that filters on direction.
        let refused = record(&db, &sideways).await;
        assert!(
            matches!(refused, Err(LetterError::Db(_))),
            "got {refused:?}"
        );
    }

    #[tokio::test]
    async fn both_directions_are_accepted() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;
        for direction in [DIRECTION_INCOMING, DIRECTION_OUTGOING] {
            let mut letter = a_letter(mailroom_id, direction);
            letter.direction = direction.to_string();
            let written = record(&db, &letter).await.unwrap();
            assert_eq!(written.direction, direction);
        }
        assert_eq!(count(&db).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn the_seeders_natural_key_finds_only_that_letter() {
        let db = mem().await;
        let mailroom_id = a_mailroom(&db, "HQ").await;
        let written = record(&db, &a_letter(mailroom_id, "Form 990 reminder"))
            .await
            .unwrap();

        assert_eq!(
            find_by_mailroom_sender_summary(&db, mailroom_id, "IRS", "Form 990 reminder")
                .await
                .unwrap(),
            Some(written)
        );
        assert_eq!(
            find_by_mailroom_sender_summary(&db, mailroom_id, "IRS", "Something else")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_letter_is_scoped_to_its_own_mailroom() {
        let db = mem().await;
        let hq = a_mailroom(&db, "HQ").await;
        let annex = a_mailroom(&db, "Annex").await;
        record(&db, &a_letter(hq, "Form 990 reminder"))
            .await
            .unwrap();

        assert_eq!(
            find_by_mailroom_sender_summary(&db, annex, "IRS", "Form 990 reminder")
                .await
                .unwrap(),
            None,
            "the same sender and summary at another mailroom is another letter"
        );
    }

    #[tokio::test]
    async fn counting_an_empty_table_is_zero_rather_than_an_error() {
        let db = mem().await;
        assert_eq!(count(&db).await.unwrap(), 0);
        let mailroom_id = a_mailroom(&db, "HQ").await;
        record(&db, &a_letter(mailroom_id, "One")).await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);
        assert_eq!(list_all(&db).await.unwrap().len(), 1);
    }
}
