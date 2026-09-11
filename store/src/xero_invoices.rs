//! Xero invoice mirror helpers.
//!
//! A matter's invoice is raised **in Xero**, by lawyer, at the price they
//! agreed with the client — Navigator never raises one. This table is the
//! local mirror of those invoices, so the portal can show a per-project
//! invoice list without calling Xero live. A matter carries many invoices
//! over time (ENG-588), not one. Two writers touch a row:
//!
//! - [`upsert`] — captures the Xero `InvoiceID` + total. A re-run updates
//!   the one row rather than inserting a second (preserving any reconciled
//!   `amount_paid_cents`).
//! - [`record_reconcile`] — the nightly reconcile workflow folds Xero's
//!   `Status` + `AmountPaid` back onto the mirror.
//!
//! **The mirror's own record id is the Xero invoice id, not an
//! independently minted one.** A UNIQUE index on the field alone reads like
//! what serializes concurrent creates and does not: racers that each mint
//! their own record id write to distinct keys, so the engine's optimistic
//! layer has nothing to conflict on and can commit two rows for one Xero
//! invoice before either observes the other's index entry — `store::persons`
//! hit the identical shape (see `store/tests/person_mailbox_race.rs`) and it
//! reproduces here in
//! [`tests::the_unique_index_alone_does_not_serialize_racers`]. Keying the
//! row on the Xero invoice id instead makes the *primary* key the collision
//! point: a second `CREATE` for the same Xero invoice collides on that key
//! and reports a typed [`surrealdb::types::AlreadyExistsError::Record`],
//! which the optimistic layer does serialize.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::{AlreadyExistsError, ErrorDetails, RecordId, SurrealValue};
use thiserror::Error;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The SurrealDB table holding the local Xero invoice mirror.
pub(crate) const TABLE: &str = "xero_invoice";
const SELECT: &str = "project_id, xero_invoice_id, reference, status, amount_cents, \
                     amount_paid_cents, currency, issued_at, due_at, inserted_at, updated_at";

