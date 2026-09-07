//! Add-only schema drift detector.
//!
//! Run policy:
//!
//! - First run for a table: write the current fingerprint, ship.
//! - Subsequent runs:
//!   - **Equal** → ship.
//!   - **Added columns** → ship, update the stored fingerprint.
//!   - **Removed or renamed columns** → bail. A removed column
//!     drops data on the floor; a rename presents as
//!     (removed old + added new) and we refuse to guess.
//!
//! Drift state lives alongside the snapshot data in the same
//! bucket at `<lane>/<table>/_schema.json` — a tiny JSON file
//! that's cheap to read on every run.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredFingerprint {
    pub table: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriftDecision {
    Unchanged,
    Added(Vec<String>),
}

/// Compare a fresh fingerprint against the previously stored one.
/// `previous = None` indicates a first run for this table.
pub fn classify(previous: Option<&StoredFingerprint>, current: &[String]) -> Result<DriftDecision> {
    let Some(prev) = previous else {
        return Ok(DriftDecision::Added(current.to_vec()));
    };
    let prev: BTreeSet<&str> = prev.columns.iter().map(String::as_str).collect();
    let now: BTreeSet<&str> = current.iter().map(String::as_str).collect();
    let removed: Vec<String> = prev.difference(&now).map(|s| (*s).to_string()).collect();
    if !removed.is_empty() {
        bail!(
            "schema drift: column(s) removed or renamed: {removed:?}. \
             Add-only drift is the v1 policy; rename/remove requires a \
             coordinated migration that updates the stored fingerprint."
        );
    }
    let added: Vec<String> = now.difference(&prev).map(|s| (*s).to_string()).collect();
    if added.is_empty() {
        Ok(DriftDecision::Unchanged)
    } else {
        Ok(DriftDecision::Added(added))
    }
}

/// Storage key for the per-table fingerprint sidecar, under its lane
/// ([`crate::snapshot::APPLICATION_LANE`] or
/// [`crate::snapshot::TELEMETRY_LANE`]).
#[must_use]
pub fn fingerprint_key(lane: &str, table: &str) -> String {
    format!("{lane}/{table}/_schema.json")
}

#[cfg(test)]
mod tests {
    use super::{classify, fingerprint_key, DriftDecision, StoredFingerprint};

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn first_run_for_table_is_treated_as_added() {
        let now = cols(&["id", "name"]);
        let got = classify(None, &now).unwrap();
        assert_eq!(got, DriftDecision::Added(cols(&["id", "name"])));
    }

    #[test]
    fn equal_column_sets_are_unchanged() {
        let prev = StoredFingerprint {
            table: "person".into(),
            columns: cols(&["id", "name"]),
        };
        let got = classify(Some(&prev), &cols(&["id", "name"])).unwrap();
        assert_eq!(got, DriftDecision::Unchanged);
    }

    #[test]
    fn added_columns_pass_with_only_the_new_names_reported() {
        let prev = StoredFingerprint {
            table: "person".into(),
            columns: cols(&["id", "name"]),
        };
        let got = classify(Some(&prev), &cols(&["id", "name", "created_at"])).unwrap();
        assert_eq!(got, DriftDecision::Added(cols(&["created_at"])));
    }

    #[test]
    fn removed_columns_fail_loud() {
        let prev = StoredFingerprint {
            table: "person".into(),
            columns: cols(&["id", "name", "email"]),
        };
        let err = classify(Some(&prev), &cols(&["id", "name"])).unwrap_err();
        assert!(format!("{err}").contains("email"), "got {err}");
    }

    #[test]
    fn renamed_column_fails_loud_because_it_looks_like_remove_plus_add() {
        let prev = StoredFingerprint {
            table: "person".into(),
            columns: cols(&["id", "name", "email"]),
        };
        let err = classify(Some(&prev), &cols(&["id", "name", "email_address"])).unwrap_err();
        assert!(format!("{err}").contains("email"), "got {err}");
    }

    #[test]
    fn fingerprint_key_is_deterministic_per_table_and_lane() {
        assert_eq!(
            fingerprint_key("application", "person"),
            "application/person/_schema.json"
        );
        assert_eq!(
            fingerprint_key("application", "entity"),
            "application/entity/_schema.json"
        );
        assert_eq!(
            fingerprint_key("telemetry", "otel_logs"),
            "telemetry/otel_logs/_schema.json"
        );
    }
}
