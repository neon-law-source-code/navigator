//! Data-driven brand rows (ENG-496).
//!
//! A [`Brand`] is a name a practice presents under, distinct from the
//! compiled [`views::brand::BrandKey`] registry `store` cannot depend on:
//! that enum still owns every served host, the marketing-page catalog, and
//! the compiled `Branding` copy — nothing here replaces it, and no runtime
//! brand row publishes a host or a marketing page in this cut. This table
//! is the authorization and identity record CRUD acts on: who may create a
//! brand, what it is named, and which Firm (if any) it belongs to.
//!
//! `firm_id: None` means system-wide — visible to every Firm, created only
//! by Owner. A live `firm_id` means Firm-scoped — created only by that
//! Firm's Admin DRI (`person_firm_role.is_dri`, ENG-499), who cannot also
//! create a system-wide brand through this same command. `is_law_firm` and
//! `legal_entity` are Owner's own call for a system-wide brand; a
//! Firm-scoped brand does not take them as input at all — it always
//! inherits `is_law_firm = true` and its Firm's own Entity name, because a
//! Firm-scoped brand presents that Firm's own practice.

use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::Role;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const TABLE: &str = "brand";
const FIRM_TABLE: &str = "firm";
const SELECT: &str = "id, name, brand_key, firm_id, primary_color, accent_color, typeface, \
                       is_law_firm, legal_entity, inserted_at, updated_at";

/// A data-driven brand row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brand {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    /// `None` is system-wide; `Some(firm_id)` is scoped to that Firm.
    pub firm_id: Option<Uuid>,
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub typeface: Option<String>,
    pub is_law_firm: bool,
    pub legal_entity: Option<String>,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct BrandRow {
    id: surrealdb::types::RecordId,
    name: String,
    brand_key: String,
    firm_id: Option<surrealdb::types::RecordId>,
    primary_color: Option<String>,
    accent_color: Option<String>,
    typeface: Option<String>,
    is_law_firm: bool,
    legal_entity: Option<String>,
    inserted_at: String,
    updated_at: String,
}

