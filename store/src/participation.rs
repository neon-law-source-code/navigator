//! Participation commands for the SurrealDB projects cluster.
//!
//! `participation` controls project visibility, so every mutation validates its
//! native person/project links and preserves the lawyer-DRI invariant at this
//! shared command boundary: a matter's accountable lawyers are a set, and that
//! set is never empty.
//!
//! Accountability is also the one thing here that names its actor. Every door
//! that moves a marker takes a [`DriActor`], which answers two questions at
//! once — whether this caller may change that side, and whose name goes on the
//! audit row. They are the same field because a rule enforced against one person
//! and recorded against another is not an audit trail.
//!
//! No caller names a participation. Which side of a matter someone is on
//! follows from `persons.role`, so these commands derive it through
//! [`projects::participation_for_role`] and the three doors above them — the
//! lawyer form, `POST /app/api/projects/{id}/participants`, and
//! `aida_link_person_project` — all take a person and nothing else. A word that
//! disagreed with the tier used to be typeable, and a `client` recorded as
//! `attorney` is a firm-side row: the matter's own client reading `/app/lawyer`.

use uuid::Uuid;

use crate::persons::Person;
use crate::projects::{self, DriSide, PersonProjectRole};
use crate::surreal::SurrealDb;

/// What a write door asks of the matter's accountability markers.
///
/// Participation says who reaches a matter; this says who is *accountable* for
/// it. Every door that does not offer the control leaves it [`Unchanged`], so
/// adding a person never quietly moves accountability.
///
/// Each side is a **set**. A matter can have as many accountable lawyers, and as
/// many accountable client contacts, as the firm has put on it, so
/// [`Designate`] adds to a side and displaces nobody, and [`Clear`] is the only
/// way out of one.
///
/// [`Unchanged`]: DriRequest::Unchanged
/// [`Designate`]: DriRequest::Designate
/// [`Clear`]: DriRequest::Clear
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriRequest {
    /// Leave both markers exactly as they are.
    #[default]
    Unchanged,
    /// Take this row's person out of whichever side they are accountable on.
    /// Refused for the last lawyer DRI: the lawyer set is never empty.
    Clear,
    /// Add this row's person to the given side's set.
    Designate(DriSide),
}

/// Who is asking for a DRI change.
///
/// Accountability is the one thing on a matter that has to name the person who
/// moved it, so the commands carry the actor rather than leaving the handler to
/// check one thing and the audit trail to record another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriActor {
    /// A trusted internal caller with no person behind it — matter open, the
    /// seed, a workflow step. Not gated, and audited with no actor.
    System,
    /// A signed-in person, gated by [`authorize`] and named in the audit row.
    Person(Uuid),
}

/// Refusals shared by both write doors when a DRI request cannot be honored.
///
/// Each maps into the door's own error enum, so a caller still matches one type.
#[derive(Debug, thiserror::Error)]
pub enum DriError {
    /// The side does not match the person's tier: the lawyer DRI is the
    /// accountable lawyer (`owner`/`admin`/`lawyer`), and the client DRI is the
    /// client-side contact.
    #[error("that person's tier cannot carry that side's accountability marker")]
    TierMismatch,
    /// This actor may not change that side's accountability on this matter.
    #[error("you may not change that side's accountability on this matter")]
    NotPermitted,
    /// The acting person has no `persons` row, so no rule can be applied to
    /// them. Distinct from [`NotPermitted`]: nothing was decided.
    ///
    /// [`NotPermitted`]: DriError::NotPermitted
    #[error("the acting person is not on file")]
    ActorUnknown,
    /// The last lawyer DRI cannot step off — a matter always has at least one.
    #[error("a matter always has a lawyer DRI; add another before removing this one")]
    LawyerDriRequired,
    #[error("database: {0}")]
    Db(String),
}

#[derive(Debug, Clone)]
pub struct AddParticipantCommand {
    pub project_id: Uuid,
    pub person_id: Uuid,
    /// The accountability marker this add designates, if any.
    pub dri: DriRequest,
    /// Who is asking. Consulted only when `dri` would move a marker.
    pub actor: DriActor,
}

#[derive(Debug, thiserror::Error)]
pub enum AddParticipantError {
    #[error("no such project")]
    ProjectNotFound,
    #[error("no such person")]
    PersonNotFound,
    #[error("that person is already assigned to this matter")]
    Duplicate,
    #[error(transparent)]
    Dri(DriError),
    #[error("database: {0}")]
    Db(String),
}

/// Whether this tier may carry that side's marker.
///
/// The lawyer DRI is the accountable lawyer, so it is the `owner`/`admin`/`lawyer`
/// tier — never a Clerk, who is a supervised non-lawyer, and never a client. The
/// client DRI is the client-side contact, which is the `client` tier.
fn tier_may_carry(person: &Person, side: DriSide) -> bool {
    match side {
        DriSide::Lawyer => person.role.is_lawyer_tier(),
        DriSide::Client => !person.role.is_lawyer_tier() && !person.role.is_clerk(),
    }
}

/// The marker write a validated request resolved to.
///
/// Carrying the direction as well as the side is what lets one audit row
/// describe the change without re-deriving it from the flags afterwards, by
/// which time they have already moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DriChange {
    side: DriSide,
    /// `true` to join the side's set, `false` to leave it.
    designating: bool,
}

