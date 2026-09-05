//! `notarizations` — the notary counterpart to [`crate::signatures`], and
//! every query against the table.
//!
//! # This table lives in SurrealDB
//!
//! `notarizations` moved with wave five of #1093 (ENG-121), in the
//! satellite-ring slice.
//!
//! A Notation's document sent for remote online notarization records a row
//! here, correlated back from the provider by `(provider, provider_id)` —
//! unique, the callback's correlation key. `notary_person_id`/`asset_id`
//! name who notarized what (null until known); `notarized_at` is stamped on
//! completion.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

pub use crate::signatures::SignatureProvider;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "notarization";
const ASSET_TABLE: &str = "asset";

/// One notarization request/execution.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`NotarizationRow`] is the seam that turns it into (and back out of)
/// what the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notarization {
    pub id: Uuid,
    pub notation_id: Uuid,
    pub notary_person_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    /// The provider's stored string form — see [`SignatureProvider::as_str`].
    pub provider: String,
    pub provider_id: String,
    pub notarized_at: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct NotarizationRow {
    id: surrealdb::types::RecordId,
    notation_id: surrealdb::types::RecordId,
    notary_person_id: Option<surrealdb::types::RecordId>,
    asset_id: Option<surrealdb::types::RecordId>,
    provider: String,
    provider_id: String,
    notarized_at: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl NotarizationRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_notarization(self) -> Option<Notarization> {
        Some(Notarization {
            id: record_uuid(&self.id)?,
            notation_id: record_uuid(&self.notation_id)?,
            notary_person_id: self.notary_person_id.as_ref().and_then(record_uuid),
            asset_id: self.asset_id.as_ref().and_then(record_uuid),
            provider: self.provider,
            provider_id: self.provider_id,
            notarized_at: self.notarized_at,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`NotarizationRow`] from only one query.
const SELECT: &str = "id, notation_id, notary_person_id, asset_id, provider, provider_id, \
     notarized_at, inserted_at, updated_at";

/// Errors reading or writing a notarization.
#[derive(Debug, thiserror::Error)]
pub enum NotarizationError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a notarization returned no usable row")]
    WriteReturnedNothing,
}

fn is_provider_request_conflict(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("notarization_provider_request")
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, surrealdb::Error>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Notarization>, surrealdb::Error> {
    let row: Option<NotarizationRow> = response.take(0)?;
    Ok(row.and_then(NotarizationRow::into_notarization))
}

/// Record the provider's request id for a Notation's notarization.
/// Idempotent on `(provider, provider_id)`: a concurrent double-send loses
/// the `notarization_provider_request` unique index and re-reads the
/// winner's row.
///
/// # Errors
///
/// [`NotarizationError::Db`] if the write fails.
pub async fn record_request(
    db: &SurrealDb,
    notation_id: Uuid,
    provider: SignatureProvider,
    provider_id: &str,
) -> Result<Notarization, NotarizationError> {
    let id = Uuid::now_v7();
    match db
        .query(format!(
            "CREATE $id SET \
             notation_id = $notation_id, provider = $provider, provider_id = $provider_id \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "notation_id",
            record_id(crate::notations::TABLE, notation_id),
        ))
        .bind(("provider", provider.as_str().to_string()))
        .bind(("provider_id", provider_id.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)
    {
        Ok(mut response) => {
            let row: Option<NotarizationRow> = response.take(0)?;
            row.and_then(NotarizationRow::into_notarization)
                .ok_or(NotarizationError::WriteReturnedNothing)
        }
        Err(error) if is_provider_request_conflict(&error) => {
            by_provider(db, provider, provider_id)
                .await?
                .ok_or(NotarizationError::WriteReturnedNothing)
        }
        Err(error) => Err(NotarizationError::Db(error)),
    }
}

/// The notarization row for `(provider, provider_id)`, if any.
///
/// # Errors
///
/// [`NotarizationError::Db`] if the lookup fails.
pub async fn by_provider(
    db: &SurrealDb,
    provider: SignatureProvider,
    provider_id: &str,
) -> Result<Option<Notarization>, NotarizationError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE provider = $provider AND provider_id = $provider_id \
             LIMIT 1"
        ))
        .bind(("provider", provider.as_str().to_string()))
        .bind(("provider_id", provider_id.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?)
}

/// Stamp `notarized_at` when the provider reports completion. Returns
/// `false` for an unknown request.
///
/// # Errors
///
/// [`NotarizationError::Db`] if the lookup or write fails.
pub async fn stamp_notarized(
    db: &SurrealDb,
    provider: SignatureProvider,
    provider_id: &str,
    notarized_at: &str,
) -> Result<bool, NotarizationError> {
    let Some(row) = by_provider(db, provider, provider_id).await? else {
        return Ok(false);
    };
    writing(|| {
        db.query("UPDATE $id SET notarized_at = $notarized_at, updated_at = time::now()")
            .bind(("id", record_id(TABLE, row.id)))
            .bind(("notarized_at", notarized_at.to_string()))
    })
    .await?;
    Ok(true)
}

/// Point a notarization request at the notarized asset, once known.
///
/// # Errors
///
/// [`NotarizationError::Db`] if the write fails.
pub async fn set_asset(db: &SurrealDb, id: Uuid, asset_id: Uuid) -> Result<(), NotarizationError> {
    writing(|| {
        db.query("UPDATE $id SET asset_id = $asset_id, updated_at = time::now()")
            .bind(("id", record_id(TABLE, id)))
            .bind(("asset_id", record_id(ASSET_TABLE, asset_id)))
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_notation;

    #[tokio::test]
    async fn record_is_idempotent_and_stamp_marks_notarized() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let first = record_request(&surreal, notation_id, SignatureProvider::DocuSign, "nz-1")
            .await
            .unwrap();
        let again = record_request(&surreal, notation_id, SignatureProvider::DocuSign, "nz-1")
            .await
            .unwrap();
        assert_eq!(first.id, again.id);
        assert!(stamp_notarized(
            &surreal,
            SignatureProvider::DocuSign,
            "nz-1",
            "2026-06-30T00:00:00Z"
        )
        .await
        .unwrap());
        let after = by_provider(&surreal, SignatureProvider::DocuSign, "nz-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.notarized_at.as_deref(), Some("2026-06-30T00:00:00Z"));
    }
}
