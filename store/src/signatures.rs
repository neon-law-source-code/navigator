//! `signatures` — one signature request/execution on a Notation's document,
//! correlated back from the provider by `(provider, provider_id)`, and
//! every query against the table.
//!
//! # This table lives in SurrealDB
//!
//! `signatures` moved with wave five of #1093 (ENG-121), in the
//! satellite-ring slice.
//!
//! A Notation's document is sent to an e-signature provider (DocuSign); the
//! provider issues an opaque request id (an envelope id). This row records
//! that request so the inbound completion webhook
//! (`portal::esignature_webhook`) can resolve a callback back to its
//! Notation by matching `(provider, provider_id)` — the pair is unique.
//! `signer_person_id`/`field` name who signs where (null until known);
//! `signed_at` is stamped when the provider reports completion.
//!
//! `provider` was a SeaORM `DeriveActiveEnum` over `TEXT`; on this engine it
//! is a plain stored string, and [`SignatureProvider`] is the closed set
//! this module enforces at the Rust boundary instead of a column type.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
pub(crate) const TABLE: &str = "signature";

/// The e-signature (and, for [`crate::notarizations`], notarization)
/// provider that executed a request. A closed set keeps call sites and the
/// webhook from inventing provider strings; today the firm signs
/// exclusively through DocuSign.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureProvider {
    /// DocuSign — the production e-signature seam (`portal::signature`).
    DocuSign,
}

impl SignatureProvider {
    /// String form stored in the `provider` column and matched by the
    /// completion webhook.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DocuSign => "docusign",
        }
    }
}

/// One signature request/execution.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`SignatureRow`] is the seam that turns it into (and back out of) what
/// the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub id: Uuid,
    pub notation_id: Uuid,
    pub signer_person_id: Option<Uuid>,
    pub field: Option<String>,
    /// The provider's stored string form — see [`SignatureProvider::as_str`].
    pub provider: String,
    pub provider_id: String,
    pub signed_at: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct SignatureRow {
    id: surrealdb::types::RecordId,
    notation_id: surrealdb::types::RecordId,
    signer_person_id: Option<surrealdb::types::RecordId>,
    field: Option<String>,
    provider: String,
    provider_id: String,
    signed_at: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl SignatureRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_signature(self) -> Option<Signature> {
        Some(Signature {
            id: record_uuid(&self.id)?,
            notation_id: record_uuid(&self.notation_id)?,
            signer_person_id: self.signer_person_id.as_ref().and_then(record_uuid),
            field: self.field,
            provider: self.provider,
            provider_id: self.provider_id,
            signed_at: self.signed_at,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`SignatureRow`] from only one query.
const SELECT: &str = "id, notation_id, signer_person_id, field, provider, provider_id, \
     signed_at, inserted_at, updated_at";

/// Errors reading or writing a signature.
#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a signature returned no usable row")]
    WriteReturnedNothing,
}

fn is_provider_request_conflict(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("signature_provider_request")
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

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Signature>, surrealdb::Error> {
    let row: Option<SignatureRow> = response.take(0)?;
    Ok(row.and_then(SignatureRow::into_signature))
}

/// Record the provider's request id for a Notation when the envelope is
/// created. Idempotent on `(provider, provider_id)`: re-recording the same
/// envelope returns the existing row rather than inserting a duplicate — a
/// concurrent double-send loses the `signature_provider_request` unique
/// index and re-reads the winner's row.
///
/// # Errors
///
/// [`SignatureError::Db`] if the write fails.
pub async fn record_request(
    db: &SurrealDb,
    notation_id: Uuid,
    provider: SignatureProvider,
    provider_id: &str,
) -> Result<Signature, SignatureError> {
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
            let row: Option<SignatureRow> = response.take(0)?;
            row.and_then(SignatureRow::into_signature)
                .ok_or(SignatureError::WriteReturnedNothing)
        }
        Err(error) if is_provider_request_conflict(&error) => {
            by_provider(db, provider, provider_id)
                .await?
                .ok_or(SignatureError::WriteReturnedNothing)
        }
        Err(error) => Err(SignatureError::Db(error)),
    }
}

/// The signature row for `(provider, provider_id)`, if any — the webhook's
/// correlation lookup.
///
/// # Errors
///
/// [`SignatureError::Db`] if the lookup fails.
pub async fn by_provider(
    db: &SurrealDb,
    provider: SignatureProvider,
    provider_id: &str,
) -> Result<Option<Signature>, SignatureError> {
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

/// The provider request id sent for a Notation, if one has been sent. The
/// notation-scoped read that replaces `notation.signature_request_id`;
/// returns the most recently recorded envelope for the notation.
///
/// # Errors
///
/// [`SignatureError::Db`] if the lookup fails.
pub async fn request_id_for_notation(
    db: &SurrealDb,
    notation_id: Uuid,
) -> Result<Option<String>, SignatureError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE notation_id = $notation \
             ORDER BY inserted_at DESC LIMIT 1"
        ))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?.map(|s| s.provider_id))
}

