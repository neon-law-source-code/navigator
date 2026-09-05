//! The jurisdiction reference table — US states (plus DC) and foreign
//! sovereigns an Entity can be organized under — and every query that
//! reads or writes a `jurisdiction` row.
//!
//! # This table lives in SurrealDB
//!
//! `jurisdictions` moved with its slice of #1093 (ENG-20). It is a leaf
//! lookup table — no outbound references — so the port could not cascade
//! further, and it closes the one cross-engine reference the persons
//! slice knowingly left behind: `credential.jurisdiction_id` is a real
//! `record<jurisdiction>` link now instead of a bare UUID resolved in
//! Rust.
//!
//! # Engine facts this module is shaped around
//!
//! **A unique violation carries no typed detail.** It arrives as
//! [`surrealdb::types::ErrorDetails::Internal`] with the index name in
//! the message, so the shared classifier discriminates on
//! `jurisdiction_code` — a `DEFINE INDEX` identifier this workspace
//! chose in `store/src/schema/navigator.surql`, not prose.
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! Two writers touching one record conflict, the loser is rolled back,
//! and the engine reports a typed `TransactionConflict`; [`writing`]
//! re-runs the statement under the crate's one retry policy,
//! [`crate::surreal::retry`].
//!
//! **`code` needs no stored lowercase twin.** `person.email_lower`
//! exists because an IdP may present any casing of a mailbox; a
//! jurisdiction code is a canonical spelling from
//! `store/seeds/Jurisdiction.yaml` that every lookup matches exactly, so
//! the plain UNIQUE index is the whole constraint.
//!
//! # A link is not validated
//!
//! `entity.jurisdiction_id` is a `record<jurisdiction>` link, and the
//! engine accepts one naming a row that was never written. A caller that
//! needs the jurisdiction behind a link resolves it here, which is what
//! [`find_by_id`] exists for.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "jurisdiction";

/// One jurisdiction.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`JurisdictionRow`] is the seam that turns it into (and back out of)
/// what the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Jurisdiction {
    pub id: Uuid,
    /// Display name (`Nevada`, `United States`).
    pub name: String,
    /// Short canonical code (`NV`, `CA`, `US`). Unique.
    pub code: String,
    /// `state` (US state or DC) or `country` (federal sovereign). The
    /// schema ASSERTs the closed set, so a row cannot carry anything
    /// else — the intake `country` picker filters on this field, and a
    /// typo'd value would otherwise silently vanish from it.
    pub jurisdiction_type: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from
/// [`Jurisdiction`] because the SDK owns its own `RecordId` and
/// `Datetime`, and the conversion belongs at this seam rather than in
/// every caller.
#[derive(SurrealValue)]
struct JurisdictionRow {
    id: surrealdb::types::RecordId,
    name: String,
    code: String,
    jurisdiction_type: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl JurisdictionRow {
    /// `None` when the record id is not a native UUID key — a row
    /// written by something that bypassed [`crate::surreal::record_id`].
    fn into_jurisdiction(self) -> Option<Jurisdiction> {
        Some(Jurisdiction {
            id: record_uuid(&self.id)?,
            name: self.name,
            code: self.code,
            jurisdiction_type: self.jurisdiction_type,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`JurisdictionRow`] from only one
/// query.
const SELECT: &str = "id, name, code, jurisdiction_type, inserted_at, updated_at";

/// Errors reading or writing a jurisdiction.
#[derive(Debug, thiserror::Error)]
pub enum JurisdictionError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `jurisdiction_code` — another row already
    /// holds this code.
    #[error("that jurisdiction code is already in use")]
    CodeTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see
    /// [`JurisdictionRow::into_jurisdiction`].
    #[error("writing a jurisdiction returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator, and it is a `DEFINE INDEX` identifier this workspace
/// chose, through the shared classifier in [`crate::surreal::retry`].
fn classify_write(error: surrealdb::Error) -> JurisdictionError {
    if crate::surreal::retry::unique_violation(&error) == Some("jurisdiction_code") {
        JurisdictionError::CodeTaken
    } else {
        JurisdictionError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, JurisdictionError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// The fields a new jurisdiction row carries.
#[derive(Debug, Clone)]
pub struct NewJurisdiction {
    pub name: String,
    pub code: String,
    /// `state` or `country`; anything else is refused by the schema
    /// ASSERT.
    pub jurisdiction_type: String,
}

impl NewJurisdiction {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        code: impl Into<String>,
        jurisdiction_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            code: code.into(),
            jurisdiction_type: jurisdiction_type.into(),
        }
    }
}

/// Read one jurisdiction out of a query response, dropping a row this
/// module could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Jurisdiction>, JurisdictionError> {
    let row: Option<JurisdictionRow> = response.take(0)?;
    Ok(row.and_then(JurisdictionRow::into_jurisdiction))
}

/// Read every jurisdiction out of a query response, in the order the
/// engine returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Jurisdiction>, JurisdictionError> {
    let rows: Vec<JurisdictionRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(JurisdictionRow::into_jurisdiction)
        .collect())
}

/// Resolve a jurisdiction by id.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if the lookup fails.
pub async fn find_by_id(
    db: &SurrealDb,
    id: Uuid,
) -> Result<Option<Jurisdiction>, JurisdictionError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve a jurisdiction by its canonical `code` (`NV`, `US`). Exact
/// match — codes are canonical spellings, never user-typed.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if the lookup fails.
pub async fn find_by_code(
    db: &SurrealDb,
    code: &str,
) -> Result<Option<Jurisdiction>, JurisdictionError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE code = $code LIMIT 1"
        ))
        .bind(("code", code.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve a jurisdiction by display name (`Nevada`). Exact match — the
/// names are the seed's canonical spellings, and the seed and importer
/// resolve rows by the name their source data carries.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if the lookup fails.
pub async fn find_by_name(
    db: &SurrealDb,
    name: &str,
) -> Result<Option<Jurisdiction>, JurisdictionError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE name = $name LIMIT 1"
        ))
        .bind(("name", name.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every jurisdiction, ordered by name — what a reference picker with no
/// type filter offers.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Jurisdiction>, JurisdictionError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY name ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every jurisdiction of one `jurisdiction_type`, ordered by name — the
/// `country` picker (an N-400's country of birth) filters here so it
/// never offers a US state.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if the lookup fails.
pub async fn list_by_type(
    db: &SurrealDb,
    jurisdiction_type: &str,
) -> Result<Vec<Jurisdiction>, JurisdictionError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE jurisdiction_type = $jurisdiction_type ORDER BY name ASC"
        ))
        .bind(("jurisdiction_type", jurisdiction_type.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Write a new jurisdiction row. The record id is minted from a fresh v7
/// `Uuid` through [`crate::surreal::record_id`], so the key stays the
/// native UUID spelling every cross-engine `jurisdiction_id` still
/// addresses.
///
/// # Errors
///
/// [`JurisdictionError::CodeTaken`] when another row already holds this
/// code, and [`JurisdictionError::Db`] for anything else — including the
/// schema ASSERT refusing a `jurisdiction_type` outside
/// `state`/`country`.
pub async fn create(
    db: &SurrealDb,
    input: &NewJurisdiction,
) -> Result<Jurisdiction, JurisdictionError> {
    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             name = $name, \
             code = $code, \
             jurisdiction_type = $jurisdiction_type \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("name", input.name.trim().to_string()))
        .bind(("code", input.code.trim().to_string()))
        .bind(("jurisdiction_type", input.jurisdiction_type.clone()))
    })
    .await?;

    let row: Option<JurisdictionRow> = response.take(0)?;
    row.and_then(JurisdictionRow::into_jurisdiction)
        .ok_or(JurisdictionError::WriteReturnedNothing)
}

