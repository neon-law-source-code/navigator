//! One bank transfer out of a state's pooled IOLTA account, and how it splits
//! across the invoices it settles.
//!
//! # Why a split is modelled rather than inferred
//!
//! The bank sees one transfer. A $10,000 withdrawal from the Nevada pool may
//! settle four matters' invoices at once, and each of those clients is
//! entitled to see how *their* money left trust — and to see nothing about
//! the other three. So the transfer is mirrored as one
//! [`IoltaWithdrawal`] plus one [`IoltaAllocation`] per invoice it settles,
//! and a client's matter page renders only the lines carrying their
//! `project_id`. The pooled total is a firm-side number; it never reaches a
//! client surface.
//!
//! # All or nothing
//!
//! [`apply`] refuses a withdrawal that does not hold together, and refuses it
//! **whole**: nothing is written, no draw is posted, and the caller reports
//! it for a human. A withdrawal is refused when
//!
//! * its lines do not sum to exactly the transfer
//!   ([`IoltaWithdrawalError::LinesDoNotSum`]) — a partially explained
//!   transfer would leave the pool and the ledgers disagreeing;
//! * a line names an invoice no mirror row carries
//!   ([`IoltaWithdrawalError::UnknownInvoice`]);
//! * a line's matter does not sit on the state pool the money left
//!   ([`IoltaWithdrawalError::WrongPool`]);
//! * a matter's lines exceed what that matter actually holds
//!   ([`IoltaWithdrawalError::Overdraw`]). Drawing more than a client has in
//!   trust is the error trust accounting exists to prevent, so it is never
//!   clipped to what fits — the numbers are wrong and a human reconciles
//!   them.
//!
//! # What lands when it does hold together
//!
//! The withdrawal row, its allocation lines, and one
//! [`crate::trust::MovementKind::EarnedDraw`] per matter for the total of
//! that matter's lines, carrying the Xero transaction id as its
//! `external_ref`. The draw is what moves the matter's `held_cents` down;
//! the allocations are what let the client see which invoices it went to.
//! Re-reading the same transfer on a later night posts nothing new.
//!
//! Navigator never creates the transfer in Xero. Xero is the books.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::{RecordId, SurrealValue};
use thiserror::Error;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

pub(crate) const WITHDRAWAL_TABLE: &str = "iolta_withdrawal";
pub(crate) const ALLOCATION_TABLE: &str = "iolta_allocation";
const WITHDRAWAL_SELECT: &str = "xero_transaction_id, jurisdiction_id, total_cents, currency, \
                                 occurred_at, inserted_at, updated_at";
const ALLOCATION_SELECT: &str = "withdrawal_id, xero_invoice_id, project_id, invoice_reference, \
                                 amount_cents, occurred_at, inserted_at, updated_at";

/// One mirrored transfer out of a state's pooled account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IoltaWithdrawal {
    /// Xero `BankTransactionID`, and this row's own record id.
    pub xero_transaction_id: String,
    /// The state pool the money left.
    pub jurisdiction_id: Uuid,
    /// The whole transfer, minor units. Firm-side: never rendered to a
    /// client, who is shown only their own lines.
    pub total_cents: i64,
    pub currency: String,
    pub occurred_at: DateTime<Utc>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One line of the split: this much of the transfer settled this invoice for
/// this matter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IoltaAllocation {
    /// The withdrawal this line belongs to.
    pub withdrawal_id: String,
    /// The Xero invoice it settles. Internal — never rendered to a client.
    pub xero_invoice_id: String,
    /// The matter billed by that invoice, copied from the invoice mirror so
    /// a line cannot name a matter its invoice does not bill.
    pub project_id: Uuid,
    /// The invoice's human reference, which is what a client is shown.
    pub invoice_reference: String,
    pub amount_cents: i64,
    pub occurred_at: DateTime<Utc>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(serde::Deserialize, SurrealValue)]
