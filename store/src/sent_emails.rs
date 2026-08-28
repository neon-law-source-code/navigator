//! The outbound-email audit trail — one row per message that went
//! through `EmailService`, and every query against it.
//!
//! # This table lives in SurrealDB
//!
//! `sent_emails` moved with wave two of the flat-table ports (#1093;
//! ENG-20). It is a leaf audit table — nothing references it and it
//! references nothing — so the port could not cascade. Rows are written
//! by `portal::email`'s `LoggingEmail` decorator and read by
//! `/app/admin/email-log`.
//!
//! # Append-only
//!
//! There is no update and no delete. [`record`] is the only writer, and
//! it is best-effort at its call site: a failed audit insert must not
//! fail the send that was already attempted. That is also why nothing
//! here retries a transaction conflict — a lost race on an append-only
//! table with no unique index means the row is simply written again by
//! the caller's next attempt, and the decorator logs the miss rather
//! than propagating it.
//!
//! # The listing needs an explicit transaction
//!
//! [`page`] asks two questions — how many rows are there, and what are
//! the rows on this page — and the answers have to describe the same
//! table, so both read one snapshot. A row appended between the two
//! pushes onto page 1 and shifts the rest down, leaving the oldest row on
//! an unreachable page.
//!
//! **Putting the statements in one `query` call is not enough.**
//! SurrealDB gives every statement outside a `BEGIN` block its own
//! transaction — a batch of three `SELECT`s is three independent
//! snapshots taken at three different instants, and the batch does not
//! even stop at the first error. Only `BEGIN` consumes the following
//! statements into a single transaction, so [`page`] spells it out.
//!
//! Two consequences worth knowing. `BEGIN` always opens a *write*
//! transaction, even for a read-only block, so this pays write-conflict
//! exposure for a read. And inside a block, a transaction conflict is
//! reported on the `COMMIT` row while every earlier row reads
//! "not executed" — so the retryable kind is not on the row you would
//! naturally inspect.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "sent_email";

/// One journaled outbound message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SentEmail {
    pub id: Uuid,
    pub recipient: String,
    pub subject: String,
    /// The `From:` actually used, so the trail reflects what the
    /// backend sent rather than what the default was.
    pub sender: String,
    /// Slug of the template that rendered the body (`welcome`, …).
    /// `None` for ad-hoc messages.
    pub template_slug: Option<String>,
    pub body: String,
    /// `sent` on success, `failed:<reason>` on failure.
    pub outcome: String,
    /// SendGrid's `X-Message-Id`, captured on a 202. `None` for failed
    /// sends and for the capturing dev backend.
    pub sg_message_id: Option<String>,
    pub sent_at: DateTime<Utc>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Everything [`record`] needs to journal one attempt. A struct rather