/// Find the jurisdiction holding `input.code`, creating it if absent.
/// Race-safe without a lock: a concurrent creator that wins the
/// `jurisdiction_code` unique index turns this call's insert into
/// [`JurisdictionError::CodeTaken`], which is re-read as the winner's
/// row. The seed runs this on every boot, so idempotence is the
/// contract.
///
/// # Errors
///
/// [`JurisdictionError::Db`] if a lookup or the insert fails.
pub async fn find_or_create(
    db: &SurrealDb,
    input: &NewJurisdiction,
) -> Result<Jurisdiction, JurisdictionError> {
    if let Some(existing) = find_by_code(db, &input.code).await? {
        return Ok(existing);
    }
    match create(db, input).await {
        Ok(created) => Ok(created),
        Err(JurisdictionError::CodeTaken) => find_by_code(db, &input.code)
            .await?
            .ok_or(JurisdictionError::WriteReturnedNothing),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create, find_by_code, find_by_id, find_by_name, find_or_create, list_all, list_by_type,
        JurisdictionError, NewJurisdiction,
    };
    use crate::surreal::test_support::mem;

    fn nevada() -> NewJurisdiction {
        NewJurisdiction::new("Nevada", "NV", "state")
    }

    #[tokio::test]
    async fn a_created_jurisdiction_reads_back_by_id_code_and_name() {
        let db = mem().await;
        let created = create(&db, &nevada()).await.unwrap();
        assert_eq!(created.name, "Nevada");
        assert_eq!(created.code, "NV");
        assert_eq!(created.jurisdiction_type, "state");

        assert_eq!(
            find_by_id(&db, created.id).await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            find_by_code(&db, "NV").await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            find_by_name(&db, "Nevada").await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(find_by_code(&db, "CA").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_duplicate_code_is_reported_as_the_code_being_taken() {
        let db = mem().await;
        create(&db, &nevada()).await.unwrap();

        let duplicate = create(&db, &NewJurisdiction::new("Nevada Again", "NV", "state")).await;
        assert!(
            matches!(duplicate, Err(JurisdictionError::CodeTaken)),
            "the unique `jurisdiction_code` index is the gate, got {duplicate:?}"
        );
    }

    #[tokio::test]
    async fn find_or_create_is_idempotent_on_the_code() {
        let db = mem().await;
        let first = find_or_create(&db, &nevada()).await.unwrap();
        let second = find_or_create(&db, &nevada()).await.unwrap();
        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(list_all(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_jurisdiction_type_outside_the_closed_set_is_refused() {
        let db = mem().await;
        let refused = create(&db, &NewJurisdiction::new("Atlantis", "AT", "myth")).await;
        assert!(
            matches!(refused, Err(JurisdictionError::Db(_))),
            "the schema ASSERT must reject an unknown jurisdiction_type, got {refused:?}"
        );
    }

    #[tokio::test]
    async fn listing_filters_by_type_and_orders_by_name() {
        let db = mem().await;
        for (name, code, kind) in [
            ("Nevada", "NV", "state"),
            ("Germany", "DE", "country"),
            ("California", "CA", "state"),
            ("United States", "US", "country"),
        ] {
            create(&db, &NewJurisdiction::new(name, code, kind))
                .await
                .unwrap();
        }

        let states: Vec<String> = list_by_type(&db, "state")
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.name)
            .collect();
        assert_eq!(states, ["California", "Nevada"]);

        let countries: Vec<String> = list_by_type(&db, "country")
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.name)
            .collect();
        assert_eq!(countries, ["Germany", "United States"]);

        let all: Vec<String> = list_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.name)
            .collect();
        assert_eq!(all, ["California", "Germany", "Nevada", "United States"]);
    }
}
