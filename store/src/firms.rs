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
///
/// Creation is atomic with an initial Admin DRI (ENG-499): there is no
/// setup state a Firm passes through before it has one.
/// `admin_dri_person_id` must name a person carrying `person.role = admin`;
/// [`create`] refuses anything else rather than leaving the new Firm without
/// its one accountable administrator.
#[derive(Debug, Clone)]
pub struct NewFirm {
    pub name: String,
    pub status: String,
    pub entity_id: Uuid,
    /// The person who becomes this Firm's first Admin DRI, in the same
    /// transaction that creates the Firm.
    pub admin_dri_person_id: Uuid,
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
    #[error("person {0} is a client and cannot hold a firm membership")]
    ClientCannotJoinFirm(Uuid),
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
    /// The proposed Admin DRI does not carry `person.role = admin`. Refused
    /// for Owner, Lawyer, Clerk, and Client alike (ENG-499).
    #[error("person {0} does not hold the admin role and cannot be an Admin DRI")]
    IneligibleAdminDriTier(Uuid),
    /// The person holds a `person_firm_role` row on this Firm, but its
    /// membership is not `admin`.
    #[error("person {0} is not an admin member of firm {1}")]
    WrongAdminDriMembership(Uuid, Uuid),
    /// The person holds no membership row on this Firm at all. Distinct from
    /// [`Self::WrongAdminDriMembership`]: this person is not a member of the
    /// target Firm, whatever membership they hold elsewhere.
    #[error("person {0} is not a member of firm {1}")]
    AdminDriCrossFirm(Uuid, Uuid),
    /// Only Owner may appoint or transfer a Firm's Admin DRI.
    #[error("only Owner may appoint or transfer a Firm's Admin DRI")]
    NotAuthorized,
    /// Removing this membership, or changing it away from `admin`, would
    /// leave an active Firm with no Admin DRI. Transfer the designation
    /// first with [`appoint_admin_dri`].
    #[error("firm {0} would be left without an Admin DRI")]
    WouldLeaveFirmWithoutAdminDri(Uuid),
    /// This Firm still owns one or more Projects; deletion is refused
    /// rather than orphaning a matter's owning practice (ENG-494).
    #[error("firm {0} still owns one or more projects and cannot be deleted")]
    FirmOwnsProjects(Uuid),
    /// The person holds no `person_firm_role` row on this Firm — there is
    /// nothing for [`update_membership`] or [`remove_membership`] to change.
    #[error("person {0} is not a member of firm {1}")]
    NotAMember(Uuid, Uuid),
}

fn classify_write(error: surrealdb::Error) -> FirmError {
    match crate::surreal::retry::unique_violation(&error) {
        Some("person_firm_role_pair") => FirmError::DuplicateMembership,
        Some("firm_entity") => FirmError::DuplicateEntity,
        Some("firm_brand_key" | "firm_brand_pair") => FirmError::DuplicateBrand,
        _ => FirmError::Db(error),
    }
}

async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, FirmError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Create a firm under a fresh UUID record key, atomically with its first
/// Admin DRI.
///
/// `admin_dri_person_id` must resolve to a person carrying
/// `person.role = admin`; anything else — Owner, Lawyer, Clerk, Client, or a
/// dangling id — is refused before either row is written, so a Firm is never
/// created without one (ENG-499: there is no setup state). The Firm row and
/// its `person_firm_role` membership (carrying `is_dri = true`) are written
/// in one transaction, so a reader never observes a Firm with zero Admin
/// DRIs.
pub async fn create(surreal: &SurrealDb, input: &NewFirm) -> Result<Firm, FirmError> {
    if crate::entities::find_by_id(surreal, input.entity_id)
        .await?
        .is_none()
    {
        return Err(FirmError::NoSuchEntity(input.entity_id));
    }
    let admin = crate::persons::find_by_id(surreal, input.admin_dri_person_id)
        .await?
        .ok_or(FirmError::NoSuchPerson(input.admin_dri_person_id))?;
    if admin.role != Role::Admin {
        return Err(FirmError::IneligibleAdminDriTier(input.admin_dri_person_id));
    }
    let firm_id = Uuid::now_v7();
    let membership_id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "BEGIN; \
                 CREATE $firm_id SET name = $name, status = $status, entity_id = $entity_id, \
                 inserted_at = $now, updated_at = $now RETURN {FIRM_SELECT}; \
                 CREATE $membership_id SET person_id = $person_id, firm_id = $firm_id, \
                 membership = 'admin', is_dri = true, inserted_at = $now, updated_at = $now; \
                 COMMIT;"
            ))
            .bind(("firm_id", record_id(TABLE, firm_id)))
            .bind(("membership_id", record_id(MEMBERSHIP_TABLE, membership_id)))
            .bind((
                "person_id",
                record_id(PERSON_TABLE, input.admin_dri_person_id),
            ))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
            .bind(("entity_id", record_id(ENTITY_TABLE, input.entity_id)))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<FirmRow> = response.take(1)?;
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
            "SELECT {FIRM_SELECT} FROM {TABLE} WHERE entity_id = $entity_id LIMIT 1"
        ))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<FirmRow> = response.take(0)?;
    Ok(rows.into_iter().find_map(FirmRow::into_firm))
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

/// A partial edit to a [`Firm`]'s own fields (ENG-494). Absent fields are
/// left untouched; this never touches `person_firm_role` or `firm_brand`.
#[derive(Debug, Clone, Default)]
pub struct FirmEdit {
    pub name: Option<String>,
    /// `active`, `suspended`, or `archived`.
    pub status: Option<String>,
    pub entity_id: Option<Uuid>,
}

/// Gate a Firm-scoped settings/membership/brand write through
/// [`crate::firm_capability::FirmCapability::ManageMembership`] — the same
/// capability that already gates who may add a member, since editing a
/// Firm's own settings, its people's memberships, and the brands it wears
/// are one administrative surface (ENG-494). Owner passes on every Firm;
/// only the Admin membership tier passes on its own.
async fn authorize_manage_firm(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    firm_id: Uuid,
) -> Result<(), FirmError> {
    use crate::firm_capability::{resolve, FirmCapability, FirmCapabilityDecision};
    match resolve(
        surreal,
        actor_role,
        actor_person_id,
        firm_id,
        FirmCapability::ManageMembership,
    )
    .await?
    {
        FirmCapabilityDecision::Allowed => Ok(()),
        FirmCapabilityDecision::FirmNotFound => Err(FirmError::NoSuchFirm(firm_id)),
        FirmCapabilityDecision::Forbidden => Err(FirmError::NotAuthorized),
    }
}