/// The completed signature for a Notation, if the provider has ever
/// confirmed execution — a row with `signed_at` set. `None` whether no
/// envelope was ever sent, one is still outstanding, or one was declined
/// or voided: in every one of those cases there is no provider-confirmed
/// execution to report, which is exactly the distinction this query
/// exists to preserve for a caller (the client portal's "Signed" label)
/// that must not infer execution from anything else — an object's
/// presence in storage, a workflow state, or a row's mere existence.
///
/// # Errors
///
/// [`SignatureError::Db`] if the lookup fails.
pub async fn completed_for_notation(
    db: &SurrealDb,
    notation_id: Uuid,
) -> Result<Option<Signature>, SignatureError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE notation_id = $notation AND signed_at != NONE \
             ORDER BY inserted_at DESC LIMIT 1"
        ))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?)
}

/// Stamp `signed_at` on the signature for `(provider, provider_id)` when
/// the provider reports completion. A no-op (returns `false`) for an
/// unknown envelope — the callback may arrive for one we never tracked, or
/// twice.
///
/// # Errors
///
/// [`SignatureError::Db`] if the lookup or write fails.
pub async fn stamp_signed(
    db: &SurrealDb,
    provider: SignatureProvider,
    provider_id: &str,
    signed_at: &str,
) -> Result<bool, SignatureError> {
    let Some(row) = by_provider(db, provider, provider_id).await? else {
        return Ok(false);
    };
    writing(|| {
        db.query("UPDATE $id SET signed_at = $signed_at, updated_at = time::now()")
            .bind(("id", record_id(TABLE, row.id)))
            .bind(("signed_at", signed_at.to_string()))
    })
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_notation;

    #[tokio::test]
    async fn record_request_is_idempotent_on_provider_and_provider_id() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        let first = record_request(&surreal, notation_id, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap();
        let again = record_request(&surreal, notation_id, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap();
        assert_eq!(first.id, again.id);
    }

    #[tokio::test]
    async fn request_id_for_notation_reads_the_latest_envelope() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        assert!(request_id_for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .is_none());
        record_request(&surreal, notation_id, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap();
        assert_eq!(
            request_id_for_notation(&surreal, notation_id)
                .await
                .unwrap(),
            Some("env-1".to_string())
        );
    }

    #[tokio::test]
    async fn completed_for_notation_is_none_until_stamp_signed_runs() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        record_request(&surreal, notation_id, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap();
        assert!(completed_for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .is_none());
        stamp_signed(
            &surreal,
            SignatureProvider::DocuSign,
            "env-1",
            "2026-06-30T00:00:00Z",
        )
        .await
        .unwrap();
        let completed = completed_for_notation(&surreal, notation_id)
            .await
            .unwrap()
            .expect("stamped signature is now completed");
        assert_eq!(completed.provider_id, "env-1");
        assert_eq!(completed.signed_at.as_deref(), Some("2026-06-30T00:00:00Z"));
    }

    #[tokio::test]
    async fn stamp_signed_marks_completion_and_is_a_no_op_for_unknown_envelopes() {
        let surreal = mem().await;
        let notation_id = seed_notation(&surreal).await;
        record_request(&surreal, notation_id, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap();
        assert!(!stamp_signed(
            &surreal,
            SignatureProvider::DocuSign,
            "unknown",
            "2026-06-30T00:00:00Z"
        )
        .await
        .unwrap());
        assert!(stamp_signed(
            &surreal,
            SignatureProvider::DocuSign,
            "env-1",
            "2026-06-30T00:00:00Z"
        )
        .await
        .unwrap());
        let after = by_provider(&surreal, SignatureProvider::DocuSign, "env-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.signed_at.as_deref(), Some("2026-06-30T00:00:00Z"));
    }
}
