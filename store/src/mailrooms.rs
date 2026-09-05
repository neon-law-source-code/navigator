//! Mail-receiving destinations, and every query against a `mailroom`
//! row.
//!
//! # This table lives in SurrealDB
//!
//! `mailrooms` moved with wave two of the flat-table ports (#1093;
//! ENG-20), between `addresses` (which it links to) and `letters`
//! (which links to it).
//!
//! **The engine does not validate a link.** `address_id` is a
//! `record<address>`, but a link to an address that was never written is
//! accepted with no constraint violation to catch it. The read-back in
//! [`create`] is the check, not the `record<T>` type.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::addresses::{self, AddressError};
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "mailroom";
/// The table `address_id` links into.
const ADDRESS_TABLE: &str = "address";

/// One mail-receiving destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mailroom {
    pub id: Uuid,
    /// Unique.
    pub name: String,
    pub address_id: Uuid,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it — the seam between
/// [`Mailroom`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct MailroomRow {
    id: surrealdb::types::RecordId,
    name: String,
    address_id: surrealdb::types::RecordId,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl MailroomRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_mailroom(self) -> Option<Mailroom> {
        Some(Mailroom {
            id: record_uuid(&self.id)?,
            name: self.name,
            address_id: record_uuid(&self.address_id)?,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`MailroomRow`] from only one query.
const SELECT: &str = "id, name, address_id, inserted_at, updated_at";

/// Errors reading or writing a mailroom.
#[derive(Debug, thiserror::Error)]
pub enum MailroomError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the address a write links to failed.
    #[error(transparent)]
    Address(#[from] AddressError),
    /// The address named by a write does not exist. The engine would
    /// have accepted the dangling link; this is the check that does not.
    #[error("no address {0}")]
    NoSuchAddress(Uuid),
    /// The write collided with `mailroom_name` — another mailroom
    /// already holds this name.
    #[error("that mailroom name is already taken")]
    NameTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back.
    #[error("writing a mailroom returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator, identified by the shared classifier in
/// [`crate::surreal::retry`].
fn classify_write(error: surrealdb::Error) -> MailroomError {
    if crate::surreal::retry::unique_violation(&error) == Some("mailroom_name") {
        MailroomError::NameTaken
    } else {
        MailroomError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, MailroomError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Register a mailroom at `address_id`.
///
/// # Errors
///
/// [`MailroomError::NoSuchAddress`] when the address does not exist —
/// checked here because the engine would accept the dangling link —
/// [`MailroomError::NameTaken`] on a duplicate name, and
/// [`MailroomError::Db`] for anything else.
pub async fn create(
    db: &SurrealDb,
    name: &str,
    address_id: Uuid,
) -> Result<Mailroom, MailroomError> {
    if addresses::find_by_id(db, address_id).await?.is_none() {
        return Err(MailroomError::NoSuchAddress(address_id));
    }

    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET name = $name, address_id = $address_id RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("name", name.to_string()))
        .bind(("address_id", record_id(ADDRESS_TABLE, address_id)))
    })
    .await?;

    let row: Option<MailroomRow> = response.take(0)?;
    row.and_then(MailroomRow::into_mailroom)
        .ok_or(MailroomError::WriteReturnedNothing)
}

/// Register a mailroom unless one already holds `name`.
///
/// Race-safe without a lock, the way `credentials::find_or_grant` is: a
/// concurrent seeder that wins `mailroom_name` turns this call's insert
/// into [`MailroomError::NameTaken`], which is re-read as the winner's
/// row. Read-then-create is not enough, and the cucumber suites are why
/// — they run scenarios concurrently against **one shared** embedded
/// engine (`features::shared_surreal`), so two scenarios seeding at once
/// both miss the read.
///
/// `address_id` is only consulted when this call is the one that
/// creates. A caller that loses the race has already written a
/// placeholder address that nothing will point at — harmless
/// dev-portfolio data, and the alternative (wrapping both writes in one
/// transaction) buys tidiness the seed does not need.
///
/// # Errors
///
/// As [`create`], except that a lost name race resolves to the winning
/// row instead of [`MailroomError::NameTaken`].
pub async fn find_or_create(
    db: &SurrealDb,
    name: &str,
    address_id: Uuid,
) -> Result<Mailroom, MailroomError> {
    if let Some(existing) = find_by_name(db, name).await? {
        return Ok(existing);
    }
    match create(db, name, address_id).await {
        Ok(created) => Ok(created),
        Err(MailroomError::NameTaken) => find_by_name(db, name)
            .await?
            .ok_or(MailroomError::WriteReturnedNothing),
        Err(error) => Err(error),
    }
}

/// One mailroom by name. Exact match — the name is the natural key the
/// canonical seed and the letters seeder both resolve on.
///
/// # Errors
///
/// [`MailroomError::Db`] if the lookup fails.
pub async fn find_by_name(db: &SurrealDb, name: &str) -> Result<Option<Mailroom>, MailroomError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE name = $name LIMIT 1"
        ))
        .bind(("name", name.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<MailroomRow> = response.take(0)?;
    Ok(row.and_then(MailroomRow::into_mailroom))
}

/// One mailroom by id.
///
/// # Errors
///
/// [`MailroomError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Mailroom>, MailroomError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<MailroomRow> = response.take(0)?;
    Ok(row.and_then(MailroomRow::into_mailroom))
}

/// Every mailroom, oldest first — a stable order for the lawyer listing
/// and for the inbound router, which routes through the first one.
///
/// # Errors
///
/// [`MailroomError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Mailroom>, MailroomError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC, id ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<MailroomRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(MailroomRow::into_mailroom)
        .collect())
}