/// Edit a Firm's own fields. Owner, or that Firm's own Admin membership,
/// only (`authorize_manage_firm`). Refuses an `entity_id` that does not
/// resolve, the same guarantee [`create`] gives; leaves the Admin DRI and
/// every membership row untouched — those move only through
/// [`appoint_admin_dri`], [`update_membership`], and [`remove_membership`].
pub async fn update(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    firm_id: Uuid,
    input: &FirmEdit,
) -> Result<Firm, FirmError> {
    authorize_manage_firm(surreal, actor_role, actor_person_id, firm_id).await?;
    if let Some(entity_id) = input.entity_id {
        if crate::entities::find_by_id(surreal, entity_id)
            .await?
            .is_none()
        {
            return Err(FirmError::NoSuchEntity(entity_id));
        }
    }

    let mut assignments: Vec<&str> = vec!["updated_at = $updated_at"];
    if input.name.is_some() {
        assignments.push("name = $name");
    }
    if input.status.is_some() {
        assignments.push("status = $status");
    }
    if input.entity_id.is_some() {
        assignments.push("entity_id = $entity_id");
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET {} RETURN {FIRM_SELECT}",
                assignments.join(", ")
            ))
            .bind(("id", record_id(TABLE, firm_id)))
            .bind(("updated_at", now.clone()))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
            .bind((
                "entity_id",
                input.entity_id.map(|id| record_id(ENTITY_TABLE, id)),
            ))
    })
    .await?;
    let row: Option<FirmRow> = response.take(0)?;
    row.and_then(FirmRow::into_firm)
        .ok_or(FirmError::WriteReturnedNothing)
}

