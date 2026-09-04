//! Owning-practice records and firm membership.
//!
//! A [`Firm`] is the ownership boundary for Projects and for the people who
//! work them. It sits beneath the house-brand registry: a brand is which
//! storefront a client walked through; a firm is which practice owns the
//! matter. A Firm is an Entity: [`Firm::entity_id`] is the legal person that
//! practice is. [`firm_brand`](attach_brand) records which closed house-brand
//! keys that practice wears.
//!
//! Membership is a join table, shaped like `person_project_role`. It does
//! not replace `person.role`. The deployment-wide Owner tier stays on
//! `person`; a client reaches a matter through `person_project_role` and
//! does not get a firm-membership row.

use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::Role;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

pub(crate) const TABLE: &str = "firm";
const MEMBERSHIP_TABLE: &str = "person_firm_role";
const PERSON_TABLE: &str = "person";
const BRAND_TABLE: &str = "firm_brand";
const ENTITY_TABLE: &str = "entity";
const FIRM_SELECT: &str = "id, name, status, entity_id, inserted_at, updated_at";
const MEMBERSHIP_SELECT: &str =
    "id, person_id, firm_id, membership, is_dri, inserted_at, updated_at";

/// Closed house-brand keys a firm may wear. Matches the `ASSERT` on
/// `firm_brand.brand_key` and `project.brand`. `store` does not depend on
/// `views`, so this is the string form of `BrandKey::ALL`.
pub const CLOSED_BRAND_KEYS: &[&str] = &["neon", "delete-your-data"];

/// A practice that owns Projects and firm-side people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Firm {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    /// The legal Entity this practice is. `None` only on a historical row
    /// written before `entity_id` existed.
    pub entity_id: Option<Uuid>,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct FirmRow {
    id: surrealdb::types::RecordId,
    name: String,
    status: String,
    entity_id: Option<surrealdb::types::RecordId>,
    inserted_at: String,
    updated_at: String,
}

impl FirmRow {
    fn into_firm(self) -> Option<Firm> {
        Some(Firm {
            id: record_uuid(&self.id)?,
            name: self.name,
            status: self.status,
            entity_id: match self.entity_id.as_ref() {
                Some(id) => Some(record_uuid(id)?),
                None => None,
            },
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(SurrealValue)]
struct ProjectIdRow {
    id: surrealdb::types::RecordId,
}

#[derive(SurrealValue)]
struct PersonIdRow {
    person_id: surrealdb::types::RecordId,
}

#[derive(SurrealValue)]
struct TouchedProject {
    id: surrealdb::types::RecordId,
}

/// Inputs for creating a [`Firm`].
#[derive(Debug, Clone)]
pub struct NewFirm {
    pub name: String,
    pub status: String,
    pub entity_id: Uuid,
}

/// Which membership a person holds at a firm.
///
/// Distinct from [`crate::persons::Role`]: that enum is the system-wide
/// authorization tier, including Owner and Client. This closed set is
/// only the practice-side memberships a `person_firm_role` row may
/// carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FirmMembership {
    Admin,
    Lawyer,
    Clerk,
}

impl FirmMembership {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Lawyer => "lawyer",
            Self::Clerk => "clerk",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "admin" => Some(Self::Admin),
            "lawyer" => Some(Self::Lawyer),
            "clerk" => Some(Self::Clerk),
            _ => None,
        }
    }

    /// The practice membership that corresponds to a system-wide [`Role`].
    /// Owner and Client have none: Owner is deployment-wide, and a client
    /// reaches a matter through `person_project_role`.
    #[must_use]
    pub fn for_role(role: Role) -> Option<Self> {
        match role {
            Role::Admin => Some(Self::Admin),
            Role::Lawyer => Some(Self::Lawyer),
            Role::Clerk => Some(Self::Clerk),
            Role::Owner | Role::Client => None,
        }
    }
}

/// One person's membership at a firm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersonFirmRole {
    pub id: Uuid,
    pub person_id: Uuid,
    pub firm_id: Uuid,
    pub membership: FirmMembership,
    pub is_dri: bool,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct PersonFirmRoleRow {
    id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    firm_id: surrealdb::types::RecordId,
    membership: String,
    is_dri: bool,
    inserted_at: String,
    updated_at: String,
}

