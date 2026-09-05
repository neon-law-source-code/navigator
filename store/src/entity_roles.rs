//! The `entity_role` relation — a Person's structural tie to an Entity
//! (manages, owns, member-of), and one of the two edge tables the
//! pre-matter conflict check walks.
//!
//! # This is a relation, not a table with two ids
//!
//! The tie *is* the edge: `RELATE person->entity_role->entity`, with
//! `in` and `out` carrying the endpoints rather than a surrogate `id`
//! beside two UUID columns. The
//! conflict traversal walks it natively (`<->entity_role`) rather than
//! joining on columns, which is the whole reason the graph could stop
//! being a projection (ENG-120).
//!
//! The surrogate key is deliberately **not** carried across. Nothing
//! referenced `person_entity_roles.id` — it addressed no row and named
//! no edge in any surface — so re-minting it here would be a column
//! kept for the shape of the table it came from. The identity of a tie
//! is its endpoints plus its `role`, which is exactly what the UNIQUE
//! `entity_role_tie` index says.
//!
//! # Idempotence is the contract
//!
//! `store::seed` and `import::apply` both re-run over a live database
//! and must not duplicate a tie. A hand-rolled find-then-insert is a race
//! with no constraint behind it, so
//! the index closes it and [`grant`] re-reads the winner's row when it
//! loses — the pattern `store::persons::find_or_create` established.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const TABLE: &str = "entity_role";
const PERSON_TABLE: &str = "person";
const ENTITY_TABLE: &str = "entity";

