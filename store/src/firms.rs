//! Owning-practice records and firm membership.
//!
//! A [`Firm`] is the ownership boundary for Projects and for the people who
//! work them. It sits beneath the house-brand registry: a brand is which
//! storefront a client walked through; a firm is which practice owns the
//! matter. Binding a brand key to a firm row is a later change.
//!
//! Membership is a join table, shaped like `person_project_role`. It does
//! not replace `person.role`. The deployment-wide Owner tier stays on
//! `person`; a client reaches a matter through `person_project_role` and
//! does not get a firm-membership row.

use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

pub(crate) const TABLE: &str = "firm";
const MEMBERSHIP_TABLE: &str = "person_firm_role";
const PERSON_TABLE: &str = "person";
const FIRM_SELECT: &str = "id, name, status, inserted_at, updated_at";
const MEMBERSHIP_SELECT: &str =
    "id, person_id, firm_id, membership, is_dri, inserted_at, updated_at";

/// A practice that owns Projects and firm-side people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Firm {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct FirmRow {
    id: surrealdb::types::RecordId,
    name: String,
    status: String,
    inserted_at: String,
    updated_at: String,
}

impl FirmRow {
    fn into_firm(self) -> Option<Firm> {
        Some(Firm {
            id: record_uuid(&self.id)?,
            name: self.name,
            status: self.status,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

/// Inputs for creating a [`Firm`].
#[derive(Debug, Clone)]
pub struct NewFirm {
    pub name: String,
    pub status: String,
}

impl Default for NewFirm {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: "active".to_string(),
        }
    }
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
    #[error("writing a firm returned no usable row")]
    WriteReturnedNothing,
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    #[error("no firm {0}")]
    NoSuchFirm(Uuid),
    #[error("that person is already a member of this firm")]
    DuplicateMembership,
}

fn classify_write(error: surrealdb::Error) -> FirmError {
    let message = error.to_string();
    if message.contains("person_firm_role_pair") {
        FirmError::DuplicateMembership
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
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET name = $name, status = $status, \
                 inserted_at = $inserted_at, updated_at = $updated_at \
                 RETURN {FIRM_SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persons::{NewPerson, Role};
    use crate::projects::{self, NewProject};
    use crate::schema::apply;
    use crate::surreal::test_support::unmigrated;
    use crate::test_support::{mem_surreal, seed_entity};

    #[tokio::test]
    async fn create_round_trips_a_firm() {
        let db = mem_surreal().await;
        let created = create(
            &db,
            &NewFirm {
                name: "Shook Law PLLC".to_string(),
                status: "active".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.name, "Shook Law PLLC");
        assert_eq!(created.status, "active");
        let reloaded = find_by_id(&db, created.id).await.unwrap().unwrap();
        assert_eq!(reloaded, created);
    }

    #[tokio::test]
    async fn membership_round_trips_and_refuses_a_duplicate_pair() {
        let db = mem_surreal().await;
        let firm = create(
            &db,
            &NewFirm {
                name: "Practice One".to_string(),
                ..NewFirm::default()
            },
        )
        .await
        .unwrap();
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
        let firm = create(
            &db,
            &NewFirm {
                name: "Practice Two".to_string(),
                ..NewFirm::default()
            },
        )
        .await
        .unwrap();
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
        let firm = create(
            &db,
            &NewFirm {
                name: "Practice Three".to_string(),
                ..NewFirm::default()
            },
        )
        .await
        .unwrap();
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
}
