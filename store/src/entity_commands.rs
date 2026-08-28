//! Shared Entity command layer for every write adapter.
//!
//! Creating an Entity — the blank-name check, the firm-anchor guard, and
//! the insert itself — lives here so the JSON `POST /app/api/entities`
//! surface, the `/app/admin/entities` form, and the inline "Add entity" modal
//! on the project form travel one command boundary. The adapters
//! authorize and render; none of them re-implements the write.
//!
//! # The firm anchor
//!
//! The firm's own Entity row is neither forkable, renameable, nor
//! removable, and the schema is what makes that hold. SurrealDB has no
//! advisory lock and a multi-statement query is not one transaction, so
//! there is nothing for a read-then-write guard to serialize against.
//!
//! [`firm_anchor_key`] computes a key for a row this module judges to be
//! an anchor, `None` for every other row, and the UNIQUE
//! `entity_firm_anchor` index refuses the second row carrying one. The
//! index refuses the fork outright, including from a writer that never
//! called [`firm_anchor_exists`] at all.
//!
//! [`is_firm_anchor`] stays here and stays public, because the rename
//! and delete doors in `portal::admin` ask the same question. It reads
//! configuration, which is why the *policy* lives in this module and
//! only the *enforcement* lives in the schema.
//!
//! # Deleting is a read, not a caught violation
//!
//! A Surreal `record<>` link is not a foreign key, so nothing in the
//! engine refuses a delete that strands a reference. [`delete_entity`]
//! counts the dependents first and refuses on its own evidence, which
//! also lets it name *every* referencing table rather than whichever
//! constraint happened to fire first.

use serde::Deserialize;
use uuid::Uuid;

use crate::entities::{self, Entity, EntityError, NewEntity};
use crate::surreal::SurrealDb;

/// Request body for creating an Entity through the command boundary.
#[derive(Debug, Deserialize)]
pub struct CreateEntityCommand {
    pub name: String,
    pub entity_type_id: Uuid,
    pub jurisdiction_id: Uuid,
}

/// Request body for updating an Entity through the command boundary. Every
/// field is a full replacement — there is no partial-field semantics to
/// preserve here, unlike the People update's structured legal-name parts.
#[derive(Debug, Deserialize)]
pub struct UpdateEntityCommand {
    pub name: String,
    pub entity_type_id: Uuid,
    pub jurisdiction_id: Uuid,
}

#[derive(Debug)]
pub enum EntityCommandError {
    /// The request is malformed in a way the caller can correct.
    Invalid(&'static str),
    /// No Entity with the requested id.
    NotFound,
    /// A unique constraint refused the row.
    Conflict,
    /// The write would put a second row under the firm anchor's name.
    FirmAnchorExists,
    /// The target row *is* the firm anchor and the write would rename it.
    FirmAnchorImmutable,
    /// The target row *is* the firm anchor and the write would delete it.
    FirmAnchorProtected,
    /// Other rows still reference this Entity, so the delete was refused.
    /// Carries the tables and counts [`entities::dependents`] found, so an
    /// operator sees *what* to detach rather than a generic refusal.
    Referenced(String),
    /// Reading the jurisdiction a write references failed — the lookup
    /// itself, not a missing row (that is [`Self::Invalid`] with
    /// [`UNKNOWN_REFERENCE_MESSAGE`]).
    Jurisdictions(crate::jurisdictions::JurisdictionError),
    /// Reading the entity type a write references failed — the lookup
    /// itself, not a missing row (that is [`Self::Invalid`] with
    /// [`UNKNOWN_REFERENCE_MESSAGE`]).
    EntityTypes(crate::entity_types::EntityTypeError),
    /// Reading or writing the entity itself failed.
    Entities(EntityError),
}

/// Shown when a write would fork the firm anchor. It names the constraint
/// rather than the firm, so a white-label operator's own anchor reads the
/// same way.
pub const FIRM_ANCHOR_EXISTS_MESSAGE: &str =
    "The firm entity already exists and cannot be duplicated.";

/// Shown when the create points at an entity type or jurisdiction that does
/// not exist. Both are caller-supplied reference ids the caller can correct,
/// so this is a validation failure, not a server fault. Both are caught by
/// read-back — [`require_entity_type`] and [`require_jurisdiction`] — because
/// a `record<>` link is not validated by the engine, exactly as
/// `store::credentials` handles its own links.
pub const UNKNOWN_REFERENCE_MESSAGE: &str = "Unknown entity type or jurisdiction.";

/// Shown when a write would rename the firm anchor itself. The row's other
/// columns stay editable, so the message says which part is frozen.
pub const FIRM_ANCHOR_IMMUTABLE_MESSAGE: &str =
    "The firm entity's name is immutable. Its type and jurisdiction remain editable.";

/// Shown when a delete would remove the firm anchor itself. `store::seed`
/// re-creates that row by exact name on every boot, so removing it never
/// sticks and the surface must not offer the option.
pub const FIRM_ANCHOR_PROTECTED_MESSAGE: &str =
    "The bootstrap company is protected and cannot be deleted.";

impl EntityCommandError {
    /// The message to show a human — the error line on the lawyer create
    /// form or the inline "Add entity" modal. Kept here so every adapter
    /// renders the same wording per failure.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::Invalid(message) => (*message).to_string(),
            Self::NotFound => "That entity no longer exists.".to_string(),
            Self::Conflict => "An entity with that key already exists.".to_string(),
            Self::FirmAnchorExists => FIRM_ANCHOR_EXISTS_MESSAGE.to_string(),
            Self::FirmAnchorImmutable => FIRM_ANCHOR_IMMUTABLE_MESSAGE.to_string(),
            Self::FirmAnchorProtected => FIRM_ANCHOR_PROTECTED_MESSAGE.to_string(),
            // The dependent counts name what still points at the row, so
            // the operator reads *why* rather than a generic refusal.
            Self::Referenced(detail) => format!("Couldn't delete this entity — {detail}."),
            Self::Jurisdictions(_) | Self::EntityTypes(_) | Self::Entities(_) => {
                "Could not save entity.".to_string()
            }
        }
    }
}

