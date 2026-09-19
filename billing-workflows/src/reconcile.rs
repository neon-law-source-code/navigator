//! The `ReconcileInvoices` Restate workflow — the nightly job that
//! discovers Xero accounts-receivable invoices onto the local
//! `xero_invoice` mirror, then folds paid-status onto rows still open.
//!
//! The portal reads the mirror, never Xero live. Lawyers raise invoices in
//! Xero tagged `Matter <project uuid or code>`. This workflow lists those
//! invoices, upserts each that resolves to a live Project, skips DRAFT and
//! DELETED, and counts unscoped references (no matching Project) without
//! writing a row. It then re-checks every mirror row not yet in a terminal
//! state (`PAID` / `VOIDED`) via `get_invoice`.
//!
//! The `billing-reconcile-trigger` `CronJob` starts one invocation per day
//! (keyed on the UTC date, so a same-day re-fire is a no-op); Restate owns
//! the retry schedule. Identical split to the canary/archives triggers.
//!
//! [`reconcile_once`] is provider-agnostic so it unit-tests against the
//! [`billing::StubBillingProvider`] + a test database without a worker.

use billing::{BillingProvider, XeroBillingProvider};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use store::surreal::SurrealDb;

/// Request body for `ReconcileInvoices::run`. Empty — the trigger only
/// starts the workflow — but kept as a struct so fields can be threaded
/// later without changing the handler signature.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ReconcileRequest {}

/// What a reconcile run touched, surfaced as the invocation output.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReconcileReport {
    /// Listed invoices whose `Reference` resolved to a Project and were
    /// upserted (including a metadata update of an existing row).
    pub ingested: usize,
    /// Listed invoices that were not DRAFT/DELETED and whose `Reference`
    /// did not resolve to a live Project. No row is written.
    pub unscoped: usize,
    /// Mirror rows that were still open and got re-checked against Xero.
    pub checked: usize,
    /// How many of those actually changed (status or amount paid).
    pub updated: usize,
    /// Xero bank accounts whose name declared a state (`IOLTA NV …`) and
    /// whose pooled balance was mirrored onto `iolta_account`.
    pub iolta_accounts: usize,
    /// Listed bank accounts that declared no state, or a state this
    /// deployment does not carry. No row is written — the firm's operating
    /// and payroll accounts land here, and so does a typo.
    pub iolta_unscoped: usize,
    /// Client deposits into trust posted onto a matter's ledger this run. A
    /// deposit already mirrored on an earlier night is not counted again.
    pub trust_deposits: usize,
    /// Refunds of unearned funds posted onto a matter's ledger this run.
    pub trust_refunds: usize,
    /// Trust transactions this run did not post: no `Matter` reference, a
    /// matter that does not sit on the account the money moved through, or a
    /// matter with no notation to anchor a posting to.
    pub trust_unscoped: usize,
    /// Pooled withdrawals mirrored this run: one bank transfer out of a
    /// state's IOLTA account, split across the invoices it settles, with one
    /// earned draw posted per matter.
    pub iolta_withdrawals: usize,
    /// Pooled withdrawals refused whole and left for a human — lines that do
    /// not sum to the transfer, a line naming an unmirrored invoice, a
    /// matter on another state's pool, or a matter drawn beyond what it
    /// holds. Nothing of a refused withdrawal is written.
    pub iolta_withdrawals_refused: usize,
}

/// Service registered with the Restate endpoint. Holds a SurrealDB clone (the
/// same connection the worker opened at boot); the Xero provider is built from
/// env inside the step so no token sits idle between nightly runs.
#[derive(Clone)]
pub struct ReconcileInvoicesService {
    surreal: SurrealDb,
}

impl ReconcileInvoicesService {
    #[must_use]
    pub fn new(surreal: SurrealDb) -> Self {
        Self { surreal }
    }
}

