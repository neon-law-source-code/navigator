//! Snapshot orchestration — the work the old `archives nightly`
//! subcommand did, now a plain library function the Restate workflow
//! drives inside a journaled `ctx.run("snapshot", …)` step.
//!
//! [`snapshot_all`] walks [`crate::tables::ALL_TABLES`], encodes each
//! non-empty table to Parquet, uploads it under the canonical
//! `iceberg/<table>/data/<date>/part-<uuid>.parquet` key, and applies
//! the add-only [`crate::drift`] policy. Per-table failures are
//! collected into [`SnapshotSummary::failures`] rather than aborting
//! the run, so the Slack digest still reports what did and didn't
//! succeed. Only a failure to acquire the database / storage handles
//! propagates as an error (Restate retries the whole step).

use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use cloud::{StorageError, StorageService};
use serde::{Deserialize, Serialize};
use store::surreal::{SurrealConfig, SurrealDb};

use billing::gcp_cost::{
    adc_token_provider, billing_export_tables, BillingClient, CostReport, CostRow,
};

use crate::tables::fetch_batch;
use crate::{
    batch_from_rows, classify, encode_parquet, fingerprint, fingerprint_key, snapshot_key,
    DriftDecision, SnapshotConfig, SnapshotEntry, StoredFingerprint, ALL_TABLES,
};

/// One table that failed to snapshot, with the rendered error so the
/// Slack digest can surface it as a ⚠️ line. Serializable because the whole
/// summary is the journaled output of the snapshot workflow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableFailure {
    pub table: String,
    pub error: String,
}

/// The journaled result of the snapshot phase. `run_date` is captured
/// here (inside the `ctx.run` step) so it is recorded once and stays
/// stable across Restate replays.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub run_date: NaiveDate,
    pub entries: Vec<SnapshotEntry>,
    pub failures: Vec<TableFailure>,
}

/// Open the `SurrealDB` connection and object-storage backend the
/// snapshot phase needs. A failure here propagates so the workflow
/// step retries — the database or GCS being unreachable is transient.
pub async fn open_resources() -> Result<(SurrealDb, Arc<dyn StorageService>)> {
    let config = SurrealConfig::from_env().context("read SurrealDB coordinates")?;
    let db = store::surreal::connect(&config)
        .await
        .context("open SurrealDB connection")?;
    // Exports lane: nightly Parquet lands in the dedicated exports bucket
    // (`NAVIGATOR_STORAGE_BUCKET`), never the documents bucket — even
    // though the worker pod also carries `NAVIGATOR_DOCUMENTS_BUCKET` for
    // its document-render lane.
    let storage = cloud::exports_from_env()
        .await
        .context("open object storage")?;
    Ok((db, storage))
}

/// Snapshot every registered table. Returns a [`SnapshotSummary`]
/// even when individual tables fail; only the inability to acquire
/// the shared handles is an error.
pub async fn snapshot_all(db: &SurrealDb, storage: &dyn StorageService) -> SnapshotSummary {
    let run_date = Utc::now().date_naive();
    let mut entries: Vec<SnapshotEntry> = Vec::new();
    let mut failures: Vec<TableFailure> = Vec::new();
    for table in ALL_TABLES.iter() {
        match snapshot_table(db, storage, table).await {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => tracing::info!(table, "skipped (empty)"),
            Err(err) => {
                tracing::error!(table, error = ?err, "snapshot failed");
                failures.push(TableFailure {
                    table: table.clone(),
                    error: format!("{err:#}"),
                });
            }
        }
    }
    SnapshotSummary {
        run_date,
        entries,
        failures,
    }
}

/// Snapshot one table to Parquet on object storage. `Ok(None)` for an
/// empty table.
async fn snapshot_table(
    db: &SurrealDb,
    storage: &dyn StorageService,
    table: &str,
) -> Result<Option<SnapshotEntry>> {
    let Some(batch) = fetch_batch(db, table).await? else {
        return Ok(None);
    };
    let rows = batch.num_rows();

    let current_fp = fingerprint(&batch);
    let prev_fp = read_fingerprint(storage, table).await?;
    let decision = classify(prev_fp.as_ref(), &current_fp)?;

    let bytes = encode_parquet(&batch)?;
    let cfg = SnapshotConfig::now(table);
    let key = snapshot_key(&cfg);
    storage
        .put(&key, &bytes, "application/vnd.apache.parquet")
        .await
        .with_context(|| format!("upload {key}"))?;

    if needs_fingerprint_write(prev_fp.as_ref(), &decision) {
        write_fingerprint(
            storage,
            &StoredFingerprint {
                table: table.to_string(),
                columns: current_fp,
            },
        )
        .await?;
    }

    tracing::info!(
        table,
        rows,
        key = %key,
        bytes = bytes.len(),
        decision = ?decision,
        "snapshot uploaded"
    );
    Ok(Some(SnapshotEntry {
        table: table.to_string(),
        rows,
        bytes: bytes.len(),
        key,
        drift: decision,
    }))
}