impl From<EntityError> for EntityCommandError {
    fn from(error: EntityError) -> Self {
        match error {
            // The index caught a fork the read-below-it did not. Both
            // doors report the same thing to the caller.
            EntityError::FirmAnchorTaken => Self::FirmAnchorExists,
            other => Self::Entities(other),
        }
    }
}

/// Whether `entity_name` is a firm anchor that application users may not
/// delete or rename. The canonical seed's row always qualifies: a
/// `NAVIGATOR_BOOTSTRAP_COMPANY` naming a row that does not exist — a
/// white-label operator's own firm before it is created, or a typo — must
/// not leave the seeded firm deletable.
#[must_use]
pub fn is_firm_anchor(configured: &str, entity_name: &str) -> bool {
    let name = entity_name.trim();
    configured.trim().eq_ignore_ascii_case(name)
        || crate::seed::FIRM_ENTITY_NAME.eq_ignore_ascii_case(name)
}

/// The value `entities.firm_anchor_key` carries for a row named
/// `entity_name`, or `None` when the row is an ordinary Entity.
///
/// Entity names are deliberately not unique — two unrelated Entities may
/// share one, in one jurisdiction, because namesakes are real — and this
/// is what keeps the UNIQUE index off them: an ordinary row stores `None`,
/// and multiple `NONE`s do not collide on a Surreal unique index.
///
/// The key is normalized case-insensitively and across whitespace because
/// [`is_firm_anchor`] protects every such variant. A narrower key would
/// let a fork in through the spelling it does not watch.
#[must_use]
pub fn firm_anchor_key(configured: &str, entity_name: &str) -> Option<String> {
    is_firm_anchor(configured, entity_name).then(|| entity_name.trim().to_lowercase())
}

/// Whether any row already carries the firm anchor's identity.
///
/// This is the *courtesy* half of the guard: it turns the ordinary case
/// into a clean validation error instead of a write that has to fail
/// first. It is emphatically not what makes the guard safe — the
/// `entity_firm_anchor` index is — because a read and the write it gates
/// are two statements with no transaction spanning them.
///
/// # Errors
///
/// [`EntityCommandError::Entities`] if the lookup fails.
pub async fn firm_anchor_exists(
    surreal: &SurrealDb,
    configured: &str,
    name: &str,
) -> Result<bool, EntityCommandError> {
    match firm_anchor_key(configured, name) {
        Some(key) => Ok(entities::firm_anchor_exists(surreal, &key).await?),
        None => Ok(false),
    }
}

/// Insert one Entity. The single write for every adapter: the JSON
/// `POST /app/api/entities` command, the standalone `/app/admin/entities` create
/// form, and the inline "Add entity" modal on the project form. Name is
/// required; the type is validated by [`require_entity_type`] and the
/// jurisdiction by [`require_jurisdiction`] — the engine does not
/// validate a `record<>` link, so a dangling id has to be caught here.
///
/// # Errors
///
/// [`EntityCommandError::FirmAnchorExists`] when the write would fork the
/// firm anchor — from the read below, or from the index when a concurrent
/// door slipped past it.
pub async fn create_entity(
    surreal: &SurrealDb,
    firm_anchor: &str,
    input: &CreateEntityCommand,
) -> Result<Entity, EntityCommandError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(EntityCommandError::Invalid("Name is required."));
    }
    require_entity_type(surreal, input.entity_type_id).await?;
    require_jurisdiction(surreal, input.jurisdiction_id).await?;

    let anchor_key = firm_anchor_key(firm_anchor, name);
    if let Some(key) = &anchor_key {
        if entities::firm_anchor_exists(surreal, key).await? {
            return Err(EntityCommandError::FirmAnchorExists);
        }
    }

    Ok(entities::create(
        surreal,
        &NewEntity {
            name: name.to_string(),
            entity_type_id: input.entity_type_id,
            jurisdiction_id: input.jurisdiction_id,
            phone: None,
            url: None,
            firm_anchor_key: anchor_key,
        },
    )
    .await?)
}

