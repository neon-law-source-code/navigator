//! The `Archives` Restate workflow.
//!
//! Hosted by the `workflows-service` worker (which binds it alongside
//! the `Notation` virtual object — all workflows live on that one
//! endpoint, so there is no separate always-on archives pod).
//!
//! One invocation == one nightly export. Four durable steps:
//!
//! 1. `ctx.run("snapshot", …)` — open the database + object storage,
//!    snapshot every registered table to Parquet on GCS, return a
//!    journaled [`SnapshotSummary`]. A transient failure (database or
//!    GCS unreachable) replays just this step.
//! 2. `ctx.run("cost", …)` — when `BILLING_EXPORT_TABLE` is set, query
//!    every listed GCP billing export (one table per billing account)
//!    for trailing-window spend, merge the accounts by service, and
//!    snapshot the result to the export lake as `gcp_cost`. A clean
//!    no-op when the env var is unset (KIND / dev / OSS forks).
//! 3. `ctx.run("iceberg_telemetry", …)` — promote the day's telemetry
//!    Parquet (`telemetry/otel_*/data/dt=<date>/`) AND every store
//!    table's day of snapshot Parquet, written in phase 1
//!    (`application/<table>/data/dt=<date>/`), to Iceberg tables via
//!    the entity-table writer ([`crate::author_iceberg_for_prefix`]).
//!    Infallible per table: a lake-metadata hiccup never fails the
//!    export of the underlying Parquet, which already landed. A
//!    telemetry table is a no-op until the collector's OTLP→Parquet
//!    shim writes those files.
//! 4. `ctx.run("email", …)` — render the Slack digest (one line per
//!    snapshotted table, each linking to its Iceberg files in the Google
//!    Cloud console) and post it to firm ops through the worker's
//!    [`Notifier`]. Each step is journaled, so a retry re-uses the cached
//!    prior results rather than re-snapshotting. The step name stays `email`
//!    (its journal key on replay) even though it now posts Slack, not email.
//!
//! The `CronJob` `archives-trigger` POSTs to the Restate ingress to start
//! one invocation per night; Restate owns the retry schedule and the
//! invocation history a failed run is diagnosed from.

use std::sync::Arc;

use chrono::NaiveDate;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use workflows::Notifier;

use billing::gcp_cost::CostReport;

use crate::digest::{render_archives_slack, DiagnosticReport};
use crate::runner::{cost_phase, open_resources, snapshot_all, SnapshotSummary};
use crate::snapshot::{APPLICATION_LANE, TELEMETRY_LANE};
use crate::tables::ALL_TABLES;
use cloud::StorageService;

/// Request body for `Archives::run`. Empty today — the trigger only
/// needs to start the workflow — but kept as a struct (rather than
/// `()`) so fields like an override run-date can be threaded later
/// without breaking the handler signature.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct RunRequest {}

/// Summary returned to the caller (and visible in Restate Cloud as
/// the invocation's output).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunReport {
    pub run_date: NaiveDate,
    pub invocation_id: String,
    pub tables: usize,
    pub rows: usize,
    pub failures: usize,
}

/// Service struct registered with the Restate endpoint. Holds only the
/// worker-side [`Notifier`] (for the Slack digest); the database and
/// object-storage handles are opened inside the snapshot step so no
/// connection is held idle between nightly runs. Posts to the notifier
/// directly rather than through the `SlackOpsDelivery` code-fence adapter,
/// because the digest is mrkdwn with clickable links a code fence would
/// break — the same reason `Heartbeat` talks to the notifier directly.
#[derive(Clone)]
pub struct ArchivesService {
    notifier: Arc<dyn Notifier>,
}

impl ArchivesService {
    #[must_use]
    pub fn new(notifier: Arc<dyn Notifier>) -> Self {
        Self { notifier }
    }
}