/// Whether this actor may change that side's accountability on this matter.
///
/// The two sides answer to different people. The lawyer side is
/// **self-governing**: the lawyers already accountable for a matter are the ones
/// who decide who else is, so any current lawyer DRI may add or remove any
/// other, themselves included — bounded only by the rule that the set never
/// empties. The client side is the firm's call, so it takes the lawyer tier and
/// above; a client never designates their own counterpart, and a Clerk is a
/// supervised non-lawyer who designates neither.
///
/// Owner and Admin pass both. A matter whose lawyer set is empty has no holder
/// for the self-governing rule to admit, so the first lawyer DRI is named by a
/// lawyer-tier participant already on the matter, by Owner/Admin from outside
/// the ledger, or by [`DriActor::System`] at open time.
async fn authorize(
    surreal: &SurrealDb,
    project_id: Uuid,
    actor: DriActor,
    side: DriSide,
) -> Result<(), DriError> {
    let DriActor::Person(actor_id) = actor else {
        return Ok(());
    };
    let acting = crate::persons::find_by_id(surreal, actor_id)
        .await
        .map_err(|error| DriError::Db(error.to_string()))?
        .ok_or(DriError::ActorUnknown)?;
    if acting.role.is_admin_tier() {
        return Ok(());
    }
    let permitted = match side {
        DriSide::Client => acting.role.is_lawyer_tier(),
        DriSide::Lawyer => {
            let current = holders(surreal, project_id, DriSide::Lawyer).await?;
            current.contains(&actor_id)
                || (current.is_empty()
                    && acting.role.is_lawyer_tier()
                    && actor_is_firm_participant(surreal, project_id, actor_id).await?)
        }
    };
    permitted.then_some(()).ok_or(DriError::NotPermitted)
}

/// Whether this person already sits on the firm side of the matter.
///
/// The empty lawyer-DRI set has no self-governing holder to admit a designation,
/// so the first marker is named by a lawyer-tier participant who is already on
/// the matter — not by someone reaching in from outside it.
async fn actor_is_firm_participant(
    surreal: &SurrealDb,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, DriError> {
    let Some(row) = projects::participation_for_person(surreal, actor_id, project_id)
        .await
        .map_err(|error| DriError::Db(error.to_string()))?
    else {
        return Ok(false);
    };
    Ok(!projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str()))
}

/// Validate a DRI request against the matter's current markers, before anything
/// is written. Returns the change to write, or `None` for a request that needs
/// no marker write at all.
///
/// `clearing` is the row whose marker is being dropped on a [`DriRequest::Clear`]
/// — `None` on the add door, where there is no existing row to clear.
async fn check_dri(
    surreal: &SurrealDb,
    project_id: Uuid,
    person: &Person,
    actor: DriActor,
    request: DriRequest,
    clearing: Option<&PersonProjectRole>,
) -> Result<Option<DriChange>, DriError> {
    match request {
        DriRequest::Unchanged => Ok(None),
        DriRequest::Clear => {
            // Which side to leave is a fact about the row, not something the
            // caller names. A row carrying neither marker has nothing to clear.
            let Some(side) = clearing.and_then(|row| {
                if row.is_lawyer_dri {
                    Some(DriSide::Lawyer)
                } else if row.is_client_dri {
                    Some(DriSide::Client)
                } else {
                    None
                }
            }) else {
                return Ok(None);
            };
            if side == DriSide::Lawyer && holders(surreal, project_id, side).await?.len() <= 1 {
                return Err(DriError::LawyerDriRequired);
            }
            authorize(surreal, project_id, actor, side).await?;
            Ok(Some(DriChange {
                side,
                designating: false,
            }))
        }
        DriRequest::Designate(side) => {
            if !tier_may_carry(person, side) {
                return Err(DriError::TierMismatch);
            }
            // Already accountable on this side: designation is additive, so
            // re-affirming someone changes nothing and records nothing.
            if holders(surreal, project_id, side)
                .await?
                .contains(&person.id)
            {
                return Ok(None);
            }
            authorize(surreal, project_id, actor, side).await?;
            Ok(Some(DriChange {
                side,
                designating: true,
            }))
        }
    }
}

/// Everyone carrying `side`'s marker on this matter.
///
/// A matter's accountability on each side is a set, so this is the shape every
/// rule above reads — "is it empty", "am I in it", "how many are left".
pub async fn holders(
    surreal: &SurrealDb,
    project_id: Uuid,
    side: DriSide,
) -> Result<Vec<Uuid>, DriError> {
    let rows = projects::participations_for_project(surreal, project_id)
        .await
        .map_err(|error| DriError::Db(error.to_string()))?;
    Ok(rows
        .into_iter()
        .filter(|row| match side {
            DriSide::Lawyer => row.is_lawyer_dri,
            DriSide::Client => row.is_client_dri,
        })
        .map(|row| row.person_id)
        .collect())
}