/// One structural tie between a Person and an Entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityRole {
    pub person_id: Uuid,
    pub entity_id: Uuid,
    /// Role token (`manages`, `member`, `beneficiary`, …). Open by
    /// design: the vocabulary grows without a migration, and the
    /// conflict check treats every structural tie the same way.
    pub role: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct EntityRoleRow {
    r#in: surrealdb::types::RecordId,
    out: surrealdb::types::RecordId,
    role: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl EntityRoleRow {
    fn into_role(self) -> Option<EntityRole> {
        Some(EntityRole {
            person_id: record_uuid(&self.r#in)?,
            entity_id: record_uuid(&self.out)?,
            role: self.role,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const SELECT: &str = "in, out, role, inserted_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum EntityRoleError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or one this module
    /// could not read back.
    #[error("writing an entity role returned no usable row")]
    WriteReturnedNothing,
}

fn is_duplicate_tie(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("entity_role_tie")
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<EntityRole>, EntityRoleError> {
    let rows: Vec<EntityRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(EntityRoleRow::into_role)
        .collect())
}

/// Record that `person_id` holds `role` in `entity_id`, returning the
/// tie whether this call created it or found it already there.
///
/// Idempotent and race-safe: the `entity_role_tie` index makes the
/// second writer lose, and the loser re-reads rather than failing. The
/// seed runs this on every boot.
///
/// # Errors
///
/// [`EntityRoleError::Db`] if the lookup or the write fails.
pub async fn grant(
    db: &SurrealDb,
    person_id: Uuid,
    entity_id: Uuid,
    role: &str,
) -> Result<EntityRole, EntityRoleError> {
    let role = role.trim();
    if let Some(existing) = find(db, person_id, entity_id, role).await? {
        return Ok(existing);
    }

    let written = retry::writing(|| {
        db.query(format!(
            "RELATE $person->{TABLE}->$entity \
             SET role = $role, inserted_at = time::now(), updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .bind(("entity", record_id(ENTITY_TABLE, entity_id)))
        .bind(("role", role.to_string()))
    })
    .await;

    match written {
        Ok(mut response) => {
            let rows: Vec<EntityRoleRow> = response.take(0)?;
            rows.into_iter()
                .next()
                .and_then(EntityRoleRow::into_role)
                .ok_or(EntityRoleError::WriteReturnedNothing)
        }
        // Another writer minted the same tie between the read above and
        // this write. Its row is the answer. A unique violation is not
        // retryable, so the shared policy hands it straight back here
        // rather than spending the budget on a write that cannot win.
        Err(error) if is_duplicate_tie(&error) => find(db, person_id, entity_id, role)
            .await?
            .ok_or(EntityRoleError::WriteReturnedNothing),
        Err(error) => Err(EntityRoleError::Db(error)),
    }
}

/// The one tie matching all three parts of the natural key, if any.
///
/// # Errors
///
/// [`EntityRoleError::Db`] if the lookup fails.
pub async fn find(
    db: &SurrealDb,
    person_id: Uuid,
    entity_id: Uuid,
    role: &str,
) -> Result<Option<EntityRole>, EntityRoleError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE in = $person AND out = $entity AND role = $role LIMIT 1"
        ))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .bind(("entity", record_id(ENTITY_TABLE, entity_id)))
        .bind(("role", role.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<EntityRoleRow> = response.take(0)?;
    Ok(rows.into_iter().next().and_then(EntityRoleRow::into_role))
}

/// Every structural tie, ordered so a listing is stable.
///
/// # Errors
///
/// [`EntityRoleError::Db`] if the lookup fails.
pub async fn all(db: &SurrealDb) -> Result<Vec<EntityRole>, EntityRoleError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every tie held by one Person.
///
/// # Errors
///
/// [`EntityRoleError::Db`] if the lookup fails.
pub async fn for_person(
    db: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<EntityRole>, EntityRoleError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE in = $person ORDER BY inserted_at ASC"
        ))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every tie into one Entity.
///
/// # Errors
///
/// [`EntityRoleError::Db`] if the lookup fails.
pub async fn for_entity(
    db: &SurrealDb,
    entity_id: Uuid,
) -> Result<Vec<EntityRole>, EntityRoleError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE out = $entity ORDER BY inserted_at ASC"
        ))
        .bind(("entity", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

#[cfg(test)]
mod tests {
    use super::{all, find, for_entity, for_person, grant};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    async fn person(db: &SurrealDb, name: &str) -> Uuid {
        crate::persons::create(
            db,
            &crate::persons::NewPerson::new(name, format!("{}@example.com", Uuid::now_v7())),
        )
        .await
        .unwrap()
        .id
    }

    async fn entity(db: &SurrealDb, name: &str) -> Uuid {
        crate::entities::create(
            db,
            &crate::entities::NewEntity {
                name: name.into(),
                entity_type_id: Uuid::now_v7(),
                jurisdiction_id: Uuid::now_v7(),
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn a_granted_tie_reads_back_on_both_of_its_ends() {
        let db = mem().await;
        let holder = person(&db, "Tie Holder").await;
        let acme = entity(&db, "Acme LLC").await;

        let tie = grant(&db, holder, acme, "manages").await.unwrap();
        assert_eq!(tie.person_id, holder);
        assert_eq!(tie.entity_id, acme);
        assert_eq!(tie.role, "manages");

        assert_eq!(for_person(&db, holder).await.unwrap(), vec![tie.clone()]);
        assert_eq!(for_entity(&db, acme).await.unwrap(), vec![tie.clone()]);
        assert_eq!(find(&db, holder, acme, "manages").await.unwrap(), Some(tie));
        assert_eq!(find(&db, holder, acme, "owns").await.unwrap(), None);
    }

    /// The seed and the bulk importer both re-run over a live database.
    #[tokio::test]
    async fn granting_the_same_tie_twice_is_one_edge() {
        let db = mem().await;
        let holder = person(&db, "Repeat Holder").await;
        let acme = entity(&db, "Repeat LLC").await;

        let first = grant(&db, holder, acme, "manages").await.unwrap();
        let second = grant(&db, holder, acme, "manages").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(all(&db).await.unwrap().len(), 1);
    }

    /// The find-then-insert this replaced had no constraint behind it,
    /// so two concurrent seeds could both write. The index closes that,
    /// and the loser adopts the winner's row rather than failing.
    #[tokio::test]
    async fn concurrent_grants_of_one_tie_settle_on_a_single_edge() {
        let db = mem().await;
        let holder = person(&db, "Racing Holder").await;
        let acme = entity(&db, "Racing LLC").await;

        let (first, second) = tokio::join!(
            grant(&db, holder, acme, "manages"),
            grant(&db, holder, acme, "manages"),
        );
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(all(&db).await.unwrap().len(), 1);
    }

    /// A person may hold several roles in one entity, so the tie is the
    /// triple rather than the pair — the index must not collapse them.
    #[tokio::test]
    async fn one_person_may_hold_several_roles_in_one_entity() {
        let db = mem().await;
        let holder = person(&db, "Many Hats").await;
        let acme = entity(&db, "Many Hats LLC").await;

        grant(&db, holder, acme, "manages").await.unwrap();
        grant(&db, holder, acme, "owns").await.unwrap();

        let mut roles: Vec<String> = for_person(&db, holder)
            .await
            .unwrap()
            .into_iter()
            .map(|tie| tie.role)
            .collect();
        roles.sort();
        assert_eq!(roles, ["manages", "owns"]);
    }

    /// The edge has to be a real graph edge, not two columns that
    /// happen to hold ids: the conflict traversal walks it with
    /// `->entity_role->`, which only resolves if `in`/`out` were
    /// written as links to records that exist.
    #[tokio::test]
    async fn the_tie_is_traversable_as_a_graph_edge() {
        let db = mem().await;
        let holder = person(&db, "Graph Holder").await;
        let acme = entity(&db, "Graph LLC").await;
        grant(&db, holder, acme, "manages").await.unwrap();

        let reached: Option<Vec<String>> = db
            .query("SELECT VALUE ->entity_role->entity.name FROM $person")
            .bind(("person", crate::surreal::record_id("person", holder)))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            reached,
            Some(vec!["Graph LLC".to_string()]),
            "the edge's `out` link did not resolve to the entity row"
        );
    }
}