#[restate_sdk::workflow(name = "Archives")]
impl ArchivesService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        _req: Json<RunRequest>,
    ) -> Result<Json<RunReport>, HandlerError> {
        let invocation_id = ctx.invocation_id().to_string();

        // Join the caller's trace: extract the W3C `traceparent` the trigger
        // injected (telemetry) from the invocation headers and parent this
        // run's span on it, so a `web`-initiated "run nightly export now" and
        // its durable steps appear as one trace. A no-op when none is present.
        let span = tracing::info_span!("archives.run", invocation_id = %invocation_id);
        {
            let headers = ctx.headers();
            telemetry::set_span_parent(
                &span,
                headers.get("traceparent").map(String::as_str),
                headers.get("tracestate").map(String::as_str),
            );
        }

        // `.instrument(span)` rather than `span.enter()`: the worker runs on a
        // multi-thread runtime, where a guard held across `.await` is the
        // documented footgun (it can leak to another task). Instrumenting the
        // future keeps the span attached only while this future is polled.
        async {
            // Phase 1 — snapshot. The whole loop is one journaled step:
            // open the handles fresh, snapshot every table, return the
            // serializable summary. `?` on `open_resources` yields a
            // retryable HandlerError so a database/GCS outage replays the
            // step.
            let summary: SnapshotSummary = ctx
                .run(|| async {
                    let (db, storage) = open_resources().await?;
                    Ok(Json(snapshot_all(&db, storage.as_ref()).await))
                })
                .name("snapshot")
                .await?
                .into_inner();

            // Phase 2 — GCP cost summary (no-op unless BILLING_EXPORT_TABLE
            // is set). Journaled like the snapshot. Its side effect — the
            // trailing-window spend snapshotted to the export lake as
            // `gcp_cost` — is the point; the returned report no longer feeds
            // the digest, which is now just the table list.
            let _cost: Option<CostReport> = ctx
                .run(|| async { Ok(Json(cost_phase(|k| std::env::var(k).ok()).await?)) })
                .name("cost")
                .await?
                .into_inner();

            // Phase 3 — promote the day's telemetry Parquet (otel_*) AND
            // every store table's day of snapshot Parquet (phase 1) to
            // Iceberg tables, reusing the entity-table writer. Journaled and
            // infallible per table: a lake-metadata hiccup never fails the
            // export. Run for the promotion side effect; the per-table
            // summary lines are no longer surfaced in the digest.
            //
            // The step keeps its original name `iceberg_telemetry`: it is the
            // journal key Restate matches on replay, and renaming it would
            // raise JOURNAL_MISMATCH for any invocation replayed across the
            // deploy that grew this step to cover the application lane too.
            let _iceberg_promotion: Vec<String> = ctx
                .run(|| async {
                    Ok::<_, HandlerError>(Json(
                        promote_iceberg_metadata(summary.run_date, |k| std::env::var(k).ok()).await,
                    ))
                })
                .name("iceberg_telemetry")
                .await?
                .into_inner();

            // Phase 4 — Slack digest, rendered from the journaled snapshot so a
            // retry re-posts without re-running the prior steps. Posts straight
            // to the notifier (mrkdwn with links a code fence would break); a
            // Slack failure surfaces as a `HandlerError` so Restate retries the
            // step rather than dropping the only copy of the signal.
            //
            // The durable step keeps its original name `email`: the name is the
            // journal key Restate matches on replay (`RunCommand` header
            // equality covers it), so renaming it would raise JOURNAL_MISMATCH
            // for any invocation replayed across the deploy that flipped this
            // step from email to Slack. The step now posts Slack, not email.
            let message = digest_message(&summary, |k| std::env::var(k).ok());
            let notifier = Arc::clone(&self.notifier);
            ctx.run(
                move || async move { notifier.notify(message).await.map_err(HandlerError::from) },
            )
            .name("email")
            .await?;

            Ok(Json(RunReport {
                run_date: summary.run_date,
                invocation_id,
                tables: summary.entries.len(),
                rows: summary.entries.iter().map(|e| e.rows).sum(),
                failures: summary.failures.len(),
            }))
        }
        .instrument(span)
        .await
    }
}