/// than eight positional arguments, three of which are `Option<String>`.
#[derive(Debug, Clone)]
pub struct NewSentEmail {
    pub recipient: String,
    pub subject: String,
    pub sender: String,
    pub template_slug: Option<String>,
    pub body: String,
    pub outcome: String,
    pub sg_message_id: Option<String>,
    pub sent_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it — the seam between
/// [`SentEmail`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct SentEmailRow {
    id: surrealdb::types::RecordId,
    recipient: String,
    subject: String,
    sender: String,
    template_slug: Option<String>,
    body: String,
    outcome: String,
    sg_message_id: Option<String>,
    sent_at: surrealdb::types::Datetime,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl SentEmailRow {
    /// `None` when the record id is not a native UUID key — a row
    /// written by something that bypassed [`crate::surreal::record_id`].
    fn into_sent_email(self) -> Option<SentEmail> {
        Some(SentEmail {
            id: record_uuid(&self.id)?,
            recipient: self.recipient,
            subject: self.subject,
            sender: self.sender,
            template_slug: self.template_slug,
            body: self.body,
            outcome: self.outcome,
            sg_message_id: self.sg_message_id,
            sent_at: self.sent_at.into(),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`SentEmailRow`] from only one query.
const SELECT: &str = "id, recipient, subject, sender, template_slug, body, outcome, \
                      sg_message_id, sent_at, inserted_at, updated_at";

/// Errors reading or writing the audit trail.
#[derive(Debug, thiserror::Error)]
pub enum SentEmailError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see
    /// [`SentEmailRow::into_sent_email`].
    #[error("writing a sent email returned no usable row")]
    WriteReturnedNothing,
}

/// One page of the audit trail, with the totals that describe the same
/// snapshot the rows came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The rows on this page, newest first.
    pub rows: Vec<SentEmail>,
    /// How many pages exist. At least 1, so an empty table still reads
    /// as "Page 1 of 1" rather than "Page 1 of 0".
    pub total_pages: u64,
    /// The page actually returned — the request clamped into range, so
    /// an out-of-range `?page=` yields the final page and says so.
    pub page: u64,
}

/// Journal one outbound attempt.
///
/// # Errors
///
/// [`SentEmailError::Db`] if the insert fails. The caller is expected
/// to treat that as a logged audit miss, not a failed send.
pub async fn record(db: &SurrealDb, new: &NewSentEmail) -> Result<SentEmail, SentEmailError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             recipient = $recipient, \
             subject = $subject, \
             sender = $sender, \
             template_slug = $template_slug, \
             body = $body, \
             outcome = $outcome, \
             sg_message_id = $sg_message_id, \
             sent_at = $sent_at \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("recipient", new.recipient.clone()))
        .bind(("subject", new.subject.clone()))
        .bind(("sender", new.sender.clone()))
        .bind(("template_slug", new.template_slug.clone()))
        .bind(("body", new.body.clone()))
        .bind(("outcome", new.outcome.clone()))
        .bind(("sg_message_id", new.sg_message_id.clone()))
        .bind(("sent_at", surrealdb::types::Datetime::from(new.sent_at)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<SentEmailRow> = response.take(0)?;
    row.and_then(SentEmailRow::into_sent_email)
        .ok_or(SentEmailError::WriteReturnedNothing)
}

/// One page of the trail, newest first, with `requested` clamped into
/// range.
///
/// The count, the clamp, and the fetch run inside one explicit
/// `BEGIN`/`COMMIT` transaction, so all three describe one snapshot of
/// the table — see the module header for why a bare statement batch
/// would not. `sent_at` is not unique, so the sort breaks ties on
/// the record id (also descending, and time-ordered because the ids are
/// v7) — without a total order, rows sharing a timestamp across a page
/// boundary could be duplicated or dropped between two page requests.
///
/// # Errors
///
/// [`SentEmailError::Db`] if the query fails.
pub async fn page(db: &SurrealDb, requested: u64, per_page: u64) -> Result<Page, SentEmailError> {
    let per_page = per_page.max(1);
    let requested = requested.max(1);
    let mut response = db
        .query(format!(
            // The explicit `BEGIN`/`COMMIT` is load-bearing, not
            // decoration. SurrealDB does **not** run a multi-statement
            // query in one transaction: every statement outside a
            // `BEGIN` block gets its own, so an unwrapped batch would
            // count in one snapshot and fetch in another — exactly the
            // race this pager must not lose. `BEGIN` is what consumes
            // the following statements into a single transaction.
            //
            // `GROUP ALL` yields one `{ count: n }` object rather than a
            // bare number, so the count is reached through `.count`;
            // dividing the object itself produces NaN. It yields an
            // empty array on an empty table, which is what `?? 0`
            // catches.
            //
            // `/` on two ints truncates, so the page count is the exact
            // integer ceiling `(n + per - 1) / per` rather than
            // `math::ceil` over a float — no rounding to reason about,
            // and it already yields 0 for an empty table, which the
            // `math::max` lifts to the one empty page. LIMIT and START
            // accept only ints, hence the casts.
            "BEGIN; \
             LET $total = (SELECT count() FROM {TABLE} GROUP ALL)[0].count ?? 0; \
             LET $pages = <int> math::max([1, ($total + $per_page - 1) / $per_page]); \
             LET $page = <int> math::min([$requested, $pages]); \
             LET $start = <int> (($page - 1) * $per_page); \
             SELECT {SELECT} FROM {TABLE} \
             ORDER BY sent_at DESC, id DESC LIMIT $per_page START $start; \
             RETURN [$pages, $page]; \
             COMMIT;"
        ))
        .bind(("per_page", per_page))
        .bind(("requested", requested))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    // `BEGIN` is slot 0 and the four `LET`s are 1..=4, so the `SELECT`
    // is 5 and the `RETURN` is 6 (`COMMIT` is 7). Every one of those
    // statements consumes a slot, and an out-of-range `take` returns an
    // empty result rather than an error — so a miscount here fails
    // silently, which is why `paging_walks_the_trail_…` pins it.
    let rows: Vec<SentEmailRow> = response.take(5)?;
    let totals: Vec<i64> = response.take(6)?;
    let as_u64 = |v: Option<&i64>| u64::try_from(v.copied().unwrap_or(1)).unwrap_or(1).max(1);

    Ok(Page {
        rows: rows
            .into_iter()
            .filter_map(SentEmailRow::into_sent_email)
            .collect(),
        total_pages: as_u64(totals.first()),
        page: as_u64(totals.get(1)),
    })
}

/// Every journaled message, newest first. The whole trail, for tests
/// and for callers that need it without paging.
///
/// # Errors
///
/// [`SentEmailError::Db`] if the lookup fails.
pub async fn all(db: &SurrealDb) -> Result<Vec<SentEmail>, SentEmailError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY sent_at DESC, id DESC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<SentEmailRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(SentEmailRow::into_sent_email)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{all, page, record, NewSentEmail};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use chrono::{DateTime, TimeZone, Utc};

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 4, 12, minute, 0).unwrap()
    }

    fn a_message(recipient: &str, sent_at: DateTime<Utc>) -> NewSentEmail {
        NewSentEmail {
            recipient: recipient.to_string(),
            subject: "Your matter".to_string(),
            sender: "support@neonlaw.com".to_string(),
            template_slug: Some("welcome".to_string()),
            body: "Hello.".to_string(),
            outcome: "sent".to_string(),
            sg_message_id: Some("sg-1".to_string()),
            sent_at,
        }
    }

    async fn journal(db: &SurrealDb, count: u32) {
        for n in 0..count {
            record(db, &a_message(&format!("a{n}@example.com"), at(n)))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_recorded_message_reads_back_whole() {
        let db = mem().await;
        let written = record(&db, &a_message("virgo@example.com", at(0)))
            .await
            .unwrap();

        assert_eq!(written.recipient, "virgo@example.com");
        assert_eq!(written.outcome, "sent");
        assert_eq!(written.sent_at, at(0));
        assert_eq!(written.sg_message_id.as_deref(), Some("sg-1"));
        assert_eq!(all(&db).await.unwrap(), vec![written]);
    }

    #[tokio::test]
    async fn a_failed_send_journals_its_reason_and_no_upstream_id() {
        let db = mem().await;
        // The audit trail has no closed set of outcomes: it records what
        // the backend actually said.
        let mut failed = a_message("virgo@example.com", at(0));
        failed.outcome = "failed:connection refused".to_string();
        failed.sg_message_id = None;
        failed.template_slug = None;

        let written = record(&db, &failed).await.unwrap();
        assert_eq!(written.outcome, "failed:connection refused");
        assert_eq!(written.sg_message_id, None);
        assert_eq!(written.template_slug, None);
    }

    #[tokio::test]
    async fn the_trail_reads_newest_first() {
        let db = mem().await;
        journal(&db, 3).await;
        let recipients: Vec<String> = all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.recipient)
            .collect();
        assert_eq!(
            recipients,
            ["a2@example.com", "a1@example.com", "a0@example.com"]
        );
    }

    #[tokio::test]
    async fn paging_walks_the_trail_without_dropping_or_repeating_a_row() {
        let db = mem().await;
        journal(&db, 5).await;

        let first = page(&db, 1, 2).await.unwrap();
        assert_eq!(first.total_pages, 3);
        assert_eq!(first.page, 1);
        assert_eq!(first.rows.len(), 2);

        let second = page(&db, 2, 2).await.unwrap();
        let third = page(&db, 3, 2).await.unwrap();
        assert_eq!(third.rows.len(), 1, "the final page is partial");

        let walked: Vec<String> = first
            .rows
            .iter()
            .chain(&second.rows)
            .chain(&third.rows)
            .map(|r| r.recipient.clone())
            .collect();
        let everything: Vec<String> = all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.recipient)
            .collect();
        assert_eq!(walked, everything, "paging visits every row exactly once");
    }

    #[tokio::test]
    async fn an_out_of_range_page_is_clamped_to_the_last_one() {
        let db = mem().await;
        journal(&db, 3).await;

        // The pager renders `page` against `total_pages`; an unclamped
        // request would fetch nothing while the label still claimed a
        // real page.
        let beyond = page(&db, 99, 2).await.unwrap();
        assert_eq!(beyond.total_pages, 2);
        assert_eq!(beyond.page, 2, "clamped to the final page");
        assert_eq!(beyond.rows.len(), 1);
    }

    #[tokio::test]
    async fn an_empty_trail_is_one_empty_page() {
        let db = mem().await;
        let empty = page(&db, 1, 50).await.unwrap();
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty.total_pages, 1,
            "an empty table reads as Page 1 of 1, never Page 1 of 0"
        );
        assert_eq!(empty.page, 1);
    }

    #[tokio::test]
    async fn a_full_page_boundary_does_not_invent_an_empty_last_page() {
        let db = mem().await;
        journal(&db, 4).await;
        let exact = page(&db, 1, 2).await.unwrap();
        assert_eq!(exact.total_pages, 2, "4 rows at 2 per page is 2 pages");
    }
}
