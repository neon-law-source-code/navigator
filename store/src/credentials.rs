//! A person's licensure in a jurisdiction — the pair that makes an
//! attorney an attorney in a state.
//!
//! # This table lives in SurrealDB
//!
//! `credential` moved with the first tranche of the persons slice
//! (#1093; ENG-19), because a credential is a fact *about a person* and
//! a link that crossed engines would have to be resolved in Rust on
//! every read.
//!
//! Both `person_id` and `jurisdiction_id` are real links, so licensure is
//! a single-engine question.
//!
//! This module is also the *only* store of licensure: the canonical seed
//! writes here through [`find_or_grant`].
//!
//! **The engine does not validate a link.** `record<T>` accepts a link
//! to a row that was never written — no foreign key, no constraint
//! violation, nothing to catch it. Referential integrity is this
//! module's job: [`grant`] reads the person back through
//! [`crate::persons::find_by_id`] and the jurisdiction back through
//! [`crate::jurisdictions::find_by_id`] before writing, and those checks
//! are the only thing between a typo and a credential attached to
//! nobody, or to nowhere.

use chrono::{DateTime, Utc};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::jurisdictions::{self, JurisdictionError};
use crate::persons::{self, PersonError};
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "credential";
/// The table `person_id` links into.
const PERSON_TABLE: &str = "person";
/// The table `jurisdiction_id` links into.
const JURISDICTION_TABLE: &str = "jurisdiction";