/// Compose the Slack digest message a completed run posts: assemble the
/// [`DiagnosticReport`] from the journaled summary and the export bucket,
/// then render it to mrkdwn. Split out of the handler — where it sits inside
/// a Restate `ctx.run` step — so the whole summary → message pipeline is
/// unit-tested without a workflow context. Shares `build_report`'s `get` seam,
/// so the bucket in each per-table link is exercised without touching env.
fn digest_message<F: Fn(&str) -> Option<String>>(summary: &SnapshotSummary, get: F) -> String {
    render_archives_slack(&build_report(summary, get))
}

/// Assemble the [`DiagnosticReport`] from the journaled summary and the
/// export bucket. Takes a `key -> value` lookup seam so the mapping is
/// unit-testable without mutating process env. The bucket is the only env
/// the digest needs — it is the root of every per-table GCS console link.
fn build_report<F: Fn(&str) -> Option<String>>(
    summary: &SnapshotSummary,
    get: F,
) -> DiagnosticReport {
    let bucket = get("NAVIGATOR_STORAGE_BUCKET")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "<unset>".to_string());
    DiagnosticReport {
        run_date: summary.run_date,
        bucket,
        snapshots: summary.entries.clone(),
        failures: summary.failures.clone(),
    }
}

/// The `otel_*` tables promoted from the telemetry lake's daily Parquet.
const TELEMETRY_TABLES: &[&str] = &["otel_logs", "otel_traces", "otel_metrics"];

/// Promote the day's telemetry Parquet (`telemetry/otel_*/data/dt=<date>/`)
/// AND every store table's day of snapshot Parquet
/// (`application/<table>/data/dt=<date>/`, written by phase 1) to Iceberg
/// tables, reusing the entity-table writer
/// ([`crate::author_iceberg_for_prefix`]).
///
/// **Infallible per table by design** — a lake-metadata hiccup must never
/// fail the nightly export, whose Parquet already landed in phase 1 — so it
/// returns one human-readable line per table for the diagnostic email rather
/// than a `Result`. A clean no-op ("no data") until a data file exists under
/// that table's prefix for the run date.
async fn promote_iceberg_metadata<F: Fn(&str) -> Option<String>>(
    run_date: NaiveDate,
    get: F,
) -> Vec<String> {
    let storage = match cloud::exports_from_env().await {
        Ok(s) => s,
        Err(e) => {
            return vec![format!(
                "(iceberg metadata promotion skipped — storage unavailable: {e})"
            )]
        }
    };
    let bucket = get("NAVIGATOR_STORAGE_BUCKET")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "exports".to_string());
    // Stamped once inside the journaled step (so a replay reuses the cached
    // result, not a new clock read).
    let now_ms = chrono::Utc::now().timestamp_millis();
    promote_iceberg_metadata_over(storage.as_ref(), &bucket, run_date, now_ms).await
}