#[restate_sdk::workflow(name = "ReconcileInvoices")]
impl ReconcileInvoicesService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        _req: Json<ReconcileRequest>,
    ) -> Result<Json<ReconcileReport>, HandlerError> {
        let surreal = self.surreal.clone();
        let report = ctx
            .run(move || async move {
                let provider = XeroBillingProvider::from_env().ok_or_else(|| {
                    TerminalError::new("Xero is not configured (XERO_* env unset)")
                })?;
                Ok(Json(reconcile_once(&provider, &surreal).await?))
            })
            .name("reconcile")
            .await?
            .into_inner();
        Ok(Json(report))
    }
}

/// List provider invoices onto the mirror, then re-check every open mirror
/// row. Provider-agnostic; unit-tested against the stub + an in-memory
/// SurrealDB.
///
/// # Errors
///
/// Propagates any database or billing-provider error.
pub async fn reconcile_once(
    provider: &dyn BillingProvider,
    surreal: &SurrealDb,
) -> anyhow::Result<ReconcileReport> {
    let (ingested, unscoped) = ingest_listed(provider, surreal).await?;
    let rows = store::xero_invoices::needing_reconcile(surreal).await?;
    let mut updated = 0;
    for row in &rows {
        let latest = provider.get_invoice(&row.xero_invoice_id).await?;
        if latest.status != row.status || latest.amount_paid_cents != row.amount_paid_cents {
            updated += 1;
        }
        store::xero_invoices::record_reconcile(
            surreal,
            &row.xero_invoice_id,
            &latest.status,
            latest.amount_paid_cents,
        )
        .await?;
    }
    let (iolta_accounts, iolta_unscoped) = mirror_iolta_accounts(provider, surreal).await?;
    let trust = mirror_trust_movements(provider, surreal).await?;
    Ok(ReconcileReport {
        ingested,
        unscoped,
        checked: rows.len(),
        updated,
        iolta_accounts,
        iolta_unscoped,
        trust_deposits: trust.deposits,
        trust_refunds: trust.refunds,
        trust_unscoped: trust.unscoped,
        iolta_withdrawals: trust.withdrawals,
        iolta_withdrawals_refused: trust.withdrawals_refused,
    })
}

/// What one pass over the trust transactions posted.
#[derive(Debug, Default)]
struct TrustMirrorTally {
    deposits: usize,
    refunds: usize,
    unscoped: usize,
    withdrawals: usize,
    withdrawals_refused: usize,
}

/// What one pooled withdrawal did.
enum WithdrawalOutcome {
    Applied,
    /// Mirrored on an earlier night; nothing written.
    AlreadyApplied,
    /// Refused whole, with the reason a human needs to reconcile it in Xero.
    Refused(String),
}

/// Mirror one pooled withdrawal and its allocations.
///
/// A refusal is a *reported* outcome rather than an error that abandons the
/// night: one bookkeeping mistake in one transfer must not stop the rest of
/// the run. Only a database or ledger failure propagates.
async fn apply_withdrawal(
    surreal: &SurrealDb,
    transaction: &billing::TrustBankTransaction,
) -> anyhow::Result<WithdrawalOutcome> {
    let input = store::iolta_withdrawals::WithdrawalInput {
        xero_transaction_id: transaction.transaction_id.clone(),
        xero_account_id: transaction.account_id.clone(),
        total_cents: transaction.amount_cents,
        currency: transaction.currency.clone(),
        occurred_at: transaction.occurred_at,
        lines: transaction
            .line_items
            .iter()
            .filter_map(|line| {
                line.invoice_reference.as_ref().map(|reference| {
                    store::iolta_withdrawals::AllocationInput {
                        invoice_reference: reference.clone(),
                        amount_cents: line.amount_cents,
                    }
                })
            })
            .collect(),
    };
    match store::iolta_withdrawals::apply(surreal, &input).await {
        Ok(store::iolta_withdrawals::Applied::Posted { .. }) => Ok(WithdrawalOutcome::Applied),
        Ok(store::iolta_withdrawals::Applied::AlreadyApplied) => {
            Ok(WithdrawalOutcome::AlreadyApplied)
        }
        Err(store::iolta_withdrawals::IoltaWithdrawalError::Db(error)) => Err(error.into()),
        Err(refusal) => Ok(WithdrawalOutcome::Refused(refusal.to_string())),
    }
}