/// Refuse a write that references a jurisdiction with no row behind it.
///
/// `jurisdiction_id` is a real `record<jurisdiction>` link, but **the
/// engine does not validate a link** — it accepts one naming a row that was never
/// written — so this read-back is still the only thing between a typo and
/// an entity domiciled nowhere, exactly as `store::credentials::grant`
/// treats its own links.
async fn require_jurisdiction(
    surreal: &SurrealDb,
    jurisdiction_id: Uuid,
) -> Result<(), EntityCommandError> {
    match crate::jurisdictions::find_by_id(surreal, jurisdiction_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(EntityCommandError::Invalid(UNKNOWN_REFERENCE_MESSAGE)),
        Err(error) => Err(EntityCommandError::Jurisdictions(error)),
    }
}

/// Refuse a write that references an entity type with no row behind it.
/// Same contract as [`require_jurisdiction`].
async fn require_entity_type(
    surreal: &SurrealDb,
    entity_type_id: Uuid,
) -> Result<(), EntityCommandError> {
    match crate::entity_types::find_by_id(surreal, entity_type_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(EntityCommandError::Invalid(UNKNOWN_REFERENCE_MESSAGE)),
        Err(error) => Err(EntityCommandError::EntityTypes(error)),
    }
}

/// Update one Entity by id. The single write behind both the JSON
/// `PATCH /app/api/entities/{id}` command and the `/app/admin/entities/{id}` edit
/// form, so neither door can rename the firm anchor or fork it.
///
/// The row is read before the guards run, and between that read and the
/// write a concurrent rename could in principle mint the anchor. The
/// `entity_firm_anchor` index closes that window, and it closes it for the *write*
/// rather than for the check — a rename that would land a second anchor
/// key fails at the engine and surfaces as
/// [`EntityCommandError::FirmAnchorExists`] through [`From<EntityError>`].
///
/// # Errors
///
/// [`EntityCommandError::NotFound`] when no such row exists, and the
/// firm-anchor errors when a guard refuses the write.
pub async fn update_entity(
    surreal: &SurrealDb,
    id: Uuid,
    firm_anchor: &str,
    input: &UpdateEntityCommand,
) -> Result<Entity, EntityCommandError> {
    if input.name.trim().is_empty() {
        return Err(EntityCommandError::Invalid("Name is required."));
    }
    require_entity_type(surreal, input.entity_type_id).await?;
    require_jurisdiction(surreal, input.jurisdiction_id).await?;

    let existing = entities::find_by_id(surreal, id)
        .await?
        .ok_or(EntityCommandError::NotFound)?;

    // Compared byte for byte against the stored name, deliberately: a firm
    // anchor's name is immutable down to case and spacing. `store::seed` looks
    // the row up by exact name, so even a whitespace variant forks the anchor
    // into a duplicate row on the next boot, with both copies protected.
    let renaming = input.name != existing.name;
    if renaming && is_firm_anchor(firm_anchor, &existing.name) {
        return Err(EntityCommandError::FirmAnchorImmutable);
    }
    // Renaming an ordinary Entity *into* the anchor's name forks it exactly as
    // a create would, and the row is protected the moment it lands — so this
    // door needs the same guard as `create_entity` rather than the delete
    // guard's later, and by then useless, refusal.
    let anchor_key = firm_anchor_key(firm_anchor, &input.name);
    if renaming {
        if let Some(key) = &anchor_key {
            if entities::firm_anchor_exists(surreal, key).await? {
                return Err(EntityCommandError::FirmAnchorExists);
            }
        }
    }

    if let Some(updated) = entities::update(
        surreal,
        id,
        &NewEntity {
            name: input.name.clone(),
            entity_type_id: input.entity_type_id,
            jurisdiction_id: input.jurisdiction_id,
            // The port carries these across untouched: they are set by
            // the bulk-contact importer and by no field on this form, so
            // a full-replacement update must not blank them.
            phone: existing.phone,
            url: existing.url,
            firm_anchor_key: anchor_key,
        },
    )
    .await?
    {
        return Ok(updated);
    }
    // Nothing was written, and the two reasons are different answers: the
    // row is gone, or `entities::update`'s own guard refused to rename a
    // row that became the anchor after the read above. The immutability
    // check earlier in this function reads a snapshot; this one is the
    // authority, exactly as the delete door's is.
    match entities::find_by_id(surreal, id).await? {
        Some(_) => Err(EntityCommandError::FirmAnchorImmutable),
        None => Err(EntityCommandError::NotFound),
    }
}

