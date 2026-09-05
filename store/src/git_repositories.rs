//! `git_repositories` — provenance for repositories that carried
//! imported notation content, and every query against a
//! `git_repository` row.
//!
//! # This table lives in SurrealDB
//!
//! `git_repositories` moved with its slice of #1093 (ENG-20). It is a
//! leaf provenance table — nothing references it and it references
//! nothing — so the port could not cascade. Rows come from the
//! canonical seed; `navigator site seed` seeds from a local
//! directory and never writes here.
//!
//! The unique `git_repository_remote_hash` index is what enforces one row
//! per remote; a violation has no typed detail, so
//! [`crate::surreal::retry::unique_violation`] discriminates on that index
//! name.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "git_repository";

/// One tracked repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitRepository {
    pub id: Uuid,
    /// SHA-256 hash of `git remote get-url origin`. Unique.
    pub remote_hash: String,
    /// Last imported commit SHA.
    pub last_commit_sha: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it — the seam between
/// [`GitRepository`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct GitRepositoryRow {
    id: surrealdb::types::RecordId,
    remote_hash: String,
    last_commit_sha: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl GitRepositoryRow {
    /// `None` when the record id is not a native UUID key — a row
    /// written by something that bypassed [`crate::surreal::record_id`].
    fn into_repository(self) -> Option<GitRepository> {
        Some(GitRepository {
            id: record_uuid(&self.id)?,
            remote_hash: self.remote_hash,
            last_commit_sha: self.last_commit_sha,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`GitRepositoryRow`] from only one
/// query.
const SELECT: &str = "id, remote_hash, last_commit_sha, inserted_at, updated_at";

/// Errors reading or writing a git repository row.
#[derive(Debug, thiserror::Error)]
pub enum GitRepositoryError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `git_repository_remote_hash` — another
    /// row already holds this remote hash.
    #[error("that repository remote is already tracked")]
    RemoteTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see
    /// [`GitRepositoryRow::into_repository`].
    #[error("writing a git repository returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator.
fn classify_write(error: surrealdb::Error) -> GitRepositoryError {
    if crate::surreal::retry::unique_violation(&error) == Some("git_repository_remote_hash") {
        GitRepositoryError::RemoteTaken
    } else {
        GitRepositoryError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, GitRepositoryError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Resolve a repository by its `remote_hash`. Exact match — the hash is
/// computed, never user-typed.
///
/// # Errors
///
/// [`GitRepositoryError::Db`] if the lookup fails.
pub async fn find_by_remote_hash(
    db: &SurrealDb,
    remote_hash: &str,
) -> Result<Option<GitRepository>, GitRepositoryError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE remote_hash = $remote_hash LIMIT 1"
        ))
        .bind(("remote_hash", remote_hash.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<GitRepositoryRow> = response.take(0)?;
    Ok(row.and_then(GitRepositoryRow::into_repository))
}

/// Every tracked repository, ordered by remote hash for a deterministic
/// listing.
///
/// # Errors
///
/// [`GitRepositoryError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<GitRepository>, GitRepositoryError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY remote_hash ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<GitRepositoryRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(GitRepositoryRow::into_repository)
        .collect())
}

/// Write a new repository row under a fresh v7 UUID record id.
///
/// # Errors
///
/// [`GitRepositoryError::RemoteTaken`] when another row already holds
/// this remote hash, and [`GitRepositoryError::Db`] for anything else.
pub async fn create(
    db: &SurrealDb,
    remote_hash: &str,
    last_commit_sha: &str,
) -> Result<GitRepository, GitRepositoryError> {
    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             remote_hash = $remote_hash, \
             last_commit_sha = $last_commit_sha \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("remote_hash", remote_hash.to_string()))
        .bind(("last_commit_sha", last_commit_sha.to_string()))
    })
    .await?;
    let row: Option<GitRepositoryRow> = response.take(0)?;
    row.and_then(GitRepositoryRow::into_repository)
        .ok_or(GitRepositoryError::WriteReturnedNothing)
}

/// Find the repository holding `remote_hash`, creating it if absent.
/// Race-safe without a lock: a concurrent creator that wins the
/// `git_repository_remote_hash` unique index turns this call's insert
/// into [`GitRepositoryError::RemoteTaken`], which is re-read as the
/// winner's row. The seed runs this on every boot, so idempotence is
/// the contract.
///
/// # Errors
///
/// [`GitRepositoryError::Db`] if a lookup or the insert fails.
pub async fn find_or_create(
    db: &SurrealDb,
    remote_hash: &str,
    last_commit_sha: &str,
) -> Result<GitRepository, GitRepositoryError> {
    if let Some(existing) = find_by_remote_hash(db, remote_hash).await? {
        return Ok(existing);
    }
    match create(db, remote_hash, last_commit_sha).await {
        Ok(created) => Ok(created),
        Err(GitRepositoryError::RemoteTaken) => find_by_remote_hash(db, remote_hash)
            .await?
            .ok_or(GitRepositoryError::WriteReturnedNothing),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{create, find_by_remote_hash, find_or_create, list_all, GitRepositoryError};
    use crate::surreal::test_support::mem;

    #[tokio::test]
    async fn a_created_repository_reads_back_by_remote_hash() {
        let db = mem().await;
        let created = create(&db, "abc123", "deadbeef").await.unwrap();
        assert_eq!(created.remote_hash, "abc123");
        assert_eq!(created.last_commit_sha, "deadbeef");

        assert_eq!(
            find_by_remote_hash(&db, "abc123").await.unwrap(),
            Some(created)
        );
        assert_eq!(find_by_remote_hash(&db, "zzz").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_duplicate_remote_is_reported_as_taken() {
        let db = mem().await;
        create(&db, "abc123", "deadbeef").await.unwrap();
        let duplicate = create(&db, "abc123", "cafebabe").await;
        assert!(
            matches!(duplicate, Err(GitRepositoryError::RemoteTaken)),
            "the unique `git_repository_remote_hash` index is the gate, got {duplicate:?}"
        );
    }

    #[tokio::test]
    async fn find_or_create_is_idempotent_on_the_remote_hash() {
        let db = mem().await;
        let first = find_or_create(&db, "abc123", "deadbeef").await.unwrap();
        let second = find_or_create(&db, "abc123", "cafebabe").await.unwrap();
        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(list_all(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn listing_orders_by_remote_hash() {
        let db = mem().await;
        create(&db, "beta", "2".repeat(40).as_str()).await.unwrap();
        create(&db, "alpha", "1".repeat(40).as_str()).await.unwrap();
        let hashes: Vec<String> = list_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.remote_hash)
            .collect();
        assert_eq!(hashes, ["alpha", "beta"]);
    }
}