/// Whether a spend settles invoices — the pooled withdrawal — rather than
/// refunding a client. The lines say which: an allocation line names the
/// invoice it settles, a refund's lines name none.
fn settles_invoices(transaction: &billing::TrustBankTransaction) -> bool {
    transaction
        .line_items
        .iter()
        .any(|line| line.invoice_reference.is_some())
}

/// Mirror client deposits into trust, and refunds out of it, onto each
/// matter's `store::trust` ledger.
///
/// Every posting is refused unless it holds together three ways: the
/// transaction's `Matter <code>` reference resolves to a live Project, that
/// Project's governing jurisdiction has a mirrored pooled account, and the
/// money actually moved through *that* account. A Nevada deposit landing
/// against a California-governed matter is a bookkeeping error, so it is
/// counted and left for a human rather than posted somewhere plausible.
///
/// Idempotent on the Xero transaction id: the same night's read, re-run,
/// posts nothing.
async fn mirror_trust_movements(
    provider: &dyn BillingProvider,
    surreal: &SurrealDb,
) -> anyhow::Result<TrustMirrorTally> {
    let listed = provider.list_trust_transactions().await?;
    let mut tally = TrustMirrorTally::default();
    for transaction in listed {
        if matches!(transaction.kind, billing::TrustTransactionKind::Spend)
            && settles_invoices(&transaction)
        {
            match apply_withdrawal(surreal, &transaction).await? {
                WithdrawalOutcome::Applied => tally.withdrawals += 1,
                WithdrawalOutcome::Refused(reason) => {
                    tracing::warn!(
                        transaction_id = %transaction.transaction_id,
                        reason = %reason,
                        "IOLTA withdrawal refused; nothing written"
                    );
                    tally.withdrawals_refused += 1;
                }
                WithdrawalOutcome::AlreadyApplied => {}
            }
            continue;
        }
        let Some(project_id) =
            store::xero_invoices::resolve_project_scope(surreal, &transaction.reference).await?
        else {
            tally.unscoped += 1;
            continue;
        };
        let Some(account) = store::iolta_accounts::for_project(surreal, project_id).await? else {
            tally.unscoped += 1;
            continue;
        };
        if account.xero_account_id != transaction.account_id {
            tally.unscoped += 1;
            continue;
        }

        let occurred_at = transaction.occurred_at.to_rfc3339();
        let movement = match transaction.kind {
            billing::TrustTransactionKind::Receive => store::trust::Movement::deposit(
                project_id,
                transaction.currency.clone(),
                format_cents(transaction.amount_cents),
                transaction.amount_cents,
                occurred_at,
            ),
            billing::TrustTransactionKind::Spend => {
                store::trust::Movement::refund(project_id, transaction.amount_cents, occurred_at)
            }
        }
        .with_external_ref(transaction.transaction_id.clone());

        match store::trust::record_project_movement(surreal, project_id, &movement)
            .await
            .map_err(anyhow::Error::msg)?
        {
            store::trust::Recorded::Posted => match transaction.kind {
                billing::TrustTransactionKind::Receive => tally.deposits += 1,
                billing::TrustTransactionKind::Spend => tally.refunds += 1,
            },
            // Already on the ledger from an earlier night, or the matter has
            // no notation to anchor a posting to. Neither is a new fact.
            store::trust::Recorded::AlreadyRecorded => {}
            store::trust::Recorded::NoAnchor => tally.unscoped += 1,
        }
    }
    Ok(tally)
}