struct WithdrawalRow {
    xero_transaction_id: String,
    jurisdiction_id: RecordId,
    total_cents: i64,
    currency: String,
    occurred_at: surrealdb::types::Datetime,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl WithdrawalRow {
    fn into_withdrawal(self) -> Option<IoltaWithdrawal> {
        Some(IoltaWithdrawal {
            xero_transaction_id: self.xero_transaction_id,
            jurisdiction_id: record_uuid(&self.jurisdiction_id)?,
            total_cents: self.total_cents,
            currency: self.currency,
            occurred_at: self.occurred_at.into(),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

#[derive(serde::Deserialize, SurrealValue)]
struct AllocationRow {
    withdrawal_id: String,
    xero_invoice_id: String,
    project_id: RecordId,
    invoice_reference: String,
    amount_cents: i64,
    occurred_at: surrealdb::types::Datetime,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AllocationRow {
    fn into_allocation(self) -> Option<IoltaAllocation> {
        Some(IoltaAllocation {
            withdrawal_id: self.withdrawal_id,
            xero_invoice_id: self.xero_invoice_id,
            project_id: record_uuid(&self.project_id)?,
            invoice_reference: self.invoice_reference,
            amount_cents: self.amount_cents,
            occurred_at: self.occurred_at.into(),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// Why a withdrawal was refused. Every variant names the numbers a human
/// needs to reconcile it in Xero — none of them is recoverable by guessing.
#[derive(Debug, Error)]
pub enum IoltaWithdrawalError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Boxed, like its sibling below: these two carry the largest payloads
    /// in this enum, and every function here returns it — an unboxed variant
    /// would widen every `Ok` on the happy path too.
    #[error(transparent)]
    Invoices(Box<crate::xero_invoices::XeroInvoiceError>),
    #[error(transparent)]
    Accounts(Box<crate::iolta_accounts::IoltaAccountError>),
    /// The transfer left a pool no mirrored account matches.
    #[error("no mirrored IOLTA account for Xero account {0}")]
    NoSuchPool(String),
    /// A withdrawal with no lines explains nothing.
    #[error("withdrawal {transaction_id} allocates nothing")]
    NoAllocations { transaction_id: String },
    /// The lines do not add up to the transfer.
    #[error(
        "withdrawal {transaction_id} moved {total_cents} cents but its lines allocate \
         {allocated_cents}"
    )]
    LinesDoNotSum {
        transaction_id: String,
        total_cents: i64,
        allocated_cents: i64,
    },
    /// A line names an invoice the mirror does not carry, so which matter it
    /// settles is unknown.
    #[error("withdrawal {transaction_id} allocates to unmirrored invoice {invoice_reference}")]
    UnknownInvoice {
        transaction_id: String,
        invoice_reference: String,
    },
    /// The money left one state's pool but a line settles a matter governed
    /// by another state — or by no state at all.
    #[error(
        "withdrawal {transaction_id} left the pool for jurisdiction {jurisdiction_id}, but \
         matter {project_id} does not sit on it"
    )]
    WrongPool {
        transaction_id: String,
        jurisdiction_id: Uuid,
        project_id: Uuid,
    },
    /// A matter's lines exceed what it holds in trust. Refused whole: the
    /// numbers are wrong, and clipping the draw to what fits would hide it.
    #[error(
        "withdrawal {transaction_id} draws {requested_cents} cents for matter {project_id}, \
         which holds {held_cents}"
    )]
    Overdraw {
        transaction_id: String,
        project_id: Uuid,
        held_cents: i64,
        requested_cents: i64,
    },
    /// Reading or writing the matter's trust ledger failed.
    #[error("trust ledger: {0}")]
    Trust(String),
    #[error("writing an IOLTA withdrawal returned no usable row")]
    WriteReturnedNothing,
}

impl From<crate::xero_invoices::XeroInvoiceError> for IoltaWithdrawalError {
    fn from(error: crate::xero_invoices::XeroInvoiceError) -> Self {
        Self::Invoices(Box::new(error))
    }
}

impl From<crate::iolta_accounts::IoltaAccountError> for IoltaWithdrawalError {
    fn from(error: crate::iolta_accounts::IoltaAccountError) -> Self {
        Self::Accounts(Box::new(error))
    }
}

async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, IoltaWithdrawalError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt)
        .await
        .map_err(IoltaWithdrawalError::Db)
}

/// One line of a withdrawal as the mirror reads it from Xero: how much of
/// the transfer settled which invoice.
#[derive(Clone, Debug)]
pub struct AllocationInput {
    /// The invoice reference carried on the Xero line
    /// (`store::xero_invoices::XeroInvoice::reference`, or the Xero invoice
    /// id itself).
    pub invoice_reference: String,
    pub amount_cents: i64,
}

/// One transfer out of a pooled account, with its split.
#[derive(Clone, Debug)]
pub struct WithdrawalInput {
    /// Xero `BankTransactionID`.
    pub xero_transaction_id: String,
    /// The Xero bank account the money left, matched to a mirrored pool.
    pub xero_account_id: String,
    pub total_cents: i64,
    pub currency: String,
    pub occurred_at: DateTime<Utc>,
    pub lines: Vec<AllocationInput>,
}

/// What [`apply`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The withdrawal, its lines, and one earned draw per matter landed.
    /// Carries the matters drawn against, for the caller's report.
    Posted { projects: Vec<Uuid> },
    /// This Xero transfer was already mirrored on an earlier night. Nothing
    /// was written and no draw was posted again.
    AlreadyApplied,
}