/// Append the audit entry for one DRI change.
///
/// The matter is the subject, because "who has been accountable for this matter"
/// is the question the trail gets asked; the person moved is named in the
/// detail.
async fn record_dri_change(
    surreal: &SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    actor: DriActor,
    change: DriChange,
) -> Result<(), DriError> {
    let action = match (change.side, change.designating) {
        (DriSide::Lawyer, true) => "lawyer_dri_designated",
        (DriSide::Lawyer, false) => "lawyer_dri_removed",
        (DriSide::Client, true) => "client_dri_designated",
        (DriSide::Client, false) => "client_dri_removed",
    };
    crate::relationship_logs::record(
        surreal,
        &crate::relationship_logs::NewRelationshipLog {
            actor_person_id: match actor {
                DriActor::Person(id) => Some(id),
                DriActor::System => None,
            },
            subject_type: "project".to_string(),
            subject_id: project_id,
            action: action.to_string(),
            detail: person_id.to_string(),
        },
    )
    .await
    .map(|_| ())
    .map_err(|error| DriError::Db(error.to_string()))
}

/// Write the validated DRI change and return this person's refreshed row.
///
/// **The audit entry lands first.** These are two writes and SurrealDB gives no
/// transaction across them, so one of them has to be able to fail alone. An
/// entry describing a change that did not happen is a discrepancy the trail
/// itself shows; a marker that moved with nothing recording it is the thing the
/// trail exists to prevent. Ordering it this way makes the recoverable failure
/// the possible one.
async fn apply_dri(
    surreal: &SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    actor: DriActor,
    change: Option<DriChange>,
    row: PersonProjectRole,
) -> Result<PersonProjectRole, DriError> {
    let Some(change) = change else {
        return Ok(row);
    };
    record_dri_change(surreal, project_id, person_id, actor, change).await?;
    if change.designating {
        projects::designate_dri_in_surreal(surreal, project_id, person_id, change.side)
            .await
            .map_err(|error| DriError::Db(error.to_string()))?;
    } else {
        projects::clear_dri_in_surreal(surreal, row.id, change.side)
            .await
            .map_err(|error| DriError::Db(error.to_string()))?;
    }
    // The write rewrote the flags on this row, so the caller's copy is stale —
    // re-read it rather than hand back a row that lies about the marker.
    projects::participation_by_id(surreal, row.id)
        .await
        .map_err(|error| DriError::Db(error.to_string()))
        .map(|refreshed| refreshed.unwrap_or(row))
}

pub async fn add_participant(
    surreal: &SurrealDb,
    input: &AddParticipantCommand,
) -> Result<PersonProjectRole, AddParticipantError> {
    let person = crate::persons::find_by_id(surreal, input.person_id)
        .await
        .map_err(|error| AddParticipantError::Db(error.to_string()))?
        .ok_or(AddParticipantError::PersonNotFound)?;
    if projects::participation_for_person(surreal, input.person_id, input.project_id)
        .await
        .map_err(db_add)?
        .is_some()
    {
        return Err(AddParticipantError::Duplicate);
    }
    // Every refusal lands before the first write, so a rejected designation
    // never leaves a bare participation row behind.
    let change = check_dri(
        surreal,
        input.project_id,
        &person,
        input.actor,
        input.dri,
        None,
    )
    .await
    .map_err(AddParticipantError::Dri)?;
    let participation = projects::participation_for_role(person.role);
    let row =
        projects::add_participation(surreal, input.project_id, input.person_id, participation)
            .await
            .map_err(db_add)?;
    apply_dri(
        surreal,
        input.project_id,
        input.person_id,
        input.actor,
        change,
        row,
    )
    .await
    .map_err(AddParticipantError::Dri)
}

