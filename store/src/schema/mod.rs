//! The SurrealDB schema, applied rather than migrated.
//!
//! # Schema, not history
//!
//! The schema is a *statement of the present*
//! ([`navigator.surql`](https://github.com/neon-law-source-code/navigator/blob/main/store/src/schema/navigator.surql)):
//! one idempotent file describing the tables and fields that should
//! exist, applied whole by [`apply`] on every boot and by every test
//! that opens an embedded engine. The current shape is what the file
//! says, not what a chain of steps left behind.
//!
//! A describable present costs replayable history, which is why
//! [`SCHEMA_VERSION`] exists. Applying the file converges a database's
//! *definitions*, but it cannot perform a data change (a backfill, a
//! column split), and `DEFINE TABLE IF NOT EXISTS` deliberately leaves
//! an existing table's rows alone. The version record is what lets a
//! process notice it is looking at a database some other version of
//! the code prepared, rather than discovering it one confusing query
//! at a time. Backfills stay explicit one-shot jobs (#1093).
//!
//! Bump [`SCHEMA_VERSION`] in the same change that edits the `.surql`
//! file.

use std::collections::BTreeMap;

use surrealdb::types::{ErrorDetails, NotFoundError};
use surrealdb::Error as SurrealQueryError;
use thiserror::Error;

use crate::surreal::SurrealDb;

/// The version this build of Navigator applies. Bump it whenever
/// `navigator.surql` changes so a database prepared by another build
/// reports as drifted instead of silently disagreeing.
pub const SCHEMA_VERSION: u32 = 23;

/// The table holding the applied version.
const VERSION_TABLE: &str = "schema_version";

/// The record holding the applied version — exactly one, always.
const VERSION_RECORD: &str = "schema_version:current";

/// The schema itself. Embedded rather than read from disk so a
/// deployed binary carries its own schema and cannot be pointed at a
/// stale copy on a volume.
const DEFINITIONS: &str = include_str!("navigator.surql");

/// Every table declared in the shipped Surreal schema, in stable order.
///
/// Consumers that must cover the whole operational store (rather than a
/// hand-maintained subset) use this list. The schema remains the sole source
/// of truth: adding a `DEFINE TABLE` makes it visible to those consumers in
/// the same build.
#[must_use]
pub fn table_names() -> Vec<String> {
    let mut tables: Vec<String> = DEFINITIONS
        .lines()
        .filter_map(|line| line.trim().strip_prefix("DEFINE TABLE "))
        .filter_map(|line| {
            let words: Vec<_> = line.split_whitespace().collect();
            if words.starts_with(&["IF", "NOT", "EXISTS"]) {
                words.get(3).copied()
            } else {
                words.first().copied()
            }
        })
        .map(ToOwned::to_owned)
        .collect();
    tables.sort_unstable();
    tables.dedup();
    tables
}

/// How a database's applied schema compares to this build's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    /// No schema has been applied — a fresh database.
    Absent,
    /// The applied version is this build's.
    InSync,
    /// Some other build prepared this database. `installed` may be
    /// older (this build is ahead) or newer (this build is behind);
    /// both are the same fact to the caller — the code and the
    /// database disagree.
    Drifted { installed: u32, expected: u32 },
}

/// One table as the engine describes it: its `DEFINE TABLE` statement
/// and its fields' `DEFINE FIELD` statements, keyed by field name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDefinition {
    pub definition: String,
    pub fields: BTreeMap<String, String>,
}

