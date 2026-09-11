//! The `ReconcileInvoices` Restate workflow — the nightly job that folds
//! Xero's payment state back onto the local `xero_invoice` mirror.
//!
//! The portal reads the mirror, never Xero live, so something has to keep
//! the mirror's `status` / `amount_paid_cents` current. This workflow does
//! it once a night: it lists every mirror row not yet in a terminal state
//! (`PAID` / `VOIDED`), reads each invoice back from Xero, and records the
//! result. A settled invoice is never polled again.
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
    /// Mirror rows that were still open and got re-checked against Xero.
    pub checked: usize,
    /// How many of those actually changed (status or amount paid).
    pub updated: usize,
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

/// Re-check every open mirror row against the provider and fold the result
/// back. Provider-agnostic; unit-tested against the stub + an in-memory
/// SurrealDB.
///
/// # Errors
///
/// Propagates any database or billing-provider error.
pub async fn reconcile_once(
    provider: &dyn BillingProvider,
    surreal: &SurrealDb,
) -> anyhow::Result<ReconcileReport> {
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
    Ok(ReconcileReport {
        checked: rows.len(),
        updated,
    })
}

#[cfg(test)]
mod tests {
    use super::reconcile_once;
    use billing::{InvoiceStatus, StubBillingProvider};

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
}