/// Delete one Entity by id. The single write behind both the JSON
/// `DELETE /app/api/entities/{id}` command and the `/app/admin/entities/{id}/delete`
/// button, so neither door can remove the firm anchor.
///
/// Two guards, in order. The anchor guard refuses the protected row. The
/// dependent guard then counts what still points at it — the check that
/// replaced a foreign-key violation, since nothing in the engine refuses
/// a delete that strands a link. Returns the removed row.
///
/// The anchor guard is read **twice**, deliberately. The read below is
/// for the message; the authority is
/// [`entities::delete_unless_firm_anchor`]'s own `WHERE`, because a
/// concurrent rename can mint the anchor in the window between a check
/// and the write. The delete statement carries its own refusal, so there
/// is no window to close.
///
/// # Errors
///
/// [`EntityCommandError::FirmAnchorProtected`] for the firm's own row —
/// including one that became the firm's own row a moment ago —
/// [`EntityCommandError::Referenced`] when rows still point at it, and
/// [`EntityCommandError::NotFound`] when there was nothing to remove,
/// including when a concurrent delete won the race.
pub async fn delete_entity(
    surreal: &SurrealDb,
    id: Uuid,
    firm_anchor: &str,
) -> Result<Entity, EntityCommandError> {
    let target = entities::find_by_id(surreal, id)
        .await?
        .ok_or(EntityCommandError::NotFound)?;
    if is_firm_anchor(firm_anchor, &target.name) {
        return Err(EntityCommandError::FirmAnchorProtected);
    }

    let dependents = entities::dependents(surreal, id).await?;
    if !dependents.is_empty() {
        return Err(EntityCommandError::Referenced(describe_dependents(
            &dependents,
        )));
    }

    // Tie the success to an actual removal rather than to the read above.
    if let Some(removed) = entities::delete_unless_firm_anchor(surreal, id).await? {
        return Ok(removed);
    }
    // Nothing was removed, and the two reasons are not the same answer:
    // the row is gone (a racing delete won) or it is still there and
    // protected (a racing rename minted the anchor on it).
    match entities::find_by_id(surreal, id).await? {
        Some(_) => Err(EntityCommandError::FirmAnchorProtected),
        None => Err(EntityCommandError::NotFound),
    }
}