/// Delete a Firm. Owner, or that Firm's own Admin membership, only
/// (`authorize_manage_firm`). Refused when any Project still names it as
/// `firm_id`: a matter must never lose its owning practice. Its
/// `person_firm_role` and `firm_brand` rows are removed in the same
/// transaction — nothing is left pointing at a Firm that no longer exists.
pub async fn delete(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    firm_id: Uuid,
) -> Result<(), FirmError> {
    authorize_manage_firm(surreal, actor_role, actor_person_id, firm_id).await?;
    let mut projects = surreal
        .query("SELECT id FROM project WHERE firm_id = $firm_id LIMIT 1")
        .bind(("firm_id", record_id(TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let owned: Vec<ProjectIdRow> = projects.take(0)?;
    if !owned.is_empty() {
        return Err(FirmError::FirmOwnsProjects(firm_id));
    }

    writing(|| {
        surreal
            .query(
                "BEGIN; \
                 DELETE person_firm_role WHERE firm_id = $firm_id; \
                 DELETE firm_brand WHERE firm_id = $firm_id; \
                 DELETE $firm_id; \
                 COMMIT;",
            )
            .bind(("firm_id", record_id(TABLE, firm_id)))
    })
    .await?;
    Ok(())
}

/// Record one person's membership at a firm.
///
/// Reads both referenced rows before writing: a `record<>` link constrains
/// the target table but does not prove the row exists. A client-tier person
/// reaches a matter through `person_project_role`, never through firm
/// membership, so this refuses to write a row for one regardless of the
/// requested `membership` value.
pub async fn add_membership(
    surreal: &SurrealDb,
    input: &NewPersonFirmRole,
) -> Result<PersonFirmRole, FirmError> {
    let person = crate::persons::find_by_id(surreal, input.person_id)
        .await?
        .ok_or(FirmError::NoSuchPerson(input.person_id))?;
    if person.role == Role::Client {
        return Err(FirmError::ClientCannotJoinFirm(input.person_id));
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

/// Every membership row on this Firm — the inverse of
/// [`memberships_for_person`], for a Firm detail view listing who belongs to
/// it rather than which Firms one person belongs to.
pub async fn memberships_for_firm(
    surreal: &SurrealDb,
    firm_id: Uuid,
) -> Result<Vec<PersonFirmRole>, FirmError> {
    let mut response = surreal
        .query(format!(
            "SELECT {MEMBERSHIP_SELECT} FROM {MEMBERSHIP_TABLE} \
             WHERE firm_id = $firm_id ORDER BY inserted_at, id"
        ))
        .bind(("firm_id", record_id(TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonFirmRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonFirmRoleRow::into_role)
        .collect())
}

/// Change a person's `membership` at a Firm. Owner, or that Firm's own Admin
/// membership, only (`authorize_manage_firm`). Leaves `is_dri` untouched — a
/// membership form must never write that marker; only [`appoint_admin_dri`]
/// does. Refused with [`FirmError::WouldLeaveFirmWithoutAdminDri`] when the
/// person is the Firm's current sole Admin DRI and `membership` is not
/// `admin` (ENG-499's guard, [`refuse_admin_dri_orphaning`]).
pub async fn update_membership(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    person_id: Uuid,
    firm_id: Uuid,
    membership: FirmMembership,
) -> Result<PersonFirmRole, FirmError> {
    authorize_manage_firm(surreal, actor_role, actor_person_id, firm_id).await?;
    if membership_for_person(surreal, person_id, firm_id)
        .await?
        .is_none()
    {
        return Err(FirmError::NotAMember(person_id, firm_id));
    }
    refuse_admin_dri_orphaning(surreal, firm_id, person_id, Some(membership)).await?;

    let now = chrono::Utc::now().to_rfc3339();
    writing(|| {
        surreal
            .query(
                "UPDATE person_firm_role SET membership = $membership, updated_at = $now \
                 WHERE person_id = $person_id AND firm_id = $firm_id",
            )
            .bind(("person_id", record_id(PERSON_TABLE, person_id)))
            .bind(("firm_id", record_id(TABLE, firm_id)))
            .bind(("membership", membership.as_str().to_string()))
            .bind(("now", now.clone()))
    })
    .await?;

    membership_for_person(surreal, person_id, firm_id)
        .await?
        .ok_or(FirmError::WriteReturnedNothing)
}

/// Remove a person's membership at a Firm entirely. Owner, or that Firm's
/// own Admin membership, only (`authorize_manage_firm`). Refused with
/// [`FirmError::WouldLeaveFirmWithoutAdminDri`] when the person is the
/// Firm's current sole Admin DRI (ENG-499's guard,
/// [`refuse_admin_dri_orphaning`]) — transfer the designation first with
/// [`appoint_admin_dri`].
pub async fn remove_membership(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    person_id: Uuid,
    firm_id: Uuid,
) -> Result<(), FirmError> {
    authorize_manage_firm(surreal, actor_role, actor_person_id, firm_id).await?;
    if membership_for_person(surreal, person_id, firm_id)
        .await?
        .is_none()
    {
        return Err(FirmError::NotAMember(person_id, firm_id));
    }
    refuse_admin_dri_orphaning(surreal, firm_id, person_id, None).await?;

    writing(|| {
        surreal
            .query("DELETE person_firm_role WHERE person_id = $person_id AND firm_id = $firm_id")
            .bind(("person_id", record_id(PERSON_TABLE, person_id)))
            .bind(("firm_id", record_id(TABLE, firm_id)))
    })
    .await?;
    Ok(())
}

/// Person ids an Admin of these firms may see: members of those firms, plus
/// anyone with a `person_project_role` on a matter those firms own.
///
/// Routes through [`crate::firm_capability::allowed_firm_ids`] with
/// [`crate::firm_capability::FirmCapability::ViewDirectory`] (ENG-463) rather
/// than deriving its own membership set, so this stays in step with every
/// other Firm-scoped directory read.
pub async fn visible_person_ids(
    surreal: &SurrealDb,
    admin_person_id: Uuid,
) -> Result<Vec<Uuid>, FirmError> {
    let firm_ids = crate::firm_capability::allowed_firm_ids(
        surreal,
        Role::Admin,
        Some(admin_person_id),
        crate::firm_capability::FirmCapability::ViewDirectory,
    )
    .await?;
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
    match attach_brand(surreal, firm_id, brand_key).await {
        Ok(()) => Ok(()),
        Err(FirmError::DuplicateBrand) => {
            let keys = brand_keys_for_firm(surreal, firm_id).await?;
            if keys.iter().any(|key| key == brand_key) {
                Ok(())
            } else {
                Err(FirmError::DuplicateBrand)
            }
        }
        Err(error) => Err(error),
    }
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

/// Detach a house-brand key from a firm. Owner, or that Firm's own Admin
/// membership, only (`authorize_manage_firm`). A key the firm does not wear
/// is a no-op — there is nothing to remove and nothing to refuse.
pub async fn detach_brand(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    firm_id: Uuid,
    brand_key: &str,
) -> Result<(), FirmError> {
    authorize_manage_firm(surreal, actor_role, actor_person_id, firm_id).await?;
    writing(|| {
        surreal
            .query(format!(
                "DELETE {BRAND_TABLE} WHERE firm_id = $firm_id AND brand_key = $brand_key"
            ))
            .bind(("firm_id", record_id(TABLE, firm_id)))
            .bind(("brand_key", brand_key.to_string()))
    })
    .await?;
    Ok(())
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

/// Appoint or transfer a Firm's Admin DRI (ENG-499).
///
/// Owner-only: gated through
/// [`crate::firm_capability::FirmCapability::ManageAdminDri`], which no
/// membership tier admits, so an Admin — even the Firm's own — is refused
/// with [`FirmError::NotAuthorized`]. The proposed DRI must carry
/// `person.role = admin` ([`FirmError::IneligibleAdminDriTier`]) and hold an
/// `admin` membership on this exact Firm: no row on this Firm at all is
/// [`FirmError::AdminDriCrossFirm`], and a row here carrying a different
/// membership is [`FirmError::WrongAdminDriMembership`].
///
/// The transfer itself is one transaction — clearing any current DRI on this
/// Firm and setting the new one — so a concurrent reader never observes zero
/// or two Admin DRIs on the same Firm.
pub async fn appoint_admin_dri(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    firm_id: Uuid,
    person_id: Uuid,
) -> Result<PersonFirmRole, FirmError> {
    match crate::firm_capability::resolve(
        surreal,
        actor_role,
        actor_person_id,
        firm_id,
        crate::firm_capability::FirmCapability::ManageAdminDri,
    )
    .await?
    {
        crate::firm_capability::FirmCapabilityDecision::Allowed => {}
        crate::firm_capability::FirmCapabilityDecision::FirmNotFound => {
            return Err(FirmError::NoSuchFirm(firm_id))
        }
        crate::firm_capability::FirmCapabilityDecision::Forbidden => {
            return Err(FirmError::NotAuthorized)
        }
    }

    let person = crate::persons::find_by_id(surreal, person_id)
        .await?
        .ok_or(FirmError::NoSuchPerson(person_id))?;
    if person.role != Role::Admin {
        return Err(FirmError::IneligibleAdminDriTier(person_id));
    }
    match membership_for_person(surreal, person_id, firm_id).await? {
        None => return Err(FirmError::AdminDriCrossFirm(person_id, firm_id)),
        Some(row) if row.membership != FirmMembership::Admin => {
            return Err(FirmError::WrongAdminDriMembership(person_id, firm_id));
        }
        Some(_) => {}
    }

    let now = chrono::Utc::now().to_rfc3339();
    writing(|| {
        surreal
            .query(
                "BEGIN; \
                 UPDATE person_firm_role SET is_dri = false, updated_at = $now \
                 WHERE firm_id = $firm_id AND is_dri = true; \
                 UPDATE person_firm_role SET is_dri = true, updated_at = $now \
                 WHERE person_id = $person_id AND firm_id = $firm_id; \
                 COMMIT;",
            )
            .bind(("firm_id", record_id(TABLE, firm_id)))
            .bind(("person_id", record_id(PERSON_TABLE, person_id)))
            .bind(("now", now.clone()))
    })
    .await?;

    membership_for_person(surreal, person_id, firm_id)
        .await?
        .ok_or(FirmError::WriteReturnedNothing)
}

/// Refuse a `person_firm_role` edit that would leave an active Firm with no
/// Admin DRI (ENG-499 scope item 3).
///
/// Called by [`crate::firms`]'s own membership-removal doors before they
/// delete a row or change its `membership` away from `admin`.
/// `new_membership` is `None` for a removal and `Some(m)` for an update to
/// `m`; a no-op when the row is not the Firm's current DRI, or the Firm is
/// not active. Transferring the designation first with
/// [`appoint_admin_dri`] is the only way past this guard for the current
/// sole DRI.
pub async fn refuse_admin_dri_orphaning(
    surreal: &SurrealDb,
    firm_id: Uuid,
    person_id: Uuid,
    new_membership: Option<FirmMembership>,
) -> Result<(), FirmError> {
    if new_membership == Some(FirmMembership::Admin) {
        return Ok(());
    }
    let Some(row) = membership_for_person(surreal, person_id, firm_id).await? else {
        return Ok(());
    };
    if !row.is_dri {
        return Ok(());
    }
    let Some(firm) = find_by_id(surreal, firm_id).await? else {
        return Ok(());
    };
    if firm.status != "active" {
        return Ok(());
    }
    Err(FirmError::WouldLeaveFirmWithoutAdminDri(firm_id))
}

/// One active Firm's Admin-DRI standing, for the deployment-wide invariant
/// report ([`admin_dri_invariant_report`]). Every active Firm reports
/// `problem: None` when exactly one eligible Admin DRI holds the
/// designation — anything else is an actionable state a human resolves,
/// never one this report repairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDriStatus {
    pub firm_id: Uuid,
    pub firm_name: String,
    pub problem: Option<AdminDriProblem>,
}

/// Why a Firm's Admin-DRI designation is not the invariant ENG-499 requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminDriProblem {
    /// No `person_firm_role` row on this Firm carries `is_dri = true`.
    Missing,
    /// More than one row does. Carries every such person's id.
    Multiple(Vec<Uuid>),
    /// Exactly one row carries `is_dri = true`, but the person it names no
    /// longer carries `person.role = admin` (or the row's own membership has
    /// drifted off `admin`) — a designation that was valid at appointment
    /// and is not any more.
    Ineligible(Uuid),
}

/// Scan every active Firm and report which ones do not hold exactly one
/// eligible Admin DRI. Read-only: it never appoints, clears, or otherwise
/// repairs a row — [`appoint_admin_dri`] is the only writer.
pub async fn admin_dri_invariant_report(
    surreal: &SurrealDb,
) -> Result<Vec<AdminDriStatus>, FirmError> {
    let mut out = Vec::new();
    for firm in all(surreal).await? {
        if firm.status != "active" {
            continue;
        }
        let mut response = surreal
            .query(format!(
                "SELECT {MEMBERSHIP_SELECT} FROM {MEMBERSHIP_TABLE} \
                 WHERE firm_id = $firm_id AND is_dri = true ORDER BY inserted_at, id"
            ))
            .bind(("firm_id", record_id(TABLE, firm.id)))
            .await
            .and_then(surrealdb::IndexedResults::check)?;
        let rows: Vec<PersonFirmRoleRow> = response.take(0)?;
        let dris: Vec<PersonFirmRole> = rows
            .into_iter()
            .filter_map(PersonFirmRoleRow::into_role)
            .collect();
        let problem = if dris.is_empty() {
            Some(AdminDriProblem::Missing)
        } else if dris.len() > 1 {
            Some(AdminDriProblem::Multiple(
                dris.iter().map(|row| row.person_id).collect(),
            ))
        } else {
            let dri = &dris[0];
            if dri.membership == FirmMembership::Admin {
                match crate::persons::find_by_id(surreal, dri.person_id).await? {
                    Some(person) if person.role == Role::Admin => None,
                    _ => Some(AdminDriProblem::Ineligible(dri.person_id)),
                }
            } else {
                Some(AdminDriProblem::Ineligible(dri.person_id))
            }
        };
        out.push(AdminDriStatus {
            firm_id: firm.id,
            firm_name: firm.name.clone(),
            problem,
        });
    }
    Ok(out)
}

/// The deployment's anchor Firm — the practice wearing the anchor Entity
/// (`NAVIGATOR_BOOTSTRAP_COMPANY`, or the shipped [`crate::seed::FIRM_ENTITY_NAME`]
/// when unset) — or `None` when no Firm row wraps that Entity yet.
///
/// ENG-495: this is the default a newly created Lawyer or Clerk joins when no
/// other Firm is named. A deployment holding exactly one Firm degenerates to
/// that Firm, which is today's single-practice answer; a deployment holding
/// several still resolves deterministically, because the anchor Entity is
/// unique by construction (`entity_firm_anchor`).
///
/// Reads the same environment variable
/// [`crate::seed::BOOTSTRAP_COMPANY_ENV`] `portal::admin::bootstrap_company_from_env`
/// resolves for the Entity surface, so the two agree on which Entity is the
/// anchor without either importing the other.
pub async fn anchor_firm(surreal: &SurrealDb) -> Result<Option<Firm>, FirmError> {
    let configured = std::env::var(crate::seed::BOOTSTRAP_COMPANY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| crate::seed::FIRM_ENTITY_NAME.to_string());
    // The stored `firm_anchor` key is the anchor Entity's own name, lowercased
    // — never the configured string directly (`entity_commands::firm_anchor_key`).
    // The two are guaranteed to match: `is_firm_anchor` only let that Entity
    // claim the key by matching `configured` case-insensitively in the first
    // place, so the lowercased forms are identical.
    let key = configured.to_lowercase();
    let Some(entity_id) = crate::entities::firm_anchor_holder(surreal, &key).await? else {
        return Ok(None);
    };
    find_by_entity_id(surreal, entity_id).await
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
        practice_with_admin(db, name, admin_dri_person(db).await).await
    }

    /// A fresh `person.role = admin` person, fit to name as a Firm's
    /// `admin_dri_person_id`. Its own identity is opaque to callers that do
    /// not need to name it — only [`practice`] uses this directly.
    async fn admin_dri_person(db: &SurrealDb) -> Uuid {
        crate::persons::create(
            db,
            &NewPerson::with_role(
                "Admin DRI",
                format!("admin-dri-{}@example.com", Uuid::now_v7()),
                Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id
    }

    async fn practice_with_admin(db: &SurrealDb, name: &str, admin_dri_person_id: Uuid) -> Firm {
        let entity_id = seed_entity(db).await;
        create(
            db,
            &NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
                admin_dri_person_id,
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

    /// ENG-499: creation is atomic with the first Admin DRI. There is no
    /// setup state — the moment the Firm row exists, so does exactly one
    /// `person_firm_role` row carrying `membership = admin, is_dri = true`.
    #[tokio::test]
    async fn create_grants_the_named_admin_dri_atomically() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Atomic Practice", admin).await;

        let membership = membership_for_person(&db, admin, firm.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.membership, FirmMembership::Admin);
        assert!(membership.is_dri);

        let report = admin_dri_invariant_report(&db).await.unwrap();
        let status = report.iter().find(|s| s.firm_id == firm.id).unwrap();
        assert_eq!(status.problem, None, "{status:?}");
    }

    /// Creation refuses every ineligible tier: Owner, Lawyer, Clerk, and
    /// Client all fail the same way a dangling person id would.
    #[tokio::test]
    async fn create_refuses_a_non_admin_dri() {
        let db = mem_surreal().await;
        let entity_id = seed_entity(&db).await;

        for (tag, role) in [
            ("owner", Role::Owner),
            ("lawyer", Role::Lawyer),
            ("clerk", Role::Clerk),
            ("client", Role::Client),
        ] {
            let person = crate::persons::create(
                &db,
                &NewPerson::with_role(format!("{tag} Person"), format!("{tag}@example.com"), role),
            )
            .await
            .unwrap();
            let err = create(
                &db,
                &NewFirm {
                    name: format!("{tag} Practice"),
                    status: "active".to_string(),
                    entity_id: seed_entity(&db).await,
                    admin_dri_person_id: person.id,
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, FirmError::IneligibleAdminDriTier(id) if id == person.id),
                "{tag}: {err}"
            );
        }

        let missing = Uuid::now_v7();
        let err = create(
            &db,
            &NewFirm {
                name: "Ghost Admin Practice".to_string(),
                status: "active".to_string(),
                entity_id,
                admin_dri_person_id: missing,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchPerson(id) if id == missing));
    }

    /// Owner appoints and transfers the designation; every other tier,
    /// including the Firm's own Admin, is refused. The transfer clears the
    /// outgoing DRI and sets the incoming one atomically.
    #[tokio::test]
    async fn appoint_admin_dri_transfers_atomically_and_is_owner_only() {
        let db = mem_surreal().await;
        let first_admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Transfer Practice", first_admin).await;

        let second_admin = crate::persons::create(
            &db,
            &NewPerson::with_role("Second Admin", "second-admin@example.com", Role::Admin),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: second_admin.id,
                firm_id: firm.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap();

        for (role, person_id) in [(Role::Admin, first_admin), (Role::Lawyer, second_admin.id)] {
            let err = appoint_admin_dri(&db, role, Some(first_admin), firm.id, second_admin.id)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FirmError::NotAuthorized),
                "{person_id:?}: {err}"
            );
        }

        let transferred = appoint_admin_dri(&db, Role::Owner, None, firm.id, second_admin.id)
            .await
            .unwrap();
        assert!(transferred.is_dri);
        assert_eq!(transferred.person_id, second_admin.id);

        assert!(
            !membership_for_person(&db, first_admin, firm.id)
                .await
                .unwrap()
                .unwrap()
                .is_dri,
            "the outgoing DRI must be cleared, not merely superseded"
        );

        let report = admin_dri_invariant_report(&db).await.unwrap();
        let status = report.iter().find(|s| s.firm_id == firm.id).unwrap();
        assert_eq!(status.problem, None);
    }

    /// Appointment refuses ineligible tiers, a person with no membership on
    /// this Firm at all (cross-Firm), and a person whose membership on this
    /// Firm is not `admin` — three distinct typed refusals.
    #[tokio::test]
    async fn appoint_admin_dri_refuses_ineligible_wrong_membership_and_cross_firm() {
        let db = mem_surreal().await;
        let firm_a = practice(&db, "Practice A").await;
        let firm_b = practice(&db, "Practice B").await;

        let lawyer = crate::persons::create(
            &db,
            &NewPerson::with_role("Lawyer A", "lawyer-a@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        let err = appoint_admin_dri(&db, Role::Owner, None, firm_a.id, lawyer.id)
            .await
            .unwrap_err();
        assert!(matches!(err, FirmError::IneligibleAdminDriTier(id) if id == lawyer.id));

        let admin_on_b = crate::persons::create(
            &db,
            &NewPerson::with_role("Admin On B", "admin-on-b@example.com", Role::Admin),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: admin_on_b.id,
                firm_id: firm_b.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let err = appoint_admin_dri(&db, Role::Owner, None, firm_a.id, admin_on_b.id)
            .await
            .unwrap_err();
        assert!(
            matches!(err, FirmError::AdminDriCrossFirm(person, firm) if person == admin_on_b.id && firm == firm_a.id)
        );

        let lawyer_member_of_a = crate::persons::create(
            &db,
            &NewPerson::with_role(
                "Admin Wrong Membership",
                "wrong-membership@example.com",
                Role::Admin,
            ),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer_member_of_a.id,
                firm_id: firm_a.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let err = appoint_admin_dri(&db, Role::Owner, None, firm_a.id, lawyer_member_of_a.id)
            .await
            .unwrap_err();
        assert!(
            matches!(err, FirmError::WrongAdminDriMembership(person, firm) if person == lawyer_member_of_a.id && firm == firm_a.id)
        );
    }

    /// The guard used by membership-removal doors: removing or demoting the
    /// current sole DRI on an active Firm is refused; anything else is a
    /// no-op that never errors.
    #[tokio::test]
    async fn refuse_admin_dri_orphaning_only_blocks_the_active_firm_s_sole_dri() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Guarded Practice", admin).await;

        let err = refuse_admin_dri_orphaning(&db, firm.id, admin, None)
            .await
            .unwrap_err();
        assert!(matches!(err, FirmError::WouldLeaveFirmWithoutAdminDri(id) if id == firm.id));
        let err = refuse_admin_dri_orphaning(&db, firm.id, admin, Some(FirmMembership::Lawyer))
            .await
            .unwrap_err();
        assert!(matches!(err, FirmError::WouldLeaveFirmWithoutAdminDri(id) if id == firm.id));

        // Reassigning to `admin` is not an orphan and is not refused.
        refuse_admin_dri_orphaning(&db, firm.id, admin, Some(FirmMembership::Admin))
            .await
            .unwrap();

        // A non-DRI row on the same Firm is never refused.
        let lawyer = crate::persons::create(
            &db,
            &NewPerson::with_role("Lawyer", "guard-lawyer@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        refuse_admin_dri_orphaning(&db, firm.id, lawyer.id, None)
            .await
            .unwrap();
    }

    /// The deployment-wide report names missing and multiple designations,
    /// and never writes anything — the two invalid states a direct database
    /// edit could still produce underneath the atomic doors above.
    #[tokio::test]
    async fn admin_dri_invariant_report_names_missing_and_multiple() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Reported Practice", admin).await;

        // Multiple: a second Admin membership is force-marked `is_dri` by a
        // direct write underneath `appoint_admin_dri` — exactly the kind of
        // state this report exists to catch rather than silently repair.
        let second_admin = crate::persons::create(
            &db,
            &NewPerson::with_role("Second", "second-reported@example.com", Role::Admin),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: second_admin.id,
                firm_id: firm.id,
                membership: FirmMembership::Admin,
                is_dri: true,
            },
        )
        .await
        .unwrap();
        let report = admin_dri_invariant_report(&db).await.unwrap();
        let status = report.iter().find(|s| s.firm_id == firm.id).unwrap();
        assert!(
            matches!(&status.problem, Some(AdminDriProblem::Multiple(ids)) if ids.len() == 2),
            "{status:?}"
        );

        // Missing: an empty Firm reports it, never guesses one.
        let empty_entity = seed_entity(&db).await;
        let empty_admin = admin_dri_person(&db).await;
        let empty_firm = create(
            &db,
            &NewFirm {
                name: "Empty Practice".to_string(),
                status: "active".to_string(),
                entity_id: empty_entity,
                admin_dri_person_id: empty_admin,
            },
        )
        .await
        .unwrap();
        // Clear the only DRI with a direct write — the same underneath-the-door
        // edit the report exists to catch.
        db.query("UPDATE person_firm_role SET is_dri = false WHERE firm_id = $firm_id")
            .bind(("firm_id", record_id(TABLE, empty_firm.id)))
            .await
            .unwrap()
            .check()
            .unwrap();
        let report = admin_dri_invariant_report(&db).await.unwrap();
        let status = report.iter().find(|s| s.firm_id == empty_firm.id).unwrap();
        assert_eq!(status.problem, Some(AdminDriProblem::Missing));
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

        // The inverse read: every membership on this Firm, including the
        // atomic Admin DRI `practice` granted.
        let on_firm = memberships_for_firm(&db, firm.id).await.unwrap();
        let ids: Vec<Uuid> = on_firm.iter().map(|row| row.person_id).collect();
        assert!(ids.contains(&person.id));
        assert_eq!(on_firm.len(), 2, "the DRI plus Pat Lawyer: {on_firm:?}");
    }

    #[tokio::test]
    async fn add_membership_refuses_a_client_but_allows_every_firm_tier() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Tier Practice").await;

        for (tag, role, membership) in [
            ("owner", Role::Owner, FirmMembership::Admin),
            ("admin", Role::Admin, FirmMembership::Admin),
            ("lawyer", Role::Lawyer, FirmMembership::Lawyer),
            ("clerk", Role::Clerk, FirmMembership::Clerk),
        ] {
            let person = crate::persons::create(
                &db,
                &NewPerson::with_role(format!("{tag} Person"), format!("{tag}@example.com"), role),
            )
            .await
            .unwrap();
            add_membership(
                &db,
                &NewPersonFirmRole {
                    person_id: person.id,
                    firm_id: firm.id,
                    membership,
                    is_dri: false,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{tag} should be allowed to join a firm: {error}"));
        }

        let client = crate::persons::create(
            &db,
            &NewPerson::with_role("Client Person", "client-tier@example.com", Role::Client),
        )
        .await
        .unwrap();
        let err = add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: client.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::ClientCannotJoinFirm(id) if id == client.id));
        assert!(membership_for_person(&db, client.id, firm.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn ensure_membership_refuses_a_client() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Ensure Practice").await;
        let client = crate::persons::create(
            &db,
            &NewPerson::with_role("Client Person", "ensure-client@example.com", Role::Client),
        )
        .await
        .unwrap();
        let err = ensure_membership(
            &db,
            &NewPersonFirmRole {
                person_id: client.id,
                firm_id: firm.id,
                membership: FirmMembership::Clerk,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::ClientCannotJoinFirm(id) if id == client.id));
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
        let admin = admin_dri_person(&db).await;
        let missing = Uuid::now_v7();
        let err = create(
            &db,
            &NewFirm {
                name: "Ghost Practice".to_string(),
                status: "active".to_string(),
                entity_id: missing,
                admin_dri_person_id: admin,
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
                admin_dri_person_id: admin,
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
                admin_dri_person_id: admin,
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

        detach_brand(&db, Role::Owner, None, firm.id, "neon")
            .await
            .unwrap();
        assert!(brand_keys_for_firm(&db, firm.id).await.unwrap().is_empty());
        // Detaching a key the firm never wore is a no-op, not an error.
        detach_brand(&db, Role::Owner, None, firm.id, "neon")
            .await
            .unwrap();
    }

    /// ENG-494: `update` edits name/status/entity_id and leaves the Admin
    /// DRI and every membership row untouched. `archived` is now an
    /// admitted status (the schema `ASSERT` grew a third value alongside
    /// `active`/`suspended`).
    #[tokio::test]
    async fn update_edits_firm_fields_and_leaves_membership_alone() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Editable Practice", admin).await;
        let new_entity = seed_entity(&db).await;

        let edited = update(
            &db,
            Role::Owner,
            None,
            firm.id,
            &FirmEdit {
                name: Some("Renamed Practice".to_string()),
                status: Some("archived".to_string()),
                entity_id: Some(new_entity),
            },
        )
        .await
        .unwrap();
        assert_eq!(edited.name, "Renamed Practice");
        assert_eq!(edited.status, "archived");
        assert_eq!(edited.entity_id, Some(new_entity));

        let membership = membership_for_person(&db, admin, firm.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.membership, FirmMembership::Admin);
        assert!(membership.is_dri, "editing the firm must not touch the DRI");

        // A partial edit touches only the named fields.
        let partial = update(
            &db,
            Role::Owner,
            None,
            firm.id,
            &FirmEdit {
                status: Some("active".to_string()),
                ..FirmEdit::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(partial.name, "Renamed Practice");
        assert_eq!(partial.status, "active");

        let missing = Uuid::now_v7();
        let err = update(&db, Role::Owner, None, missing, &FirmEdit::default())
            .await
            .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchFirm(id) if id == missing));

        let missing_entity = Uuid::now_v7();
        let err = update(
            &db,
            Role::Owner,
            None,
            firm.id,
            &FirmEdit {
                entity_id: Some(missing_entity),
                ..FirmEdit::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::NoSuchEntity(id) if id == missing_entity));
    }

    /// `delete` refuses a Firm that still owns Projects, and otherwise
    /// removes it along with its membership and brand rows.
    #[tokio::test]
    async fn delete_refuses_when_projects_reference_the_firm_else_cascades_memberships() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Deletable Practice", admin).await;
        attach_brand(&db, firm.id, "neon").await.unwrap();

        let entity_id = seed_entity(&db).await;
        let project = projects::create(
            &db,
            &NewProject {
                code: "owning-matter".to_string(),
                name: "Owning Matter".to_string(),
                status: "open".to_string(),
                entity_id,
                firm_id: Some(firm.id),
                ..NewProject::default()
            },
        )
        .await
        .unwrap();
        let err = delete(&db, Role::Owner, None, firm.id).await.unwrap_err();
        assert!(matches!(err, FirmError::FirmOwnsProjects(id) if id == firm.id));

        // Once the matter no longer exists, deletion proceeds and cascades
        // the membership and brand rows with it.
        projects::delete_project_with_surreal(&db, project.id)
            .await
            .unwrap();
        delete(&db, Role::Owner, None, firm.id).await.unwrap();
        assert!(find_by_id(&db, firm.id).await.unwrap().is_none());
        assert!(membership_for_person(&db, admin, firm.id)
            .await
            .unwrap()
            .is_none());
        assert!(brand_keys_for_firm(&db, firm.id).await.unwrap().is_empty());

        let missing = Uuid::now_v7();
        let err = delete(&db, Role::Owner, None, missing).await.unwrap_err();
        assert!(matches!(err, FirmError::NoSuchFirm(id) if id == missing));
    }

    /// `update_membership` changes only `membership`, never `is_dri`, and is
    /// refused for the current sole DRI unless the new membership is still
    /// `admin`.
    #[tokio::test]
    async fn update_membership_changes_tier_never_is_dri_and_guards_the_sole_dri() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Membership Practice", admin).await;
        let lawyer = crate::persons::create(
            &db,
            &NewPerson::with_role("Lawyer", "membership-lawyer@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();

        let updated = update_membership(
            &db,
            Role::Owner,
            None,
            lawyer.id,
            firm.id,
            FirmMembership::Clerk,
        )
        .await
        .unwrap();
        assert_eq!(updated.membership, FirmMembership::Clerk);
        assert!(!updated.is_dri);

        let err = update_membership(
            &db,
            Role::Owner,
            None,
            admin,
            firm.id,
            FirmMembership::Lawyer,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FirmError::WouldLeaveFirmWithoutAdminDri(id) if id == firm.id));

        // Reassigning the sole DRI's own row to `admin` again is a no-op the
        // guard admits.
        let reaffirmed = update_membership(
            &db,
            Role::Owner,
            None,
            admin,
            firm.id,
            FirmMembership::Admin,
        )
        .await
        .unwrap();
        assert!(reaffirmed.is_dri);

        let stranger = Uuid::now_v7();
        let err = update_membership(
            &db,
            Role::Owner,
            None,
            stranger,
            firm.id,
            FirmMembership::Lawyer,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, FirmError::NotAMember(person, f) if person == stranger && f == firm.id)
        );
    }

    /// `remove_membership` deletes the row and is refused for the current
    /// sole DRI.
    #[tokio::test]
    async fn remove_membership_deletes_the_row_and_guards_the_sole_dri() {
        let db = mem_surreal().await;
        let admin = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Removal Practice", admin).await;
        let clerk = crate::persons::create(
            &db,
            &NewPerson::with_role("Clerk", "removal-clerk@example.com", Role::Clerk),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: clerk.id,
                firm_id: firm.id,
                membership: FirmMembership::Clerk,
                is_dri: false,
            },
        )
        .await
        .unwrap();

        remove_membership(&db, Role::Owner, None, clerk.id, firm.id)
            .await
            .unwrap();
        assert!(membership_for_person(&db, clerk.id, firm.id)
            .await
            .unwrap()
            .is_none());

        let err = remove_membership(&db, Role::Owner, None, admin, firm.id)
            .await
            .unwrap_err();
        assert!(matches!(err, FirmError::WouldLeaveFirmWithoutAdminDri(id) if id == firm.id));

        let err = remove_membership(&db, Role::Owner, None, clerk.id, firm.id)
            .await
            .unwrap_err();
        assert!(
            matches!(err, FirmError::NotAMember(person, f) if person == clerk.id && f == firm.id)
        );
    }

    /// The authorization matrix ENG-494 asks for: Owner passes on every
    /// Firm; an Admin who is a member of *this* Firm passes; an Admin who
    /// is not (a member elsewhere or nowhere), and every Lawyer/Clerk,
    /// is refused — for every write door this issue adds.
    #[tokio::test]
    async fn write_doors_admit_owner_and_the_firm_s_own_admin_only() {
        let db = mem_surreal().await;
        let admin_on_firm = admin_dri_person(&db).await;
        let firm = practice_with_admin(&db, "Matrix Practice", admin_on_firm).await;
        let other_firm = practice(&db, "Other Matrix Practice").await;

        let admin_off_firm = crate::persons::create(
            &db,
            &NewPerson::with_role("Admin Off Firm", "admin-off-firm@example.com", Role::Admin),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: admin_off_firm.id,
                firm_id: other_firm.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let lawyer = crate::persons::create(
            &db,
            &NewPerson::with_role("Matrix Lawyer", "matrix-lawyer@example.com", Role::Lawyer),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: lawyer.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let clerk = crate::persons::create(
            &db,
            &NewPerson::with_role("Matrix Clerk", "matrix-clerk@example.com", Role::Clerk),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: clerk.id,
                firm_id: firm.id,
                membership: FirmMembership::Clerk,
                is_dri: false,
            },
        )
        .await
        .unwrap();

        // Every actor below asks the same question: may they change
        // `lawyer`'s membership on `firm`? Owner and the Firm's own Admin
        // may; an Admin scoped to a different Firm, a Lawyer, and a Clerk
        // may not.
        let allowed = [(Role::Owner, None), (Role::Admin, Some(admin_on_firm))];
        let forbidden = [
            (Role::Admin, Some(admin_off_firm.id)),
            (Role::Lawyer, Some(lawyer.id)),
            (Role::Clerk, Some(clerk.id)),
        ];

        for (role, person_id) in allowed {
            update_membership(
                &db,
                role,
                person_id,
                lawyer.id,
                firm.id,
                FirmMembership::Lawyer,
            )
            .await
            .unwrap_or_else(|error| panic!("{role:?} should be allowed: {error}"));
            update(&db, role, person_id, firm.id, &FirmEdit::default())
                .await
                .unwrap_or_else(|error| panic!("{role:?} should be allowed to edit: {error}"));
        }
        for (role, person_id) in forbidden {
            let err = update_membership(
                &db,
                role,
                person_id,
                lawyer.id,
                firm.id,
                FirmMembership::Lawyer,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, FirmError::NotAuthorized),
                "{role:?} must be refused: {err}"
            );
            let err = update(&db, role, person_id, firm.id, &FirmEdit::default())
                .await
                .unwrap_err();
            assert!(
                matches!(err, FirmError::NotAuthorized),
                "{role:?} must be refused to edit: {err}"
            );
        }
    }

    #[tokio::test]
    async fn admin_visibility_stays_inside_the_admin_s_firms() {
        let db = mem_surreal().await;
        let admin_a = crate::persons::create(
            &db,
            &NewPerson::with_role("Admin A", "admin-a@example.com", Role::Admin),
        )
        .await
        .unwrap();
        let firm_a = practice_with_admin(&db, "Practice A", admin_a.id).await;
        let firm_b = practice(&db, "Practice B").await;
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
    async fn visible_person_ids_never_surfaces_a_client_through_a_firm_row() {
        let db = mem_surreal().await;
        let admin = crate::persons::create(
            &db,
            &NewPerson::with_role("Admin Only", "admin-only@example.com", Role::Admin),
        )
        .await
        .unwrap();
        let firm = practice_with_admin(&db, "No Client Members Practice", admin.id).await;

        let client = crate::persons::create(
            &db,
            &NewPerson::with_role(
                "Would Be Member",
                "would-be-member@example.com",
                Role::Client,
            ),
        )
        .await
        .unwrap();
        add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: client.id,
                firm_id: firm.id,
                membership: FirmMembership::Lawyer,
                is_dri: false,
            },
        )
        .await
        .unwrap_err();

        let visible = visible_person_ids(&db, admin.id).await.unwrap();
        assert!(visible.contains(&admin.id));
        assert!(!visible.contains(&client.id));
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

    /// An Entity carrying `firm_anchor_key`, wrapped in a Firm — the shape
    /// [`anchor_firm`] resolves. `name` is exactly [`crate::seed::FIRM_ENTITY_NAME`]
    /// in these tests, because no test sets `NAVIGATOR_BOOTSTRAP_COMPANY` (a
    /// process-wide environment variable every parallel test would race on),
    /// so `anchor_firm` always falls back to that default.
    async fn anchor_practice(db: &SurrealDb, name: &str) -> Firm {
        let entity_id = crate::entities::create(
            db,
            &crate::entities::NewEntity {
                name: name.to_string(),
                entity_type_id: crate::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: crate::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                firm_anchor_key: Some(name.trim().to_lowercase()),
            },
        )
        .await
        .unwrap()
        .id;
        create(
            db,
            &NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
                admin_dri_person_id: admin_dri_person(db).await,
            },
        )
        .await
        .unwrap()
    }

    /// ENG-495: the default a newly created Lawyer or Clerk joins when no
    /// other Firm is named — resolved through the two-hop
    /// `firm_anchor_holder` → `find_by_entity_id` the issue asked to be
    /// verified before anything is built on it.
    #[tokio::test]
    async fn anchor_firm_resolves_the_firm_wearing_the_bootstrap_entity() {
        let db = mem_surreal().await;
        let anchor = anchor_practice(&db, crate::seed::FIRM_ENTITY_NAME).await;

        assert_eq!(anchor_firm(&db).await.unwrap(), Some(anchor));
    }

    /// No Entity carries the anchor key at all — an ordinary Firm alone
    /// resolves to no default, rather than an arbitrary one.
    #[tokio::test]
    async fn anchor_firm_is_none_when_no_entity_holds_the_anchor_key() {
        let db = mem_surreal().await;
        practice(&db, "Ordinary Practice").await;

        assert_eq!(anchor_firm(&db).await.unwrap(), None);
    }

    /// The two-firm case: a deployment holding several Firms still resolves
    /// the one wearing the anchor Entity, not merely the first or the last
    /// one created — this is the assertion that would pass vacuously against
    /// a single seeded firm and is the whole point of ENG-495.
    #[tokio::test]
    async fn anchor_firm_picks_the_anchor_among_several_firms() {
        let db = mem_surreal().await;
        let _ordinary_first = practice(&db, "Ordinary Practice One").await;
        let anchor = anchor_practice(&db, crate::seed::FIRM_ENTITY_NAME).await;
        let _ordinary_second = practice(&db, "Ordinary Practice Two").await;

        assert_eq!(anchor_firm(&db).await.unwrap(), Some(anchor));
    }
}
