//! The `entity_types` reference table — the kinds of legal entity a
//! firm forms (LLC, Corporation, Trust, …) — and every query that reads
//! or writes an `entity_type` row.
//!
//! # This table lives in SurrealDB
//!
//! `entity_types` is a leaf lookup table — one `name` column, no
//! outbound references. `entity.entity_type_id` is a
//! `record<entity_type>` link into it, and the engine does not validate a
//! link, so `entity_commands::require_entity_type` reads a row
//! back here before any write that stores one, which is what
//! [`find_by_id`] exists for.
//!
//! # Engine facts this module is shaped around
//!
//! **A unique violation carries no typed detail.** It arrives as
//! [`surrealdb::types::ErrorDetails::Internal`] with the index name in
//! the message, so the shared classifier discriminates on
//! `entity_type_name` — a `DEFINE INDEX` identifier this workspace
//! chose in `store/src/schema/navigator.surql`, not prose.
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! [`writing`] re-runs the statement under the crate's one retry policy,
//! [`crate::surreal::retry`].
//!
//! **`name` needs no stored lowercase twin.** The names are canonical
//! spellings from `store/seeds/EntityType.yaml` that every lookup
//! matches exactly, so the plain UNIQUE index is the whole constraint.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "entity_type";

/// One entity type.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`EntityTypeRow`] is the seam that turns it into (and back out of)
/// what the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityType {
    pub id: Uuid,
    /// Display name (`LLC`, `Trust`, `Human`). Unique.
    pub name: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from
