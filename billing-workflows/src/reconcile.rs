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
    Ok(ReconcileReport {
        ingested,
        unscoped,
        checked: rows.len(),
        updated,
    })
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
    use billing::{InvoiceStatus, ReceivableInvoice, StubBillingProvider};
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
}
