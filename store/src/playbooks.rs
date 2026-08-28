//! Helpers for the `playbook` table — a client Entity's stored
//! contract-negotiation positions.
//!
//! The `positions` column is JSONB; this module owns the typed view
//! ([`Position`]) and the (de)serialization, so `web` and the
//! contract-review analysis reach a `Vec<Position>` rather than a raw
//! `serde_json::Value`. A playbook is scoped to the client Entity, so one
//! playbook serves every matter for that client.
//!
//! # This table lives in SurrealDB
//!
//! `playbooks` moved with wave five of #1093 (ENG-121), in the
//! playbooks-and-contract-reviews slice. Unlike most tables in this wave,
//! `playbook_entity_name` (its `(entity_id, name)` uniqueness) **is** a real
//! `UNIQUE` index in the Surreal schema, so [`create`] can rely on the
//! engine to refuse a duplicate rather than reading first.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

const TABLE: &str = "playbook";
const ENTITY_TABLE: &str = "entity";

/// Severity a deviation from this position carries: `low` | `medium` |
/// `high`. Descriptive — used to rank findings, not enforced.
pub const SEVERITY_LOW: &str = "low";
pub const SEVERITY_MEDIUM: &str = "medium";
pub const SEVERITY_HIGH: &str = "high";

/// One stored position in a playbook — the firm's stance on a single
/// contract topic, with its fallback and walk-away lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// The contract topic this position governs (e.g. `Limitation of
    /// liability`, `Auto-renewal`, `Governing law`).
    pub topic: String,
    /// The preferred outcome the client wants.
    pub preferred: String,
    /// The acceptable fallback if the preferred outcome is refused.
    pub fallback: String,
    /// The line past which the client should not sign.
    pub walkaway: String,
    /// Severity of a deviation: see the `SEVERITY_*` constants.
    pub severity: String,
}

/// One `playbook` row: a client Entity's stored contract-negotiation
/// positions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Playbook {
    pub id: Uuid,
    pub entity_id: Uuid,
    pub name: String,
    /// The JSONB positions array — see [`positions_of`] for the typed
    /// view.
    pub positions: Json,
    pub active: bool,
    pub inserted_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SurrealValue)]
struct PlaybookRow {
    id: surrealdb::types::RecordId,
    entity_id: surrealdb::types::RecordId,
    name: String,
    positions: Json,
    active: bool,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl PlaybookRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_playbook(self) -> Option<Playbook> {
        Some(Playbook {
            id: record_uuid(&self.id)?,
            entity_id: record_uuid(&self.entity_id)?,
            name: self.name,
            positions: self.positions,
            active: self.active,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const SELECT: &str = "id, entity_id, name, positions, active, inserted_at, updated_at";

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Playbook>, surrealdb::Error> {
    let row: Option<PlaybookRow> = response.take(0)?;
    Ok(row.and_then(PlaybookRow::into_playbook))
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Playbook>, surrealdb::Error> {
    let rows: Vec<PlaybookRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PlaybookRow::into_playbook)
        .collect())
}

/// Why a playbook command refused.
#[derive(Debug, thiserror::Error)]
pub enum PlaybookError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a playbook returned no usable row")]
    WriteReturnedNothing,
    /// The JSON (de)serialization of `positions` failed — a schema/data
    /// drift, never expected at runtime.
    #[error("playbook positions JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// `create` was given a `(entity_id, name)` pair that already has a
    /// playbook — the `playbook_entity_name` unique index refused it.
    #[error("a playbook named `{0}` already exists for this entity")]
    DuplicateName(String),
    /// [`update_positions`] was given an `id` that does not exist.
    #[error("playbook `{0}` not found")]
    NotFound(Uuid),
}

fn is_duplicate_name(error: &surrealdb::Error) -> bool {
    error.to_string().contains("playbook_entity_name")
}

/// What to record for one new playbook.
#[derive(Debug, Clone)]
pub struct NewPlaybook<'a> {
    pub entity_id: Uuid,
    /// Human label, unique per Entity (e.g. `SaaS vendor MSA`).
    pub name: &'a str,
    pub positions: &'a [Position],
}

/// Insert one active `playbook` row, returning its id.
///
/// # Errors
///
/// [`PlaybookError::DuplicateName`] if `(entity_id, name)` already has a
/// playbook, or a database error.
pub async fn create(db: &SurrealDb, new: &NewPlaybook<'_>) -> Result<Uuid, PlaybookError> {
    let positions = serde_json::to_value(new.positions)?;
    let id = Uuid::now_v7();
    match db
        .query(format!(
            "CREATE $id SET \
             entity_id = $entity_id, name = $name, positions = $positions, active = true \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, new.entity_id)))
        .bind(("name", new.name.to_string()))
        .bind(("positions", positions))
        .await
        .and_then(surrealdb::IndexedResults::check)
    {
        Ok(mut response) => {
            let row: Option<PlaybookRow> = response.take(0)?;
            row.and_then(PlaybookRow::into_playbook)
                .map(|p| p.id)
                .ok_or(PlaybookError::WriteReturnedNothing)
        }
        Err(error) if is_duplicate_name(&error) => {
            Err(PlaybookError::DuplicateName(new.name.to_string()))
        }
        Err(error) => Err(PlaybookError::Db(error)),
    }
}

/// Load one playbook by id.
///
/// # Errors
///
/// Propagates any database error.
pub async fn by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Playbook>, PlaybookError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?)
}

/// All playbooks for an Entity, name order.
///
/// # Errors
///
/// Propagates any database error.
pub async fn for_entity(db: &SurrealDb, entity_id: Uuid) -> Result<Vec<Playbook>, PlaybookError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE entity_id = $entity ORDER BY name ASC"
        ))
        .bind(("entity", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(many(response)?)
}

/// Every playbook, name order — the lawyer `/app/admin/playbooks` listing,
/// which is not scoped to one Entity.
///
/// # Errors
///
/// Propagates any database error.
pub async fn all(db: &SurrealDb) -> Result<Vec<Playbook>, PlaybookError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY name ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(many(response)?)
}