/// [`EntityType`] because the SDK owns its own `RecordId` and
/// `Datetime`, and the conversion belongs at this seam rather than in
/// every caller.
#[derive(SurrealValue)]
struct EntityTypeRow {
    id: surrealdb::types::RecordId,
    name: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl EntityTypeRow {
    /// `None` when the record id is not a native UUID key — a row
    /// written by something that bypassed [`crate::surreal::record_id`].
    fn into_entity_type(self) -> Option<EntityType> {
        Some(EntityType {
            id: record_uuid(&self.id)?,
            name: self.name,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`EntityTypeRow`] from only one query.
const SELECT: &str = "id, name, inserted_at, updated_at";

/// Errors reading or writing an entity type.
#[derive(Debug, thiserror::Error)]
pub enum EntityTypeError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `entity_type_name` — another row already
    /// holds this name.
    #[error("that entity type name is already in use")]
    NameTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see
    /// [`EntityTypeRow::into_entity_type`].
    #[error("writing an entity type returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator, identified by the shared classifier in
/// [`crate::surreal::retry`].
fn classify_write(error: surrealdb::Error) -> EntityTypeError {
    if crate::surreal::retry::unique_violation(&error) == Some("entity_type_name") {
        EntityTypeError::NameTaken
    } else {
        EntityTypeError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, EntityTypeError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Read one entity type out of a query response, dropping a row this
/// module could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<EntityType>, EntityTypeError> {
    let row: Option<EntityTypeRow> = response.take(0)?;
    Ok(row.and_then(EntityTypeRow::into_entity_type))
}

/// Read every entity type out of a query response, in the order the
/// engine returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<EntityType>, EntityTypeError> {
    let rows: Vec<EntityTypeRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(EntityTypeRow::into_entity_type)
        .collect())
}

/// Resolve an entity type by id — the read-back
/// `entity_commands::require_entity_type` performs before an entity
/// write stores the cross-engine reference.
///
/// # Errors
///
/// [`EntityTypeError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<EntityType>, EntityTypeError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve an entity type by its canonical `name` (`LLC`, `Human`).
/// Exact match — the names are the seed's canonical spellings.
///
/// # Errors
///
/// [`EntityTypeError::Db`] if the lookup fails.
pub async fn find_by_name(
    db: &SurrealDb,
    name: &str,
) -> Result<Option<EntityType>, EntityTypeError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE name = $name LIMIT 1"
        ))
        .bind(("name", name.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// List every entity type, ordered by the JSON:API `sort` spec (`(key,
/// descending)` pairs). The only advertised key is `name`; an empty
/// spec — or one that names no sortable column — falls back to
/// ascending name so the list is always stably ordered.
///
/// # Errors
///
/// [`EntityTypeError::Db`] if the lookup fails.
pub async fn list(
    db: &SurrealDb,
    sort: &[(String, bool)],
) -> Result<Vec<EntityType>, EntityTypeError> {
    let descending = sort
        .iter()
        .rev()
        .find_map(|(key, descending)| (key == "name").then_some(*descending))
        .unwrap_or(false);
    let direction = if descending { "DESC" } else { "ASC" };
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY name {direction}"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Write a new entity type row. The record id is minted from a fresh v7
/// `Uuid` through [`crate::surreal::record_id`], so the key stays the
/// native UUID spelling the cross-engine `entity_type_id` still
/// addresses.
///
/// # Errors
///
/// [`EntityTypeError::NameTaken`] when another row already holds this
/// name, and [`EntityTypeError::Db`] for anything else.
pub async fn create(db: &SurrealDb, name: &str) -> Result<EntityType, EntityTypeError> {
    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!("CREATE $id SET name = $name RETURN {SELECT}"))
            .bind(("id", record_id(TABLE, id)))
            .bind(("name", name.trim().to_string()))
    })
    .await?;

    let row: Option<EntityTypeRow> = response.take(0)?;
    row.and_then(EntityTypeRow::into_entity_type)
        .ok_or(EntityTypeError::WriteReturnedNothing)
}

/// Find the entity type holding `name`, creating it if absent.
/// Race-safe without a lock: a concurrent creator that wins the
/// `entity_type_name` unique index turns this call's insert into
/// [`EntityTypeError::NameTaken`], which is re-read as the winner's
/// row. The seed runs this on every boot, so idempotence is the
/// contract.
///
/// # Errors
///
/// [`EntityTypeError::Db`] if a lookup or the insert fails.
pub async fn find_or_create(db: &SurrealDb, name: &str) -> Result<EntityType, EntityTypeError> {
    if let Some(existing) = find_by_name(db, name).await? {
        return Ok(existing);
    }
    match create(db, name).await {
        Ok(created) => Ok(created),
        Err(EntityTypeError::NameTaken) => find_by_name(db, name)
            .await?
            .ok_or(EntityTypeError::WriteReturnedNothing),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{create, find_by_id, find_by_name, find_or_create, list, EntityTypeError};
    use crate::surreal::test_support::mem;

    #[tokio::test]
    async fn a_created_entity_type_reads_back_by_id_and_name() {
        let db = mem().await;
        let created = create(&db, "LLC").await.unwrap();
        assert_eq!(created.name, "LLC");

        assert_eq!(
            find_by_id(&db, created.id).await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            find_by_name(&db, "LLC").await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(find_by_name(&db, "Trust").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_duplicate_name_is_reported_as_the_name_being_taken() {
        let db = mem().await;
        create(&db, "LLC").await.unwrap();

        let duplicate = create(&db, "LLC").await;
        assert!(
            matches!(duplicate, Err(EntityTypeError::NameTaken)),
            "the unique `entity_type_name` index is the gate, got {duplicate:?}"
        );
    }

    #[tokio::test]
    async fn find_or_create_is_idempotent_on_the_name() {
        let db = mem().await;
        let first = find_or_create(&db, "Trust").await.unwrap();
        let second = find_or_create(&db, "Trust").await.unwrap();
        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(list(&db, &[]).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn listing_orders_by_name_and_honours_the_sort_spec() {
        let db = mem().await;
        for name in ["Trust", "Corporation", "LLC"] {
            create(&db, name).await.unwrap();
        }

        let names = |rows: Vec<super::EntityType>| -> Vec<String> {
            rows.into_iter().map(|t| t.name).collect()
        };

        // Empty spec → ascending name.
        assert_eq!(
            names(list(&db, &[]).await.unwrap()),
            ["Corporation", "LLC", "Trust"]
        );
        // Descending on the advertised key.
        assert_eq!(
            names(list(&db, &[("name".into(), true)]).await.unwrap()),
            ["Trust", "LLC", "Corporation"]
        );
        // A spec naming no sortable column falls back to ascending name.
        assert_eq!(
            names(list(&db, &[("bogus".into(), true)]).await.unwrap()),
            ["Corporation", "LLC", "Trust"]
        );
    }
}