/// Every table in the database, keyed by name. Ordered, so a consumer
/// that renders it (`navigator db erd`) is deterministic by
/// construction.
pub type Introspection = BTreeMap<String, TableDefinition>;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("apply the Surreal schema definitions")]
    Apply(#[source] SurrealQueryError),
    #[error("record the applied schema version")]
    RecordVersion(#[source] SurrealQueryError),
    #[error("read the applied schema version")]
    ReadVersion(#[source] SurrealQueryError),
    #[error("the applied schema version is {0}, which is not a version number")]
    UnreadableVersion(i64),
    #[error("read the applied schema back from the engine")]
    Introspect(#[source] SurrealQueryError),
}

/// Apply the schema and record [`SCHEMA_VERSION`].
///
/// Idempotent: running it against an already-prepared database
/// converges every definition and leaves the rows untouched. Callers
/// run it at boot without checking [`state`] first.
pub async fn apply(db: &SurrealDb) -> Result<(), SchemaError> {
    db.query(DEFINITIONS)
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(SchemaError::Apply)?;

    db.query(format!(
        "UPSERT {VERSION_RECORD} SET version = $version, applied_at = time::now()"
    ))
    .bind(("version", i64::from(SCHEMA_VERSION)))
    .await
    .and_then(surrealdb::IndexedResults::check)
    .map_err(SchemaError::RecordVersion)?;

    Ok(())
}

/// The version recorded in `db`, or `None` when no schema has been
/// applied.
pub async fn installed_version(db: &SurrealDb) -> Result<Option<u32>, SchemaError> {
    let response = db
        .query(format!("SELECT VALUE version FROM {VERSION_RECORD}"))
        .await
        .and_then(surrealdb::IndexedResults::check);

    let recorded: Option<i64> = match response {
        Ok(mut response) => response.take(0).map_err(SchemaError::ReadVersion)?,
        // A never-applied database has no `schema_version` table at
        // all, and selecting from an undefined table is an error here
        // rather than an empty result. That error IS the absent
        // answer, so it is matched structurally — on the engine's typed
        // `NotFound(Table)` detail, never on message text — and any
        // other failure still surfaces.
        Err(err) if is_undefined_table(&err, VERSION_TABLE) => return Ok(None),
        Err(err) => return Err(SchemaError::ReadVersion(err)),
    };

    recorded
        .map(|version| u32::try_from(version).map_err(|_| SchemaError::UnreadableVersion(version)))
        .transpose()
}

/// Whether `err` is the engine reporting that `table` has never been
/// defined.
fn is_undefined_table(err: &SurrealQueryError, table: &str) -> bool {
    matches!(
        err.details(),
        ErrorDetails::NotFound(Some(NotFoundError::Table { name })) if name == table
    )
}

/// Read the applied schema back out of the engine.
///
/// The engine is the authority here, not this crate's `.surql` file:
/// a diagram or a drift report has to describe the database that
/// actually exists. Surreal keeps a field's reference in its *type*
/// (`record<entity>`), so these definitions carry both the columns and
/// the relationships in one read.
pub async fn introspect(db: &SurrealDb) -> Result<Introspection, SchemaError> {
    // `take` deserializes one statement's result; the projection above
    // yields a single object, hence `Option<..>`.
    let tables: Option<BTreeMap<String, String>> = db
        .query("RETURN (INFO FOR DB).tables")
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(SchemaError::Introspect)?
        .take(0)
        .map_err(SchemaError::Introspect)?;

    let mut introspection = Introspection::new();
    for (name, definition) in tables.unwrap_or_default() {
        let fields: Option<BTreeMap<String, String>> = db
            .query(format!("RETURN (INFO FOR TABLE {name}).fields"))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .map_err(SchemaError::Introspect)?
            .take(0)
            .map_err(SchemaError::Introspect)?;
        introspection.insert(
            name,
            TableDefinition {
                definition,
                fields: fields.unwrap_or_default(),
            },
        );
    }
    Ok(introspection)
}

/// Compare `db`'s applied schema to this build's.
pub async fn state(db: &SurrealDb) -> Result<SchemaState, SchemaError> {
    Ok(match installed_version(db).await? {
        None => SchemaState::Absent,
        Some(installed) if installed == SCHEMA_VERSION => SchemaState::InSync,
        Some(installed) => SchemaState::Drifted {
            installed,
            expected: SCHEMA_VERSION,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{
        apply, installed_version, introspect, state, table_names, SchemaState, DEFINITIONS,
        SCHEMA_VERSION, VERSION_RECORD,
    };
    use crate::surreal::test_support::unmigrated;

    /// #1145: Navigator's authorization stays above the database, so
    /// every table lands `PERMISSIONS NONE`. That is also the engine's
    /// default, which is exactly the risk — an omitted clause and a
    /// deliberate one would be indistinguishable. This asserts the
    /// clause is written, not which clause it is, so the day a table
    /// wants `PERMISSIONS FOR select ...` it passes unchanged and only
    /// a silent default fails.
    #[test]
    fn every_defined_table_states_its_permissions() {
        for (file, definitions) in [("navigator.surql", DEFINITIONS)] {
            let tables: Vec<&str> = definitions
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with("DEFINE TABLE"))
                .collect();
            assert!(!tables.is_empty(), "no DEFINE TABLE statements in {file}");

            for statement in tables {
                assert!(
                    statement.contains(" PERMISSIONS "),
                    "no PERMISSIONS clause in {file}: {statement}"
                );
            }
        }
    }

    #[tokio::test]
    async fn answer_notation_id_is_a_nullable_notation_link() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();

        let fields = introspect(&db).await.unwrap();
        let notation_id = &fields["answer"].fields["notation_id"];
        assert!(notation_id.contains("record<notation>"), "{notation_id}");
        assert!(!notation_id.contains("uuid"), "{notation_id}");
    }

    #[tokio::test]
    async fn relationship_source_id_is_a_nullable_union_link() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();

        let fields = introspect(&db).await.unwrap();
        let source_id = &fields["relationship"].fields["source_id"];
        assert!(
            source_id.contains("record<relationship_log | disclosure>"),
            "{source_id}"
        );
        assert!(!source_id.contains("uuid"), "{source_id}");
    }

    #[test]
    fn table_names_cover_the_shipped_schema_in_stable_order() {
        let tables = table_names();
        assert!(tables.contains(&"schema_version".to_string()));
        assert!(tables.contains(&"person".to_string()));
        let mut sorted = tables.clone();
        sorted.sort_unstable();
        assert_eq!(tables, sorted);
    }

    #[tokio::test]
    async fn an_empty_database_reports_absent_and_applying_makes_it_in_sync() {
        let db = unmigrated().await;

        assert_eq!(state(&db).await.unwrap(), SchemaState::Absent);
        assert_eq!(installed_version(&db).await.unwrap(), None);

        apply(&db).await.unwrap();

        assert_eq!(state(&db).await.unwrap(), SchemaState::InSync);
        assert_eq!(installed_version(&db).await.unwrap(), Some(SCHEMA_VERSION));
    }

    /// Every boot re-applies, so the second apply must be a no-op that
    /// neither errors nor multiplies the version record.
    #[tokio::test]
    async fn applying_twice_succeeds_and_leaves_one_version_record() {
        let db = unmigrated().await;

        apply(&db).await.unwrap();
        apply(&db).await.unwrap();

        let records: Vec<i64> = db
            .query("SELECT VALUE version FROM schema_version")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(records, vec![i64::from(SCHEMA_VERSION)]);
        assert_eq!(state(&db).await.unwrap(), SchemaState::InSync);
    }

    /// ENG-119: a participation is one person's current scope on one
    /// Project. The natural-key uniqueness moves with the table so a second
    /// assignment cannot create competing authorization answers.
    #[tokio::test]
    async fn projects_cluster_schema_rejects_a_duplicate_participation() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();

        db.query(
            "CREATE person:lawyer SET name = 'Lawyer', email = 'lawyer@example.com', \
             role = 'lawyer'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let entity_id = uuid::Uuid::parse_str("0198a36a-55cc-7fd0-8af7-4f30e72b761c").unwrap();
        db.query(
            "CREATE project:matter SET code = 'matter', name = 'Matter', status = 'open', \
             entity_id = $entity_id, \
             inserted_at = '2026-08-04T00:00:00Z', updated_at = '2026-08-04T00:00:00Z'",
        )
        .bind(("entity_id", crate::surreal::record_id("entity", entity_id)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "CREATE person_project_role:one SET person_id = person:lawyer, \
             project_id = project:matter, participation = 'attorney', \
             inserted_at = '2026-08-04T00:00:00Z', updated_at = '2026-08-04T00:00:00Z'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let duplicate = db
            .query(
                "CREATE person_project_role:two SET person_id = person:lawyer, \
                 project_id = project:matter, participation = 'paralegal', \
                 inserted_at = '2026-08-04T00:00:00Z', updated_at = '2026-08-04T00:00:00Z'",
            )
            .await
            .unwrap()
            .check();
        assert!(duplicate.is_err(), "duplicate participation was accepted");
    }

    /// Re-applying must converge definitions without touching rows —
    /// the property `DEFINE TABLE IF NOT EXISTS` is chosen for.
    #[tokio::test]
    async fn re_applying_preserves_the_rows_already_there() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        db.query("CREATE person:alice SET name = 'Alice', email = 'alice@example.com'")
            .await
            .unwrap()
            .check()
            .unwrap();

        apply(&db).await.unwrap();

        let name: Option<String> = db
            .query("SELECT VALUE name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(name.as_deref(), Some("Alice"));
    }

    #[tokio::test]
    async fn a_database_another_build_prepared_reports_drifted_in_both_directions() {
        for installed in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
            let db = unmigrated().await;
            apply(&db).await.unwrap();
            db.query(format!("UPSERT {VERSION_RECORD} SET version = $version"))
                .bind(("version", i64::from(installed)))
                .await
                .unwrap()
                .check()
                .unwrap();

            assert_eq!(
                state(&db).await.unwrap(),
                SchemaState::Drifted {
                    installed,
                    expected: SCHEMA_VERSION,
                },
                "installed version {installed}"
            );
        }
    }

    /// Seed one person and one entity as the *deployment* schema
    /// defines them — full rows, not the name-only nodes the retired
    /// projection used. Both reference links point at rows that were
    /// never written, which the engine accepts: a `record<T>` link is a
    /// type constraint, not a foreign key, and nothing here needs the
    /// entity type or jurisdiction resolved.
    async fn graph_nodes(db: &crate::surreal::SurrealDb) {
        db.query(
            "CREATE person:alice SET name = 'Alice', email = 'alice@example.com';
             CREATE entity:acme SET name = 'Acme LLC',
                 entity_type_id = entity_type:llc, jurisdiction_id = jurisdiction:nv;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
    }

    /// The conflict graph is the deployment schema now (ENG-120), not a
    /// projection of it: `store::conflicts` traverses these two edge
    /// tables on the configured connection. So the relate-and-traverse
    /// that used to prove the *projection* works has to prove it here,
    /// against the rows the application actually writes.
    #[tokio::test]
    async fn the_conflicts_graph_relates_and_traverses() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        graph_nodes(&db).await;

        db.query("RELATE person:alice->entity_role->entity:acme SET role = 'owner'")
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query(
            "RELATE person:alice->relationship->entity:acme \
             SET kind = 'adverse', confidence_pct = 90, \
                 source_kind = 'manual', detail = 'disclosed'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let owned: Option<Vec<String>> = db
            .query("SELECT VALUE ->entity_role->entity.name FROM person:alice")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(owned, Some(vec!["Acme LLC".to_string()]));
    }

    /// An ASSERT bounds `relationship_edges.confidence_pct`, so a bad
    /// confidence is rejected by the engine rather than by whichever
    /// caller remembers to look.
    #[tokio::test]
    async fn an_out_of_range_confidence_is_rejected_by_the_schema() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        graph_nodes(&db).await;

        let rejected = db
            .query(
                "RELATE person:alice->relationship->entity:acme \
                 SET kind = 'adverse', confidence_pct = 101, source_kind = 'manual'",
            )
            .await
            .unwrap()
            .check();

        assert!(rejected.is_err(), "the engine accepted confidence_pct 101");
    }

    /// The firm anchor's protection is a schema guarantee. SurrealDB has
    /// no advisory lock, so the
    /// invariant is carried by a UNIQUE index over a key only anchor
    /// rows populate. This pins both halves: the fork is refused, and
    /// ordinary namesakes — which leave the key NONE — still coexist,
    /// because multiple NONEs do not collide on a Surreal unique index.
    #[tokio::test]
    async fn the_firm_anchor_key_refuses_a_fork_but_admits_namesakes() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();

        let anchor = |id: &str, key: &str| {
            format!(
                "CREATE entity:{id} SET name = 'Neon Law', \
                 entity_type_id = entity_type:llc, jurisdiction_id = jurisdiction:nv, \
                 firm_anchor_key = '{key}'"
            )
        };
        db.query(anchor("firm", "shook law pllc"))
            .await
            .unwrap()
            .check()
            .unwrap();

        let forked = db
            .query(anchor("fork", "shook law pllc"))
            .await
            .unwrap()
            .check();
        assert!(
            forked.is_err(),
            "a second row under the anchor's key must be refused by `entity_firm_anchor`"
        );

        // Two unrelated Betas both land: entity names are deliberately
        // non-unique, and only the anchor takes the key.
        db.query(
            "CREATE entity:beta_one SET name = 'Beta LLC',
                 entity_type_id = entity_type:llc, jurisdiction_id = jurisdiction:nv;
             CREATE entity:beta_two SET name = 'Beta LLC',
                 entity_type_id = entity_type:llc, jurisdiction_id = jurisdiction:nv;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
    }
}