/// One local mirror of a Xero invoice.
///
/// The application-facing shape: plain Rust types, no engine handles. The
/// Xero invoice id doubles as this row's own record id (see the module
/// docs), so it is the row's identity — there is no separate mirror-local
/// id. [`XeroInvoiceRow`] is the seam that turns it into what the SDK reads
/// and writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct XeroInvoice {
    /// The matter this invoice bills. Several invoices may share one.
    pub project_id: Uuid,
    /// Xero `InvoiceID` (GUID) returned on create, and this row's own record
    /// id. Internal — never surfaced on a client-facing response.
    pub xero_invoice_id: String,
    /// The invoice-level `Reference` carried into Xero (`Matter <project_id>`).
    pub reference: String,
    /// Xero invoice status mirror (`AUTHORISED`, `PAID`, `VOIDED`, …).
    pub status: String,
    /// Invoice total in minor units (cents). Avoids float.
    pub amount_cents: i64,
    /// Amount paid in minor units (cents); `0` until reconciled.
    pub amount_paid_cents: i64,
    /// ISO 4217 currency code (for example, `USD`).
    pub currency: String,
    /// When Xero raised the invoice (Xero `Date`).
    pub issued_at: DateTime<Utc>,
    /// When it falls due, if Xero recorded one (Xero `DueDate`).
    pub due_at: Option<DateTime<Utc>>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct XeroInvoiceRow {
    project_id: RecordId,
    xero_invoice_id: String,
    reference: String,
    status: String,
    amount_cents: i64,
    amount_paid_cents: i64,
    currency: String,
    issued_at: surrealdb::types::Datetime,
    due_at: Option<surrealdb::types::Datetime>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl XeroInvoiceRow {
    /// `None` when `project_id` is not a native UUID key. Navigator only
    /// writes native UUID ids through [`record_id`], so reporting another
    /// shape would invent an id this module cannot faithfully represent.
    fn into_invoice(self) -> Option<XeroInvoice> {
        Some(XeroInvoice {
            project_id: record_uuid(&self.project_id)?,
            xero_invoice_id: self.xero_invoice_id,
            reference: self.reference,
            status: self.status,
            amount_cents: self.amount_cents,
            amount_paid_cents: self.amount_paid_cents,
            currency: self.currency,
            issued_at: self.issued_at.into(),
            due_at: self.due_at.map(Into::into),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// Errors reading or writing a Xero invoice mirror.
///
/// This is deliberately not a bare [`surrealdb::Error`]: `portal`, `webapp`,
/// and `billing-workflows` consume this module without depending on the
/// SurrealDB crate.
#[derive(Debug, Error)]
pub enum XeroInvoiceError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the Projects a Firm owns, or their DRIs, failed —
    /// [`for_firm_since`] only.
    #[error(transparent)]
    Projects(#[from] crate::projects::ProjectStoreError),
    /// Another writer created this Xero invoice's mirror row after this
    /// writer checked. [`upsert`] resolves this race by applying the
    /// metadata update to that row directly, since its id is the Xero
    /// invoice id.
    #[error("that Xero invoice is already mirrored")]
    InvoiceTaken,
    /// A write claimed success but returned no usable row.
    #[error("writing a Xero invoice mirror returned no usable row")]
    WriteReturnedNothing,
}

/// Whether `error` is a second `CREATE` colliding with the mirror row this
/// Xero invoice already has.
///
/// The collision is **typed**: `CREATE` onto a taken record id reports
/// [`AlreadyExistsError::Record`] carrying that id, so the discriminator is
/// a structured value rather than prose — unlike the UNIQUE-index violation
/// [`crate::surreal::retry::unique_violation`] has to read the message for.
/// The `project_id` index is a plain lookup index now (ENG-588), not a
/// backstop against this write; it is not what this classifier checks.
fn classify_write(error: surrealdb::Error) -> XeroInvoiceError {
    match error.details() {
        ErrorDetails::AlreadyExists(Some(AlreadyExistsError::Record { id }))
            if id.starts_with(TABLE) =>
        {
            XeroInvoiceError::InvoiceTaken
        }
        _ => XeroInvoiceError::Db(error),
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, XeroInvoiceError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// The fields captured when a Xero invoice is mirrored locally. `currency`
/// defaults to `USD` at the call site; amounts are minor units (cents).
#[derive(Clone, Debug)]
pub struct UpsertXeroInvoice {
    pub project_id: Uuid,
    pub xero_invoice_id: String,
    pub reference: String,
    /// Xero invoice status at create time (`AUTHORISED`).
    pub status: String,
    pub amount_cents: i64,
    pub currency: String,
    /// When Xero raised the invoice (Xero `Date`).
    pub issued_at: DateTime<Utc>,
    /// When it falls due, if Xero recorded one (Xero `DueDate`).
    pub due_at: Option<DateTime<Utc>>,
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<XeroInvoice>, XeroInvoiceError> {
    let row: Option<XeroInvoiceRow> = response.take(0)?;
    Ok(row.and_then(XeroInvoiceRow::into_invoice))
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<XeroInvoice>, XeroInvoiceError> {
    let rows: Vec<XeroInvoiceRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(XeroInvoiceRow::into_invoice)
        .collect())
}

/// The mirror row for one Xero invoice, addressed by the record id its own
/// Xero invoice id doubles as.
async fn for_xero_id(
    db: &SurrealDb,
    xero_invoice_id: &str,
) -> Result<Option<XeroInvoice>, XeroInvoiceError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", RecordId::new(TABLE, xero_invoice_id.to_string())))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// `input.xero_invoice_id` doubles as the mirror row's own id — see the
/// module docs for why that, not a UNIQUE index, is what makes a racing
/// second `CREATE` collide reliably.
async fn create(
    db: &SurrealDb,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    let response = writing(|| {
        db.query(format!(
            "CREATE $id SET project_id = $project_id, xero_invoice_id = $xero_invoice_id, \
             reference = $reference, status = $status, amount_cents = $amount_cents, \
             currency = $currency, issued_at = $issued_at, due_at = $due_at \
             RETURN {SELECT}"
        ))
        .bind(("id", RecordId::new(TABLE, input.xero_invoice_id.clone())))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, input.project_id),
        ))
        .bind(("xero_invoice_id", input.xero_invoice_id.clone()))
        .bind(("reference", input.reference.clone()))
        .bind(("status", input.status.clone()))
        .bind(("amount_cents", input.amount_cents))
        .bind(("currency", input.currency.clone()))
        .bind((
            "issued_at",
            surrealdb::types::Datetime::from(input.issued_at),
        ))
        .bind(("due_at", input.due_at.map(surrealdb::types::Datetime::from)))
    })
    .await?;
    one(response)?.ok_or(XeroInvoiceError::WriteReturnedNothing)
}

async fn update_from_upsert(
    db: &SurrealDb,
    xero_invoice_id: &str,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    let response = writing(|| {
        db.query(format!(
            "UPDATE $id SET project_id = $project_id, reference = $reference, \
             status = $status, amount_cents = $amount_cents, currency = $currency, \
             issued_at = $issued_at, due_at = $due_at, updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("id", RecordId::new(TABLE, xero_invoice_id.to_string())))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, input.project_id),
        ))
        .bind(("reference", input.reference.clone()))
        .bind(("status", input.status.clone()))
        .bind(("amount_cents", input.amount_cents))
        .bind(("currency", input.currency.clone()))
        .bind((
            "issued_at",
            surrealdb::types::Datetime::from(input.issued_at),
        ))
        .bind(("due_at", input.due_at.map(surrealdb::types::Datetime::from)))
    })
    .await?;
    one(response)?.ok_or(XeroInvoiceError::WriteReturnedNothing)
}

/// Idempotently mirror a raised Xero invoice, keyed on its own Xero invoice
/// id.
///
/// Inserts a fresh row, or — when one already exists for that Xero invoice —
/// updates the reference / status / total / dates in place while
/// **preserving** the reconciled `amount_paid_cents` (the reconcile workflow
/// owns that field). Two different Xero invoices for the same matter are two
/// rows; the create path settles a race on the mirror's own record id (see
/// the module docs): a competing writer's `CREATE` under the same
/// Xero-invoice-id-derived id is the row this one applies its metadata-only
/// update to, with no extra read needed to find it.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] when SurrealDB cannot read or write the mirror.
pub async fn upsert(
    db: &SurrealDb,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    if for_xero_id(db, &input.xero_invoice_id).await?.is_some() {
        return update_from_upsert(db, &input.xero_invoice_id, input).await;
    }

    match create(db, input).await {
        Ok(created) => Ok(created),
        Err(XeroInvoiceError::InvoiceTaken) => {
            update_from_upsert(db, &input.xero_invoice_id, input).await
        }
        Err(error) => Err(error),
    }
}

/// Fold a reconcile result (Xero `Status` + `AmountPaid`) onto one invoice's
/// mirror row. No-op (returns `None`) when no mirror row exists yet for that
/// Xero invoice id.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] when SurrealDB cannot update the mirror.
pub async fn record_reconcile(
    db: &SurrealDb,
    xero_invoice_id: &str,
    status: &str,
    amount_paid_cents: i64,
) -> Result<Option<XeroInvoice>, XeroInvoiceError> {
    let response = writing(|| {
        db.query(format!(
            "UPDATE {TABLE} SET status = $status, amount_paid_cents = $amount_paid_cents, \
             updated_at = time::now() WHERE xero_invoice_id = $xero_invoice_id RETURN {SELECT}"
        ))
        .bind(("status", status.to_string()))
        .bind(("amount_paid_cents", amount_paid_cents))
        .bind(("xero_invoice_id", xero_invoice_id.to_string()))
    })
    .await?;
    one(response)
}

/// Fetch every invoice mirrored for a set of matters, newest first, for the
/// project-scoped portal invoice list. Empty input short-circuits to an
/// empty vec.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] if the lookup fails.
pub async fn for_projects(
    db: &SurrealDb,
    project_ids: &[Uuid],
) -> Result<Vec<XeroInvoice>, XeroInvoiceError> {
    if project_ids.is_empty() {
        return Ok(Vec::new());
    }
    let projects: Vec<RecordId> = project_ids
        .iter()
        .map(|id| record_id(crate::projects::PROJECT_TABLE, *id))
        .collect();
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id IN $projects ORDER BY issued_at DESC"
        ))
        .bind(("projects", projects))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// One invoice as the Firm-wide invoice graphs (ENG-591) consume it: the
/// mirror's own fields plus the Project's brand and lawyer DRI names, so
/// that caller never re-joins per row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FirmInvoice {
    pub project_id: Uuid,
    pub xero_invoice_id: String,
    pub status: String,
    pub amount_cents: i64,
    pub amount_paid_cents: i64,
    pub issued_at: DateTime<Utc>,
    /// The Project's brand key, for grouping by brand.
    pub brand: String,
    /// The Project's lawyer DRI names, alphabetical, or empty when
    /// unassigned — mirrors `store::projects::MatterDirectoryEntry`.
    pub lawyer_dris: Vec<String>,
}

/// Every invoice issued on or after `since`, for every Project a Firm owns —
/// the read the Firm show page's invoice graphs (ENG-591) consume.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] if the lookup fails; [`XeroInvoiceError::Projects`]
/// if reading the Firm's Projects or their lawyer DRIs fails.
pub async fn for_firm_since(
    db: &SurrealDb,
    firm_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<FirmInvoice>, XeroInvoiceError> {
    let brand_by_project: std::collections::HashMap<Uuid, String> = crate::projects::all(db)
        .await?
        .into_iter()
        .filter(|project| project.firm_id == Some(firm_id))
        .map(|project| (project.id, project.brand))
        .collect();
    if brand_by_project.is_empty() {
        return Ok(Vec::new());
    }
    let project_ids: Vec<Uuid> = brand_by_project.keys().copied().collect();
    let lawyer_dris = crate::projects::dri_names_by_project(db, "is_lawyer_dri").await?;

    Ok(for_projects(db, &project_ids)
        .await?
        .into_iter()
        .filter(|invoice| invoice.issued_at >= since)
        .filter_map(|invoice| {
            let brand = brand_by_project.get(&invoice.project_id)?.clone();
            Some(FirmInvoice {
                lawyer_dris: lawyer_dris
                    .get(&invoice.project_id)
                    .cloned()
                    .unwrap_or_default(),
                brand,
                project_id: invoice.project_id,
                xero_invoice_id: invoice.xero_invoice_id,
                status: invoice.status,
                amount_cents: invoice.amount_cents,
                amount_paid_cents: invoice.amount_paid_cents,
                issued_at: invoice.issued_at,
            })
        })
        .collect())
}

/// The mirror rows that the nightly reconcile should re-check: anything not
/// already in a terminal state (`PAID` / `VOIDED`). A settled invoice is never
/// polled again.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] if the lookup fails.
pub async fn needing_reconcile(db: &SurrealDb) -> Result<Vec<XeroInvoice>, XeroInvoiceError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE status NOT IN ['PAID', 'VOIDED']"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

#[cfg(test)]
mod tests {
    use super::{
        for_firm_since, for_projects, needing_reconcile, record_reconcile, upsert,
        UpsertXeroInvoice,
    };
    use crate::surreal::SurrealDb;
    use chrono::{Duration, TimeZone, Utc};

    async fn seed_project(db: &SurrealDb, name: &str) -> uuid::Uuid {
        crate::test_support::seed_project_surreal(db, name).await
    }

    fn input(project_id: uuid::Uuid, xero_id: &str, cents: i64) -> UpsertXeroInvoice {
        UpsertXeroInvoice {
            project_id,
            xero_invoice_id: xero_id.into(),
            reference: format!("Matter {project_id}"),
            status: "AUTHORISED".into(),
            amount_cents: cents,
            currency: "USD".into(),
            issued_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
            due_at: None,
        }
    }

    #[tokio::test]
    async fn upsert_inserts_one_row() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        let row = upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();
        assert_eq!(row.project_id, project_id);
        assert_eq!(row.xero_invoice_id, "xero-1");
        assert_eq!(row.amount_cents, 333_300);
        assert_eq!(row.amount_paid_cents, 0);

        assert_eq!(
            for_projects(&surreal, &[project_id]).await.unwrap().len(),
            1
        );
    }

    /// ENG-588: a matter carries many invoices over time — two different
    /// Xero ids for one Project must land as two rows, not a collision.
    #[tokio::test]
    async fn two_different_invoices_for_one_matter_are_two_rows() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        upsert(&surreal, &input(project_id, "xero-1", 10_000))
            .await
            .unwrap();
        upsert(&surreal, &input(project_id, "xero-2", 20_000))
            .await
            .unwrap();

        let rows = for_projects(&surreal, &[project_id]).await.unwrap();
        assert_eq!(rows.len(), 2, "two Xero ids must mirror as two rows");
        let ids: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.xero_invoice_id.as_str()).collect();
        assert_eq!(ids, ["xero-1", "xero-2"].into_iter().collect());
    }

    #[tokio::test]
    async fn invoice_id_index_refuses_a_second_row_for_the_same_invoice() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;
        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();

        let error = surreal
            .query(
                "CREATE xero_invoice:⟨xero-1⟩ SET project_id = $project, \
                 xero_invoice_id = 'xero-1', reference = 'duplicate', \
                 status = 'AUTHORISED', amount_cents = 333300, currency = 'USD', \
                 issued_at = time::now()",
            )
            .bind((
                "project",
                crate::surreal::record_id(crate::projects::PROJECT_TABLE, project_id),
            ))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("a second CREATE under the same Xero invoice id must be refused");
        assert!(
            matches!(
                error.details(),
                surrealdb::types::ErrorDetails::AlreadyExists(Some(
                    surrealdb::types::AlreadyExistsError::Record { .. }
                ))
            ),
            "expected an AlreadyExists(Record) refusal, got: {error}"
        );
    }

    #[tokio::test]
    async fn upsert_is_idempotent_on_xero_invoice_id() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();
        let updated = upsert(&surreal, &input(project_id, "xero-1", 444_400))
            .await
            .unwrap();
        assert_eq!(updated.amount_cents, 444_400, "the repeat updates the row");

        assert_eq!(
            for_projects(&surreal, &[project_id]).await.unwrap().len(),
            1,
            "a repeated Xero id must update in place, not insert a second row"
        );
    }

    /// The control this module's guard exists against: a UNIQUE index alone,
    /// with each racer minting its own record id. This is the shape
    /// [`super::create`] used before it started keying the row on the Xero
    /// invoice id, kept here so a future refactor cannot drop that
    /// convention as redundant with a schema index.
    ///
    /// The assertion is not that every round forks — it needs a loaded
    /// machine to lose reliably, the same caveat
    /// `store::persons::tests`' analogous control carries — but that
    /// nothing about this shape refuses a second row on its own merits.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_unique_index_alone_does_not_serialize_racers() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        let racers: Vec<_> =
            (0..8)
                .map(|_| {
                    let surreal = surreal.clone();
                    tokio::spawn(async move {
                        surreal
                        .query(
                            "CREATE $id SET project_id = $project, xero_invoice_id = 'xero-1', \
                             reference = 'race', status = 'AUTHORISED', \
                             amount_cents = 333300, currency = 'USD', issued_at = time::now()",
                        )
                        .bind(("id", crate::surreal::record_id(super::TABLE, uuid::Uuid::now_v7())))
                        .bind((
                            "project",
                            crate::surreal::record_id(crate::projects::PROJECT_TABLE, project_id),
                        ))
                        .await
                        .and_then(surrealdb::IndexedResults::check)
                    })
                })
                .collect();

        let mut landed = 0;
        for racer in racers {
            if racer.await.expect("racer task").is_ok() {
                landed += 1;
            }
        }
        assert!(landed >= 1, "at least one unguarded write must land");
    }

    /// The mirror's own record id is the concurrency boundary (see the
    /// module docs), not merely an invariant observed after sequential
    /// calls. Every mirror replay must settle on the one winner rather than
    /// surface its create race to Restate.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_upserts_for_one_invoice_settle_on_one_row() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;
        let input = input(project_id, "xero-1", 333_300);

        let racers: Vec<_> = (0..8)
            .map(|_| {
                let surreal = surreal.clone();
                let input = input.clone();
                tokio::spawn(async move { upsert(&surreal, &input).await })
            })
            .collect();

        let mut ids = std::collections::BTreeSet::new();
        for (number, racer) in racers.into_iter().enumerate() {
            let row = racer.await.expect("racer task").unwrap_or_else(|error| {
                panic!("racer {number} was refused instead of settling: {error:?}")
            });
            ids.insert(row.xero_invoice_id);
        }

        assert_eq!(ids.len(), 1, "the racers disagreed about which row won");
        assert_eq!(
            for_projects(&surreal, &[project_id]).await.unwrap().len(),
            1,
            "a race must not leave a second mirror row behind",
        );
    }

    #[tokio::test]
    async fn upsert_preserves_reconciled_amount_paid() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();
        record_reconcile(&surreal, "xero-1", "PAID", 333_300)
            .await
            .unwrap();
        let row = upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();
        assert_eq!(row.amount_paid_cents, 333_300);
        assert_eq!(row.status, "AUTHORISED", "raise resets the create-status");
    }

    #[tokio::test]
    async fn record_reconcile_updates_status_and_paid() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;
        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();

        let row = record_reconcile(&surreal, "xero-1", "PAID", 333_300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "PAID");
        assert_eq!(row.amount_paid_cents, 333_300);
    }

    /// Two invoices on one matter reconcile independently — folding a result
    /// onto one must not touch the other's status or paid amount.
    #[tokio::test]
    async fn record_reconcile_only_touches_its_own_invoice() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;
        upsert(&surreal, &input(project_id, "xero-1", 10_000))
            .await
            .unwrap();
        upsert(&surreal, &input(project_id, "xero-2", 20_000))
            .await
            .unwrap();

        record_reconcile(&surreal, "xero-1", "PAID", 10_000)
            .await
            .unwrap();

        let rows = for_projects(&surreal, &[project_id]).await.unwrap();
        let one = rows.iter().find(|r| r.xero_invoice_id == "xero-1").unwrap();
        let two = rows.iter().find(|r| r.xero_invoice_id == "xero-2").unwrap();
        assert_eq!(one.status, "PAID");
        assert_eq!(two.status, "AUTHORISED", "the other invoice is untouched");
        assert_eq!(two.amount_paid_cents, 0);
    }

    #[tokio::test]
    async fn record_reconcile_is_noop_without_a_row() {
        let surreal = crate::surreal::test_support::mem().await;
        assert!(record_reconcile(&surreal, "no-such-invoice", "PAID", 100)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn needing_reconcile_excludes_settled_invoices() {
        let surreal = crate::surreal::test_support::mem().await;
        let open = seed_project(&surreal, "open").await;
        let paid = seed_project(&surreal, "paid").await;
        let void = seed_project(&surreal, "void").await;
        upsert(&surreal, &input(open, "x-open", 100)).await.unwrap();
        upsert(&surreal, &input(paid, "x-paid", 200)).await.unwrap();
        upsert(&surreal, &input(void, "x-void", 300)).await.unwrap();
        record_reconcile(&surreal, "x-paid", "PAID", 200)
            .await
            .unwrap();
        record_reconcile(&surreal, "x-void", "VOIDED", 0)
            .await
            .unwrap();

        let rows = needing_reconcile(&surreal).await.unwrap();
        assert_eq!(rows.len(), 1, "only the AUTHORISED invoice is re-checked");
        assert_eq!(rows[0].project_id, open);
    }

    #[tokio::test]
    async fn for_projects_filters_to_the_requested_matters_and_orders_newest_first() {
        let surreal = crate::surreal::test_support::mem().await;
        let a = seed_project(&surreal, "a").await;
        let b = seed_project(&surreal, "b").await;
        let c = seed_project(&surreal, "c").await;
        let mut older = input(a, "xero-a-older", 100);
        older.issued_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut newer = input(a, "xero-a-newer", 150);
        newer.issued_at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        upsert(&surreal, &older).await.unwrap();
        upsert(&surreal, &newer).await.unwrap();
        upsert(&surreal, &input(b, "xero-b", 200)).await.unwrap();
        upsert(&surreal, &input(c, "xero-c", 300)).await.unwrap();

        let rows = for_projects(&surreal, &[a, c]).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .all(|row| row.project_id == a || row.project_id == c));
        assert_eq!(
            rows[0].xero_invoice_id, "xero-a-newer",
            "newest issued invoice sorts first"
        );
        assert!(for_projects(&surreal, &[]).await.unwrap().is_empty());
    }

    /// ENG-588: the read `for_firm_since` exists for (ENG-591's Firm invoice
    /// graphs) scopes to one Firm's Projects and excludes anything issued
    /// before the window, joining in each invoice's brand and lawyer DRI.
    #[tokio::test]
    async fn for_firm_since_scopes_to_the_firm_and_the_window() {
        let surreal = crate::surreal::test_support::mem().await;
        let admin = crate::test_support::ensure_person(
            &surreal,
            &crate::persons::NewPerson::with_role(
                "Firm Admin",
                "firm-admin@example.com",
                crate::persons::Role::Admin,
            ),
        )
        .await;
        let firm = crate::firms::create(
            &surreal,
            &crate::firms::NewFirm {
                name: "Invoice Firm".to_string(),
                status: "active".to_string(),
                entity_id: crate::test_support::seed_entity(&surreal).await,
                admin_dri_person_id: admin.id,
            },
        )
        .await
        .unwrap();
        let entity_id = crate::test_support::seed_entity(&surreal).await;
        let owned = crate::projects::create(
            &surreal,
            &crate::projects::NewProject {
                code: "firm-owned-matter".to_string(),
                name: "Firm Owned Matter".to_string(),
                status: "open".to_string(),
                entity_id,
                firm_id: Some(firm.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let other_entity = crate::test_support::seed_entity(&surreal).await;
        let unowned = crate::projects::create(
            &surreal,
            &crate::projects::NewProject {
                code: "other-firm-matter".to_string(),
                name: "Other Firm Matter".to_string(),
                status: "open".to_string(),
                entity_id: other_entity,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        crate::projects::designate_dri_in_surreal(
            &surreal,
            owned.id,
            admin.id,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();

        let since = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut too_old = input(owned.id, "xero-too-old", 100);
        too_old.issued_at = since - Duration::days(1);
        let mut in_window = input(owned.id, "xero-in-window", 200);
        in_window.issued_at = since + Duration::days(1);
        upsert(&surreal, &too_old).await.unwrap();
        upsert(&surreal, &in_window).await.unwrap();
        upsert(&surreal, &input(unowned.id, "xero-other-firm", 300))
            .await
            .unwrap();

        let rows = for_firm_since(&surreal, firm.id, since).await.unwrap();
        assert_eq!(rows.len(), 1, "only the in-window, firm-owned invoice");
        assert_eq!(rows[0].xero_invoice_id, "xero-in-window");
        assert_eq!(rows[0].brand, owned.brand);
        assert_eq!(rows[0].lawyer_dris, vec!["Firm Admin".to_string()]);
    }
}