/// The testable core of [`promote_iceberg_metadata`]: everything after the
/// storage handle is open and the clock has been read, so a test can inject
/// an `FsStorage` temp dir and a pinned `now_ms` without touching process env
/// or the clock.
async fn promote_iceberg_metadata_over(
    storage: &dyn StorageService,
    bucket: &str,
    run_date: NaiveDate,
    now_ms: i64,
) -> Vec<String> {
    let location_base = format!("gs://{bucket}");
    // 30-day cutoff for the short-retention telemetry tables (traces/metrics);
    // their GCS lifecycle deletes data at 30d, so the snapshot log is pruned
    // to match. Every store table keeps its FULL snapshot log below — these
    // are binding records, never pruned to match a telemetry lifecycle rule.
    let cutoff_30d_ms = now_ms - 30 * 24 * 60 * 60 * 1000;

    let mut plan: Vec<(&str, &str, Option<i64>)> = TELEMETRY_TABLES
        .iter()
        .map(|&table| {
            // otel_logs keeps its full snapshot log (10-year, content-free);
            // otel_traces / otel_metrics prune to the 30-day lifecycle window.
            let expire_before_ms = (table != "otel_logs").then_some(cutoff_30d_ms);
            (TELEMETRY_LANE, table, expire_before_ms)
        })
        .collect();
    plan.extend(
        ALL_TABLES
            .iter()
            .map(|table| (APPLICATION_LANE, table.as_str(), None)),
    );

    let mut lines = Vec::with_capacity(plan.len());
    for (i, (lane, table, expire_before_ms)) in plan.into_iter().enumerate() {
        let snapshot_id = now_ms.saturating_add(i64::try_from(i).unwrap_or(0));
        match crate::author_iceberg_for_prefix(
            storage,
            crate::PromotionRequest {
                lane,
                table,
                location_base: &location_base,
                run_date,
                snapshot_id,
                timestamp_ms: now_ms,
                expire_before_ms,
            },
        )
        .await
        {
            Ok(Some(authored)) => lines.push(format!("{lane}/{table} v{}", authored.version)),
            Ok(None) => lines.push(format!("{lane}/{table} (no data)")),
            Err(e) => lines.push(format!("{lane}/{table} FAILED: {e}")),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{build_report, digest_message, promote_iceberg_metadata_over, SnapshotSummary};
    use crate::digest::SnapshotEntry;
    use crate::drift::DriftDecision;
    use crate::runner::TableFailure;
    use chrono::NaiveDate;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn empty_summary() -> SnapshotSummary {
        summary_with(Vec::new(), Vec::new())
    }

    fn summary_with(entries: Vec<SnapshotEntry>, failures: Vec<TableFailure>) -> SnapshotSummary {
        SnapshotSummary {
            run_date: NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(),
            entries,
            failures,
        }
    }

    fn entry(table: &str, rows: usize) -> SnapshotEntry {
        SnapshotEntry {
            table: table.into(),
            rows,
            bytes: rows * 16,
            key: format!("application/{table}/data/dt=2026-05-29/part-0.parquet"),
            drift: DriftDecision::Unchanged,
        }
    }

    #[test]
    fn build_report_defaults_bucket_when_env_unset() {
        let report = build_report(&empty_summary(), |_| None);
        assert_eq!(report.bucket, "<unset>");
        assert!(report.snapshots.is_empty());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn build_report_threads_bucket_through() {
        let report = build_report(
            &empty_summary(),
            lookup(&[("NAVIGATOR_STORAGE_BUCKET", "proj-exports")]),
        );
        assert_eq!(report.bucket, "proj-exports");
    }

    #[test]
    fn build_report_treats_empty_bucket_as_unset() {
        let report = build_report(
            &empty_summary(),
            lookup(&[("NAVIGATOR_STORAGE_BUCKET", "")]),
        );
        assert_eq!(report.bucket, "<unset>");
    }

    #[test]
    fn build_report_carries_snapshots_and_failures_through() {
        // The empty-summary tests above prove the bucket default; this proves
        // the report actually threads the journaled snapshot data — the lines
        // the digest renders from — rather than silently dropping it.
        let report = build_report(
            &summary_with(
                vec![entry("persons", 312), entry("documents", 1_204)],
                vec![TableFailure {
                    table: "notations".into(),
                    error: "connection reset".into(),
                }],
            ),
            |_| None,
        );
        assert_eq!(
            report.run_date,
            NaiveDate::from_ymd_opt(2026, 5, 29).unwrap()
        );
        let tables: Vec<&str> = report.snapshots.iter().map(|e| e.table.as_str()).collect();
        assert_eq!(tables, ["persons", "documents"]);
        assert_eq!(report.snapshots[0].rows, 312);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].table, "notations");
    }

    #[test]
    fn digest_message_renders_tables_links_and_failures_from_a_summary() {
        // End-to-end for the seam the handler calls: a journaled summary plus
        // the export bucket becomes the exact Slack digest, per-table GCS links
        // pointed at the threaded bucket, with any failure surfaced as a ⚠️ line.
        let msg = digest_message(
            &summary_with(
                vec![entry("persons", 312)],
                vec![TableFailure {
                    table: "documents".into(),
                    error: "timeout".into(),
                }],
            ),
            lookup(&[("NAVIGATOR_STORAGE_BUCKET", "proj-exports")]),
        );
        assert!(msg.starts_with("🧊"), "leads with the iceberg glyph: {msg}");
        assert!(msg.contains("(1 tables)"), "header counts snapshots: {msg}");
        assert!(
            msg.contains("• *persons* — 312 rows · "),
            "per-table line: {msg}"
        );
        assert!(
            msg.contains(
                "<https://console.cloud.google.com/storage/browser/proj-exports/application/persons|view in GCP>"
            ),
            "link points into the threaded bucket: {msg}"
        );
        assert!(
            msg.contains("• ⚠️ *documents* — snapshot failed"),
            "failed table surfaces as a warning: {msg}"
        );
    }

    #[test]
    fn digest_message_links_into_unset_bucket_when_env_is_absent() {
        // With no NAVIGATOR_STORAGE_BUCKET the report falls back to `<unset>`,
        // so the per-table link is still well-formed rather than dropping the
        // bucket segment — the gap is visible in the URL, not a broken link.
        let msg = digest_message(&summary_with(vec![entry("persons", 1)], vec![]), |_| None);
        assert!(
            msg.contains(
                "<https://console.cloud.google.com/storage/browser/<unset>/application/persons|view in GCP>"
            ),
            "unset bucket still yields a structured link: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn promotion_authors_iceberg_metadata_for_a_store_table_and_chains_a_second_run() {
        use crate::runner::snapshot_all;
        use cloud::{FsStorage, StorageService};
        use iceberg::spec::TableMetadata;

        let db = store::surreal::test_support::mem().await;
        db.query(
            "CREATE person:analyst SET \
             name = 'Analyst', \
             email = 'analyst@example.com', \
             inserted_at = d'2026-08-09T00:00:00Z', \
             updated_at = d'2026-08-09T00:00:00Z'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let storage = FsStorage::new(dir.path().to_path_buf()).await.unwrap();
        let run_date = chrono::Utc::now().date_naive();

        // Phase 1: writes `application/person/data/dt=<today>/*.parquet`.
        let summary = snapshot_all(&db, &storage).await;
        assert!(summary.failures.is_empty(), "{:#?}", summary.failures);

        // Phase 3, first run: author Iceberg metadata over that data file.
        let now_ms = chrono::Utc::now().timestamp_millis();
        promote_iceberg_metadata_over(&storage, "exports", run_date, now_ms).await;

        let meta = storage
            .get("application/person/metadata/v1.metadata.json")
            .await
            .unwrap();
        let table_metadata: TableMetadata = serde_json::from_slice(&meta.bytes).unwrap();
        assert_eq!(
            table_metadata.snapshots().count(),
            1,
            "first run authors exactly one snapshot"
        );
        let hint = storage
            .get("application/person/metadata/version-hint.text")
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&hint.bytes).trim(), "1");

        // Second run over the same storage and day: chains onto the prior
        // snapshot log rather than starting a fresh table (store tables never
        // prune — expire_before_ms is always None for the application lane).
        promote_iceberg_metadata_over(&storage, "exports", run_date, now_ms + 1000).await;

        let meta = storage
            .get("application/person/metadata/v2.metadata.json")
            .await
            .unwrap();
        let table_metadata: TableMetadata = serde_json::from_slice(&meta.bytes).unwrap();
        assert_eq!(
            table_metadata.snapshots().count(),
            2,
            "second run chains onto the prior log — a two-entry snapshot log"
        );
        let hint = storage
            .get("application/person/metadata/version-hint.text")
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&hint.bytes).trim(), "2");
    }
}