fn db_add(error: projects::ProjectStoreError) -> AddParticipantError {
    match error {
        projects::ProjectStoreError::NoSuchProject(_) => AddParticipantError::ProjectNotFound,
        projects::ProjectStoreError::NoSuchPerson(_) => AddParticipantError::PersonNotFound,
        other if other.to_string().contains("person_project_role_pair") => {
            AddParticipantError::Duplicate
        }
        other => AddParticipantError::Db(other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct UpdateParticipantCommand {
    pub project_id: Uuid,
    pub role_id: Uuid,
    pub person_id: Uuid,
    /// The accountability marker this edit designates, clears, or leaves alone.
    pub dri: DriRequest,
    /// Who is asking. Consulted only when `dri` would move a marker.
    pub actor: DriActor,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateParticipantError {
    #[error("no such participation")]
    NotFound,
    #[error("no such person")]
    PersonNotFound,
    #[error("that person is already assigned to this matter")]
    Duplicate,
    #[error("that row is the matter's lawyer DRI and cannot be moved off the firm side")]
    DriLockout,
    #[error(transparent)]
    Dri(DriError),
    #[error("database: {0}")]
    Db(String),
}

pub async fn update_participant(
    surreal: &SurrealDb,
    input: &UpdateParticipantCommand,
) -> Result<PersonProjectRole, UpdateParticipantError> {
    let existing = projects::participation_by_id(surreal, input.role_id)
        .await
        .map_err(db_update)?
        .filter(|role| role.project_id == input.project_id)
        .ok_or(UpdateParticipantError::NotFound)?;
    let person = crate::persons::find_by_id(surreal, input.person_id)
        .await
        .map_err(|error| UpdateParticipantError::Db(error.to_string()))?
        .ok_or(UpdateParticipantError::PersonNotFound)?;
    // Re-pointing the row re-derives its side of the matter. A lawyer DRI whose
    // incoming person is client-tier would flip client-side and strand the
    // matter's accountable lawyer, so the lockout reads the derived value.
    let participation = projects::participation_for_role(person.role);
    if existing.is_lawyer_dri
        && (existing.person_id != input.person_id
            || projects::PARTICIPATION_CLIENT_SIDE.contains(&participation))
    {
        return Err(UpdateParticipantError::DriLockout);
    }
    if existing.person_id != input.person_id
        && projects::participation_for_person(surreal, input.person_id, input.project_id)
            .await
            .map_err(db_update)?
            .is_some()
    {
        return Err(UpdateParticipantError::Duplicate);
    }
    // Validated before the update, so a refused designation leaves the row
    // pointing where it already pointed.
    let change = check_dri(
        surreal,
        input.project_id,
        &person,
        input.actor,
        input.dri,
        Some(&existing),
    )
    .await
    .map_err(UpdateParticipantError::Dri)?;
    let row =
        projects::update_participation(surreal, input.role_id, input.person_id, participation)
            .await
            .map_err(db_update)?
            .ok_or(UpdateParticipantError::NotFound)?;
    apply_dri(
        surreal,
        input.project_id,
        input.person_id,
        input.actor,
        change,
        row,
    )
    .await
    .map_err(UpdateParticipantError::Dri)
}

fn db_update(error: projects::ProjectStoreError) -> UpdateParticipantError {
    match error {
        projects::ProjectStoreError::NoSuchPerson(_) => UpdateParticipantError::PersonNotFound,
        other if other.to_string().contains("person_project_role_pair") => {
            UpdateParticipantError::Duplicate
        }
        other => UpdateParticipantError::Db(other.to_string()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveParticipantError {
    #[error("no such participation")]
    NotFound,
    #[error("that row is the matter's last lawyer DRI and cannot be removed")]
    DriLockout,
    #[error(transparent)]
    Dri(DriError),
    #[error("database: {0}")]
    Db(String),
}

/// Take one person off a matter.
///
/// Removing a participation that carries a marker is also a DRI change, so it
/// answers to the same two rules the designation door does: the lawyer set never
/// empties, and the actor has to be entitled to change that side. It is audited
/// on the same trail, for the same reason — a matter's accountability should not
/// be able to change through the door nobody was watching.
pub async fn remove_participant(
    surreal: &SurrealDb,
    project_id: Uuid,
    role_id: Uuid,
    actor: DriActor,
) -> Result<(), RemoveParticipantError> {
    let existing = projects::participation_by_id(surreal, role_id)
        .await
        .map_err(|error| RemoveParticipantError::Db(error.to_string()))?
        .filter(|role| role.project_id == project_id)
        .ok_or(RemoveParticipantError::NotFound)?;
    let side = if existing.is_lawyer_dri {
        Some(DriSide::Lawyer)
    } else if existing.is_client_dri {
        Some(DriSide::Client)
    } else {
        None
    };
    if let Some(side) = side {
        if side == DriSide::Lawyer
            && holders(surreal, project_id, side)
                .await
                .map_err(RemoveParticipantError::Dri)?
                .len()
                <= 1
        {
            return Err(RemoveParticipantError::DriLockout);
        }
        authorize(surreal, project_id, actor, side)
            .await
            .map_err(RemoveParticipantError::Dri)?;
        // Audited before the row is gone, so the trail records the removal even
        // if the delete itself then fails — the same ordering `apply_dri` uses.
        record_dri_change(
            surreal,
            project_id,
            existing.person_id,
            actor,
            DriChange {
                side,
                designating: false,
            },
        )
        .await
        .map_err(RemoveParticipantError::Dri)?;
    }
    projects::remove_participation(surreal, role_id)
        .await
        .map_err(|error| RemoveParticipantError::Db(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        add_participant, remove_participant, update_participant, AddParticipantCommand,
        AddParticipantError, DriActor, DriError, DriRequest, RemoveParticipantError,
        UpdateParticipantCommand, UpdateParticipantError,
    };
    use crate::persons::{self, NewPerson, Role};
    use crate::projects::{self, DriSide, NewProject};
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    /// Add `who` to the matter, asking for `dri`, as `actor`.
    async fn add(
        surreal: &SurrealDb,
        matter: Uuid,
        who: Uuid,
        dri: DriRequest,
        actor: DriActor,
    ) -> Result<projects::PersonProjectRole, AddParticipantError> {
        add_participant(
            surreal,
            &AddParticipantCommand {
                project_id: matter,
                person_id: who,
                dri,
                actor,
            },
        )
        .await
    }

    /// Everyone accountable on `side`, as a set the assertions can compare.
    async fn holders(surreal: &SurrealDb, matter: Uuid, side: DriSide) -> Vec<Uuid> {
        let mut ids = super::holders(surreal, matter, side).await.unwrap();
        ids.sort();
        ids
    }

    /// The audit trail this matter's DRI changes wrote, oldest first.
    async fn dri_trail(surreal: &SurrealDb, matter: Uuid) -> Vec<(String, String)> {
        let mut trail: Vec<_> = crate::relationship_logs::all(surreal)
            .await
            .unwrap()
            .into_iter()
            .filter(|log| log.subject_id == matter && log.action.contains("_dri_"))
            .map(|log| (log.action, log.detail))
            .collect();
        trail.reverse();
        trail
    }

    async fn person(surreal: &SurrealDb, tag: &str, role: Role) -> Uuid {
        persons::create(
            surreal,
            &NewPerson::with_role(tag, format!("{tag}@example.com"), role),
        )
        .await
        .unwrap()
        .id
    }

    async fn project(surreal: &SurrealDb, code: &str) -> Uuid {
        projects::create(
            surreal,
            &NewProject {
                code: code.into(),
                name: code.into(),
                status: "open".into(),
                entity_id: crate::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    /// The command is the only place a participation is chosen, and it reads
    /// `persons.role`. Every tier lands under its own word, and `client` is the
    /// only one on the client side.
    #[tokio::test]
    async fn add_derives_the_participation_from_the_person_tier() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "derive-add").await;
        for (tier, expected) in [
            (Role::Owner, "owner"),
            (Role::Admin, "admin"),
            (Role::Lawyer, "lawyer"),
            (Role::Clerk, "clerk"),
            (Role::Client, "client"),
        ] {
            let who = person(&surreal, expected, tier).await;
            let row = add(
                &surreal,
                matter,
                who,
                DriRequest::Unchanged,
                DriActor::System,
            )
            .await
            .unwrap();
            assert_eq!(row.participation, expected);
            assert_eq!(
                projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str()),
                tier == Role::Client,
                "{expected} sits on the wrong side of the matter"
            );
        }
    }

    /// Re-pointing a row re-runs the derivation against the incoming person, so
    /// the stored word can never drift from the tier that justifies it.
    #[tokio::test]
    async fn update_re_derives_from_the_incoming_person() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "derive-update").await;
        let lawyer = person(&surreal, "lawyer", Role::Lawyer).await;
        let libra = person(&surreal, "libra", Role::Client).await;

        let row = add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Unchanged,
            DriActor::System,
        )
        .await
        .unwrap();
        assert_eq!(row.participation, "lawyer");

        let moved = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: row.id,
                person_id: libra,
                dri: DriRequest::Unchanged,
                actor: DriActor::System,
            },
        )
        .await
        .unwrap();
        assert_eq!(moved.person_id, libra);
        assert_eq!(moved.participation, "client");
    }

    /// The lawyer-DRI lockout now reads the derived value: handing the DRI's row
    /// to a client-tier person would flip it client-side and strand the matter's
    /// accountable lawyer, so it is refused rather than silently demoted.
    #[tokio::test]
    async fn re_pointing_the_lawyer_dri_at_a_client_tier_person_is_refused() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "derive-dri").await;
        let lawyer = person(&surreal, "dri", Role::Lawyer).await;
        let libra = person(&surreal, "libra", Role::Client).await;
        projects::designate_dri_in_surreal(&surreal, matter, lawyer, DriSide::Lawyer)
            .await
            .unwrap();
        let dri_row = projects::participations_for_project(&surreal, matter)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.is_lawyer_dri)
            .expect("the designation wrote a row");
        // Derived from the lawyer's tier, not a word this call picked.
        assert_eq!(dri_row.participation, "lawyer");

        let refused = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: dri_row.id,
                person_id: libra,
                dri: DriRequest::Unchanged,
                actor: DriActor::System,
            },
        )
        .await;
        assert!(matches!(refused, Err(UpdateParticipantError::DriLockout)));
    }

    /// The add door can designate, and the designation is what makes the row the
    /// accountable one — not a word anyone typed.
    #[tokio::test]
    async fn add_designates_the_lawyer_dri_it_was_asked_for() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-add").await;
        let lawyer = person(&surreal, "lawyer", Role::Lawyer).await;

        let row = add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();

        assert!(row.is_lawyer_dri, "the returned row carries the marker");
        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, [lawyer]);
    }

    /// ENG-35: there is no separate Clerk-visibility flag. Adding a Clerk's
    /// participation row through this command is the whole toggle — it is what
    /// makes `store::access::can_see_project` admit them to the matter and its
    /// portal, and removing the row is the toggle back off. Both API doors
    /// funnel through this exact command, so proving it here proves the
    /// behavior for either.
    #[tokio::test]
    async fn adding_and_removing_a_clerks_participation_toggles_matter_visibility() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "clerk-toggle").await;
        let lawyer = person(&surreal, "lawyer", Role::Lawyer).await;
        let clerk = person(&surreal, "clerk", Role::Clerk).await;

        // A matter always has a lawyer DRI in production (the invariant this
        // module enforces); a Clerk's supervised view requires one to exist,
        // so designate it here rather than relying on an unstated default.
        add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();

        assert!(
            !crate::access::can_see_project(&surreal, Some(clerk), Role::Clerk, matter)
                .await
                .unwrap(),
            "off before any participation row exists"
        );

        let row = add(
            &surreal,
            matter,
            clerk,
            DriRequest::Unchanged,
            DriActor::System,
        )
        .await
        .unwrap();
        assert_eq!(row.participation, "clerk");

        assert!(
            crate::access::can_see_project(&surreal, Some(clerk), Role::Clerk, matter)
                .await
                .unwrap(),
            "on once a lawyer has added the Clerk's participation row"
        );

        remove_participant(&surreal, matter, row.id, DriActor::System)
            .await
            .unwrap();

        assert!(
            !crate::access::can_see_project(&surreal, Some(clerk), Role::Clerk, matter)
                .await
                .unwrap(),
            "off again once the row is removed"
        );
    }

    /// Accountability on each side is a set. Designating a second lawyer adds
    /// them beside the first rather than taking the marker away, which is the
    /// whole point: a matter can be two lawyers' responsibility at once.
    #[tokio::test]
    async fn a_matter_carries_several_lawyer_dris_and_several_client_dris() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-set").await;
        let first = person(&surreal, "first", Role::Lawyer).await;
        let second = person(&surreal, "second", Role::Lawyer).await;
        let libra = person(&surreal, "libra", Role::Client).await;
        let virgo = person(&surreal, "virgo", Role::Client).await;

        for who in [first, second] {
            add(
                &surreal,
                matter,
                who,
                DriRequest::Designate(DriSide::Lawyer),
                DriActor::System,
            )
            .await
            .unwrap();
        }
        for who in [libra, virgo] {
            add(
                &surreal,
                matter,
                who,
                DriRequest::Designate(DriSide::Client),
                DriActor::System,
            )
            .await
            .unwrap();
        }

        let mut lawyers = vec![first, second];
        lawyers.sort();
        let mut clients = vec![libra, virgo];
        clients.sort();
        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, lawyers);
        assert_eq!(holders(&surreal, matter, DriSide::Client).await, clients);
    }

    /// The lawyer side governs itself: a lawyer already accountable for the
    /// matter may add a peer and take one back off, with no admin in the loop.
    #[tokio::test]
    async fn a_lawyer_dri_may_add_and_remove_a_peer() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-peer").await;
        let held = person(&surreal, "held", Role::Lawyer).await;
        let peer = person(&surreal, "peer", Role::Lawyer).await;
        add(
            &surreal,
            matter,
            held,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();

        let added = add(
            &surreal,
            matter,
            peer,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::Person(held),
        )
        .await
        .unwrap();
        assert!(added.is_lawyer_dri);

        // And the peer, now accountable, can hand the first one's marker back.
        let held_row = projects::participation_for_person(&surreal, held, matter)
            .await
            .unwrap()
            .expect("the designation wrote a row");
        update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: held_row.id,
                person_id: held,
                dri: DriRequest::Clear,
                actor: DriActor::Person(peer),
            },
        )
        .await
        .unwrap();

        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, [peer]);
    }

    /// Self-governing means *the people already accountable*, not the tier. A
    /// lawyer on the matter who holds no marker is refused, and so is a lawyer
    /// who is not on the matter at all.
    #[tokio::test]
    async fn a_lawyer_holding_no_marker_may_not_change_the_lawyer_set() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-outsider").await;
        let accountable = person(&surreal, "accountable", Role::Lawyer).await;
        let bystander = person(&surreal, "bystander", Role::Lawyer).await;
        let stranger = person(&surreal, "stranger", Role::Lawyer).await;
        add(
            &surreal,
            matter,
            accountable,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();
        add(
            &surreal,
            matter,
            bystander,
            DriRequest::Unchanged,
            DriActor::System,
        )
        .await
        .unwrap();

        for actor in [bystander, stranger] {
            let refused = add(
                &surreal,
                matter,
                person(&surreal, &format!("candidate-{actor}"), Role::Lawyer).await,
                DriRequest::Designate(DriSide::Lawyer),
                DriActor::Person(actor),
            )
            .await;
            assert!(
                matches!(
                    refused,
                    Err(AddParticipantError::Dri(DriError::NotPermitted))
                ),
                "a lawyer with no marker must not designate one: {refused:?}"
            );
        }
        assert_eq!(
            holders(&surreal, matter, DriSide::Lawyer).await,
            [accountable]
        );
    }

    /// The client side is the firm's call. Lawyer tier and above may designate
    /// one; a client — including the matter's own client DRI — may not, and
    /// neither may a Clerk.
    #[tokio::test]
    async fn only_the_lawyer_tier_and_above_designates_a_client_dri() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-client-side").await;
        let lawyer = person(&surreal, "lawyer", Role::Lawyer).await;
        let clerk = person(&surreal, "clerk", Role::Clerk).await;
        let libra = person(&surreal, "libra", Role::Client).await;

        let designated = add(
            &surreal,
            matter,
            libra,
            DriRequest::Designate(DriSide::Client),
            DriActor::Person(lawyer),
        )
        .await
        .unwrap();
        assert!(designated.is_client_dri);

        // The seated client DRI cannot name a second one, and neither can a Clerk.
        for actor in [libra, clerk] {
            let refused = add(
                &surreal,
                matter,
                person(&surreal, &format!("guest-{actor}"), Role::Client).await,
                DriRequest::Designate(DriSide::Client),
                DriActor::Person(actor),
            )
            .await;
            assert!(
                matches!(
                    refused,
                    Err(AddParticipantError::Dri(DriError::NotPermitted))
                ),
                "that tier must not designate a client DRI: {refused:?}"
            );
        }
        assert_eq!(holders(&surreal, matter, DriSide::Client).await, [libra]);
    }

    /// An actor with no `persons` row decides nothing, so it is refused as
    /// unknown rather than falling through to the self-governing check.
    #[tokio::test]
    async fn an_unknown_actor_is_refused() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-ghost").await;
        let lawyer = person(&surreal, "lawyer", Role::Lawyer).await;

        let refused = add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::Person(Uuid::now_v7()),
        )
        .await;

        assert!(matches!(
            refused,
            Err(AddParticipantError::Dri(DriError::ActorUnknown))
        ));
    }

    /// Every designation and every removal lands on the append-only trail,
    /// naming who moved it and over whom.
    #[tokio::test]
    async fn every_dri_change_is_audited() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-audit").await;
        let first = person(&surreal, "first", Role::Lawyer).await;
        let second = person(&surreal, "second", Role::Lawyer).await;
        add(
            &surreal,
            matter,
            first,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();
        let second_row = add(
            &surreal,
            matter,
            second,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::Person(first),
        )
        .await
        .unwrap();
        remove_participant(&surreal, matter, second_row.id, DriActor::Person(first))
            .await
            .unwrap();

        assert_eq!(
            dri_trail(&surreal, matter).await,
            vec![
                ("lawyer_dri_designated".to_string(), first.to_string()),
                ("lawyer_dri_designated".to_string(), second.to_string()),
                ("lawyer_dri_removed".to_string(), second.to_string()),
            ]
        );
        let entries = crate::relationship_logs::all(&surreal).await.unwrap();
        let designating_actor = entries
            .iter()
            .find(|log| log.detail == second.to_string() && log.action.ends_with("designated"))
            .expect("the peer designation is on the trail");
        assert_eq!(designating_actor.actor_person_id, Some(first));
    }

    /// Re-affirming someone already accountable changes nothing — no second
    /// marker, and no audit entry for a change that did not happen.
    #[tokio::test]
    async fn re_affirming_an_existing_dri_records_nothing() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-reaffirm").await;
        let lawyer = person(&surreal, "same", Role::Lawyer).await;
        add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();
        let row = projects::participation_for_person(&surreal, lawyer, matter)
            .await
            .unwrap()
            .expect("the designation wrote a row");
        let before = dri_trail(&surreal, matter).await;

        let saved = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: row.id,
                person_id: lawyer,
                dri: DriRequest::Designate(DriSide::Lawyer),
                actor: DriActor::System,
            },
        )
        .await
        .unwrap();

        assert!(saved.is_lawyer_dri);
        assert_eq!(dri_trail(&surreal, matter).await, before);
    }

    /// The lawyer DRI is the accountable lawyer, so the tier has to be able to
    /// carry it: never a client, and never a Clerk, who is a supervised
    /// non-lawyer. The client marker is the mirror of that rule.
    #[tokio::test]
    async fn a_tier_that_cannot_carry_the_marker_is_refused() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-tier").await;
        for (tag, tier, side) in [
            ("client-as-lawyer", Role::Client, DriSide::Lawyer),
            ("clerk-as-lawyer", Role::Clerk, DriSide::Lawyer),
            ("lawyer-as-client", Role::Lawyer, DriSide::Client),
            ("clerk-as-client", Role::Clerk, DriSide::Client),
        ] {
            let who = person(&surreal, tag, tier).await;
            let refused = add(
                &surreal,
                matter,
                who,
                DriRequest::Designate(side),
                DriActor::System,
            )
            .await;
            assert!(
                matches!(
                    refused,
                    Err(AddParticipantError::Dri(DriError::TierMismatch))
                ),
                "{tag} must not carry that marker"
            );
        }
    }

    /// Owner and Admin are lawyer-tier, so either may be designated the
    /// matter's accountable lawyer DRI — the same marker an ordinary Lawyer
    /// carries.
    #[tokio::test]
    async fn owner_and_admin_may_be_designated_lawyer_dri() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-admin-tier").await;
        for (tag, tier) in [("owner", Role::Owner), ("admin", Role::Admin)] {
            let who = person(&surreal, tag, tier).await;
            let designated = add(
                &surreal,
                matter,
                who,
                DriRequest::Designate(DriSide::Lawyer),
                DriActor::System,
            )
            .await
            .unwrap();
            assert!(designated.is_lawyer_dri, "{tag} must carry the lawyer DRI");
        }
    }

    /// A lawyer already on a matter whose lawyer set is empty may name the first
    /// DRI — including themselves. Owner and Admin still bootstrap from outside
    /// the ledger; a lawyer who is not on the matter still cannot.
    #[tokio::test]
    async fn a_firm_participant_names_the_first_lawyer_dri_when_the_set_is_empty() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-empty-bootstrap").await;
        let lawyer = person(&surreal, "on-the-matter", Role::Lawyer).await;
        let row = add(
            &surreal,
            matter,
            lawyer,
            DriRequest::Unchanged,
            DriActor::System,
        )
        .await
        .unwrap();
        assert!(!row.is_lawyer_dri);
        assert!(holders(&surreal, matter, DriSide::Lawyer).await.is_empty());

        let designated = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: row.id,
                person_id: lawyer,
                dri: DriRequest::Designate(DriSide::Lawyer),
                actor: DriActor::Person(lawyer),
            },
        )
        .await
        .unwrap();

        assert!(designated.is_lawyer_dri);
        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, [lawyer]);
    }

    /// An empty set does not let a lawyer who is not on the matter reach in.
    #[tokio::test]
    async fn a_lawyer_off_the_matter_cannot_name_the_first_lawyer_dri() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-empty-outsider").await;
        let candidate = person(&surreal, "candidate", Role::Lawyer).await;
        let outsider = person(&surreal, "outsider", Role::Lawyer).await;
        let refused = add(
            &surreal,
            matter,
            candidate,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::Person(outsider),
        )
        .await;
        assert!(matches!(
            refused,
            Err(AddParticipantError::Dri(DriError::NotPermitted))
        ));
        assert!(holders(&surreal, matter, DriSide::Lawyer).await.is_empty());
    }

    /// Owner and Admin bypass the lawyer side's self-governing check — the rule
    /// that only a current lawyer DRI may name another. Neither holds a marker
    /// on this matter at all, yet each may still designate one, which is also
    /// what bootstraps a matter whose lawyer set is empty.
    #[tokio::test]
    async fn owner_and_admin_designate_a_lawyer_dri_without_holding_the_marker() {
        let surreal = crate::test_support::mem_surreal().await;
        for (tag, tier) in [("owner", Role::Owner), ("admin", Role::Admin)] {
            let matter = project(&surreal, &format!("dri-bootstrap-{tag}")).await;
            let actor = person(&surreal, &format!("{tag}-actor"), tier).await;
            let candidate = person(&surreal, &format!("{tag}-candidate"), Role::Lawyer).await;
            let designated = add(
                &surreal,
                matter,
                candidate,
                DriRequest::Designate(DriSide::Lawyer),
                DriActor::Person(actor),
            )
            .await
            .unwrap();
            assert!(
                designated.is_lawyer_dri,
                "{tag} must be able to designate a lawyer DRI without holding one"
            );
        }
    }

    /// The lawyer set is never empty. The last accountable lawyer cannot step
    /// off and cannot be removed from the matter — the two doors defend the same
    /// invariant from opposite sides.
    #[tokio::test]
    async fn the_last_lawyer_dri_cannot_step_off_or_be_removed() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-required").await;
        let lawyer = person(&surreal, "accountable", Role::Lawyer).await;
        projects::designate_dri_in_surreal(&surreal, matter, lawyer, DriSide::Lawyer)
            .await
            .unwrap();
        let row = projects::participation_for_person(&surreal, lawyer, matter)
            .await
            .unwrap()
            .expect("the designation wrote a row");

        let cleared = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: row.id,
                person_id: lawyer,
                dri: DriRequest::Clear,
                actor: DriActor::System,
            },
        )
        .await;
        assert!(matches!(
            cleared,
            Err(UpdateParticipantError::Dri(DriError::LawyerDriRequired))
        ));

        let removed = remove_participant(&surreal, matter, row.id, DriActor::System).await;
        assert!(matches!(removed, Err(RemoveParticipantError::DriLockout)));
        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, [lawyer]);
    }

    /// With a second lawyer accountable, the first may step off — the rule is
    /// about the set, not about any one person's row.
    #[tokio::test]
    async fn a_lawyer_dri_may_step_off_while_another_remains() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-step-off").await;
        let leaving = person(&surreal, "leaving", Role::Lawyer).await;
        let staying = person(&surreal, "staying", Role::Lawyer).await;
        let leaving_row = add(
            &surreal,
            matter,
            leaving,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();
        add(
            &surreal,
            matter,
            staying,
            DriRequest::Designate(DriSide::Lawyer),
            DriActor::System,
        )
        .await
        .unwrap();

        remove_participant(&surreal, matter, leaving_row.id, DriActor::Person(leaving))
            .await
            .unwrap();

        assert_eq!(holders(&surreal, matter, DriSide::Lawyer).await, [staying]);
    }

    /// The client marker is not load-bearing the way the lawyer one is, so it can
    /// be handed back — the matter simply has no client DRI again.
    #[tokio::test]
    async fn the_client_marker_can_be_cleared() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "dri-client-clear").await;
        let libra = person(&surreal, "libra", Role::Client).await;
        projects::designate_dri_in_surreal(&surreal, matter, libra, DriSide::Client)
            .await
            .unwrap();
        let row = projects::participation_for_person(&surreal, libra, matter)
            .await
            .unwrap()
            .expect("the designation wrote a row");

        let saved = update_participant(
            &surreal,
            &UpdateParticipantCommand {
                project_id: matter,
                role_id: row.id,
                person_id: libra,
                dri: DriRequest::Clear,
                actor: DriActor::System,
            },
        )
        .await
        .unwrap();

        assert!(!saved.is_client_dri);
        assert!(holders(&surreal, matter, DriSide::Client).await.is_empty());
    }

    #[tokio::test]
    async fn an_unknown_person_is_rejected_before_anything_is_written() {
        let surreal = crate::test_support::mem_surreal().await;
        let matter = project(&surreal, "derive-missing").await;
        let refused = add(
            &surreal,
            matter,
            Uuid::now_v7(),
            DriRequest::Unchanged,
            DriActor::System,
        )
        .await;
        assert!(matches!(refused, Err(AddParticipantError::PersonNotFound)));
        assert!(projects::participations_for_project(&surreal, matter)
            .await
            .unwrap()
            .is_empty());
    }
}