/// The GCP-cost phase. Env-gated on `BILLING_EXPORT_TABLE`: unset →
/// `Ok(None)` (KIND / dev / OSS forks skip it cleanly, needing no
/// `BigQuery` credentials). When set, query every listed billing export —
/// the value is a comma-separated list, one table per billing account —
/// for trailing-window cost by service, merge the accounts into one
/// firm-wide set, snapshot it to the export lake as the `gcp_cost` table
/// (so it is queryable in `BigQuery` like the data tables), and return the
/// report. The lake snapshot is the point; the workflow runs this phase for
/// that side effect.
pub async fn cost_phase<F: Fn(&str) -> Option<String>>(get: F) -> Result<Option<CostReport>> {
    let non_empty = |k: &str| get(k).filter(|s| !s.is_empty());
    let tables = non_empty("BILLING_EXPORT_TABLE")
        .map(|raw| billing_export_tables(&raw))
        .unwrap_or_default();
    if tables.is_empty() {
        return Ok(None);
    }
    let project = non_empty("BIGQUERY_PROJECT").context(
        "BILLING_EXPORT_TABLE is set but BIGQUERY_PROJECT is not — both are required to query \
         the billing export",
    )?;
    let days: u32 = non_empty("ARCHIVES_COST_WINDOW_DAYS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let token = adc_token_provider().await?;
    let client = BillingClient::new(project, token);
    // One export table per billing account. The lake's `gcp_cost` table is a
    // firm-wide cost snapshot, so every account's rows are merged by service —
    // reading only the first table would silently under-report the total.
    let mut per_account = Vec::with_capacity(tables.len());
    for table in &tables {
        per_account.push(
            client
                .cost_by_service(table, days)
                .await
                .with_context(|| format!("query billing export {table}"))?,
        );
    }
    let rows = merge_cost_rows(per_account);

    // Snapshot the cost rows to the export lake the same way data
    // tables are written, so `gcp_cost` is queryable in BigQuery too.
    let key = match batch_from_rows(&rows)? {
        Some(batch) => {
            let storage = cloud::exports_from_env()
                .await
                .context("open object storage for cost snapshot")?;
            let bytes = encode_parquet(&batch)?;
            let cfg = SnapshotConfig::now("gcp_cost");
            let key = snapshot_key(&cfg);
            storage
                .put(&key, &bytes, "application/vnd.apache.parquet")
                .await
                .with_context(|| format!("upload {key}"))?;
            Some(key)
        }
        None => None,
    };
    Ok(Some(CostReport { rows, key }))
}

/// Merge each billing account's cost-by-service rows into one firm-wide set,
/// summing a service billed to several accounts and returning highest-cost
/// first — the same ordering a single-table read produces, so the `gcp_cost`
/// snapshot's shape does not depend on how many accounts are configured.
fn merge_cost_rows(per_account: Vec<Vec<CostRow>>) -> Vec<CostRow> {
    // BTreeMap so equal-cost services land in a deterministic (alphabetical)
    // order — a snapshot that reshuffles run to run reads as spurious drift.
    let mut totals: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for rows in per_account {
        for row in rows {
            *totals.entry(row.service).or_default() += row.cost;
        }
    }
    let mut rows: Vec<CostRow> = totals
        .into_iter()
        .map(|(service, cost)| CostRow { service, cost })
        .collect();
    rows.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    rows
}

fn needs_fingerprint_write(previous: Option<&StoredFingerprint>, decision: &DriftDecision) -> bool {
    matches!(decision, DriftDecision::Added(_)) || previous.is_none()
}

async fn read_fingerprint(
    storage: &dyn StorageService,
    table: &str,
) -> Result<Option<StoredFingerprint>> {
    let key = fingerprint_key(table);
    match storage.get(&key).await {
        Ok(obj) => {
            let parsed: StoredFingerprint = serde_json::from_slice(&obj.bytes)
                .with_context(|| format!("parse stored fingerprint at {key}"))?;
            Ok(Some(parsed))
        }
        Err(StorageError::NotFound(_)) => Ok(None),
        Err(other) => Err(other).with_context(|| format!("read fingerprint at {key}")),
    }
}

async fn write_fingerprint(storage: &dyn StorageService, fp: &StoredFingerprint) -> Result<()> {
    let key = fingerprint_key(&fp.table);
    let bytes = serde_json::to_vec_pretty(fp).context("serialize fingerprint")?;
    storage
        .put(&key, &bytes, "application/json")
        .await
        .with_context(|| format!("write fingerprint at {key}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        merge_cost_rows, needs_fingerprint_write, snapshot_all, SnapshotSummary, TableFailure,
    };
    use crate::{fetch_batch, fingerprint, DriftDecision, StoredFingerprint};
    use arrow::array::{Array, StringArray};
    use billing::gcp_cost::CostRow;
    use cloud::FsStorage;

    fn row(service: &str, cost: f64) -> CostRow {
        CostRow {
            service: service.into(),
            cost,
        }
    }

    #[test]
    fn cost_rows_merge_every_billing_account_into_one_snapshot() {
        // The `gcp_cost` snapshot is firm-wide, so a service billed to two
        // accounts sums rather than appearing twice or shadowing the other.
        let merged = merge_cost_rows(vec![
            vec![row("Kubernetes Engine", 100.00), row("Cloud SQL", 20.00)],
            vec![
                row("Kubernetes Engine", 50.00),
                row("Compute Engine", 30.00),
            ],
        ]);
        assert_eq!(
            merged,
            vec![
                row("Kubernetes Engine", 150.00),
                row("Compute Engine", 30.00),
                row("Cloud SQL", 20.00),
            ],
            "merged highest-cost first"
        );
    }

    #[test]
    fn merging_one_account_preserves_the_single_table_shape() {
        // A single-account deploy must produce exactly what a plain
        // `cost_by_service` read did, or the snapshot shape depends on config.
        let rows = vec![row("Kubernetes Engine", 100.00), row("Cloud SQL", 20.00)];
        assert_eq!(merge_cost_rows(vec![rows.clone()]), rows);
        assert!(merge_cost_rows(vec![]).is_empty());
    }

    #[test]
    fn first_run_writes_fingerprint_even_if_unchanged_decision() {
        assert!(needs_fingerprint_write(
            None,
            &DriftDecision::Added(vec!["id".into()])
        ));
    }

    #[test]
    fn added_decision_writes_fingerprint() {
        let prev = StoredFingerprint {
            table: "persons".into(),
            columns: vec!["id".into()],
        };
        assert!(needs_fingerprint_write(
            Some(&prev),
            &DriftDecision::Added(vec!["email".into()])
        ));
    }

    #[test]
    fn unchanged_decision_skips_fingerprint_rewrite() {
        let prev = StoredFingerprint {
            table: "persons".into(),
            columns: vec!["id".into()],
        };
        assert!(!needs_fingerprint_write(
            Some(&prev),
            &DriftDecision::Unchanged
        ));
    }

    #[tokio::test]
    async fn surreal_snapshot_uses_record_and_datetime_strings_for_schema_drift() {
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

        let batch = fetch_batch(&db, "person").await.unwrap().unwrap();
        let columns = fingerprint(&batch);
        let expected_columns = vec![
            "email".into(),
            "email_confirmed".into(),
            "email_lower".into(),
            "id".into(),
            "inserted_at".into(),
            "name".into(),
            "role".into(),
            "updated_at".into(),
        ];
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(columns, expected_columns);
        assert_eq!(string_cell(&batch, "id"), "person:analyst");
        assert_eq!(string_cell(&batch, "inserted_at"), "2026-08-09T00:00:00Z");

        let dir = tempfile::tempdir().unwrap();
        let storage = FsStorage::new(dir.path()).await.unwrap();
        let summary = snapshot_all(&db, &storage).await;
        let person = summary
            .entries
            .iter()
            .find(|entry| entry.table == "person")
            .expect("seeded person table is snapshotted");
        assert_eq!(person.rows, 1);
        assert_eq!(person.drift, DriftDecision::Added(expected_columns));
        assert!(summary.failures.is_empty(), "{:#?}", summary.failures);
    }

    fn string_cell<'a>(batch: &'a arrow::array::RecordBatch, name: &str) -> &'a str {
        let index = batch.schema().index_of(name).unwrap();
        batch
            .column(index)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0)
    }

    #[test]
    fn snapshot_summary_round_trips_through_serde() {
        // The whole summary is the output of `ctx.run("snapshot", …)`,
        // so it must round-trip through serde for Restate to journal
        // and replay it. A live database-backed `snapshot_all` runs in
        // the KIND smoke test; the per-binary testcontainers
        // cost isn't worth it for a thin loop over the per-table
        // dispatch already covered in `tables.rs`.
        let summary = SnapshotSummary {
            run_date: chrono::NaiveDate::from_ymd_opt(2026, 5, 29).unwrap(),
            entries: Vec::new(),
            failures: vec![TableFailure {
                table: "persons".into(),
                error: "boom".into(),
            }],
        };
        let json = serde_json::to_string(&summary).unwrap();
        let back: SnapshotSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.failures.len(), 1);
        assert_eq!(back.run_date, summary.run_date);
    }
}
