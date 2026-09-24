//! Postal addresses, and every query against an `address` row.
//!
//! # This table lives in SurrealDB
//!
//! `addresses` moved with wave two of the flat-table ports (#1093;
//! ENG-20), together with `mailrooms` and `letters` — the three form one
//! chain (`letter -> mailroom -> address`), so moving them apart would
//! have meant resolving a link in Rust that is a native record link now.
//!
//! # Both links are real
//!
//! `person_id` became a `record<person>` with the persons slice, and
//! `entity_id` a `record<entity>` when the entities cluster ported
//! (ENG-120) — the last cross-engine id this table carried.
//!
//! **The engine does not validate a link.** `record<T>` accepts a link
//! to a row that was never written. [`create`] reads the person back
//! before writing, which is the only thing standing between a typo and
//! an address attached to nobody.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::{self, PersonError};
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "address";
/// The table `person_id` links into.
const PERSON_TABLE: &str = "person";
/// The table `entity_id` links into (ENG-120).
const ENTITY_TABLE: &str = "entity";

/// One postal address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Address {
    pub id: Uuid,
    /// The person this address belongs to, if any.
    pub person_id: Option<Uuid>,
    /// The entity this address belongs to, if any.
    pub entity_id: Option<Uuid>,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Everything [`create`] needs. A struct rather than nine positional
/// arguments, four of which are optional or empty strings.
#[derive(Debug, Clone, Default)]
pub struct NewAddress {
    pub person_id: Option<Uuid>,
    pub entity_id: Option<Uuid>,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

/// The row as the engine reads and writes it — the seam between
/// [`Address`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct AddressRow {
    id: surrealdb::types::RecordId,
    person_id: Option<surrealdb::types::RecordId>,
    entity_id: Option<surrealdb::types::RecordId>,
    line1: String,
    line2: Option<String>,
    city: String,
    region: String,
    postal_code: String,
    country: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AddressRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_address(self) -> Option<Address> {
        Some(Address {
            id: record_uuid(&self.id)?,
            person_id: self.person_id.as_ref().and_then(record_uuid),
            entity_id: self.entity_id.as_ref().and_then(record_uuid),
            line1: self.line1,
            line2: self.line2,
            city: self.city,
            region: self.region,
            postal_code: self.postal_code,
            country: self.country,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`AddressRow`] from only one query.
const SELECT: &str = "id, person_id, entity_id, line1, line2, city, region, postal_code, \
                      country, inserted_at, updated_at";

/// Errors reading or writing an address.
#[derive(Debug, thiserror::Error)]
pub enum AddressError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the person a write links to failed.
    #[error(transparent)]
    Person(#[from] PersonError),
    /// The person named by a write does not exist. The engine would have
    /// accepted the dangling link; this is the check that does not.
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see [`AddressRow::into_address`].
    #[error("writing an address returned no usable row")]
    WriteReturnedNothing,
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, AddressError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(AddressError::Db)
}

/// Write a new address under a fresh v7 UUID record id.
///
/// # Errors
///
/// [`AddressError::NoSuchPerson`] when `person_id` names no person —
/// checked here because the engine would accept the dangling link — and
/// [`AddressError::Db`] if the insert fails.
pub async fn create(db: &SurrealDb, new: &NewAddress) -> Result<Address, AddressError> {
    if let Some(person_id) = new.person_id {
        if persons::find_by_id(db, person_id).await?.is_none() {
            return Err(AddressError::NoSuchPerson(person_id));
        }
    }

    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             person_id = $person_id, \
             entity_id = $entity_id, \
             line1 = $line1, \
             line2 = $line2, \
             city = $city, \
             region = $region, \
             postal_code = $postal_code, \
             country = $country \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "person_id",
            new.person_id.map(|p| record_id(PERSON_TABLE, p)),
        ))
        .bind((
            "entity_id",
            new.entity_id.map(|id| record_id(ENTITY_TABLE, id)),
        ))
        .bind(("line1", new.line1.clone()))
        .bind(("line2", new.line2.clone()))
        .bind(("city", new.city.clone()))
        .bind(("region", new.region.clone()))
        .bind(("postal_code", new.postal_code.clone()))
        .bind(("country", new.country.clone()))
    })
    .await?;

    let row: Option<AddressRow> = response.take(0)?;
    row.and_then(AddressRow::into_address)
        .ok_or(AddressError::WriteReturnedNothing)
}

