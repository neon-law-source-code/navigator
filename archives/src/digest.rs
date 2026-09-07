//! Slack digest for the nightly `archives` workflow.
//!
//! Builds the one Slack message a completed nightly export posts to the
//! firm-ops channel: an iceberg glyph and the run date, then one line per
//! snapshotted table — table name, row count, and a link that opens the
//! table's Iceberg files in the Google Cloud Storage browser. A table that
//! failed to snapshot surfaces as its own ⚠️ line, so a silent gap can't hide
//! in the nightly signal.
//!
//! Slack **mrkdwn**, not a fenced code block: the per-table links must render
//! as clickable `<url|label>`, which a code fence would print as literal text.
//! That is why `Archives` posts this message straight to the [`Notifier`] like
//! `Heartbeat` does, rather than routing a plain-text body through the
//! `SlackOpsDelivery` code-fence adapter the other ops digests use.
//!
//! [`Notifier`]: workflows::Notifier

use std::fmt::Write as _;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::drift::DriftDecision;
use crate::runner::TableFailure;
use crate::snapshot::APPLICATION_LANE;

/// One table's snapshot outcome. Serializable because it is part of the
/// journaled snapshot-phase output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub table: String,
    pub rows: usize,
    pub bytes: usize,
    pub key: String,
    pub drift: DriftDecision,
}

/// The data a completed workflow run hands to [`render_archives_slack`]:
/// the run date, the export bucket the per-table links point into, and the
/// snapshot outcomes (successes and failures).
#[derive(Debug, Clone)]
pub struct DiagnosticReport {
    pub run_date: NaiveDate,
    pub bucket: String,
    pub snapshots: Vec<SnapshotEntry>,
    pub failures: Vec<TableFailure>,
}

/// The Google Cloud Storage browser. Every snapshotted table here is a
/// `SurrealDB` entity table, so its Iceberg files live under the
/// `application/<table>/` prefix of the export bucket (see
/// [`crate::snapshot`]) — linking to that prefix opens the table's `data/`
/// and `metadata/` objects in the console.
const GCS_BROWSER_BASE: &str = "https://console.cloud.google.com/storage/browser";

/// Render the nightly export as a single Slack **mrkdwn** message: an iceberg
/// glyph and the run date, then one line per snapshotted table — name, row
/// count, and a link into the Google Cloud Storage browser for that table's
/// Iceberg files. A table that failed to snapshot gets a ⚠️ line instead of a
/// link. Pure and exposed so the formatting is unit-tested.
#[must_use]
pub fn render_archives_slack(report: &DiagnosticReport) -> String {
    let mut out = format!(
        "🧊 *Iceberg nightly export — {}*  ({} tables)\n",
        report.run_date,
        report.snapshots.len()
    );
    for entry in &report.snapshots {
        let url = format!(
            "{GCS_BROWSER_BASE}/{}/{APPLICATION_LANE}/{}",
            report.bucket, entry.table
        );
        let _ = writeln!(
            out,
            "• *{}* — {} rows · <{url}|view in GCP>",
            entry.table, entry.rows
        );
    }
    for failure in &report.failures {
        let _ = writeln!(out, "• ⚠️ *{}* — snapshot failed", failure.table);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{render_archives_slack, DiagnosticReport, SnapshotEntry};
    use crate::drift::DriftDecision;
    use crate::runner::TableFailure;
    use chrono::NaiveDate;

    fn sample(snapshots: Vec<SnapshotEntry>, failures: Vec<TableFailure>) -> DiagnosticReport {
        DiagnosticReport {
            run_date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            bucket: "navigator-exports".into(),
            snapshots,
            failures,
        }
    }

    fn snap(table: &str, rows: usize) -> SnapshotEntry {
        SnapshotEntry {
            table: table.into(),
            rows,
            bytes: rows * 32,
            key: format!("application/{table}/data/dt=2026-07-10/part-abc.parquet"),
            drift: DriftDecision::Unchanged,
        }
    }

    #[test]
    fn message_opens_with_iceberg_glyph_and_run_date() {
        let msg = render_archives_slack(&sample(vec![snap("persons", 10)], vec![]));
        assert!(
            msg.starts_with("🧊"),
            "message must lead with the iceberg glyph: {msg}"
        );
        assert!(
            msg.contains("2026-07-10"),
            "message must carry the run date"
        );
    }

    #[test]
    fn header_counts_the_snapshotted_tables() {
        let msg = render_archives_slack(&sample(
            vec![snap("persons", 10), snap("documents", 20)],
            vec![],
        ));
        assert!(
            msg.contains("(2 tables)"),
            "header should count the tables: {msg}"
        );
    }

    #[test]
    fn each_table_line_carries_name_rows_and_a_gcs_browser_link() {
        let msg = render_archives_slack(&sample(
            vec![snap("persons", 312), snap("documents", 1_204)],
            vec![],
        ));
        // Table name in bold, the row count, and a mrkdwn link — one line each.
        assert!(msg.contains("• *persons* — 312 rows · "));
        assert!(msg.contains("• *documents* — 1204 rows · "));
        // The link points into the GCS browser at this table's application/ prefix.
        assert!(msg.contains(
            "<https://console.cloud.google.com/storage/browser/navigator-exports/application/persons|view in GCP>"
        ));
        assert!(msg.contains(
            "<https://console.cloud.google.com/storage/browser/navigator-exports/application/documents|view in GCP>"
        ));
    }

    #[test]
    fn message_is_mrkdwn_not_a_code_block() {
        // A fenced block would render the <url|label> links as literal text, so
        // the digest must never wrap itself in ``` — that is the whole reason it
        // bypasses the SlackOpsDelivery code-fence adapter.
        let msg = render_archives_slack(&sample(vec![snap("persons", 10)], vec![]));
        assert!(
            !msg.contains("```"),
            "digest must be mrkdwn, not a code block: {msg}"
        );
    }

    #[test]
    fn a_failed_table_surfaces_as_a_warning_line_not_a_link() {
        let msg = render_archives_slack(&sample(
            vec![snap("persons", 10)],
            vec![TableFailure {
                table: "documents".into(),
                error: "connection reset".into(),
            }],
        ));
        assert!(
            msg.contains("• ⚠️ *documents* — snapshot failed"),
            "failure must be visible: {msg}"
        );
        // The failed table has no GCS link (it never landed in the bucket).
        assert!(!msg.contains("application/documents"));
    }

    #[test]
    fn only_the_requested_fields_appear() {
        // The digest is deliberately just the table list — no bytes, cost,
        // BigQuery query, drift, telemetry, or Restate footer leaks in.
        let msg = render_archives_slack(&sample(vec![snap("persons", 10)], vec![]));
        for banned in [
            "Bytes",
            "COST",
            "SELECT",
            "DRIFT",
            "invocation",
            "TELEMETRY",
        ] {
            assert!(
                !msg.contains(banned),
                "digest should omit {banned:?}: {msg}"
            );
        }
    }

    #[test]
    fn empty_export_renders_header_only_without_panicking() {
        let msg = render_archives_slack(&sample(vec![], vec![]));
        assert!(msg.contains("(0 tables)"));
        assert!(!msg.contains('•'));
    }
}