/// Replace a playbook's positions (the admin editor saves the whole set).
///
/// # Errors
///
/// [`PlaybookError::NotFound`] when `id` does not exist, or a database
/// error.
pub async fn update_positions(
    db: &SurrealDb,
    id: Uuid,
    positions: &[Position],
) -> Result<(), PlaybookError> {
    let value = serde_json::to_value(positions)?;
    let mut response = db
        .query(format!(
            "UPDATE $id SET positions = $positions, updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("positions", value))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<PlaybookRow> = response.take(0)?;
    row.and_then(PlaybookRow::into_playbook)
        .map(|_| ())
        .ok_or(PlaybookError::NotFound(id))
}

/// The typed positions stored on a playbook row.
///
/// # Errors
///
/// Returns a JSON error if the stored `positions` value is not a
/// `Vec<Position>` (a schema/data drift, never expected at runtime).
pub fn positions_of(playbook: &Playbook) -> Result<Vec<Position>, serde_json::Error> {
    serde_json::from_value(playbook.positions.clone())
}

/// Render a position set into the pipe-delimited form the admin playbook
/// textarea holds — one `topic | preferred | fallback | walk-away | severity`
/// line per position.
///
/// Lives beside [`Position`] because both the page that prefills the textarea
/// (`webapp::playbooks`) and the handler that parses it back
/// (`portal::admin_playbooks::parse_positions`) need the same rendering, and a
/// second copy would be free to drift from the parser it round-trips with.
#[must_use]
pub fn positions_to_text(positions: &[Position]) -> String {
    positions
        .iter()
        .map(|p| {
            format!(
                "{} | {} | {} | {} | {}",
                p.topic, p.preferred, p.fallback, p.walkaway, p.severity
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        by_id, create, for_entity, positions_of, update_positions, NewPlaybook, PlaybookError,
        Position,
    };
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_entity;

    fn position(topic: &str) -> Position {
        Position {
            topic: topic.to_string(),
            preferred: "preferred".to_string(),
            fallback: "fallback".to_string(),
            walkaway: "walkaway".to_string(),
            severity: super::SEVERITY_MEDIUM.to_string(),
        }
    }

    /// The JSONB round trip: a typed `Vec<Position>` goes in, and the same
    /// typed view comes back out.
    #[tokio::test]
    async fn playbook_round_trips_with_typed_positions() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let positions = vec![
            position("Limitation of liability"),
            position("Governing law"),
        ];

        let id = create(
            &surreal,
            &NewPlaybook {
                entity_id,
                name: "SaaS vendor MSA",
                positions: &positions,
            },
        )
        .await
        .expect("create");

        let row = by_id(&surreal, id)
            .await
            .expect("by_id")
            .expect("row exists");
        assert_eq!(row.entity_id, entity_id);
        assert_eq!(row.name, "SaaS vendor MSA");
        assert!(row.active);
        assert_eq!(positions_of(&row).expect("typed positions"), positions);

        let for_this_entity = for_entity(&surreal, entity_id).await.expect("for_entity");
        assert_eq!(for_this_entity.len(), 1);
        assert_eq!(for_this_entity[0].id, id);
    }

    /// The name is unique per Entity — the second playbook of the same name
    /// for the same client is refused, not silently duplicated.
    #[tokio::test]
    async fn playbook_name_is_unique_per_entity() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let positions = vec![position("Auto-renewal")];

        create(
            &surreal,
            &NewPlaybook {
                entity_id,
                name: "Standard MSA",
                positions: &positions,
            },
        )
        .await
        .expect("first create");

        let err = create(
            &surreal,
            &NewPlaybook {
                entity_id,
                name: "Standard MSA",
                positions: &positions,
            },
        )
        .await
        .expect_err("the second playbook of the same name must be refused");
        assert!(matches!(err, PlaybookError::DuplicateName(name) if name == "Standard MSA"));
    }

    /// The admin editor saves the whole position set at once.
    #[tokio::test]
    async fn update_positions_replaces_the_whole_set() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let id = create(
            &surreal,
            &NewPlaybook {
                entity_id,
                name: "Editable MSA",
                positions: &[position("Indemnification")],
            },
        )
        .await
        .expect("create");

        let replacement = vec![position("Assignment"), position("Termination")];
        update_positions(&surreal, id, &replacement)
            .await
            .expect("update");

        let row = by_id(&surreal, id)
            .await
            .expect("by_id")
            .expect("row exists");
        assert_eq!(positions_of(&row).expect("typed positions"), replacement);
    }
}