impl PersonFirmRoleRow {
    fn into_role(self) -> Option<PersonFirmRole> {
        Some(PersonFirmRole {
            id: record_uuid(&self.id)?,
            person_id: record_uuid(&self.person_id)?,
            firm_id: record_uuid(&self.firm_id)?,
            membership: FirmMembership::parse(&self.membership)?,
            is_dri: self.is_dri,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

/// Inputs for creating a [`PersonFirmRole`].
#[derive(Debug, Clone)]
pub struct NewPersonFirmRole {
    pub person_id: Uuid,
    pub firm_id: Uuid,
    pub membership: FirmMembership,
    pub is_dri: bool,
}

/// Errors from the firm command seam.
#[derive(Debug, thiserror::Error)]
pub enum FirmError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Person(#[from] crate::persons::PersonError),
    #[error(transparent)]
    Entity(#[from] crate::entities::EntityError),
    #[error("writing a firm returned no usable row")]
    WriteReturnedNothing,
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    #[error("no firm {0}")]
    NoSuchFirm(Uuid),
    #[error("no entity {0}")]
    NoSuchEntity(Uuid),
    #[error("that person is already a member of this firm")]
    DuplicateMembership,
    #[error("that entity already has a firm")]
    DuplicateEntity,
    #[error("unknown brand key {0}")]
    UnknownBrand(String),
    #[error("that brand is already attached to a firm")]
    DuplicateBrand,
}

fn classify_write(error: surrealdb::Error) -> FirmError {
    let message = error.to_string();
    if message.contains("person_firm_role_pair") {
        FirmError::DuplicateMembership
    } else if message.contains("firm_entity") {
        FirmError::DuplicateEntity
    } else if message.contains("firm_brand_key") || message.contains("firm_brand_pair") {
        FirmError::DuplicateBrand
    } else {
        FirmError::Db(error)
    }
}

async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, FirmError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Create a firm under a fresh UUID record key.
pub async fn create(surreal: &SurrealDb, input: &NewFirm) -> Result<Firm, FirmError> {
    if crate::entities::find_by_id(surreal, input.entity_id)
        .await?
        .is_none()
    {
        return Err(FirmError::NoSuchEntity(input.entity_id));
    }
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET name = $name, status = $status, entity_id = $entity_id, \
                 inserted_at = $inserted_at, updated_at = $updated_at \
                 RETURN {FIRM_SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
            .bind(("entity_id", record_id(ENTITY_TABLE, input.entity_id)))
            .bind(("inserted_at", now.clone()))
            .bind(("updated_at", now.clone()))
    })
    .await?;
    let row: Option<FirmRow> = response.take(0)?;
    row.and_then(FirmRow::into_firm)
        .ok_or(FirmError::WriteReturnedNothing)
}

/// Find the firm identified by `id`.
pub async fn find_by_id(surreal: &SurrealDb, id: Uuid) -> Result<Option<Firm>, FirmError> {
    let mut response = surreal
        .query(format!("SELECT {FIRM_SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<FirmRow> = response.take(0)?;
    Ok(row.and_then(FirmRow::into_firm))
}

/// Find the firm whose [`Firm::entity_id`] is `entity_id`.
pub async fn find_by_entity_id(
    surreal: &SurrealDb,
    entity_id: Uuid,
) -> Result<Option<Firm>, FirmError> {
    let mut response = surreal
        .query(format!(
            "SELECT {FIRM_SELECT} FROM ONLY {TABLE} WHERE entity_id = $entity_id LIMIT 1"
        ))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<FirmRow> = response.take(0)?;
    Ok(row.and_then(FirmRow::into_firm))
}

/// Every firm, name then id.
pub async fn all(surreal: &SurrealDb) -> Result<Vec<Firm>, FirmError> {
    let mut response = surreal
        .query(format!(
            "SELECT {FIRM_SELECT} FROM {TABLE} ORDER BY name, id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<FirmRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(FirmRow::into_firm).collect())
}

/// Record one person's membership at a firm.
///
/// Reads both referenced rows before writing: a `record<>` link constrains
/// the target table but does not prove the row exists.
pub async fn add_membership(
    surreal: &SurrealDb,
    input: &NewPersonFirmRole,
) -> Result<PersonFirmRole, FirmError> {
    if crate::persons::find_by_id(surreal, input.person_id)
        .await?
        .is_none()
    {
        return Err(FirmError::NoSuchPerson(input.person_id));
    }
    if find_by_id(surreal, input.firm_id).await?.is_none() {
        return Err(FirmError::NoSuchFirm(input.firm_id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET person_id = $person_id, firm_id = $firm_id, \
                 membership = $membership, is_dri = $is_dri, \
                 inserted_at = $now, updated_at = $now RETURN {MEMBERSHIP_SELECT}"
            ))
            .bind(("id", record_id(MEMBERSHIP_TABLE, Uuid::now_v7())))
            .bind(("person_id", record_id(PERSON_TABLE, input.person_id)))
            .bind(("firm_id", record_id(TABLE, input.firm_id)))
            .bind(("membership", input.membership.as_str().to_string()))
            .bind(("is_dri", input.is_dri))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<PersonFirmRoleRow> = response.take(0)?;
    row.and_then(PersonFirmRoleRow::into_role)
        .ok_or(FirmError::WriteReturnedNothing)
}

/// The membership row for this `(person, firm)` pair, if any.
pub async fn membership_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
    firm_id: Uuid,
) -> Result<Option<PersonFirmRole>, FirmError> {
    let mut response = surreal
        .query(format!(
            "SELECT {MEMBERSHIP_SELECT} FROM ONLY {MEMBERSHIP_TABLE} \
             WHERE person_id = $person_id AND firm_id = $firm_id LIMIT 1"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("firm_id", record_id(TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<PersonFirmRoleRow> = response.take(0)?;
    Ok(row.and_then(PersonFirmRoleRow::into_role))
}

/// Every membership row for this person.
pub async fn memberships_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<PersonFirmRole>, FirmError> {
    let mut response = surreal
        .query(format!(
            "SELECT {MEMBERSHIP_SELECT} FROM {MEMBERSHIP_TABLE} \
             WHERE person_id = $person_id ORDER BY inserted_at, id"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonFirmRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonFirmRoleRow::into_role)
        .collect())
}

/// Firm ids this person belongs to.
pub async fn firm_ids_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<Uuid>, FirmError> {
    Ok(memberships_for_person(surreal, person_id)
        .await?
        .into_iter()
        .map(|row| row.firm_id)
        .collect())
}

/// Person ids an Admin of these firms may see: members of those firms, plus
/// anyone with a `person_project_role` on a matter those firms own.
pub async fn visible_person_ids(
    surreal: &SurrealDb,
    admin_person_id: Uuid,
) -> Result<Vec<Uuid>, FirmError> {
    let firm_ids = firm_ids_for_person(surreal, admin_person_id).await?;
    if firm_ids.is_empty() {
        return Ok(Vec::new());
    }
    let firm_records: Vec<_> = firm_ids
        .iter()
        .copied()
        .map(|id| record_id(TABLE, id))
        .collect();
    let mut members = surreal
        .query(format!(
            "SELECT {MEMBERSHIP_SELECT} FROM {MEMBERSHIP_TABLE} \
             WHERE firm_id IN $firm_ids"
        ))
        .bind(("firm_ids", firm_records.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let member_rows: Vec<PersonFirmRoleRow> = members.take(0)?;
    let mut ids: std::collections::BTreeSet<Uuid> = member_rows
        .into_iter()
        .filter_map(PersonFirmRoleRow::into_role)
        .map(|row| row.person_id)
        .collect();

    let mut projects = surreal
        .query("SELECT id FROM project WHERE firm_id IN $firm_ids")
        .bind(("firm_ids", firm_records))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let project_rows: Vec<ProjectIdRow> = projects.take(0)?;
    let project_ids: Vec<_> = project_rows.into_iter().map(|row| row.id).collect();
    if !project_ids.is_empty() {
        let mut participants = surreal
            .query("SELECT person_id FROM person_project_role WHERE project_id IN $project_ids")
            .bind(("project_ids", project_ids))
            .await
            .and_then(surrealdb::IndexedResults::check)?;
        let participant_rows: Vec<PersonIdRow> = participants.take(0)?;
        for row in participant_rows {
            if let Some(id) = record_uuid(&row.person_id) {
                ids.insert(id);
            }
        }
    }
    Ok(ids.into_iter().collect())
}

/// Attach a closed house-brand key to a firm.
pub async fn attach_brand(
    surreal: &SurrealDb,
    firm_id: Uuid,
    brand_key: &str,
) -> Result<(), FirmError> {
    if !CLOSED_BRAND_KEYS.contains(&brand_key) {
        return Err(FirmError::UnknownBrand(brand_key.to_string()));
    }
    if find_by_id(surreal, firm_id).await?.is_none() {
        return Err(FirmError::NoSuchFirm(firm_id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    writing(|| {
        surreal
            .query(
                "CREATE $id SET firm_id = $firm_id, brand_key = $brand_key, \
                 inserted_at = $now, updated_at = $now",
            )
            .bind(("id", record_id(BRAND_TABLE, Uuid::now_v7())))
            .bind(("firm_id", record_id(TABLE, firm_id)))
            .bind(("brand_key", brand_key.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    Ok(())
}

/// Attach `brand_key` if it is not already on this firm. A key already
/// worn by this firm is a no-op; a key worn by another firm is still an
/// error.
pub async fn ensure_brand(
    surreal: &SurrealDb,
    firm_id: Uuid,
    brand_key: &str,
) -> Result<(), FirmError> {
    let existing = brand_keys_for_firm(surreal, firm_id).await?;
    if existing.iter().any(|key| key == brand_key) {
        return Ok(());
    }
    attach_brand(surreal, firm_id, brand_key).await
}

/// House-brand keys this firm wears, in registry order.
pub async fn brand_keys_for_firm(
    surreal: &SurrealDb,
    firm_id: Uuid,
) -> Result<Vec<String>, FirmError> {
    #[derive(SurrealValue)]
    struct BrandRow {
        brand_key: String,
    }
    let mut response = surreal
        .query(format!(
            "SELECT brand_key FROM {BRAND_TABLE} WHERE firm_id = $firm_id ORDER BY brand_key"
        ))
        .bind(("firm_id", record_id(TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().map(|row| row.brand_key).collect())
}

/// Point every project that still has no owner at `firm_id`. Idempotent.
pub async fn backfill_unowned_projects(
    surreal: &SurrealDb,
    firm_id: Uuid,
) -> Result<u64, FirmError> {
    if find_by_id(surreal, firm_id).await?.is_none() {
        return Err(FirmError::NoSuchFirm(firm_id));
    }
    let mut response = writing(|| {
        surreal
            .query(
                "UPDATE project SET firm_id = $firm_id \
                 WHERE firm_id IS NONE RETURN AFTER",
            )
            .bind(("firm_id", record_id(TABLE, firm_id)))
    })
    .await?;
    let rows: Vec<TouchedProject> = response.take(0).unwrap_or_default();
    Ok(rows.len() as u64)
}

/// Grant membership when missing. A duplicate pair is a no-op.
pub async fn ensure_membership(
    surreal: &SurrealDb,
    input: &NewPersonFirmRole,
) -> Result<(), FirmError> {
    match add_membership(surreal, input).await {
        Ok(_) | Err(FirmError::DuplicateMembership) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persons::{NewPerson, Role};
    use crate::projects::{self, NewProject};
    use crate::schema::apply;
    use crate::surreal::test_support::unmigrated;
    use crate::test_support::{mem_surreal, seed_entity};

    async fn practice(db: &SurrealDb, name: &str) -> Firm {
        let entity_id = seed_entity(db).await;
        create(
            db,
            &NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_round_trips_a_firm() {
        let db = mem_surreal().await;
        let created = practice(&db, "Shook Law PLLC").await;
        assert_eq!(created.name, "Shook Law PLLC");
        assert_eq!(created.status, "active");
        assert!(created.entity_id.is_some());
        let reloaded = find_by_id(&db, created.id).await.unwrap().unwrap();
        assert_eq!(reloaded, created);
        assert_eq!(
            find_by_entity_id(&db, created.entity_id.unwrap())
                .await
                .unwrap()
                .unwrap()
                .id,
            created.id
        );
    }

    #[tokio::test]
    async fn membership_round_trips_and_refuses_a_duplicate_pair() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice One").await;
        let person = crate::persons::create(
            &db,
            &NewPerson::with_role("Pat Lawyer", "pat@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        let row = add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: person.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(row.person_id, person.id);
        assert_eq!(row.firm_id, firm.id);
        assert_eq!(row.membership, FirmMembership::Lawyer);
        assert!(row.is_dri);
        let reloaded = membership_for_person(&db, person.id, firm.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded, row);

        let duplicate = add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: person.id,
                firm_id: firm.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate, FirmError::DuplicateMembership));
    }

    #[tokio::test]
    async fn membership_refuses_a_dangling_person_or_firm() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice Two").await;
        let missing_person = Uuid::now_v7();
        let err = add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: missing_person,
                firm_id: firm.id,
                membership: FirmMembership::Clerk,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchPerson(id) if id == missing_person));

        let person = crate::persons::create(
            &db,
            &NewPerson::with_role("Kim Clerk", "kim@example.com", Role::Clerk),
        )
        .await
        .unwrap();
        let missing_firm = Uuid::now_v7();
        let err = add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: person.id,
                firm_id: missing_firm,
                membership: FirmMembership::Clerk,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchFirm(id) if id == missing_firm));
    }

    #[tokio::test]
    async fn project_firm_id_round_trips() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice Three").await;
        let entity_id = seed_entity(&db).await;
        let created = projects::create(
            &db,
            &NewProject {
                code: "owned-matter".to_string(),
                name: "Owned Matter".to_string(),
                status: "open".to_string(),
                entity_id,
                firm_id: Some(firm.id),
                ..NewProject::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.firm_id, Some(firm.id));
        let reloaded = projects::find_by_id(&db, created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.firm_id, Some(firm.id));
    }

    #[tokio::test]
    async fn create_refuses_a_dangling_firm_id() {
        let db = mem_surreal().await;
        let entity_id = seed_entity(&db).await;
        let missing = Uuid::now_v7();
        let err = projects::create(
            &db,
            &NewProject {
                code: "orphan-matter".to_string(),
                name: "Orphan Matter".to_string(),
                status: "open".to_string(),
                entity_id,
                firm_id: Some(missing),
                ..NewProject::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            projects::ProjectStoreError::NoSuchFirm(id) if id == missing
        ));
    }

    /// Drop the definition, write the row, put the definition back — the
    /// same historical-row shape `project.brand` uses. An absent `firm_id`
    /// reads as `None` rather than failing deserialize.
    #[tokio::test]
    async fn reads_a_project_row_written_before_firm_id_was_defined() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        db.query("REMOVE FIELD firm_id ON project").await.unwrap();
        let id = Uuid::now_v7();
        let entity_id = Uuid::now_v7();
        db.query(
            "CREATE $id SET code = 'pre-firm-matter', name = 'Pre-Firm Matter', \
             status = 'open', entity_id = $entity_id, \
             inserted_at = '2026-09-04T00:00:00Z', updated_at = '2026-09-04T00:00:00Z'",
        )
        .bind(("id", record_id("project", id)))
        .bind(("entity_id", record_id("entity", entity_id)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query("DEFINE FIELD OVERWRITE firm_id ON project TYPE option<record<firm>>")
            .await
            .unwrap();

        let project = projects::find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(project.firm_id, None);
    }

    #[tokio::test]
    async fn create_requires_a_live_entity_and_refuses_a_second_firm_on_it() {
        let db = mem_surreal().await;
        let missing = Uuid::now_v7();
        let err = create(
            &db,
            &NewFirm {
                name: "Ghost Practice".to_string(),
                status: "active".to_string(),
                entity_id: missing,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchEntity(id) if id == missing));

        let entity_id = seed_entity(&db).await;
        create(
            &db,
            &NewFirm {
                name: "First".to_string(),
                status: "active".to_string(),
                entity_id,
            },
        )
        .await
        .unwrap();
        let duplicate = create(
            &db,
            &NewFirm {
                name: "Second".to_string(),
                status: "active".to_string(),
                entity_id,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate, FirmError::DuplicateEntity));
    }

    #[tokio::test]
    async fn reads_a_firm_row_written_before_entity_id_was_defined() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        db.query("REMOVE FIELD entity_id ON firm").await.unwrap();
        let id = Uuid::now_v7();
        db.query(
            "CREATE $id SET name = 'Pre-Entity Firm', status = 'active', \
             inserted_at = '2026-09-04T00:00:00Z', updated_at = '2026-09-04T00:00:00Z'",
        )
        .bind(("id", record_id(TABLE, id)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query("DEFINE FIELD OVERWRITE entity_id ON firm TYPE option<record<entity>>")
            .await
            .unwrap();

        let firm = find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(firm.name, "Pre-Entity Firm");
        assert_eq!(firm.entity_id, None);
    }

    #[tokio::test]
    async fn attaches_closed_brand_keys_and_refuses_an_unknown_or_taken_key() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Brand Holder").await;
        attach_brand(&db, firm.id, "neon").await.unwrap();
        assert_eq!(
            brand_keys_for_firm(&db, firm.id).await.unwrap(),
            vec!["neon".to_string()]
        );
        ensure_brand(&db, firm.id, "neon").await.unwrap();
        let unknown = attach_brand(&db, firm.id, "not-a-brand").await.unwrap_err();
        assert!(matches!(unknown, FirmError::UnknownBrand(key) if key == "not-a-brand"));

        let other = practice(&db, "Other Practice").await;
        let taken = attach_brand(&db, other.id, "neon").await.unwrap_err();
        assert!(matches!(taken, FirmError::DuplicateBrand));
    }

    #[tokio::test]
    async fn admin_visibility_stays_inside_the_admin_s_firms() {
        let db = mem_surreal().await;
        let firm_a = practice(&db, "Practice A").await;
        let firm_b = practice(&db, "Practice B").await;
        let admin_a = crate::persons::create(
            &db,
            &NewPerson::with_role("Admin A", "admin-a@example.com", Role::Admin),
        )
        .await
        .unwrap();
        let lawyer_a = crate::persons::create(
            &db,
            &NewPerson::with_role("Lawyer A", "lawyer-a@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        let lawyer_b = crate::persons::create(
            &db,
            &NewPerson::with_role("Lawyer B", "lawyer-b@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: admin_a.id,
                firm_id: firm_a.id,
                membership: FirmMembership::Admin,
                is_dri: true,
            },
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer_a.id,
                firm_id: firm_a.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer_b.id,
                firm_id: firm_b.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();

        let visible = visible_person_ids(&db, admin_a.id).await.unwrap();
        assert!(visible.contains(&admin_a.id));
        assert!(visible.contains(&lawyer_a.id));
        assert!(!visible.contains(&lawyer_b.id));
    }

    #[tokio::test]
    async fn backfill_points_unowned_projects_at_the_firm_once() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Backfill Practice").await;
        let entity_id = seed_entity(&db).await;
        let project = projects::create(
            &db,
            &NewProject {
                code: "unowned-matter".to_string(),
                name: "Unowned Matter".to_string(),
                status: "open".to_string(),
                entity_id,
                ..NewProject::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(project.firm_id, None);
        assert_eq!(backfill_unowned_projects(&db, firm.id).await.unwrap(), 1);
        assert_eq!(
            projects::find_by_id(&db, project.id)
                .await
                .unwrap()
                .unwrap()
                .firm_id,
            Some(firm.id)
        );
        assert_eq!(backfill_unowned_projects(&db, firm.id).await.unwrap(), 0);
    }
}