/// How many mailrooms exist. The production-emptiness gate asks this of
/// the engine that holds the table.
///
/// # Errors
///
/// [`MailroomError::Db`] if the count fails.
pub async fn count(db: &SurrealDb) -> Result<i64, MailroomError> {
    let mut response = db
        .query(format!("SELECT count() FROM {TABLE} GROUP ALL"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let counts: Vec<CountRow> = response.take(0)?;
    Ok(counts.first().map_or(0, |c| c.count))
}

/// The one-field shape `SELECT count() ... GROUP ALL` returns.
#[derive(SurrealValue)]
struct CountRow {
    count: i64,
}

#[cfg(test)]
mod tests {
    use super::{count, create, find_by_id, find_by_name, find_or_create, list_all, MailroomError};
    use crate::addresses::{self, NewAddress};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    async fn an_address(db: &SurrealDb, line1: &str) -> Uuid {
        addresses::create(
            db,
            &NewAddress {
                line1: line1.to_string(),
                city: "Reno".into(),
                region: "NV".into(),
                postal_code: "89501".into(),
                country: "US".into(),
                ..NewAddress::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn a_created_mailroom_reads_back_by_name_and_id() {
        let db = mem().await;
        let address_id = an_address(&db, "123 Main St").await;

        let created = create(&db, "HQ", address_id).await.unwrap();
        assert_eq!(created.name, "HQ");
        assert_eq!(created.address_id, address_id);

        assert_eq!(
            find_by_name(&db, "HQ").await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(find_by_id(&db, created.id).await.unwrap(), Some(created));
        assert_eq!(find_by_name(&db, "Nowhere").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_mailroom_at_an_address_that_does_not_exist_is_refused() {
        let db = mem().await;
        let nowhere = Uuid::now_v7();

        // The engine would accept this link — `record<address>` is not
        // validated against an existing row — so the read-back is what
        // refuses it.
        let refused = create(&db, "HQ", nowhere).await;
        assert!(matches!(
            refused,
            Err(MailroomError::NoSuchAddress(id)) if id == nowhere
        ));
    }

    #[tokio::test]
    async fn the_engine_itself_would_have_accepted_the_dangling_link() {
        let db = mem().await;
        // Proof the read-back above is load-bearing rather than
        // belt-and-braces: written straight through, the link sticks.
        let dangling = record_id_of_nothing();
        db.query("CREATE $id SET name = 'Ghost', address_id = $address_id")
            .bind(("id", crate::surreal::record_id("mailroom", Uuid::now_v7())))
            .bind(("address_id", dangling))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect("the engine accepts a link to an address that was never written");

        assert!(
            find_by_name(&db, "Ghost").await.unwrap().is_some(),
            "and the row is readable, pointing at nothing"
        );
    }

    fn record_id_of_nothing() -> surrealdb::types::RecordId {
        crate::surreal::record_id("address", Uuid::now_v7())
    }

    #[tokio::test]
    async fn a_duplicate_name_is_reported_as_taken() {
        let db = mem().await;
        let first = an_address(&db, "123 Main St").await;
        let second = an_address(&db, "456 Side St").await;

        create(&db, "HQ", first).await.unwrap();
        let duplicate = create(&db, "HQ", second).await;
        assert!(
            matches!(duplicate, Err(MailroomError::NameTaken)),
            "the unique `mailroom_name` index is the gate, got {duplicate:?}"
        );
    }

    #[tokio::test]
    async fn find_or_create_is_idempotent_on_the_name() {
        let db = mem().await;
        let first_address = an_address(&db, "123 Main St").await;
        let second_address = an_address(&db, "456 Side St").await;

        let first = find_or_create(&db, "HQ", first_address).await.unwrap();
        let second = find_or_create(&db, "HQ", second_address).await.unwrap();

        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(
            second.address_id, first_address,
            "an existing mailroom keeps its address rather than being repointed"
        );
        assert_eq!(count(&db).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_seeders_do_not_collide_on_the_name() {
        let db = mem().await;
        // The cucumber suites run scenarios concurrently against one
        // shared engine, so read-then-create loses this race and
        // surfaces `NameTaken` out of the seed.
        let racers: Vec<_> = (0..6)
            .map(|n| {
                let db = db.clone();
                tokio::spawn(async move {
                    let address = an_address(&db, &format!("{n} Main St")).await;
                    find_or_create(&db, "HQ", address).await
                })
            })
            .collect();

        let mut ids = Vec::new();
        for racer in racers {
            ids.push(racer.await.expect("task must not panic").unwrap().id);
        }
        assert_eq!(count(&db).await.unwrap(), 1, "one mailroom, not six");
        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "every racer resolves to the same row: {ids:?}"
        );
    }

    #[tokio::test]
    async fn listing_is_ordered_oldest_first_so_inbound_routing_is_stable() {
        let db = mem().await;
        let a = an_address(&db, "123 Main St").await;
        let b = an_address(&db, "456 Side St").await;
        let first = create(&db, "HQ", a).await.unwrap();
        let second = create(&db, "Annex", b).await.unwrap();

        // The inbound-email router takes the first mailroom, so this
        // order is behaviour, not cosmetics.
        let ids: Vec<_> = list_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, [first.id, second.id]);
    }

    #[tokio::test]
    async fn counting_an_empty_table_is_zero_rather_than_an_error() {
        let db = mem().await;
        assert_eq!(count(&db).await.unwrap(), 0);
        let address_id = an_address(&db, "123 Main St").await;
        create(&db, "HQ", address_id).await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);
    }
}