/// Render minor units as the decimal string `store::trust::Movement` keeps
/// its native amount in — never a float in the money path.
fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Refresh the mirrored balance of each state's pooled IOLTA account.
///
/// Read-only in both directions: Xero lists its bank accounts, Navigator
/// keeps the one row per state that `store::iolta_accounts` allows, and
/// nothing is ever written back to Xero. An account whose name does not
/// declare a state (`IOLTA NV — Trust`) is counted as unscoped rather than
/// attached to one — the firm's operating account is in the same list.
///
/// A refused write for one account does not abandon the rest: a second Xero
/// account claiming a state that is already mirrored is reported as unscoped
/// and the run continues, because one bookkeeping mistake should not stop
/// every other state's balance from refreshing.
async fn mirror_iolta_accounts(
    provider: &dyn BillingProvider,
    surreal: &SurrealDb,
) -> anyhow::Result<(usize, usize)> {
    let listed = provider.list_bank_accounts().await?;
    let mirrored_at = chrono::Utc::now();
    let mut mirrored = 0;
    let mut unscoped = 0;
    for account in listed {
        let Some(jurisdiction_id) =
            store::iolta_accounts::resolve_jurisdiction_scope(surreal, &account.name).await?
        else {
            unscoped += 1;
            continue;
        };
        let upsert = store::iolta_accounts::UpsertIoltaAccount {
            jurisdiction_id,
            xero_account_id: account.account_id.clone(),
            xero_account_code: account.code.clone(),
            name: account.name.clone(),
            currency: account.currency.clone(),
            balance_cents: account.balance_cents,
            mirrored_at,
        };
        match store::iolta_accounts::upsert(surreal, &upsert).await {
            Ok(_) => mirrored += 1,
            Err(store::iolta_accounts::IoltaAccountError::JurisdictionTaken { .. }) => {
                unscoped += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok((mirrored, unscoped))
}

fn skip_listed_status(status: &str) -> bool {
    matches!(status.to_ascii_uppercase().as_str(), "DRAFT" | "DELETED")
}

async fn ingest_listed(
    provider: &dyn BillingProvider,
    surreal: &SurrealDb,
) -> anyhow::Result<(usize, usize)> {
    let listed = provider.list_receivable_invoices().await?;
    let mut ingested = 0;
    let mut unscoped = 0;
    for invoice in listed {
        if skip_listed_status(&invoice.status) {
            continue;
        }
        let Some(project_id) =
            store::xero_invoices::resolve_project_scope(surreal, &invoice.reference).await?
        else {
            unscoped += 1;
            continue;
        };
        store::xero_invoices::upsert(
            surreal,
            &store::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: invoice.invoice_id.clone(),
                reference: invoice.reference.clone(),
                status: invoice.status.clone(),
                amount_cents: invoice.amount_cents,
                currency: invoice.currency.clone(),
                issued_at: invoice.issued_at,
                due_at: invoice.due_at,
            },
        )
        .await?;
        store::xero_invoices::record_reconcile(
            surreal,
            &invoice.invoice_id,
            &invoice.status,
            invoice.amount_paid_cents,
        )
        .await?;
        ingested += 1;
    }
    Ok((ingested, unscoped))
}

#[cfg(test)]
mod tests {
    use super::reconcile_once;
    use billing::{
        InvoiceStatus, ReceivableInvoice, StubBillingProvider, TrustBankAccount,
        TrustBankTransaction,
    };
    use chrono::{TimeZone, Utc};

    async fn seed_mirror(
        surreal: &store::surreal::SurrealDb,
        project_id: uuid::Uuid,
        name: &str,
        xero_id: &str,
    ) {
        store::xero_invoices::upsert(
            surreal,
            &store::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: xero_id.into(),
                reference: format!("Matter {name}"),
                status: "AUTHORISED".into(),
                amount_cents: 333_300,
                currency: "USD".into(),
                issued_at: chrono::Utc::now(),
                due_at: None,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_project_with_code(surreal: &store::surreal::SurrealDb, code: &str) -> uuid::Uuid {
        let entity_id = store::test_support::seed_entity(surreal).await;
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: code.to_string(),
                name: code.to_string(),
                status: "open".to_string(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    fn listed(
        invoice_id: &str,
        reference: String,
        status: &str,
        amount_cents: i64,
        amount_paid_cents: i64,
    ) -> ReceivableInvoice {
        ReceivableInvoice {
            invoice_id: invoice_id.into(),
            reference,
            status: status.into(),
            amount_cents,
            amount_paid_cents,
            currency: "USD".into(),
            issued_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
            due_at: None,
        }
    }

    #[tokio::test]
    async fn reconcile_marks_paid_invoices_and_counts_changes() {
        let surreal = store::surreal::test_support::mem().await;
        let project_id = uuid::Uuid::now_v7();
        seed_mirror(&surreal, project_id, "sample-matter", "inv-1").await;

        let stub = StubBillingProvider::new();
        stub.set_invoice_status(
            "inv-1",
            InvoiceStatus {
                status: "PAID".into(),
                amount_paid_cents: 333_300,
            },
        );

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(report.updated, 1);

        let rows = store::xero_invoices::for_projects(&surreal, &[project_id])
            .await
            .unwrap();
        assert_eq!(rows[0].status, "PAID");
        assert_eq!(rows[0].amount_paid_cents, 333_300);
    }

    #[tokio::test]
    async fn reconcile_is_a_noop_when_nothing_changed() {
        let surreal = store::surreal::test_support::mem().await;
        seed_mirror(&surreal, uuid::Uuid::now_v7(), "still-open", "inv-2").await;
        // Stub default for an unknown id is AUTHORISED / 0 — same as seeded.
        let stub = StubBillingProvider::new();
        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.checked, 1);
        assert_eq!(
            report.updated, 0,
            "no status/paid change → no update counted"
        );
    }

    /// ENG-588: a matter can carry more than one invoice, and reconcile must
    /// re-check and fold a result onto each independently — settling one
    /// must not touch the other.
    #[tokio::test]
    async fn reconcile_updates_both_invoices_on_a_two_invoice_matter() {
        let surreal = store::surreal::test_support::mem().await;
        let project_id = uuid::Uuid::now_v7();
        seed_mirror(&surreal, project_id, "two-invoice-matter", "inv-a").await;
        seed_mirror(&surreal, project_id, "two-invoice-matter", "inv-b").await;

        let stub = StubBillingProvider::new();
        stub.set_invoice_status(
            "inv-a",
            InvoiceStatus {
                status: "PAID".into(),
                amount_paid_cents: 333_300,
            },
        );
        stub.set_invoice_status(
            "inv-b",
            InvoiceStatus {
                status: "VOIDED".into(),
                amount_paid_cents: 0,
            },
        );

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.checked, 2, "both invoices on the matter are checked");
        assert_eq!(report.updated, 2, "both invoices changed");

        let rows = store::xero_invoices::for_projects(&surreal, &[project_id])
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.xero_invoice_id == "inv-a").unwrap();
        let b = rows.iter().find(|r| r.xero_invoice_id == "inv-b").unwrap();
        assert_eq!(a.status, "PAID");
        assert_eq!(a.amount_paid_cents, 333_300);
        assert_eq!(b.status, "VOIDED");
        assert_eq!(b.amount_paid_cents, 0);
    }

    #[tokio::test]
    async fn ingest_mirrors_by_project_uuid_and_code() {
        let surreal = store::surreal::test_support::mem().await;
        let by_uuid = seed_project_with_code(&surreal, "ingest-by-uuid").await;
        let by_code = seed_project_with_code(&surreal, "ingest-by-code").await;

        let stub = StubBillingProvider::new();
        stub.set_listed_invoices(vec![
            listed(
                "inv-uuid",
                format!("Matter {by_uuid}"),
                "AUTHORISED",
                10_000,
                0,
            ),
            listed(
                "inv-code",
                "Matter ingest-by-code".into(),
                "PAID",
                20_000,
                20_000,
            ),
        ]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.ingested, 2);
        assert_eq!(report.unscoped, 0);

        let uuid_rows = store::xero_invoices::for_projects(&surreal, &[by_uuid])
            .await
            .unwrap();
        assert_eq!(uuid_rows.len(), 1);
        assert_eq!(uuid_rows[0].xero_invoice_id, "inv-uuid");

        let code_rows = store::xero_invoices::for_projects(&surreal, &[by_code])
            .await
            .unwrap();
        assert_eq!(code_rows.len(), 1);
        assert_eq!(code_rows[0].status, "PAID");
        assert_eq!(code_rows[0].amount_paid_cents, 20_000);
    }

    #[tokio::test]
    async fn ingest_skips_unscoped_and_draft_and_is_idempotent() {
        let surreal = store::surreal::test_support::mem().await;
        let project_id = seed_project_with_code(&surreal, "ingest-scoped").await;
        let other = seed_project_with_code(&surreal, "ingest-other").await;

        let stub = StubBillingProvider::new();
        stub.set_listed_invoices(vec![
            listed(
                "inv-ok",
                format!("Matter {project_id}"),
                "AUTHORISED",
                10_000,
                0,
            ),
            listed(
                "inv-second",
                format!("Matter {project_id}"),
                "AUTHORISED",
                15_000,
                0,
            ),
            listed(
                "inv-missing",
                "Matter no-such-matter".into(),
                "AUTHORISED",
                1,
                0,
            ),
            listed("inv-blank", "Invoice 99".into(), "AUTHORISED", 1, 0),
            listed(
                "inv-draft",
                format!("Matter {project_id}"),
                "DRAFT",
                9_000,
                0,
            ),
            listed(
                "inv-deleted",
                format!("Matter {project_id}"),
                "DELETED",
                9_000,
                0,
            ),
        ]);

        let first = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(first.ingested, 2);
        assert_eq!(first.unscoped, 2);

        let second = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(second.ingested, 2, "a repeat updates in place");
        assert_eq!(second.unscoped, 2);

        let rows = store::xero_invoices::for_projects(&surreal, &[project_id])
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let ids: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.xero_invoice_id.as_str()).collect();
        assert_eq!(ids, ["inv-ok", "inv-second"].into_iter().collect());
        assert!(store::xero_invoices::for_projects(&surreal, &[other])
            .await
            .unwrap()
            .is_empty());
    }

    async fn seed_state(surreal: &store::surreal::SurrealDb, name: &str, code: &str) -> uuid::Uuid {
        store::jurisdictions::create(
            surreal,
            &store::jurisdictions::NewJurisdiction::new(name, code, "state"),
        )
        .await
        .unwrap()
        .id
    }

    fn bank(account_id: &str, name: &str, balance_cents: i64) -> TrustBankAccount {
        TrustBankAccount {
            account_id: account_id.into(),
            code: Some("090".into()),
            name: name.into(),
            currency: "USD".into(),
            balance_cents,
        }
    }

    /// ENG-801: two states are two mirrored pooled accounts, and the firm's
    /// own accounts — listed by the same Xero read — are counted as unscoped
    /// rather than turned into a trust pool.
    #[tokio::test]
    async fn the_nightly_run_mirrors_one_pooled_account_per_state() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        let california = seed_state(&surreal, "California", "CA").await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![
            bank("xero-nv", "IOLTA NV — Trust", 1_250_000),
            bank("xero-ca", "IOLTA CA — Trust", 400_000),
            bank("xero-op", "Business Checking", 9_000_000),
        ]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.iolta_accounts, 2);
        assert_eq!(report.iolta_unscoped, 1, "the operating account is skipped");

        let nv = store::iolta_accounts::for_jurisdiction(&surreal, nevada)
            .await
            .unwrap()
            .expect("Nevada is mirrored");
        assert_eq!(nv.xero_account_id, "xero-nv");
        assert_eq!(nv.balance_cents, 1_250_000);
        assert!(nv.mirrored_at.is_some(), "the read stamps when it happened");
        assert_eq!(
            store::iolta_accounts::for_jurisdiction(&surreal, california)
                .await
                .unwrap()
                .map(|a| a.balance_cents),
            Some(400_000)
        );
    }

    /// A second Xero account claiming a state that already has one is
    /// reported, not applied — and it does not stop the other states in the
    /// same run from refreshing.
    #[tokio::test]
    async fn a_duplicate_state_account_is_counted_and_the_run_continues() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        seed_state(&surreal, "California", "CA").await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![
            bank("xero-nv", "IOLTA NV — Trust", 1_000_000),
            bank("xero-nv-2", "IOLTA NV — Second", 7),
            bank("xero-ca", "IOLTA CA — Trust", 500_000),
        ]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.iolta_accounts, 2, "NV once, CA once");
        assert_eq!(report.iolta_unscoped, 1, "the second NV account is refused");
        assert_eq!(
            store::iolta_accounts::for_jurisdiction(&surreal, nevada)
                .await
                .unwrap()
                .map(|a| a.xero_account_id),
            Some("xero-nv".to_string()),
            "the incumbent is not replaced"
        );
    }

    /// A matter seeded with a notation (so trust postings have an anchor)
    /// and a governing jurisdiction (so they have a pooled account).
    async fn seed_matter_in(
        surreal: &store::surreal::SurrealDb,
        jurisdiction_id: uuid::Uuid,
    ) -> uuid::Uuid {
        let notation_id = store::test_support::seed_notation(surreal).await;
        let project_id = store::notations::find_by_id(surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;
        store::projects::set_jurisdiction(surreal, project_id, Some(jurisdiction_id))
            .await
            .unwrap();
        project_id
    }

    fn receipt(
        transaction_id: &str,
        account_id: &str,
        reference: String,
        amount_cents: i64,
    ) -> TrustBankTransaction {
        TrustBankTransaction {
            transaction_id: transaction_id.into(),
            kind: billing::TrustTransactionKind::Receive,
            account_id: account_id.into(),
            reference,
            amount_cents,
            currency: "USD".into(),
            occurred_at: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            line_items: Vec::new(),
        }
    }

    /// ENG-802: a Xero deposit tagged to a matter raises that matter's
    /// deposited and held balance, and leaves every other matter alone.
    #[tokio::test]
    async fn a_mirrored_deposit_raises_one_matters_held_balance() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        let first = seed_matter_in(&surreal, nevada).await;
        let second = seed_matter_in(&surreal, nevada).await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 900_000)]);
        stub.set_listed_trust_transactions(vec![receipt(
            "bt-1",
            "xero-nv",
            format!("Matter {first}"),
            500_000,
        )]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.trust_deposits, 1);
        assert_eq!(report.trust_unscoped, 0);

        let held = store::trust::position_for_project(&surreal, first)
            .await
            .unwrap();
        assert_eq!(held.deposited_cents, 500_000);
        assert_eq!(held.held_cents(), 500_000);
        assert_eq!(
            store::trust::position_for_project(&surreal, second)
                .await
                .unwrap(),
            store::trust::Position::default(),
            "the other matter on the same pooled account is untouched"
        );
    }

    /// The same night, re-run, posts nothing: the Xero transaction id is the
    /// idempotency key.
    #[tokio::test]
    async fn re_running_the_trust_mirror_does_not_double_count() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        let project_id = seed_matter_in(&surreal, nevada).await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 900_000)]);
        stub.set_listed_trust_transactions(vec![receipt(
            "bt-1",
            "xero-nv",
            format!("Matter {project_id}"),
            250_000,
        )]);

        reconcile_once(&stub, &surreal).await.unwrap();
        let second = reconcile_once(&stub, &surreal).await.unwrap();

        assert_eq!(second.trust_deposits, 0, "already mirrored, nothing new");
        assert_eq!(
            store::trust::position_for_project(&surreal, project_id)
                .await
                .unwrap()
                .deposited_cents,
            250_000
        );
    }

    /// Three ways a deposit fails to hold together, each counted rather than
    /// posted somewhere plausible: no matter reference, a matter with no
    /// pooled account, and money that moved through another state's account.
    #[tokio::test]
    async fn a_deposit_that_does_not_hold_together_is_counted_not_posted() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        let california = seed_state(&surreal, "California", "CA").await;
        let nv_matter = seed_matter_in(&surreal, nevada).await;
        let ca_matter = seed_matter_in(&surreal, california).await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 900_000)]);
        stub.set_listed_trust_transactions(vec![
            receipt("bt-1", "xero-nv", "Deposit, thanks".into(), 100_000),
            receipt("bt-2", "xero-nv", format!("Matter {ca_matter}"), 100_000),
            receipt("bt-3", "xero-ca", format!("Matter {nv_matter}"), 100_000),
        ]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.trust_deposits, 0);
        assert_eq!(report.trust_unscoped, 3);
        assert_eq!(
            store::trust::position_for_project(&surreal, nv_matter)
                .await
                .unwrap(),
            store::trust::Position::default(),
            "a Nevada matter is not credited from the California account"
        );
        assert_eq!(
            store::trust::position_for_project(&surreal, ca_matter)
                .await
                .unwrap(),
            store::trust::Position::default(),
            "California has no mirrored pooled account, so nothing posts"
        );
    }

    /// Money out of the pooled account that names no invoice is a refund of
    /// unearned funds; one that settles invoices is the pooled withdrawal,
    /// and a withdrawal naming an invoice the mirror does not carry is
    /// refused whole rather than posted against a guessed matter.
    #[tokio::test]
    async fn a_refund_posts_and_an_unmirrored_withdrawal_is_refused() {
        let surreal = store::surreal::test_support::mem().await;
        let nevada = seed_state(&surreal, "Nevada", "NV").await;
        let project_id = seed_matter_in(&surreal, nevada).await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 900_000)]);
        stub.set_listed_trust_transactions(vec![
            receipt("bt-1", "xero-nv", format!("Matter {project_id}"), 500_000),
            TrustBankTransaction {
                transaction_id: "bt-2".into(),
                kind: billing::TrustTransactionKind::Spend,
                account_id: "xero-nv".into(),
                reference: format!("Matter {project_id}"),
                amount_cents: 120_000,
                currency: "USD".into(),
                occurred_at: Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap(),
                line_items: vec![billing::TrustTransactionLine {
                    description: "Unearned balance returned".into(),
                    amount_cents: 120_000,
                    invoice_reference: None,
                }],
            },
            TrustBankTransaction {
                transaction_id: "bt-3".into(),
                kind: billing::TrustTransactionKind::Spend,
                account_id: "xero-nv".into(),
                reference: "Fees earned, September".into(),
                amount_cents: 80_000,
                currency: "USD".into(),
                occurred_at: Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap(),
                line_items: vec![billing::TrustTransactionLine {
                    description: "September fees".into(),
                    amount_cents: 80_000,
                    invoice_reference: Some("INV-001".into()),
                }],
            },
        ]);

        let report = reconcile_once(&stub, &surreal).await.unwrap();
        assert_eq!(report.trust_deposits, 1);
        assert_eq!(report.trust_refunds, 1);
        assert_eq!(
            report.iolta_withdrawals_refused, 1,
            "INV-001 is not mirrored, so the withdrawal explains nothing"
        );
        assert_eq!(report.iolta_withdrawals, 0);

        let position = store::trust::position_for_project(&surreal, project_id)
            .await
            .unwrap();
        assert_eq!(position.deposited_cents, 500_000);
        assert_eq!(position.refunded_cents, 120_000);
        assert_eq!(
            position.earned_cents, 0,
            "a refused withdrawal posts no draw"
        );
        assert_eq!(position.held_cents(), 380_000);
    }

    /// A second night refreshes the balance in place rather than adding a
    /// second master balance for the same state.
    #[tokio::test]
    async fn re_running_the_mirror_refreshes_rather_than_duplicates() {
        let surreal = store::surreal::test_support::mem().await;
        seed_state(&surreal, "Nevada", "NV").await;

        let stub = StubBillingProvider::new();
        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 100_000)]);
        reconcile_once(&stub, &surreal).await.unwrap();

        stub.set_listed_bank_accounts(vec![bank("xero-nv", "IOLTA NV — Trust", 250_000)]);
        let second = reconcile_once(&stub, &surreal).await.unwrap();

        assert_eq!(second.iolta_accounts, 1);
        let accounts = store::iolta_accounts::all(&surreal).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].balance_cents, 250_000);
    }
}