impl BrandRow {
    fn into_brand(self) -> Option<Brand> {
        Some(Brand {
            id: record_uuid(&self.id)?,
            name: self.name,
            key: self.brand_key,
            firm_id: match self.firm_id.as_ref() {
                Some(id) => Some(record_uuid(id)?),
                None => None,
            },
            primary_color: self.primary_color,
            accent_color: self.accent_color,
            typeface: self.typeface,
            is_law_firm: self.is_law_firm,
            legal_entity: self.legal_entity,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

/// Inputs for creating a [`Brand`].
///
/// `is_law_firm` and `legal_entity` are honored only for a system-wide
/// request (`firm_id: None`) — a Firm-scoped request always computes both
/// from the target Firm and ignores whatever these fields carry.
#[derive(Debug, Clone, Default)]
pub struct NewBrand {
    pub name: String,
    pub key: String,
    pub firm_id: Option<Uuid>,
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub typeface: Option<String>,
    pub is_law_firm: bool,
    pub legal_entity: Option<String>,
}

/// A partial edit to a [`Brand`]'s presentation fields. Never touches
/// `firm_id`, `is_law_firm`, or `legal_entity` — those are fixed at
/// creation and inherited, not edited.
#[derive(Debug, Clone, Default)]
pub struct BrandEdit {
    pub name: Option<String>,
    pub primary_color: Option<Option<String>>,
    pub accent_color: Option<Option<String>>,
    pub typeface: Option<Option<String>>,
}

/// Errors from the brand command seam.
#[derive(Debug, thiserror::Error)]
pub enum BrandError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Firm(#[from] crate::firms::FirmError),
    #[error("writing a brand returned no usable row")]
    WriteReturnedNothing,
    #[error("no brand {0}")]
    NoSuchBrand(Uuid),
    #[error("no firm {0}")]
    NoSuchFirm(Uuid),
    #[error("that brand name is already taken")]
    DuplicateName,
    #[error("that brand key is already taken")]
    DuplicateKey,
    /// Owner alone creates a system-wide brand; a Firm's Admin DRI alone
    /// creates one scoped to their own Firm. Every other actor, and an
    /// Admin DRI naming a different Firm, is refused this.
    #[error("you may not create, edit, or delete this brand")]
    NotAuthorized,
}

fn classify_write(error: surrealdb::Error) -> BrandError {
    match retry::unique_violation(&error) {
        Some("brand_name") => BrandError::DuplicateName,
        Some("brand_key_unique") => BrandError::DuplicateKey,
        _ => BrandError::Db(error),
    }
}

async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, BrandError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Whether `actor` may create, edit, or delete a brand at `target_firm_id`
/// (`None` for system-wide).
///
/// Side-effect-free. Owner passes only the system-wide case; a Firm's own
/// Admin DRI passes only that Firm's case. Every other combination —
/// including Owner attempting a Firm-scoped brand, or that Firm's non-DRI
/// Admin — is refused.
async fn authorize(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    target_firm_id: Option<Uuid>,
) -> Result<(), BrandError> {
    match target_firm_id {
        None => {
            if actor_role == Role::Owner {
                Ok(())
            } else {
                Err(BrandError::NotAuthorized)
            }
        }
        Some(firm_id) => {
            if crate::firms::find_by_id(surreal, firm_id).await?.is_none() {
                return Err(BrandError::NoSuchFirm(firm_id));
            }
            let Some(person_id) = actor_person_id else {
                return Err(BrandError::NotAuthorized);
            };
            match crate::firms::membership_for_person(surreal, person_id, firm_id).await? {
                Some(row)
                    if row.is_dri && row.membership == crate::firms::FirmMembership::Admin =>
                {
                    Ok(())
                }
                _ => Err(BrandError::NotAuthorized),
            }
        }
    }
}

/// Create a brand. See the module doc for the authorization split and what
/// a Firm-scoped request inherits.
pub async fn create(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    input: &NewBrand,
) -> Result<Brand, BrandError> {
    authorize(surreal, actor_role, actor_person_id, input.firm_id).await?;

    let (is_law_firm, legal_entity) = match input.firm_id {
        None => (input.is_law_firm, input.legal_entity.clone()),
        Some(firm_id) => {
            // `authorize` already proved this Firm exists.
            let firm = crate::firms::find_by_id(surreal, firm_id)
                .await?
                .ok_or(BrandError::NoSuchFirm(firm_id))?;
            let entity_name = match firm.entity_id {
                Some(entity_id) => crate::entities::find_by_id(surreal, entity_id)
                    .await
                    .map_err(crate::firms::FirmError::from)?
                    .map(|entity| entity.name),
                None => None,
            };
            (true, entity_name)
        }
    };

    let id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET name = $name, brand_key = $brand_key, firm_id = $firm_id, \
                 primary_color = $primary_color, accent_color = $accent_color, typeface = $typeface, \
                 is_law_firm = $is_law_firm, legal_entity = $legal_entity, \
                 inserted_at = $now, updated_at = $now RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("name", input.name.clone()))
            .bind(("brand_key", input.key.clone()))
            .bind(("firm_id", input.firm_id.map(|fid| record_id(FIRM_TABLE, fid))))
            .bind(("primary_color", input.primary_color.clone()))
            .bind(("accent_color", input.accent_color.clone()))
            .bind(("typeface", input.typeface.clone()))
            .bind(("is_law_firm", is_law_firm))
            .bind(("legal_entity", legal_entity.clone()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Find a brand by id.
pub async fn find_by_id(surreal: &SurrealDb, id: Uuid) -> Result<Option<Brand>, BrandError> {
    let mut response = surreal
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<BrandRow> = response.take(0)?;
    Ok(row.and_then(BrandRow::into_brand))
}

/// Find a brand by its unique key.
pub async fn find_by_key(surreal: &SurrealDb, key: &str) -> Result<Option<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE brand_key = $key LIMIT 1"
        ))
        .bind(("key", key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().find_map(BrandRow::into_brand))
}

/// Every system-wide brand (`firm_id IS NONE`), name then id — the set
/// every Firm sees regardless of its own scoped brands.
pub async fn system_wide(surreal: &SurrealDb) -> Result<Vec<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE firm_id IS NONE ORDER BY name, id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(BrandRow::into_brand).collect())
}

/// Every brand scoped to this Firm — never another Firm's, and never the
/// system-wide set (use [`system_wide`] for that).
pub async fn for_firm(surreal: &SurrealDb, firm_id: Uuid) -> Result<Vec<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE firm_id = $firm_id ORDER BY name, id"
        ))
        .bind(("firm_id", record_id(FIRM_TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(BrandRow::into_brand).collect())
}

/// Edit a brand's presentation fields. Authorized exactly as [`create`]
/// would be for this brand's existing scope: Owner for a system-wide
/// brand, that Firm's own Admin DRI for a Firm-scoped one.
pub async fn update(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
    input: &BrandEdit,
) -> Result<Brand, BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;

    let mut assignments: Vec<&str> = vec!["updated_at = $updated_at"];
    if input.name.is_some() {
        assignments.push("name = $name");
    }
    if input.primary_color.is_some() {
        assignments.push("primary_color = $primary_color");
    }
    if input.accent_color.is_some() {
        assignments.push("accent_color = $accent_color");
    }
    if input.typeface.is_some() {
        assignments.push("typeface = $typeface");
    }
    if assignments.len() == 1 {
        return Ok(existing);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET {} RETURN {SELECT}",
                assignments.join(", ")
            ))
            .bind(("id", record_id(TABLE, brand_id)))
            .bind(("updated_at", now.clone()))
            .bind(("name", input.name.clone()))
            .bind(("primary_color", input.primary_color.clone()))
            .bind(("accent_color", input.accent_color.clone()))
            .bind(("typeface", input.typeface.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Delete a brand. Authorized exactly as [`update`].
pub async fn delete(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
) -> Result<(), BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;
    writing(|| {
        surreal
            .query("DELETE $id")
            .bind(("id", record_id(TABLE, brand_id)))
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firms::{FirmMembership, NewFirm, NewPersonFirmRole};
    use crate::persons::NewPerson;
    use crate::test_support::{mem_surreal, seed_entity};

    async fn admin_dri_person(db: &SurrealDb) -> Uuid {
        crate::persons::create(
            db,
            &NewPerson::with_role(
                "Admin DRI",
                format!("brand-admin-dri-{}@example.com", Uuid::now_v7()),
                Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id
    }

    async fn practice(db: &SurrealDb, name: &str) -> (crate::firms::Firm, Uuid) {
        let entity_id = seed_entity(db).await;
        let admin = admin_dri_person(db).await;
        let firm = crate::firms::create(
            db,
            &NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
                admin_dri_person_id: admin,
            },
        )
        .await
        .unwrap();
        (firm, admin)
    }

    #[tokio::test]
    async fn owner_creates_a_system_wide_brand_visible_to_every_firm() {
        let db = mem_surreal().await;
        let brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Neon Law".to_string(),
                key: "neon".to_string(),
                is_law_firm: true,
                legal_entity: Some("Shook Law PLLC".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.firm_id, None);
        assert!(brand.is_law_firm);
        assert_eq!(brand.legal_entity.as_deref(), Some("Shook Law PLLC"));

        let (firm, _admin) = practice(&db, "Any Practice").await;
        assert!(system_wide(&db)
            .await
            .unwrap()
            .iter()
            .any(|b| b.id == brand.id));
        assert!(for_firm(&db, firm.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_firm_s_admin_dri_creates_a_scoped_brand_inheriting_the_firm_s_entity() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Scoped Practice").await;
        // The Firm's own name (`practice`'s argument) and its Entity's name
        // are deliberately distinct here — `seed_entity` names the Entity
        // itself — so this proves inheritance reads the real linked Entity
        // rather than coincidentally matching the Firm's own name.
        let entity_name = crate::entities::find_by_id(&db, firm.entity_id.unwrap())
            .await
            .unwrap()
            .unwrap()
            .name;

        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Scoped Brand".to_string(),
                key: "scoped-brand".to_string(),
                firm_id: Some(firm.id),
                // Submitted but must be ignored/overridden for a Firm-scoped brand.
                is_law_firm: false,
                legal_entity: Some("Someone Else PLLC".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.firm_id, Some(firm.id));
        assert!(
            brand.is_law_firm,
            "a Firm-scoped brand is always a law firm"
        );
        assert_eq!(brand.legal_entity.as_deref(), Some(entity_name.as_str()));
        assert_eq!(for_firm(&db, firm.id).await.unwrap(), vec![brand]);
    }

    /// Owner cannot create a Firm-scoped brand; a Firm's Admin DRI cannot
    /// create a system-wide one; a non-DRI Admin of the Firm, and an Admin
    /// DRI of a *different* Firm, may create neither.
    #[tokio::test]
    async fn create_refuses_every_mismatched_actor() {
        let db = mem_surreal().await;
        let (firm_a, admin_a) = practice(&db, "Practice A").await;
        let (_firm_b, admin_b) = practice(&db, "Practice B").await;

        let err = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Owner Scoped Attempt".to_string(),
                key: "owner-scoped-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let err = create(
            &db,
            Role::Admin,
            Some(admin_a),
            &NewBrand {
                name: "Admin System Wide Attempt".to_string(),
                key: "admin-system-wide-attempt".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let err = create(
            &db,
            Role::Admin,
            Some(admin_b),
            &NewBrand {
                name: "Cross Firm Attempt".to_string(),
                key: "cross-firm-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let non_dri_admin = crate::persons::create(
            &db,
            &NewPerson::with_role("Non DRI Admin", "non-dri-admin@example.com", Role::Admin),
        )
        .await
        .unwrap();
        crate::firms::add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: non_dri_admin.id,
                firm_id: firm_a.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let err = create(
            &db,
            Role::Admin,
            Some(non_dri_admin.id),
            &NewBrand {
                name: "Non DRI Attempt".to_string(),
                key: "non-dri-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        for role in [Role::Lawyer, Role::Clerk, Role::Client] {
            let err = create(
                &db,
                role,
                None,
                &NewBrand {
                    name: format!("{role:?} Attempt"),
                    key: format!("{role:?}-attempt").to_lowercase(),
                    ..NewBrand::default()
                },
            )
            .await
            .unwrap_err();
            assert!(matches!(err, BrandError::NotAuthorized), "{role:?}");
        }
    }

    #[tokio::test]
    async fn name_and_key_are_each_globally_unique() {
        let db = mem_surreal().await;
        create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "First".to_string(),
                key: "first".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let duplicate_name = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "First".to_string(),
                key: "second".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate_name, BrandError::DuplicateName));

        let duplicate_key = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Second".to_string(),
                key: "first".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate_key, BrandError::DuplicateKey));
    }

    #[tokio::test]
    async fn update_and_delete_are_authorized_like_create_and_leave_scope_fixed() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Editable Practice").await;
        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Editable Brand".to_string(),
                key: "editable-brand".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let err = update(
            &db,
            Role::Owner,
            None,
            brand.id,
            &BrandEdit {
                name: Some("Hijacked".to_string()),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let edited = update(
            &db,
            Role::Admin,
            Some(admin),
            brand.id,
            &BrandEdit {
                primary_color: Some(Some("#123456".to_string())),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(edited.name, "Editable Brand");
        assert_eq!(edited.primary_color.as_deref(), Some("#123456"));
        assert_eq!(edited.firm_id, Some(firm.id));

        let err = delete(&db, Role::Owner, None, brand.id).await.unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));
        delete(&db, Role::Admin, Some(admin), brand.id)
            .await
            .unwrap();
        assert!(find_by_id(&db, brand.id).await.unwrap().is_none());
    }
}