/// Errors reading or writing a credential.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the person a write links to failed.
    #[error(transparent)]
    Person(#[from] PersonError),
    /// Reading the jurisdiction a write links to failed.
    #[error(transparent)]
    Jurisdiction(#[from] JurisdictionError),
    /// The person named by a write does not exist. The engine would
    /// have accepted the dangling link; this is the check that does not.
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    /// The jurisdiction named by a write does not exist — same story as
    /// [`CredentialError::NoSuchPerson`], on the other end of the pair.
    #[error("no jurisdiction {0}")]
    NoSuchJurisdiction(Uuid),
    /// This person is already listed under this jurisdiction — the
    /// write collided with `credential_person_jurisdiction`.
    #[error("that person already holds a credential in that jurisdiction")]
    AlreadyGranted,
    /// A write reported success but returned no row, or returned one
    /// whose record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    #[error("writing a credential returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator, identified by the shared classifier in
/// [`crate::surreal::retry`].
fn classify_write(error: surrealdb::Error) -> CredentialError {
    if crate::surreal::retry::unique_violation(&error) == Some("credential_person_jurisdiction") {
        CredentialError::AlreadyGranted
    } else {
        CredentialError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, CredentialError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// One licensure record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub id: Uuid,
    pub person_id: Uuid,
    pub jurisdiction_id: Uuid,
    pub license_number: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from
/// [`Credential`] because the SDK owns its own `RecordId` and
/// `Datetime`, and the conversion belongs at this seam rather than in
/// every caller.
#[derive(SurrealValue)]
struct CredentialRow {
    id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    jurisdiction_id: surrealdb::types::RecordId,
    license_number: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl CredentialRow {
    /// `None` when any record id is not a native UUID key — see
    /// [`crate::surreal`] for why the two key spellings differ.
    fn into_credential(self) -> Option<Credential> {
        Some(Credential {
            id: record_uuid(&self.id)?,
            person_id: record_uuid(&self.person_id)?,
            jurisdiction_id: record_uuid(&self.jurisdiction_id)?,
            license_number: self.license_number,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`CredentialRow`] from only one query.
const SELECT: &str = "id, person_id, jurisdiction_id, license_number, inserted_at, updated_at";

/// Record that `person_id` is licensed in `jurisdiction_id`.
///
/// # Errors
///
/// [`CredentialError::NoSuchPerson`] when the person does not exist and
/// [`CredentialError::NoSuchJurisdiction`] when the jurisdiction does
/// not — both checked here because the engine would accept either
/// dangling link. [`CredentialError::AlreadyGranted`] when this person
/// is already listed under this jurisdiction, and
/// [`CredentialError::Db`] for anything else.
pub async fn grant(
    db: &SurrealDb,
    person_id: Uuid,
    jurisdiction_id: Uuid,
    license_number: &str,
) -> Result<Credential, CredentialError> {
    if persons::find_by_id(db, person_id).await?.is_none() {
        return Err(CredentialError::NoSuchPerson(person_id));
    }
    if jurisdictions::find_by_id(db, jurisdiction_id)
        .await?
        .is_none()
    {
        return Err(CredentialError::NoSuchJurisdiction(jurisdiction_id));
    }

    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             person_id = $person_id, \
             jurisdiction_id = $jurisdiction_id, \
             license_number = $license_number \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind((
            "jurisdiction_id",
            record_id(JURISDICTION_TABLE, jurisdiction_id),
        ))
        .bind(("license_number", license_number.to_string()))
    })
    .await?;

    let row: Option<CredentialRow> = response.take(0)?;
    row.and_then(CredentialRow::into_credential)
        .ok_or(CredentialError::WriteReturnedNothing)
}

/// The credential pairing this person with this jurisdiction, if there
/// is one. The pair is the natural key the unique index carries.
///
/// # Errors
///
/// [`CredentialError::Db`] if the lookup fails.
pub async fn find_by_person_and_jurisdiction(
    db: &SurrealDb,
    person_id: Uuid,
    jurisdiction_id: Uuid,
) -> Result<Option<Credential>, CredentialError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE person_id = $person_id AND jurisdiction_id = $jurisdiction_id \
             LIMIT 1"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind((
            "jurisdiction_id",
            record_id(JURISDICTION_TABLE, jurisdiction_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<CredentialRow> = response.take(0)?;
    Ok(row.and_then(CredentialRow::into_credential))
}

/// Grant the credential unless this person already holds one in this
/// jurisdiction. Race-safe without a lock: a concurrent grantor that
/// wins the `credential_person_jurisdiction` unique index turns this
/// call's insert into [`CredentialError::AlreadyGranted`], which is
/// re-read as the winner's row. The canonical seed runs this on every
/// boot, so idempotence is the contract.
///
/// An existing row is returned as it stands — a differing
/// `license_number` does not overwrite it. The seed is the only caller,
/// and a licence number that changed in the canonical YAML is a data
/// correction someone should make deliberately, not a silent effect of
/// the next boot.
///
/// # Errors
///
/// [`CredentialError::NoSuchPerson`] and
/// [`CredentialError::NoSuchJurisdiction`] as [`grant`] documents, and
/// [`CredentialError::Db`] if a lookup or the insert fails.
pub async fn find_or_grant(
    db: &SurrealDb,
    person_id: Uuid,
    jurisdiction_id: Uuid,
    license_number: &str,
) -> Result<Credential, CredentialError> {
    if let Some(existing) = find_by_person_and_jurisdiction(db, person_id, jurisdiction_id).await? {
        return Ok(existing);
    }
    match grant(db, person_id, jurisdiction_id, license_number).await {
        Ok(granted) => Ok(granted),
        Err(CredentialError::AlreadyGranted) => {
            find_by_person_and_jurisdiction(db, person_id, jurisdiction_id)
                .await?
                .ok_or(CredentialError::WriteReturnedNothing)
        }
        Err(error) => Err(error),
    }
}

/// Every credential held by one person, oldest first.
///
/// # Errors
///
/// [`CredentialError::Db`] if the lookup fails.
pub async fn for_person(
    db: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<Credential>, CredentialError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE person_id = $person_id ORDER BY inserted_at ASC"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let rows: Vec<CredentialRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(CredentialRow::into_credential)
        .collect())
}

/// Whether this person is licensed in this jurisdiction — the question a
/// matter-open attestation asks before letting an attorney take work in
/// a state.
///
/// # Errors
///
/// [`CredentialError::Db`] if the lookup fails.
pub async fn is_licensed_in(
    db: &SurrealDb,
    person_id: Uuid,
    jurisdiction_id: Uuid,
) -> Result<bool, CredentialError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE count() FROM {TABLE} \
             WHERE person_id = $person_id AND jurisdiction_id = $jurisdiction_id \
             GROUP ALL"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind((
            "jurisdiction_id",
            record_id(JURISDICTION_TABLE, jurisdiction_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let counts: Vec<i64> = response.take(0)?;
    Ok(counts.first().is_some_and(|count| *count > 0))
}

/// Remove a credential — a licence lapsed or was recorded in error.
/// Idempotent: revoking one that is not there is a no-op.
///
/// # Errors
///
/// [`CredentialError::Db`] if the delete fails.
pub async fn revoke(db: &SurrealDb, credential_id: Uuid) -> Result<(), CredentialError> {
    db.query("DELETE $id")
        .bind(("id", record_id(TABLE, credential_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        find_by_person_and_jurisdiction, find_or_grant, for_person, grant, is_licensed_in, revoke,
        CredentialError, JURISDICTION_TABLE, PERSON_TABLE,
    };
    use crate::jurisdictions::{self, NewJurisdiction};
    use crate::persons::{self, NewPerson, Role};
    use crate::surreal::test_support::mem;
    use crate::surreal::{record_id, SurrealDb};
    use uuid::Uuid;

    async fn an_attorney(db: &SurrealDb, email: &str) -> Uuid {
        persons::create(db, &NewPerson::with_role("Scorpio", email, Role::Lawyer))
            .await
            .unwrap()
            .id
    }

    async fn a_state(db: &SurrealDb, name: &str, code: &str) -> Uuid {
        jurisdictions::create(db, &NewJurisdiction::new(name, code, "state"))
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn a_granted_credential_reads_back_for_its_person() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;

        let granted = grant(&db, person_id, nevada, "NV-12345").await.unwrap();
        assert_eq!(granted.person_id, person_id);
        assert_eq!(granted.jurisdiction_id, nevada);
        assert_eq!(granted.license_number, "NV-12345");

        assert_eq!(for_person(&db, person_id).await.unwrap(), vec![granted]);
        assert!(is_licensed_in(&db, person_id, nevada).await.unwrap());
    }

    #[tokio::test]
    async fn the_natural_key_lookup_finds_only_that_pair() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;
        let utah = a_state(&db, "Utah", "UT").await;

        let granted = grant(&db, person_id, nevada, "NV-12345").await.unwrap();
        assert_eq!(
            find_by_person_and_jurisdiction(&db, person_id, nevada)
                .await
                .unwrap(),
            Some(granted)
        );
        assert_eq!(
            find_by_person_and_jurisdiction(&db, person_id, utah)
                .await
                .unwrap(),
            None,
            "a licence in Nevada is not a licence in Utah"
        );
    }

    #[tokio::test]
    async fn find_or_grant_is_idempotent_on_the_person_jurisdiction_pair() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;

        // The canonical seed calls this on every boot.
        let first = find_or_grant(&db, person_id, nevada, "NV-12345")
            .await
            .unwrap();
        let second = find_or_grant(&db, person_id, nevada, "NV-99999")
            .await
            .unwrap();

        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(
            second.license_number, "NV-12345",
            "an existing credential is returned as it stands, not overwritten"
        );
        assert_eq!(for_person(&db, person_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn find_or_grant_still_refuses_a_dangling_person() {
        let db = mem().await;
        let nobody = Uuid::now_v7();

        // Find-or-create does not weaken the read-backs: the lookup
        // misses, and the grant underneath it does the refusing.
        let refused = find_or_grant(&db, nobody, Uuid::now_v7(), "NV-1").await;
        assert!(matches!(
            refused,
            Err(CredentialError::NoSuchPerson(id)) if id == nobody
        ));
    }

    #[tokio::test]
    async fn a_credential_for_a_person_who_does_not_exist_is_refused() {
        let db = mem().await;
        let nobody = Uuid::now_v7();

        // The engine would accept this link — `record<person>` is not
        // validated against an existing row. This check is the only
        // thing that refuses it.
        let refused = grant(&db, nobody, Uuid::now_v7(), "NV-1").await;
        assert!(matches!(
            refused,
            Err(CredentialError::NoSuchPerson(id)) if id == nobody
        ));
    }

    #[tokio::test]
    async fn a_credential_for_a_jurisdiction_that_does_not_exist_is_refused() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nowhere = Uuid::now_v7();

        // Same story as the missing person, on the other end of the
        // pair: `record<jurisdiction>` is not validated either.
        let refused = grant(&db, person_id, nowhere, "NV-1").await;
        assert!(matches!(
            refused,
            Err(CredentialError::NoSuchJurisdiction(id)) if id == nowhere
        ));
    }

    #[tokio::test]
    async fn the_engine_itself_would_have_accepted_the_dangling_links() {
        let db = mem().await;

        // Pins the reason `grant` checks: writing the same row without
        // the guard succeeds, so nothing below this module catches it —
        // for either link.
        db.query("CREATE $id SET person_id = $person, jurisdiction_id = $j, license_number = 'X'")
            .bind(("id", record_id("credential", Uuid::now_v7())))
            .bind(("person", record_id(PERSON_TABLE, Uuid::now_v7())))
            .bind(("j", record_id(JURISDICTION_TABLE, Uuid::now_v7())))
            .await
            .unwrap()
            .check()
            .expect("the engine does not validate a record link");
    }

    #[tokio::test]
    async fn a_second_credential_in_the_same_jurisdiction_is_reported_as_granted() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;

        grant(&db, person_id, nevada, "NV-12345").await.unwrap();
        let duplicate = grant(&db, person_id, nevada, "NV-99999").await;
        assert!(
            matches!(duplicate, Err(CredentialError::AlreadyGranted)),
            "the unique `credential_person_jurisdiction` index is the gate, got {duplicate:?}"
        );
    }

    #[tokio::test]
    async fn the_same_person_may_hold_several_jurisdictions() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;
        let california = a_state(&db, "California", "CA").await;

        grant(&db, person_id, nevada, "NV-1").await.unwrap();
        grant(&db, person_id, california, "CA-2").await.unwrap();

        assert_eq!(for_person(&db, person_id).await.unwrap().len(), 2);
        assert!(is_licensed_in(&db, person_id, nevada).await.unwrap());
        assert!(is_licensed_in(&db, person_id, california).await.unwrap());
    }

    #[tokio::test]
    async fn a_credential_is_scoped_to_its_own_person() {
        let db = mem().await;
        let licensed = an_attorney(&db, "licensed@example.com").await;
        let unlicensed = an_attorney(&db, "unlicensed@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;

        grant(&db, licensed, nevada, "NV-1").await.unwrap();

        assert!(for_person(&db, unlicensed).await.unwrap().is_empty());
        assert!(!is_licensed_in(&db, unlicensed, nevada).await.unwrap());
    }

    #[tokio::test]
    async fn revoking_clears_the_licence_and_is_idempotent() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;
        let granted = grant(&db, person_id, nevada, "NV-1").await.unwrap();

        revoke(&db, granted.id).await.unwrap();
        assert!(!is_licensed_in(&db, person_id, nevada).await.unwrap());
        assert!(for_person(&db, person_id).await.unwrap().is_empty());

        // Revoking again, and revoking one that never existed, are no-ops.
        revoke(&db, granted.id).await.unwrap();
        revoke(&db, Uuid::now_v7()).await.unwrap();
    }

    #[tokio::test]
    async fn a_revoked_jurisdiction_can_be_granted_again() {
        let db = mem().await;
        let person_id = an_attorney(&db, "scorpio@example.com").await;
        let nevada = a_state(&db, "Nevada", "NV").await;

        let first = grant(&db, person_id, nevada, "NV-1").await.unwrap();
        revoke(&db, first.id).await.unwrap();

        // The unique index must not keep the pair reserved after the row
        // is gone — a lapsed licence gets reinstated.
        let again = grant(&db, person_id, nevada, "NV-2").await.unwrap();
        assert_eq!(again.license_number, "NV-2");
    }
}