/// Mirror one pooled withdrawal: validate the whole split, write the
/// withdrawal and its lines, and post one earned draw per matter.
///
/// Idempotent on the Xero transaction id. See the module docs for the four
/// ways a withdrawal is refused — all of them refuse it whole.
///
/// # Errors
///
/// Any [`IoltaWithdrawalError`]. A refusal writes nothing.
pub async fn apply(
    db: &SurrealDb,
    input: &WithdrawalInput,
) -> Result<Applied, IoltaWithdrawalError> {
    if find(db, &input.xero_transaction_id).await?.is_some() {
        return Ok(Applied::AlreadyApplied);
    }
    let pool = crate::iolta_accounts::all(db)
        .await?
        .into_iter()
        .find(|account| account.xero_account_id == input.xero_account_id)
        .ok_or_else(|| IoltaWithdrawalError::NoSuchPool(input.xero_account_id.clone()))?;

    if input.lines.is_empty() {
        return Err(IoltaWithdrawalError::NoAllocations {
            transaction_id: input.xero_transaction_id.clone(),
        });
    }
    let allocated_cents: i64 = input.lines.iter().map(|line| line.amount_cents).sum();
    if allocated_cents != input.total_cents {
        return Err(IoltaWithdrawalError::LinesDoNotSum {
            transaction_id: input.xero_transaction_id.clone(),
            total_cents: input.total_cents,
            allocated_cents,
        });
    }

    // Resolve every line to the invoice it settles, and through the invoice
    // to the matter it bills. The matter is never taken from the line
    // itself, so a line cannot claim an invoice belongs to another matter.
    let mut resolved = Vec::with_capacity(input.lines.len());
    for line in &input.lines {
        let invoice = crate::xero_invoices::find_for_allocation(db, &line.invoice_reference)
            .await?
            .ok_or_else(|| IoltaWithdrawalError::UnknownInvoice {
                transaction_id: input.xero_transaction_id.clone(),
                invoice_reference: line.invoice_reference.clone(),
            })?;
        let matter_pool = crate::iolta_accounts::for_project(db, invoice.project_id).await?;
        if matter_pool.map(|account| account.jurisdiction_id) != Some(pool.jurisdiction_id) {
            return Err(IoltaWithdrawalError::WrongPool {
                transaction_id: input.xero_transaction_id.clone(),
                jurisdiction_id: pool.jurisdiction_id,
                project_id: invoice.project_id,
            });
        }
        resolved.push((invoice, line.amount_cents));
    }

    // Every matter's whole share of this transfer, checked against what that
    // matter actually holds — before anything is written.
    let mut per_project: Vec<(Uuid, i64)> = Vec::new();
    for (invoice, amount_cents) in &resolved {
        match per_project
            .iter_mut()
            .find(|(project_id, _)| *project_id == invoice.project_id)
        {
            Some((_, total)) => *total += *amount_cents,
            None => per_project.push((invoice.project_id, *amount_cents)),
        }
    }
    for (project_id, requested_cents) in &per_project {
        let held_cents = crate::trust::position_for_project(db, *project_id)
            .await
            .map_err(IoltaWithdrawalError::Trust)?
            .held_cents();
        if *requested_cents > held_cents {
            return Err(IoltaWithdrawalError::Overdraw {
                transaction_id: input.xero_transaction_id.clone(),
                project_id: *project_id,
                held_cents,
                requested_cents: *requested_cents,
            });
        }
    }

    write_withdrawal(db, input, pool.jurisdiction_id).await?;
    for (invoice, amount_cents) in &resolved {
        write_allocation(db, input, invoice, *amount_cents).await?;
    }
    let occurred_at = input.occurred_at.to_rfc3339();
    for (project_id, cents) in &per_project {
        let draw = crate::trust::Movement::earned_draw(*project_id, *cents, occurred_at.clone())
            .with_external_ref(input.xero_transaction_id.clone());
        crate::trust::record_project_movement(db, *project_id, &draw)
            .await
            .map_err(IoltaWithdrawalError::Trust)?;
    }
    Ok(Applied::Posted {
        projects: per_project.into_iter().map(|(id, _)| id).collect(),
    })
}

