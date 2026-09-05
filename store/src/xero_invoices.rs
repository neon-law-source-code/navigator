//! Xero invoice mirror helpers.
//!
//! A matter's invoice is raised **in Xero**, by lawyer, at the price they
//! agreed with the client — Navigator never raises one. This table is the
//! local mirror of that invoice, keyed by `project_id`, so the portal can
//! show a per-project invoice card without calling Xero live. Two writers
//! touch a row:
//!
//! - [`upsert`] — captures the Xero `InvoiceID` + total. Keyed on
//!   `project_id`, so a re-run updates the one row rather than inserting a
//!   second (preserving any reconciled `amount_paid_cents`).
//! - [`record_reconcile`] — the nightly reconcile workflow folds Xero's
//!   `Status` + `AmountPaid` back onto the mirror.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use thiserror::Error;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The SurrealDB table holding one Xero mirror per matter.
pub(crate) const TABLE: &str = "xero_invoice";
const SELECT: &str = "id, project_id, xero_invoice_id, reference, status, amount_cents, \
                     amount_paid_cents, currency, inserted_at, updated_at";

/// One local mirror of a Xero invoice.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`XeroInvoiceRow`] is the seam that turns it into what the SDK reads and
/// writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct XeroInvoice {
    pub id: Uuid,
    /// The matter this invoice bills. One invoice mirror per matter.
    pub project_id: Uuid,
    /// Xero `InvoiceID` (GUID) returned on create. Internal — never surfaced
    /// on a client-facing response.
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
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct XeroInvoiceRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    xero_invoice_id: String,
    reference: String,
    status: String,
    amount_cents: i64,
    amount_paid_cents: i64,
    currency: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl XeroInvoiceRow {
    /// `None` when either record id is not a native UUID key. Navigator only
    /// writes native UUID ids through [`record_id`], so reporting another
    /// shape would invent an id this module cannot faithfully represent.
    fn into_invoice(self) -> Option<XeroInvoice> {
        Some(XeroInvoice {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            xero_invoice_id: self.xero_invoice_id,
            reference: self.reference,
            status: self.status,
            amount_cents: self.amount_cents,
            amount_paid_cents: self.amount_paid_cents,
            currency: self.currency,
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
    /// Another writer created the matter's mirror after this writer checked.
    /// [`upsert`] resolves this race by finding that row and applying the
    /// metadata update to it.
    #[error("that project already has a Xero invoice mirror")]
    ProjectTaken,
    /// A write claimed success but returned no usable row.
    #[error("writing a Xero invoice mirror returned no usable row")]
    WriteReturnedNothing,
}

/// Convert a unique-index failure into the concurrency case it identifies.
///
/// Surreal's unique violations do not carry a typed index name, so the
/// explicit schema identifier is the discriminator. The test below pins this
/// against the real engine, keeping an index rename from becoming an opaque
/// database failure.
fn classify_write(error: surrealdb::Error) -> XeroInvoiceError {
    if crate::surreal::retry::unique_violation(&error) == Some("xero_invoice_project") {
        XeroInvoiceError::ProjectTaken
    } else {
        XeroInvoiceError::Db(error)
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

async fn for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Option<XeroInvoice>, XeroInvoiceError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE project_id = $project LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

async fn create(
    db: &SurrealDb,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    let id = Uuid::now_v7();
    let response = writing(|| {
        db.query(format!(
            "CREATE $id SET project_id = $project_id, xero_invoice_id = $xero_invoice_id, \
             reference = $reference, status = $status, amount_cents = $amount_cents, \
             currency = $currency RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, input.project_id),
        ))
        .bind(("xero_invoice_id", input.xero_invoice_id.clone()))
        .bind(("reference", input.reference.clone()))
        .bind(("status", input.status.clone()))
        .bind(("amount_cents", input.amount_cents))
        .bind(("currency", input.currency.clone()))
    })
    .await?;
    one(response)?.ok_or(XeroInvoiceError::WriteReturnedNothing)
}

async fn update_from_upsert(
    db: &SurrealDb,
    id: Uuid,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    let response = writing(|| {
        db.query(format!(
            "UPDATE $id SET xero_invoice_id = $xero_invoice_id, reference = $reference, \
             status = $status, amount_cents = $amount_cents, currency = $currency, \
             updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("xero_invoice_id", input.xero_invoice_id.clone()))
        .bind(("reference", input.reference.clone()))
        .bind(("status", input.status.clone()))
        .bind(("amount_cents", input.amount_cents))
        .bind(("currency", input.currency.clone()))
    })
    .await?;
    one(response)?.ok_or(XeroInvoiceError::WriteReturnedNothing)
}

/// Idempotently mirror a raised Xero invoice, keyed on `project_id`.
///
/// Inserts a fresh row, or — when one already exists for the matter — updates
/// the Xero id / reference / status / total in place while **preserving** the
/// reconciled `amount_paid_cents` (the reconcile workflow owns that field).
/// The create path intentionally settles a unique-index race by finding the
/// row its competing writer created, then applying the same metadata-only
/// update. A read-then-write without this recovery would turn the unique
/// constraint into an observable retry failure.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] when SurrealDB cannot read or write the mirror.
pub async fn upsert(
    db: &SurrealDb,
    input: &UpsertXeroInvoice,
) -> Result<XeroInvoice, XeroInvoiceError> {
    if let Some(existing) = for_project(db, input.project_id).await? {
        return update_from_upsert(db, existing.id, input).await;
    }

    match create(db, input).await {
        Ok(created) => Ok(created),
        Err(XeroInvoiceError::ProjectTaken) => {
            let existing = for_project(db, input.project_id)
                .await?
                .ok_or(XeroInvoiceError::WriteReturnedNothing)?;
            update_from_upsert(db, existing.id, input).await
        }
        Err(error) => Err(error),
    }
}

/// Fold a reconcile result (Xero `Status` + `AmountPaid`) onto the mirror row
/// for a matter. No-op (returns `None`) when no mirror row exists yet for the
/// project.
///
/// # Errors
///
/// [`XeroInvoiceError::Db`] when SurrealDB cannot update the mirror.
pub async fn record_reconcile(
    db: &SurrealDb,
    project_id: Uuid,
    status: &str,
    amount_paid_cents: i64,
) -> Result<Option<XeroInvoice>, XeroInvoiceError> {
    let response = writing(|| {
        db.query(format!(
            "UPDATE {TABLE} SET status = $status, amount_paid_cents = $amount_paid_cents, \
             updated_at = time::now() WHERE project_id = $project RETURN {SELECT}"
        ))
        .bind(("status", status.to_string()))
        .bind(("amount_paid_cents", amount_paid_cents))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
    })
    .await?;
    one(response)
}

/// Fetch the mirror rows for a set of matters, for the project-scoped portal
/// invoice list. Empty input short-circuits to an empty vec.
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
    let projects: Vec<surrealdb::types::RecordId> = project_ids
        .iter()
        .map(|id| record_id(crate::projects::PROJECT_TABLE, *id))
        .collect();
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id IN $projects"
        ))
        .bind(("projects", projects))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
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
    use super::{for_projects, needing_reconcile, record_reconcile, upsert, UpsertXeroInvoice};
    use crate::surreal::{record_id, SurrealDb};

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

    #[tokio::test]
    async fn project_unique_index_refuses_a_second_invoice() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;
        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();

        let error = surreal
            .query(
                "CREATE xero_invoice SET project_id = $project, xero_invoice_id = 'xero-2', \
                 reference = 'duplicate', status = 'AUTHORISED', amount_cents = 333300, \
                 currency = 'USD'",
            )
            .bind((
                "project",
                record_id(crate::projects::PROJECT_TABLE, project_id),
            ))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("UNIQUE(project_id) must refuse a second mirror row");
        assert!(error.to_string().contains("xero_invoice_project"));
    }

    #[tokio::test]
    async fn upsert_is_idempotent_on_project_id() {
        let surreal = crate::surreal::test_support::mem().await;
        let project_id = seed_project(&surreal, "sample-matter").await;

        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();
        upsert(&surreal, &input(project_id, "xero-1", 333_300))
            .await
            .unwrap();

        assert_eq!(
            for_projects(&surreal, &[project_id]).await.unwrap().len(),
            1
        );
    }

    /// The unique index is the concurrency boundary, not merely an invariant
    /// observed after sequential calls. Every mirror replay must settle on
    /// the one winner rather than surface its create race to Restate.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_upserts_for_one_project_settle_on_one_row() {
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
            ids.insert(row.id);
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
        record_reconcile(&surreal, project_id, "PAID", 333_300)
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

        let row = record_reconcile(&surreal, project_id, "PAID", 333_300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "PAID");
        assert_eq!(row.amount_paid_cents, 333_300);
    }

    #[tokio::test]
    async fn record_reconcile_is_noop_without_a_row() {
        let surreal = crate::surreal::test_support::mem().await;
        let missing = uuid::Uuid::now_v7();
        assert!(record_reconcile(&surreal, missing, "PAID", 100)
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
        record_reconcile(&surreal, paid, "PAID", 200).await.unwrap();
        record_reconcile(&surreal, void, "VOIDED", 0).await.unwrap();

        let rows = needing_reconcile(&surreal).await.unwrap();
        assert_eq!(rows.len(), 1, "only the AUTHORISED invoice is re-checked");
        assert_eq!(rows[0].project_id, open);
    }

    #[tokio::test]
    async fn for_projects_filters_to_the_requested_matters() {
        let surreal = crate::surreal::test_support::mem().await;
        let a = seed_project(&surreal, "a").await;
        let b = seed_project(&surreal, "b").await;
        let c = seed_project(&surreal, "c").await;
        upsert(&surreal, &input(a, "xero-a", 100)).await.unwrap();
        upsert(&surreal, &input(b, "xero-b", 200)).await.unwrap();
        upsert(&surreal, &input(c, "xero-c", 300)).await.unwrap();

        let rows = for_projects(&surreal, &[a, c]).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.project_id == a || row.project_id == c));
        assert!(for_projects(&surreal, &[]).await.unwrap().is_empty());
    }
}