/// The human half of [`EntityCommandError::Referenced`] — "2 projects and
/// 1 address still reference it". Ordered as [`entities::dependents`]
/// returns them, which is fixed, so the sentence is stable.
fn describe_dependents(dependents: &[entities::Dependent]) -> String {
    let parts: Vec<String> = dependents
        .iter()
        .map(|dependent| format!("{} {}", dependent.count, dependent.noun()))
        .collect();
    let listed = match parts.as_slice() {
        [] => "other records".to_string(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    // One row is singular; anything else — several tables, or several
    // rows in one — is plural.
    let verb = match dependents {
        [only] if only.count == 1 => "references",
        _ => "reference",
    };
    format!("{listed} still {verb} it")
}

#[cfg(test)]
mod tests {
    use super::{
        create_entity, delete_entity, firm_anchor_key, is_firm_anchor, update_entity,
        CreateEntityCommand, EntityCommandError, UpdateEntityCommand,
    };
    use crate::entities::{self, EntityError};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    /// One engine plus one entity type and one jurisdiction, so a create
    /// has real references to point at — both are validated by read-back,
    /// because a `record<>` link is not checked by the engine.
    async fn fixture() -> (SurrealDb, Uuid, Uuid) {
        let surreal = mem().await;
        let type_id = crate::entity_types::create(&surreal, "LLC")
            .await
            .unwrap()
            .id;
        let jur_id = crate::jurisdictions::create(
            &surreal,
            &crate::jurisdictions::NewJurisdiction::new("Nevada", "NV", "state"),
        )
        .await
        .unwrap()
        .id;
        (surreal, type_id, jur_id)
    }

    fn command(name: &str, type_id: Uuid, jur_id: Uuid) -> CreateEntityCommand {
        CreateEntityCommand {
            name: name.into(),
            entity_type_id: type_id,
            jurisdiction_id: jur_id,
        }
    }

    fn edit(name: &str, type_id: Uuid, jur_id: Uuid) -> UpdateEntityCommand {
        UpdateEntityCommand {
            name: name.into(),
            entity_type_id: type_id,
            jurisdiction_id: jur_id,
        }
    }

    #[tokio::test]
    async fn create_trims_the_name_and_persists_one_row() {
        let (surreal, type_id, jur_id) = fixture().await;
        let created = create_entity(
            &surreal,
            "Acme Anchor",
            &command("  Beta LLC  ", type_id, jur_id),
        )
        .await
        .unwrap();
        assert_eq!(created.name, "Beta LLC");
        let rows = entities::all(&surreal).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, created.id);
    }

    #[tokio::test]
    async fn create_rejects_a_blank_name_without_writing() {
        let (surreal, type_id, jur_id) = fixture().await;
        assert!(matches!(
            create_entity(&surreal, "Acme Anchor", &command("   ", type_id, jur_id)).await,
            Err(EntityCommandError::Invalid(_))
        ));
        assert!(entities::all(&surreal).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_refuses_to_fork_the_configured_firm_anchor() {
        let (surreal, type_id, jur_id) = fixture().await;
        // The white-label operator's own firm can be created once...
        create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Rebrand Law PLLC", type_id, jur_id),
        )
        .await
        .unwrap();
        // ...and never a second time, in any case or spacing variant.
        assert!(matches!(
            create_entity(
                &surreal,
                "Rebrand Law PLLC",
                &command("  rebrand law pllc ", type_id, jur_id)
            )
            .await,
            Err(EntityCommandError::FirmAnchorExists)
        ));
        assert_eq!(entities::all(&surreal).await.unwrap().len(), 1);
    }

    /// Two doors race, both read a free name, and both write — so only
    /// the `entity_firm_anchor` index can decide it.
    #[tokio::test]
    async fn concurrent_creates_cannot_fork_the_firm_anchor() {
        let (surreal, type_id, jur_id) = fixture().await;
        let racer = command("Rebrand Law PLLC", type_id, jur_id);
        let other = command("Rebrand Law PLLC", type_id, jur_id);
        let (first, second) = tokio::join!(
            create_entity(&surreal, "Rebrand Law PLLC", &racer),
            create_entity(&surreal, "Rebrand Law PLLC", &other),
        );

        let outcomes = [first, second];
        assert_eq!(
            outcomes.iter().filter(|r| r.is_ok()).count(),
            1,
            "exactly one door may mint the anchor: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, Err(EntityCommandError::FirmAnchorExists)))
                .count(),
            1,
            "the loser reports the fork as a fork, not as a database fault: {outcomes:?}"
        );
        assert_eq!(entities::all(&surreal).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn create_refuses_to_fork_the_seeded_firm_under_a_white_label_config() {
        let (surreal, type_id, jur_id) = fixture().await;
        let seeded = crate::seed::FIRM_ENTITY_NAME;
        create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command(seeded, type_id, jur_id),
        )
        .await
        .unwrap();
        assert!(matches!(
            create_entity(
                &surreal,
                "Rebrand Law PLLC",
                &command(seeded, type_id, jur_id)
            )
            .await,
            Err(EntityCommandError::FirmAnchorExists)
        ));
    }

    #[tokio::test]
    async fn create_allows_ordinary_namesakes() {
        // Entity names are deliberately non-unique: only the anchor takes
        // the key, so two unrelated Betas both land.
        let (surreal, type_id, jur_id) = fixture().await;
        for _ in 0..2 {
            create_entity(
                &surreal,
                "Acme Anchor",
                &command("Beta LLC", type_id, jur_id),
            )
            .await
            .unwrap();
        }
        assert_eq!(entities::all(&surreal).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn create_maps_an_unknown_reference_to_a_caller_correctable_error() {
        // A type or jurisdiction id that points at no row is the caller's to
        // correct, so the command reports it as `Invalid` (the API renders
        // that as a 400) rather than as an internal fault (a 500).
        let (surreal, _type_id, jur_id) = fixture().await;
        let missing_type = Uuid::from_u128(0xdead_beef);
        assert!(matches!(
            create_entity(
                &surreal,
                "Acme Anchor",
                &command("Beta LLC", missing_type, jur_id)
            )
            .await,
            Err(EntityCommandError::Invalid(
                super::UNKNOWN_REFERENCE_MESSAGE
            ))
        ));
        assert!(entities::all(&surreal).await.unwrap().is_empty());
    }

    #[test]
    fn the_seeded_firm_stays_protected_under_a_white_label_configuration() {
        // A white-label operator names its own firm, which the canonical seed
        // never inserts. The configured row is protected once it exists, but
        // the seeded firm — which every boot re-creates — must not become
        // deletable in the meantime. The same holds for a typo'd value.
        for configured in ["Rebrand Law PLLC", "Neon Law LP"] {
            assert!(
                is_firm_anchor(configured, crate::seed::FIRM_ENTITY_NAME),
                "{configured} must not unprotect the seeded firm",
            );
        }
        assert!(is_firm_anchor("Rebrand Law PLLC", "Rebrand Law PLLC"));
        // An unrelated Entity stays freely deletable.
        assert!(!is_firm_anchor("Rebrand Law PLLC", "Acme LLC"));
    }

    #[test]
    fn protection_tolerates_case_and_whitespace_drift_between_env_and_row() {
        assert!(is_firm_anchor(
            "  shook law pllc  ",
            crate::seed::FIRM_ENTITY_NAME
        ));
        assert!(is_firm_anchor("Rebrand Law PLLC", " REBRAND LAW PLLC "));
    }

    /// The key is what the UNIQUE index sees, so every spelling
    /// [`is_firm_anchor`] protects has to normalize to *one* key —
    /// otherwise a variant slips past the index it is supposed to hit.
    /// An ordinary row must take no key at all, or namesakes would
    /// collide with each other.
    #[test]
    fn every_protected_spelling_normalizes_to_one_key() {
        let keys: Vec<Option<String>> = [
            "Rebrand Law PLLC",
            "  rebrand law pllc ",
            " REBRAND LAW PLLC ",
        ]
        .into_iter()
        .map(|variant| firm_anchor_key("Rebrand Law PLLC", variant))
        .collect();
        assert_eq!(
            keys,
            vec![Some("rebrand law pllc".to_string()); 3],
            "case and spacing variants must share one key"
        );

        assert_eq!(firm_anchor_key("Rebrand Law PLLC", "Beta LLC"), None);
        // The shipped default is protected under any configuration, so it
        // takes a key too — under its own spelling, not the configured one.
        assert_eq!(
            firm_anchor_key("Rebrand Law PLLC", crate::seed::FIRM_ENTITY_NAME),
            Some(crate::seed::FIRM_ENTITY_NAME.to_lowercase())
        );
    }

    /// The index refusal is the *only* thing that catches a fork the read
    /// guard missed, and which of the two racers hits it is up to the
    /// scheduler. The mapping is therefore asserted here, on the
    /// conversion itself, rather than left to whichever door
    /// `concurrent_creates_cannot_fork_the_firm_anchor` happens to lose.
    /// A caller must read a refused fork as a fork, not as a database
    /// fault it could retry.
    #[test]
    fn the_index_refusal_reads_as_a_fork_rather_than_a_database_fault() {
        let mapped = EntityCommandError::from(EntityError::FirmAnchorTaken);
        assert!(
            matches!(mapped, EntityCommandError::FirmAnchorExists),
            "{mapped:?}"
        );
        assert_eq!(mapped.user_message(), super::FIRM_ANCHOR_EXISTS_MESSAGE);
    }

    #[tokio::test]
    async fn update_replaces_every_field_and_returns_the_saved_row() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        let other_type = crate::entity_types::create(&surreal, "Trust")
            .await
            .unwrap()
            .id;

        let updated = update_entity(
            &surreal,
            row.id,
            "Acme Anchor",
            &edit("Beta Holdings LLC", other_type, jur_id),
        )
        .await
        .unwrap();

        assert_eq!(updated.id, row.id);
        assert_eq!(updated.name, "Beta Holdings LLC");
        assert_eq!(updated.entity_type_id, other_type);
    }

    /// `phone` and `url` are set by the bulk-contact importer and by no
    /// field on the edit form. A full-replacement update that forgot them
    /// would silently erase an organization's contact details on the next
    /// name correction.
    #[tokio::test]
    async fn update_preserves_contact_fields_the_form_does_not_carry() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = entities::create(
            &surreal,
            &entities::NewEntity {
                name: "Beta LLC".into(),
                entity_type_id: type_id,
                jurisdiction_id: jur_id,
                phone: Some("+1 702 555 0100".into()),
                url: Some("https://example.com".into()),
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();

        let updated = update_entity(
            &surreal,
            row.id,
            "Acme Anchor",
            &edit("Beta Holdings LLC", type_id, jur_id),
        )
        .await
        .unwrap();

        assert_eq!(updated.phone.as_deref(), Some("+1 702 555 0100"));
        assert_eq!(updated.url.as_deref(), Some("https://example.com"));
    }

    #[tokio::test]
    async fn update_rejects_a_blank_name() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        assert!(matches!(
            update_entity(
                &surreal,
                row.id,
                "Acme Anchor",
                &edit("   ", type_id, jur_id)
            )
            .await,
            Err(EntityCommandError::Invalid(_))
        ));
        // The row is untouched.
        assert_eq!(
            entities::find_by_id(&surreal, row.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Beta LLC"
        );
    }

    #[tokio::test]
    async fn update_missing_id_is_not_found() {
        let (surreal, type_id, jur_id) = fixture().await;
        assert!(matches!(
            update_entity(
                &surreal,
                Uuid::from_u128(0x00c0_ffee),
                "Acme Anchor",
                &edit("Ghost Co", type_id, jur_id)
            )
            .await,
            Err(EntityCommandError::NotFound)
        ));
    }

    #[tokio::test]
    async fn update_refuses_to_rename_the_firm_anchor_in_any_variant() {
        // `store::seed` finds the firm by exact name, so even a case or
        // whitespace variant would leave the next boot inserting a second
        // protected row. The name is immutable byte for byte.
        let (surreal, type_id, jur_id) = fixture().await;
        let firm = create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Rebrand Law PLLC", type_id, jur_id),
        )
        .await
        .unwrap();

        for variant in ["Renamed Firm", "REBRAND LAW PLLC", " Rebrand Law PLLC "] {
            assert!(
                matches!(
                    update_entity(
                        &surreal,
                        firm.id,
                        "Rebrand Law PLLC",
                        &edit(variant, type_id, jur_id)
                    )
                    .await,
                    Err(EntityCommandError::FirmAnchorImmutable)
                ),
                "{variant} must be refused"
            );
        }
        assert_eq!(
            entities::find_by_id(&surreal, firm.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Rebrand Law PLLC"
        );
    }

    #[tokio::test]
    async fn update_lets_the_firm_anchor_change_its_type_and_jurisdiction() {
        // Only the *name* is frozen: the anchor stays editable otherwise, so
        // an operator can correct a mis-picked entity type.
        let (surreal, type_id, jur_id) = fixture().await;
        let firm = create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Rebrand Law PLLC", type_id, jur_id),
        )
        .await
        .unwrap();
        let other_type = crate::entity_types::create(&surreal, "PLLC")
            .await
            .unwrap()
            .id;

        let updated = update_entity(
            &surreal,
            firm.id,
            "Rebrand Law PLLC",
            &edit("Rebrand Law PLLC", other_type, jur_id),
        )
        .await
        .unwrap();

        assert_eq!(updated.entity_type_id, other_type);
        assert_eq!(updated.name, "Rebrand Law PLLC");
        assert!(
            updated.is_firm_anchor(),
            "the anchor must keep its key across an ordinary edit, or the \
             next fork would find the index free"
        );
    }

    #[tokio::test]
    async fn update_refuses_to_rename_an_ordinary_entity_into_the_anchor() {
        // Renaming *into* the anchor's name forks it exactly as a create
        // would, and the row is protected the moment it lands.
        let (surreal, type_id, jur_id) = fixture().await;
        create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Rebrand Law PLLC", type_id, jur_id),
        )
        .await
        .unwrap();
        let ordinary = create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();

        assert!(matches!(
            update_entity(
                &surreal,
                ordinary.id,
                "Rebrand Law PLLC",
                &edit("Rebrand Law PLLC", type_id, jur_id)
            )
            .await,
            Err(EntityCommandError::FirmAnchorExists)
        ));
    }

    /// The immutability check reads a snapshot; `entities::update`'s own
    /// `WHERE` is the authority. Stage the window between them instead of
    /// racing for it: the row reads as ordinary by name, and carries the
    /// anchor key by the time the write runs, which is what a concurrent
    /// rename leaves behind. The rename must then be refused as
    /// immutable, not reported as a vanished row.
    #[tokio::test]
    async fn update_refuses_a_row_that_became_the_anchor_after_the_read() {
        let (surreal, type_id, jur_id) = fixture().await;
        let ordinary = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        entities::set_firm_anchor_key(&surreal, ordinary.id, Some("beta llc".into()))
            .await
            .unwrap();

        let refused = update_entity(
            &surreal,
            ordinary.id,
            "Acme Anchor",
            &edit("Gamma LLC", type_id, jur_id),
        )
        .await;

        assert!(
            matches!(refused, Err(EntityCommandError::FirmAnchorImmutable)),
            "{refused:?}"
        );
        assert_eq!(
            entities::find_by_id(&surreal, ordinary.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Beta LLC",
            "the refused rename must leave the row alone"
        );
    }

    #[tokio::test]
    async fn update_maps_a_missing_reference_to_a_validation_error() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        let err = update_entity(
            &surreal,
            row.id,
            "Acme Anchor",
            &edit("Beta LLC", Uuid::from_u128(0xdead_beef), jur_id),
        )
        .await
        .expect_err("a missing entity type must fail the update");
        assert!(matches!(err, EntityCommandError::Invalid(_)), "{err:?}");
        assert_eq!(err.user_message(), super::UNKNOWN_REFERENCE_MESSAGE);
    }

    #[tokio::test]
    async fn delete_removes_an_ordinary_entity_and_returns_it() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();

        let deleted = delete_entity(&surreal, row.id, "Acme Anchor")
            .await
            .unwrap();

        assert_eq!(deleted.id, row.id);
        assert_eq!(deleted.name, "Beta LLC");
        assert!(entities::find_by_id(&surreal, row.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_missing_id_is_not_found() {
        let (surreal, _type_id, _jur_id) = fixture().await;
        assert!(matches!(
            delete_entity(&surreal, Uuid::from_u128(0x00c0_ffee), "Acme Anchor").await,
            Err(EntityCommandError::NotFound)
        ));
    }

    /// Nothing in Surreal refuses a delete that strands a reference, so
    /// the refusal is the command's own — and it names every table to
    /// detach rather than one offending constraint.
    #[tokio::test]
    async fn delete_refuses_an_entity_a_matter_still_points_at() {
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        crate::projects::create(
            &surreal,
            &crate::projects::NewProject {
                code: format!("held-{}", Uuid::now_v7()),
                name: "Holding matter".into(),
                status: "open".into(),
                entity_id: row.id,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let refused = delete_entity(&surreal, row.id, "Acme Anchor").await;
        let Err(EntityCommandError::Referenced(detail)) = refused else {
            panic!("a referenced entity must not be deletable: {refused:?}");
        };
        assert!(
            detail.contains("1 project"),
            "the refusal must name what still points at the row, got: {detail}"
        );

        // And the row is still there.
        assert!(entities::find_by_id(&surreal, row.id)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn concurrent_deletes_of_one_entity_yield_one_success_and_one_not_found() {
        // Two requests race to remove the same ordinary Entity. Both pass
        // the guards, because both read the row before either delete
        // lands. Only one `DELETE` returns a row, and the loser reports
        // `NotFound` rather than claiming a removal it did not make —
        // which is why the success is tied to the delete's own result and
        // not to the read above it.
        let (surreal, type_id, jur_id) = fixture().await;
        let row = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();

        let (first, second) = tokio::join!(
            delete_entity(&surreal, row.id, "Acme Anchor"),
            delete_entity(&surreal, row.id, "Acme Anchor"),
        );

        let outcomes = [first, second];
        assert_eq!(
            outcomes.iter().filter(|r| r.is_ok()).count(),
            1,
            "exactly one delete removes the row: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, Err(EntityCommandError::NotFound)))
                .count(),
            1,
            "the losing racer reports not-found rather than a false success: {outcomes:?}"
        );
        assert!(entities::find_by_id(&surreal, row.id)
            .await
            .unwrap()
            .is_none());
    }

    /// The delete door's anchor guard reads the name; the authority is
    /// `entities::delete_unless_firm_anchor`'s own `WHERE`, which reads
    /// the key. Stage the window between them rather than racing for it,
    /// with the row ordinary by name and protected by key. The refusal
    /// has to be `FirmAnchorProtected`, because reporting the row as
    /// gone would tell an operator a removal happened.
    #[tokio::test]
    async fn delete_refuses_a_row_that_became_the_anchor_after_the_read() {
        let (surreal, type_id, jur_id) = fixture().await;
        let ordinary = create_entity(
            &surreal,
            "Acme Anchor",
            &command("Beta LLC", type_id, jur_id),
        )
        .await
        .unwrap();
        entities::set_firm_anchor_key(&surreal, ordinary.id, Some("beta llc".into()))
            .await
            .unwrap();

        let refused = delete_entity(&surreal, ordinary.id, "Acme Anchor").await;

        assert!(
            matches!(refused, Err(EntityCommandError::FirmAnchorProtected)),
            "{refused:?}"
        );
        assert!(entities::find_by_id(&surreal, ordinary.id)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn delete_refuses_the_firm_anchor_in_either_name() {
        // Both the configured firm and the shipped default are protected:
        // `store::seed` re-creates the latter by exact name on every boot, so
        // removing it never sticks.
        let (surreal, type_id, jur_id) = fixture().await;
        let configured = create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command("Rebrand Law PLLC", type_id, jur_id),
        )
        .await
        .unwrap();
        let seeded = create_entity(
            &surreal,
            "Rebrand Law PLLC",
            &command(crate::seed::FIRM_ENTITY_NAME, type_id, jur_id),
        )
        .await
        .unwrap();

        for row in [&configured, &seeded] {
            assert!(
                matches!(
                    delete_entity(&surreal, row.id, "Rebrand Law PLLC").await,
                    Err(EntityCommandError::FirmAnchorProtected)
                ),
                "{} must be undeletable",
                row.name
            );
            assert!(entities::find_by_id(&surreal, row.id)
                .await
                .unwrap()
                .is_some());
        }
    }

    /// The refusal is read by a person deciding what to detach, so the
    /// count and the noun have to agree — "1 addresse" is what deriving
    /// the singular by trimming an `s` produces.
    #[test]
    fn the_dependent_sentence_reads_as_english() {
        let dependent = |singular, plural, count| entities::Dependent {
            singular,
            plural,
            count,
        };

        assert_eq!(
            super::describe_dependents(&[dependent("project", "projects", 1)]),
            "1 project still references it"
        );
        assert_eq!(
            super::describe_dependents(&[
                dependent("project", "projects", 2),
                dependent("address", "addresses", 1),
            ]),
            "2 projects and 1 address still reference it"
        );
        assert_eq!(
            super::describe_dependents(&[
                dependent("project", "projects", 2),
                dependent("address", "addresses", 1),
                dependent("entity role", "entity roles", 3),
            ]),
            "2 projects, 1 address and 3 entity roles still reference it"
        );
    }
}