async fn write_withdrawal(
    db: &SurrealDb,
    input: &WithdrawalInput,
    jurisdiction_id: Uuid,
) -> Result<(), IoltaWithdrawalError> {
    writing(|| {
        db.query(
            "UPSERT $id SET xero_transaction_id = $xero_transaction_id, \
             jurisdiction_id = $jurisdiction_id, total_cents = $total_cents, \
             currency = $currency, occurred_at = $occurred_at, \
             inserted_at = IF inserted_at THEN inserted_at ELSE time::now() END, \
             updated_at = time::now()",
        )
        .bind((
            "id",
            RecordId::new(WITHDRAWAL_TABLE, input.xero_transaction_id.clone()),
        ))
        .bind(("xero_transaction_id", input.xero_transaction_id.clone()))
        .bind((
            "jurisdiction_id",
            record_id(crate::jurisdictions::TABLE, jurisdiction_id),
        ))
        .bind(("total_cents", input.total_cents))
        .bind(("currency", input.currency.clone()))
        .bind((
            "occurred_at",
            surrealdb::types::Datetime::from(input.occurred_at),
        ))
    })
    .await?;
    Ok(())
}

async fn write_allocation(
    db: &SurrealDb,
    input: &WithdrawalInput,
    invoice: &crate::xero_invoices::XeroInvoice,
    amount_cents: i64,
) -> Result<(), IoltaWithdrawalError> {
    // The record id is the (withdrawal, invoice) pair, so a re-read of one
    // transfer refreshes its lines instead of doubling a matter's drawn
    // total — the same primary-key discipline the invoice mirror uses.
    let line_id = format!("{}--{}", input.xero_transaction_id, invoice.xero_invoice_id);
    writing(|| {
        db.query(
            "UPSERT $id SET withdrawal_id = $withdrawal_id, \
             xero_invoice_id = $xero_invoice_id, project_id = $project_id, \
             invoice_reference = $invoice_reference, amount_cents = $amount_cents, \
             occurred_at = $occurred_at, \
             inserted_at = IF inserted_at THEN inserted_at ELSE time::now() END, \
             updated_at = time::now()",
        )
        .bind(("id", RecordId::new(ALLOCATION_TABLE, line_id.clone())))
        .bind(("withdrawal_id", input.xero_transaction_id.clone()))
        .bind(("xero_invoice_id", invoice.xero_invoice_id.clone()))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, invoice.project_id),
        ))
        .bind(("invoice_reference", invoice.reference.clone()))
        .bind(("amount_cents", amount_cents))
        .bind((
            "occurred_at",
            surrealdb::types::Datetime::from(input.occurred_at),
        ))
    })
    .await?;
    Ok(())
}

