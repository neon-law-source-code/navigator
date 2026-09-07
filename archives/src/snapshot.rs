//! Shared snapshot machinery — per-table writers live in
//! sibling modules ([`crate::generic`] does the row → batch
//! conversion).

use chrono::{DateTime, Utc};

/// The Iceberg key-layout lane for `SurrealDB` entity tables (`ALL_TABLES`) —
/// the analytical archive of the store, and binding records. See
/// [`TELEMETRY_LANE`] for the sibling lane.
pub const APPLICATION_LANE: &str = "application";

/// The Iceberg key-layout lane for `OTel` telemetry tables (`TELEMETRY_TABLES`
/// in `archives::workflow`), promoted from the 90-day telemetry landing
/// bucket. See [`APPLICATION_LANE`] for the sibling lane.
pub const TELEMETRY_LANE: &str = "telemetry";

/// Per-snapshot configuration. `run_date` is what partitions
/// data files in the bucket layout; the snapshot machinery
/// never reads wall-clock time directly so tests can pin a
/// deterministic date.
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// The key-layout lane — [`APPLICATION_LANE`] or [`TELEMETRY_LANE`].
    /// The bucket is dedicated to the archive, so this segment is what
    /// keeps the two kinds of table apart.
    pub lane: &'static str,
    pub table: String,
    pub run_date: DateTime<Utc>,
    pub partition_uuid: uuid::Uuid,
}

impl SnapshotConfig {
    pub fn now(lane: &'static str, table: impl Into<String>) -> Self {
        Self {
            lane,
            table: table.into(),
            run_date: Utc::now(),
            partition_uuid: uuid::Uuid::now_v7(),
        }
    }
}

/// What `snapshot_table` returns to its caller — table name,
/// row count, and the object-storage key the bytes were
/// uploaded under. `main` logs a meaningful summary from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotOutcome {
    pub table: String,
    pub rows: usize,
    pub key: String,
    pub bytes: usize,
}

/// Build the canonical bucket key for one snapshot data file.
///
/// Layout is frozen for v1 (see `docs/iceberg-archive.md` → "GCS layout"):
///
/// ```text
/// <lane>/<table>/data/dt=<yyyy-mm-dd>/part-<uuid>.parquet
/// ```
#[must_use]
pub fn snapshot_key(cfg: &SnapshotConfig) -> String {
    let date = cfg.run_date.format("%Y-%m-%d");
    format!(
        "{}/{}/data/dt={}/part-{}.parquet",
        cfg.lane, cfg.table, date, cfg.partition_uuid
    )
}

#[cfg(test)]
mod tests {
    use super::{snapshot_key, SnapshotConfig, APPLICATION_LANE};
    use chrono::TimeZone;

    #[test]
    fn snapshot_key_uses_lane_table_run_date_and_uuid() {
        let cfg = SnapshotConfig {
            lane: APPLICATION_LANE,
            table: "persons".into(),
            run_date: chrono::Utc.with_ymd_and_hms(2026, 5, 24, 10, 0, 0).unwrap(),
            partition_uuid: uuid::Uuid::parse_str("01234567-89ab-7def-8123-456789abcdef").unwrap(),
        };
        assert_eq!(
            snapshot_key(&cfg),
            "application/persons/data/dt=2026-05-24/part-01234567-89ab-7def-8123-456789abcdef.parquet"
        );
    }

    #[test]
    fn snapshot_config_now_populates_lane_table_and_uuid() {
        let cfg = SnapshotConfig::now(APPLICATION_LANE, "entities");
        assert_eq!(cfg.lane, APPLICATION_LANE);
        assert_eq!(cfg.table, "entities");
        // v7 UUIDs encode the timestamp; non-nil is enough here.
        assert!(!cfg.partition_uuid.is_nil());
    }
}