/// One address by id.
///
/// # Errors
///
/// [`AddressError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Address>, AddressError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<AddressRow> = response.take(0)?;
    Ok(row.and_then(AddressRow::into_address))
}

/// Every address belonging to one entity, oldest first. The entity id is
/// a cross-engine value, so this is an equality filter rather than a
/// link traversal.
///
/// # Errors
///
/// [`AddressError::Db`] if the lookup fails.
pub async fn for_entity(db: &SurrealDb, entity_id: Uuid) -> Result<Vec<Address>, AddressError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE entity_id = $entity_id ORDER BY inserted_at ASC"
        ))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AddressRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AddressRow::into_address)
        .collect())
}

/// Every address belonging to one person, oldest first — the counterpart
/// of [`for_entity`] (LAW-57).
///
/// # Errors
///
/// [`AddressError::Db`] if the lookup fails.
pub async fn for_person(db: &SurrealDb, person_id: Uuid) -> Result<Vec<Address>, AddressError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE person_id = $person_id ORDER BY inserted_at ASC"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AddressRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AddressRow::into_address)
        .collect())
}

/// Find this entity's address at `line1`/`postal_code`, creating it if
/// absent — the canonical seed's idempotence contract, on the natural
/// key the seeder matches on.
///
/// There is no unique index behind this, so two concurrent seeders could
/// both miss the read and both insert. The seed is single-writer per
/// boot and the duplicate would be cosmetic rather than corrupting,
/// which is why this does not carry the re-read a unique index would
/// make possible.
///
/// # Errors
///
/// [`AddressError::Db`] if a lookup or the insert fails.
pub async fn find_or_create_for_entity(
    db: &SurrealDb,
    new: &NewAddress,
) -> Result<(Address, bool), AddressError> {
    if let Some(entity_id) = new.entity_id {
        let existing = for_entity(db, entity_id)
            .await?
            .into_iter()
            .find(|a| a.line1 == new.line1 && a.postal_code == new.postal_code);
        if let Some(found) = existing {
            return Ok((found, false));
        }
    }
    Ok((create(db, new).await?, true))
}

/// [`find_or_create_for_entity`]'s counterpart for a person-linked address
/// (LAW-57) — `navigator site import`'s natural key for the `address`
/// seed model.
///
/// # Errors
///
/// [`AddressError::NoSuchPerson`] when `new.person_id` names no person, and
/// [`AddressError::Db`] if a lookup or the insert fails.
pub async fn find_or_create_for_person(
    db: &SurrealDb,
    new: &NewAddress,
) -> Result<(Address, bool), AddressError> {
    if let Some(person_id) = new.person_id {
        let existing = for_person(db, person_id)
            .await?
            .into_iter()
            .find(|a| a.line1 == new.line1 && a.postal_code == new.postal_code);
        if let Some(found) = existing {
            return Ok((found, false));
        }
    }
    Ok((create(db, new).await?, true))
}

/// Every address, oldest first. The whole table, for listings and tests.
///
/// # Errors
///
/// [`AddressError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Address>, AddressError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AddressRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AddressRow::into_address)
        .collect())
}