/// The mirrored withdrawal for one Xero transaction id, if any.
///
/// # Errors
///
/// [`IoltaWithdrawalError::Db`] when the lookup fails.
pub async fn find(
    db: &SurrealDb,
    xero_transaction_id: &str,
) -> Result<Option<IoltaWithdrawal>, IoltaWithdrawalError> {
    let mut response = db
        .query(format!("SELECT {WITHDRAWAL_SELECT} FROM ONLY $id"))
        .bind((
            "id",
            RecordId::new(WITHDRAWAL_TABLE, xero_transaction_id.to_string()),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<WithdrawalRow> = response.take(0)?;
    Ok(row.and_then(WithdrawalRow::into_withdrawal))
}

/// Every allocation line belonging to one matter, newest first.
///
/// **This is the client read.** It returns only the lines carrying
/// `project_id`, so a client sees how their own funds left trust and nothing
/// of the pooled transfer or of any other matter's share.
///
/// # Errors
///
/// [`IoltaWithdrawalError::Db`] when the lookup fails.
pub async fn for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<IoltaAllocation>, IoltaWithdrawalError> {
    let mut response = db
        .query(format!(
            "SELECT {ALLOCATION_SELECT} FROM {ALLOCATION_TABLE} \
             WHERE project_id = $project_id ORDER BY occurred_at DESC"
        ))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AllocationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AllocationRow::into_allocation)
        .collect())
}