/// How many addresses exist. The production-emptiness gate asks this of
/// the engine that holds the table.
///
/// # Errors
///
/// [`AddressError::Db`] if the count fails.
pub async fn count(db: &SurrealDb) -> Result<i64, AddressError> {
    let mut response = db
        .query(format!("SELECT VALUE count() FROM {TABLE} GROUP ALL"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    // `count()` under `GROUP ALL` yields a `{ count: n }` object, so it
    // cannot be taken as a bare integer the way `math::sum` can.
    let counts: Vec<CountRow> = response.take(0)?;
    Ok(counts.first().map_or(0, |c| c.count))
}

/// The one-field shape `SELECT VALUE count() ... GROUP ALL` returns.
#[derive(SurrealValue)]
struct CountRow {
    count: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        count, create, find_by_id, find_or_create_for_entity, find_or_create_for_person,
        for_entity, for_person, list_all, Address, AddressError, NewAddress,
    };
    use crate::persons::{self, NewPerson};
    use crate::surreal::test_support::mem;
    use uuid::Uuid;

    fn at(line1: &str) -> NewAddress {
        NewAddress {
            line1: line1.to_string(),
            city: "Las Vegas".to_string(),
            region: "NV".to_string(),
            postal_code: "89101".to_string(),
            country: "US".to_string(),
            ..NewAddress::default()
        }
    }

    #[tokio::test]
    async fn a_created_address_reads_back_by_id() {
        let db = mem().await;
        let written = create(&db, &at("1 Fremont St")).await.unwrap();
        assert_eq!(written.line1, "1 Fremont St");
        assert_eq!(written.city, "Las Vegas");
        assert_eq!(find_by_id(&db, written.id).await.unwrap(), Some(written));
        assert_eq!(find_by_id(&db, Uuid::now_v7()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_address_may_belong_to_a_person() {
        let db = mem().await;
        let person = persons::create(&db, &NewPerson::new("Libra", "libra@example.com"))
            .await
            .unwrap();
        let mut new = at("1 Fremont St");
        new.person_id = Some(person.id);

        let written = create(&db, &new).await.unwrap();
        assert_eq!(written.person_id, Some(person.id));
    }

    #[tokio::test]
    async fn an_address_for_a_person_who_does_not_exist_is_refused() {
        let db = mem().await;
        let nobody = Uuid::now_v7();
        let mut new = at("1 Fremont St");
        new.person_id = Some(nobody);

        // The engine would accept this link — `record<person>` is not
        // validated against an existing row. This check is the only
        // thing that refuses it.
        let refused = create(&db, &new).await;
        assert!(matches!(
            refused,
            Err(AddressError::NoSuchPerson(id)) if id == nobody
        ));
    }

    #[tokio::test]
    async fn an_address_may_belong_to_neither_a_person_nor_an_entity() {
        let db = mem().await;
        // The seed's mailroom placeholder is exactly this row, which is
        // why the schema carries no XOR assert.
        let written = create(&db, &at("(via mailroom: Reno)")).await.unwrap();
        assert_eq!(written.person_id, None);
        assert_eq!(written.entity_id, None);
    }

    #[tokio::test]
    async fn an_entity_id_is_stored_and_filtered_without_being_a_link() {
        let db = mem().await;
        // `entity_id` is a bare id rather than a record link, so it names
        // no Surreal row and is accepted unenforced.
        let entity_id = Uuid::now_v7();
        let mut new = at("1 Fremont St");
        new.entity_id = Some(entity_id);
        let written = create(&db, &new).await.unwrap();

        assert_eq!(for_entity(&db, entity_id).await.unwrap(), vec![written]);
        assert!(for_entity(&db, Uuid::now_v7()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn find_or_create_matches_the_seeders_natural_key() {
        let db = mem().await;
        let entity_id = Uuid::now_v7();
        let mut new = at("1 Fremont St");
        new.entity_id = Some(entity_id);

        let (first, created) = find_or_create_for_entity(&db, &new).await.unwrap();
        assert!(created, "the first call inserts");
        let (second, created_again) = find_or_create_for_entity(&db, &new).await.unwrap();
        assert!(!created_again, "the second call finds");
        assert_eq!(first, second);
        assert_eq!(list_all(&db).await.unwrap().len(), 1);

        // A different street at the same entity is a different address.
        let mut other = at("2 Fremont St");
        other.entity_id = Some(entity_id);
        let (_, created_other) = find_or_create_for_entity(&db, &other).await.unwrap();
        assert!(created_other);
        assert_eq!(list_all(&db).await.unwrap().len(), 2);
    }

    /// An entity keeps as many addresses as it answers mail at, and reading
    /// them back returns every one.
    ///
    /// This is a capability, not an accident: nothing caps the count —
    /// `address.entity_id` carries no unique index in `navigator.surql` —
    /// and `find_or_create_for_entity` is a seeder's natural key rather
    /// than a one-address-per-entity constraint.
    /// The brand seed leans on that to give one partnership a box in four
    /// states, so a change that makes an entity hold a single address breaks
    /// the firm's own postal identity. Asserted here at the store seam rather
    /// than only against seeded data, because the guarantee belongs to the
    /// table and not to one YAML file.
    #[tokio::test]
    async fn an_entity_keeps_every_address_it_answers_mail_at() {
        let db = mem().await;
        let entity_id = Uuid::now_v7();
        let in_state = |line1: &str, region: &str, zip: &str| {
            let mut new = at(line1);
            new.entity_id = Some(entity_id);
            new.region = region.to_string();
            new.postal_code = zip.to_string();
            new
        };
        let boxes = [
            in_state("5150 Mae Anne Ave Ste 405-9777", "NV", "89523"),
            in_state("1990 N California Blvd Ste 800", "CA", "94596"),
            in_state("12 E 49th St 18th Floor", "NY", "10017"),
            in_state("720 Seneca St Ste 107-715", "WA", "98101"),
        ];
        for one in &boxes {
            assert!(
                find_or_create_for_entity(&db, one).await.unwrap().1,
                "each jurisdiction inserts its own row"
            );
        }

        let mut held: Vec<String> = for_entity(&db, entity_id)
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.region)
            .collect();
        held.sort();
        assert_eq!(
            held,
            ["CA", "NV", "NY", "WA"],
            "for_entity returns every address the entity holds, not the first"
        );

        // Re-seeding finds all four rather than duplicating any: holding
        // several addresses must not cost idempotency.
        for one in &boxes {
            assert!(!find_or_create_for_entity(&db, one).await.unwrap().1);
        }
        assert_eq!(for_entity(&db, entity_id).await.unwrap().len(), 4);

        // The natural key is the (street, ZIP) pair, so one street in two ZIPs
        // is two addresses. Matching on street alone would silently swallow
        // the second.
        let same_street_elsewhere = in_state("12 E 49th St 18th Floor", "NY", "10017-2452");
        assert!(
            find_or_create_for_entity(&db, &same_street_elsewhere)
                .await
                .unwrap()
                .1
        );
        assert_eq!(for_entity(&db, entity_id).await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn counting_an_empty_table_is_zero_rather_than_an_error() {
        let db = mem().await;
        assert_eq!(count(&db).await.unwrap(), 0);
        create(&db, &at("1 Fremont St")).await.unwrap();
        create(&db, &at("2 Fremont St")).await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn listing_is_ordered_oldest_first() {
        let db = mem().await;
        let first = create(&db, &at("1 Fremont St")).await.unwrap();
        let second = create(&db, &at("2 Fremont St")).await.unwrap();
        let ids: Vec<_> = list_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|a: Address| a.id)
            .collect();
        assert_eq!(ids, [first.id, second.id]);
    }

    /// [`for_entity`]'s counterpart for a person (LAW-57).
    #[tokio::test]
    async fn for_person_returns_every_address_that_person_holds() {
        let db = mem().await;
        let person = persons::create(&db, &NewPerson::new("Libra", "libra@example.com"))
            .await
            .unwrap();
        let mut home = at("1 Fremont St");
        home.person_id = Some(person.id);
        let written = create(&db, &home).await.unwrap();

        assert_eq!(for_person(&db, person.id).await.unwrap(), vec![written]);
        assert!(for_person(&db, Uuid::now_v7()).await.unwrap().is_empty());
    }

    /// `navigator site import`'s natural key for the `address` seed model
    /// (LAW-57): a person's (street, ZIP) pair.
    #[tokio::test]
    async fn find_or_create_for_person_matches_the_seeders_natural_key() {
        let db = mem().await;
        let person = persons::create(&db, &NewPerson::new("Libra", "libra@example.com"))
            .await
            .unwrap();
        let mut new = at("1 Fremont St");
        new.person_id = Some(person.id);

        let (first, created) = find_or_create_for_person(&db, &new).await.unwrap();
        assert!(created, "the first call inserts");
        let (second, created_again) = find_or_create_for_person(&db, &new).await.unwrap();
        assert!(!created_again, "the second call finds");
        assert_eq!(first, second);
        assert_eq!(for_person(&db, person.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn find_or_create_for_person_refuses_a_person_who_does_not_exist() {
        let db = mem().await;
        let nobody = Uuid::now_v7();
        let mut new = at("1 Fremont St");
        new.person_id = Some(nobody);

        let refused = find_or_create_for_person(&db, &new).await;
        assert!(matches!(
            refused,
            Err(AddressError::NoSuchPerson(id)) if id == nobody
        ));
    }
}