/// Every line of one withdrawal — the firm-side read of how a pooled
/// transfer split. Never rendered on a client surface.
///
/// # Errors
///
/// [`IoltaWithdrawalError::Db`] when the lookup fails.
pub async fn lines_for(
    db: &SurrealDb,
    withdrawal_id: &str,
) -> Result<Vec<IoltaAllocation>, IoltaWithdrawalError> {
    let mut response = db
        .query(format!(
            "SELECT {ALLOCATION_SELECT} FROM {ALLOCATION_TABLE} \
             WHERE withdrawal_id = $withdrawal_id ORDER BY amount_cents DESC"
        ))
        .bind(("withdrawal_id", withdrawal_id.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AllocationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AllocationRow::into_allocation)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        apply, for_project, lines_for, AllocationInput, Applied, IoltaWithdrawalError,
        WithdrawalInput,
    };
    use crate::jurisdictions::NewJurisdiction;
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use chrono::{DateTime, TimeZone, Utc};
    use uuid::Uuid;

    fn when() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap()
    }

    async fn nevada_pool(db: &SurrealDb) -> Uuid {
        let jurisdiction_id =
            crate::jurisdictions::create(db, &NewJurisdiction::new("Nevada", "NV", "state"))
                .await
                .unwrap()
                .id;
        crate::iolta_accounts::upsert(
            db,
            &crate::iolta_accounts::UpsertIoltaAccount {
                jurisdiction_id,
                xero_account_id: "xero-nv".into(),
                xero_account_code: Some("090".into()),
                name: "IOLTA NV — Trust".into(),
                currency: "USD".into(),
                balance_cents: 1_000_000,
                mirrored_at: when(),
            },
        )
        .await
        .unwrap();
        jurisdiction_id
    }

    /// A matter on the given pool, holding `deposit_cents` in trust.
    async fn funded_matter(db: &SurrealDb, jurisdiction_id: Uuid, deposit_cents: i64) -> Uuid {
        let notation_id = crate::test_support::seed_notation(db).await;
        let project_id = crate::notations::find_by_id(db, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;
        crate::projects::set_jurisdiction(db, project_id, Some(jurisdiction_id))
            .await
            .unwrap();
        if deposit_cents > 0 {
            let deposit = crate::trust::Movement::deposit(
                project_id,
                "USD",
                "0.00",
                deposit_cents,
                "2026-09-01T00:00:00Z",
            )
            .with_external_ref(format!("bt-deposit-{project_id}"));
            crate::trust::record_project_movement(db, project_id, &deposit)
                .await
                .unwrap();
        }
        project_id
    }

    async fn mirror_invoice(db: &SurrealDb, project_id: Uuid, xero_id: &str, reference: &str) {
        crate::xero_invoices::upsert(
            db,
            &crate::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: xero_id.into(),
                reference: reference.into(),
                status: "AUTHORISED".into(),
                amount_cents: 100_000,
                currency: "USD".into(),
                issued_at: when(),
                due_at: None,
            },
        )
        .await
        .unwrap();
    }

    fn withdrawal(total_cents: i64, lines: Vec<(&str, i64)>) -> WithdrawalInput {
        WithdrawalInput {
            xero_transaction_id: "bt-withdrawal".into(),
            xero_account_id: "xero-nv".into(),
            total_cents,
            currency: "USD".into(),
            occurred_at: when(),
            lines: lines
                .into_iter()
                .map(|(invoice_reference, amount_cents)| AllocationInput {
                    invoice_reference: invoice_reference.into(),
                    amount_cents,
                })
                .collect(),
        }
    }

    /// The issue's worked example: one transfer of 1000 cents, 600 to an
    /// invoice on matter one and 400 to an invoice on matter two. Two draws,
    /// two isolated client reads.
    #[tokio::test]
    async fn one_withdrawal_splits_across_two_matters() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        let first = funded_matter(&db, pool, 1_000).await;
        let second = funded_matter(&db, pool, 1_000).await;
        mirror_invoice(&db, first, "inv-a", "INV-A").await;
        mirror_invoice(&db, second, "inv-b", "INV-B").await;

        let applied = apply(
            &db,
            &withdrawal(1_000, vec![("INV-A", 600), ("INV-B", 400)]),
        )
        .await
        .unwrap();
        assert!(matches!(applied, Applied::Posted { .. }));

        assert_eq!(
            crate::trust::position_for_project(&db, first)
                .await
                .unwrap()
                .earned_cents,
            600
        );
        assert_eq!(
            crate::trust::position_for_project(&db, second)
                .await
                .unwrap()
                .earned_cents,
            400
        );

        // The client read is scoped: one line each, naming their own invoice.
        let first_lines = for_project(&db, first).await.unwrap();
        assert_eq!(first_lines.len(), 1);
        assert_eq!(first_lines[0].amount_cents, 600);
        assert_eq!(first_lines[0].invoice_reference, "INV-A");
        let second_lines = for_project(&db, second).await.unwrap();
        assert_eq!(second_lines.len(), 1);
        assert_eq!(second_lines[0].amount_cents, 400);

        // The firm read sees the whole split.
        assert_eq!(lines_for(&db, "bt-withdrawal").await.unwrap().len(), 2);
    }

    /// Two invoices on one matter draw once, for their total.
    #[tokio::test]
    async fn two_invoices_on_one_matter_draw_their_total_once() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        let matter = funded_matter(&db, pool, 1_000).await;
        mirror_invoice(&db, matter, "inv-a", "INV-A").await;
        mirror_invoice(&db, matter, "inv-b", "INV-B").await;

        apply(&db, &withdrawal(900, vec![("INV-A", 500), ("INV-B", 400)]))
            .await
            .unwrap();

        let position = crate::trust::position_for_project(&db, matter)
            .await
            .unwrap();
        assert_eq!(position.earned_cents, 900);
        assert_eq!(position.held_cents(), 100);
        assert_eq!(for_project(&db, matter).await.unwrap().len(), 2);
    }

    /// A matter cannot be drawn beyond what it holds. The refusal is whole:
    /// no row, no line, no draw — not even for the matter that would have
    /// been fine.
    #[tokio::test]
    async fn an_overdrawn_matter_refuses_the_whole_withdrawal() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        let solvent = funded_matter(&db, pool, 1_000).await;
        let thin = funded_matter(&db, pool, 100).await;
        mirror_invoice(&db, solvent, "inv-a", "INV-A").await;
        mirror_invoice(&db, thin, "inv-b", "INV-B").await;

        let refused = apply(
            &db,
            &withdrawal(1_000, vec![("INV-A", 600), ("INV-B", 400)]),
        )
        .await;
        assert!(
            matches!(
                refused,
                Err(IoltaWithdrawalError::Overdraw {
                    held_cents: 100,
                    requested_cents: 400,
                    ..
                })
            ),
            "got {refused:?}"
        );
        assert_eq!(
            crate::trust::position_for_project(&db, solvent)
                .await
                .unwrap()
                .earned_cents,
            0,
            "the solvent matter is untouched by its neighbour's refusal"
        );
        assert!(for_project(&db, thin).await.unwrap().is_empty());
        assert!(lines_for(&db, "bt-withdrawal").await.unwrap().is_empty());
        assert!(super::find(&db, "bt-withdrawal").await.unwrap().is_none());
    }

    /// Lines that do not add up to the transfer leave the pool and the
    /// ledgers disagreeing, so the withdrawal is refused.
    #[tokio::test]
    async fn lines_that_do_not_sum_are_refused() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        let matter = funded_matter(&db, pool, 10_000).await;
        mirror_invoice(&db, matter, "inv-a", "INV-A").await;

        let refused = apply(&db, &withdrawal(1_000, vec![("INV-A", 600)])).await;
        assert!(
            matches!(
                refused,
                Err(IoltaWithdrawalError::LinesDoNotSum {
                    total_cents: 1_000,
                    allocated_cents: 600,
                    ..
                })
            ),
            "got {refused:?}"
        );
        assert_eq!(
            crate::trust::position_for_project(&db, matter)
                .await
                .unwrap()
                .earned_cents,
            0
        );
    }

    /// A line naming an invoice the mirror does not carry says nothing about
    /// which matter it settles.
    #[tokio::test]
    async fn an_unmirrored_invoice_is_refused() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        funded_matter(&db, pool, 10_000).await;

        let refused = apply(&db, &withdrawal(1_000, vec![("INV-GHOST", 1_000)])).await;
        assert!(
            matches!(refused, Err(IoltaWithdrawalError::UnknownInvoice { .. })),
            "got {refused:?}"
        );
    }

    /// Money that left the Nevada pool cannot settle a matter governed by
    /// California, or by nothing at all.
    #[tokio::test]
    async fn a_matter_on_another_pool_is_refused() {
        let db = mem().await;
        let nevada = nevada_pool(&db).await;
        let california =
            crate::jurisdictions::create(&db, &NewJurisdiction::new("California", "CA", "state"))
                .await
                .unwrap()
                .id;
        let ca_matter = funded_matter(&db, california, 10_000).await;
        mirror_invoice(&db, ca_matter, "inv-ca", "INV-CA").await;
        let _ = nevada;

        let refused = apply(&db, &withdrawal(1_000, vec![("INV-CA", 1_000)])).await;
        assert!(
            matches!(refused, Err(IoltaWithdrawalError::WrongPool { .. })),
            "got {refused:?}"
        );
    }

    /// A second night's read of the same transfer posts nothing again.
    #[tokio::test]
    async fn re_reading_one_transfer_draws_once() {
        let db = mem().await;
        let pool = nevada_pool(&db).await;
        let matter = funded_matter(&db, pool, 10_000).await;
        mirror_invoice(&db, matter, "inv-a", "INV-A").await;
        let input = withdrawal(1_000, vec![("INV-A", 1_000)]);

        apply(&db, &input).await.unwrap();
        assert_eq!(apply(&db, &input).await.unwrap(), Applied::AlreadyApplied);

        let position = crate::trust::position_for_project(&db, matter)
            .await
            .unwrap();
        assert_eq!(position.earned_cents, 1_000, "drawn once, not twice");
        assert_eq!(for_project(&db, matter).await.unwrap().len(), 1);
    }

    /// A transfer out of an account no mirrored pool matches has no state to
    /// belong to.
    #[tokio::test]
    async fn a_transfer_from_an_unmirrored_account_is_refused() {
        let db = mem().await;
        nevada_pool(&db).await;
        let mut input = withdrawal(1_000, vec![("INV-A", 1_000)]);
        input.xero_account_id = "xero-operating".into();

        let refused = apply(&db, &input).await;
        assert!(
            matches!(refused, Err(IoltaWithdrawalError::NoSuchPool(_))),
            "got {refused:?}"
        );
    }
}
