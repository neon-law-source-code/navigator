//! The typed Firm-capability resolver (ENG-463).
//!
//! Rego admits routes; it cannot query `person_firm_role`, and making it the
//! data-authorization engine would split the source of truth. This module is
//! the one place that decides whether an actor may exercise a given
//! Firm-scoped capability against a given target Firm — the resolver every
//! Firm-scoped read or command routes through instead of re-deriving its own
//! membership check.
//!
//! [`FirmCapability`] is deliberately narrow and closed: a new Firm-scoped
//! command adds its own variant rather than reusing an existing one or
//! falling back to a blanket "is admin" boolean. Owner is system-wide and
//! holds every capability on every Firm without a `person_firm_role` row
//! (`docs/access-model.md`'s Owner-governance carve-out); Admin, Lawyer, and
//! Clerk are constrained to the Firms they hold a membership row on, and
//! which capabilities that membership tier admits; Client holds no Firm
//! capability at all.
//!
//! Firm capability does not replace Project participation. Matter documents,
//! notations, and other legal content stay governed by
//! [`crate::access::matter_viewer`] and the Project-side command rules.

use crate::firms::{self, FirmError, FirmMembership};
use crate::persons::Role;
use crate::surreal::SurrealDb;
use uuid::Uuid;

/// One Firm-scoped capability an actor may exercise against a target Firm.
///
/// Both variants back a live command in `store` today: [`Self::ViewDirectory`]
/// gates [`crate::firms::visible_person_ids`] and
/// [`crate::projects::matter_directory_for`]; [`Self::ManageMembership`]
/// gates [`crate::firms::add_membership`] and
/// [`crate::firms::ensure_membership`]. [`Self::ManageAdminDri`] gates
/// [`crate::firms::appoint_admin_dri`] (ENG-499): it admits no membership
/// tier at all, so only Owner — who [`resolve`] allows before any membership
/// read — ever holds it. An Admin DRI administers their own Firm's settings
/// and integrations, but appointing or transferring the designation is
/// Owner's alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmCapability {
    /// Read the Firm's scoped people and matter directories.
    ViewDirectory,
    /// Add or change a person's membership at the Firm.
    ManageMembership,
    /// Appoint or transfer the Firm's Admin DRI designation.
    ManageAdminDri,
}

impl FirmCapability {
    /// Whether a `person_firm_role` row carrying this membership tier admits
    /// this capability. `ViewDirectory` is open to every firm tier; only
    /// `Admin` membership may manage who else belongs to the Firm.
    /// `ManageAdminDri` admits no membership tier — Owner is the only actor
    /// [`resolve`] ever grants it to.
    #[must_use]
    fn admits(self, membership: FirmMembership) -> bool {
        match self {
            Self::ViewDirectory => true,
            Self::ManageMembership => membership == FirmMembership::Admin,
            Self::ManageAdminDri => false,
        }
    }
}

/// The resolver's answer for one `(actor, target Firm, capability)` question.
///
/// [`Self::FirmNotFound`] is distinct from [`Self::Forbidden`] so a caller can
/// choose to render them identically at the response boundary — the
/// resolver hands back the fact, not the disclosure decision, but leaves it
/// possible to keep the two indistinguishable to the caller across a Firm
/// boundary where that matters (`docs/access-model.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmCapabilityDecision {
    Allowed,
    Forbidden,
    FirmNotFound,
}

impl FirmCapabilityDecision {
    #[must_use]
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Resolve whether `actor` may exercise `capability` against `target_firm_id`.
///
/// Side-effect-free: reads only, never writes. Owner is allowed every
/// capability on every Firm without a membership row. Client is refused
/// every capability. Admin, Lawyer, and Clerk need a `person_firm_role` row
/// on `target_firm_id` whose membership tier [`FirmCapability::admits`]s the
/// requested capability.
pub async fn resolve(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    target_firm_id: Uuid,
    capability: FirmCapability,
) -> Result<FirmCapabilityDecision, FirmError> {
    if firms::find_by_id(surreal, target_firm_id).await?.is_none() {
        return Ok(FirmCapabilityDecision::FirmNotFound);
    }
    if actor_role == Role::Owner {
        return Ok(FirmCapabilityDecision::Allowed);
    }
    if actor_role == Role::Client {
        return Ok(FirmCapabilityDecision::Forbidden);
    }
    let Some(person_id) = actor_person_id else {
        return Ok(FirmCapabilityDecision::Forbidden);
    };
    let member = firms::membership_for_person(surreal, person_id, target_firm_id).await?;
    Ok(match member {
        Some(row) if capability.admits(row.membership) => FirmCapabilityDecision::Allowed,
        _ => FirmCapabilityDecision::Forbidden,
    })
}

/// Every Firm id where `actor` holds `capability`.
///
/// The batch counterpart to [`resolve`], for a directory listing that scopes
/// itself to every Firm the caller may act on rather than asking about one.
/// Owner holds `capability` on every existing Firm; Client holds it on none.
pub async fn allowed_firm_ids(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    capability: FirmCapability,
) -> Result<Vec<Uuid>, FirmError> {
    if actor_role == Role::Owner {
        return Ok(firms::all(surreal)
            .await?
            .into_iter()
            .map(|f| f.id)
            .collect());
    }
    if actor_role == Role::Client {
        return Ok(Vec::new());
    }
    let Some(person_id) = actor_person_id else {
        return Ok(Vec::new());
    };
    Ok(firms::memberships_for_person(surreal, person_id)
        .await?
        .into_iter()
        .filter(|row| capability.admits(row.membership))
        .map(|row| row.firm_id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firms::{Firm, NewFirm, NewPersonFirmRole};
    use crate::persons::{NewPerson, Person};
    use crate::test_support::{mem_surreal, seed_entity};

    async fn practice(db: &SurrealDb, name: &str) -> Firm {
        let entity_id = seed_entity(db).await;
        let admin_dri_person_id = crate::persons::create(
            db,
            &NewPerson::with_role(
                format!("{name} DRI"),
                format!("dri-{}@example.com", Uuid::now_v7()),
                Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id;
        firms::create(
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

    async fn person(db: &SurrealDb, tag: &str, role: Role) -> Person {
        crate::persons::create(
            db,
            &NewPerson::with_role(format!("{tag} Person"), format!("{tag}@example.com"), role),
        )
        .await
        .unwrap()
    }

    async fn member(db: &SurrealDb, person_id: Uuid, firm_id: Uuid, membership: FirmMembership) {
        firms::add_membership(
            db,
            &NewPersonFirmRole {
                person_id,
                firm_id,
                membership,
                is_dri: false,
            },
        )
        .await
        .unwrap();
    }

    /// The full matrix: every role, both Firms, both capabilities. Owner is
    /// allowed everywhere with no membership row; Client is refused
    /// everywhere; Admin/Lawyer/Clerk are allowed only inside their own
    /// Firm, and only `ViewDirectory` admits a non-Admin membership.
    #[tokio::test]
    async fn two_firm_matrix_proves_membership_scoped_isolation() {
        let db = mem_surreal().await;
        let firm_a = practice(&db, "Practice A").await;
        let firm_b = practice(&db, "Practice B").await;

        let owner = person(&db, "owner", Role::Owner).await;
        let client = person(&db, "client", Role::Client).await;

        let admin_a = person(&db, "admin-a", Role::Admin).await;
        member(&db, admin_a.id, firm_a.id, FirmMembership::Admin).await;
        let lawyer_a = person(&db, "lawyer-a", Role::Lawyer).await;
        member(&db, lawyer_a.id, firm_a.id, FirmMembership::Lawyer).await;
        let clerk_a = person(&db, "clerk-a", Role::Clerk).await;
        member(&db, clerk_a.id, firm_a.id, FirmMembership::Clerk).await;

        for capability in [
            FirmCapability::ViewDirectory,
            FirmCapability::ManageMembership,
        ] {
            // Owner: allowed on both Firms with no membership row at all.
            for firm in [&firm_a, &firm_b] {
                assert_eq!(
                    resolve(&db, Role::Owner, Some(owner.id), firm.id, capability)
                        .await
                        .unwrap(),
                    FirmCapabilityDecision::Allowed,
                    "Owner holds {capability:?} on every Firm"
                );
            }

            // Client: refused on both Firms, membership row or not.
            for firm in [&firm_a, &firm_b] {
                assert_eq!(
                    resolve(&db, Role::Client, Some(client.id), firm.id, capability)
                        .await
                        .unwrap(),
                    FirmCapabilityDecision::Forbidden,
                    "Client holds no Firm capability"
                );
            }
        }

        // ViewDirectory: every firm-a membership tier reaches firm A, none
        // reach firm B.
        for (tag, actor) in [
            ("admin", &admin_a),
            ("lawyer", &lawyer_a),
            ("clerk", &clerk_a),
        ] {
            assert_eq!(
                resolve(
                    &db,
                    actor.role,
                    Some(actor.id),
                    firm_a.id,
                    FirmCapability::ViewDirectory
                )
                .await
                .unwrap(),
                FirmCapabilityDecision::Allowed,
                "{tag} A reads their own Firm's directory"
            );
            assert_eq!(
                resolve(
                    &db,
                    actor.role,
                    Some(actor.id),
                    firm_b.id,
                    FirmCapability::ViewDirectory
                )
                .await
                .unwrap(),
                FirmCapabilityDecision::Forbidden,
                "{tag} A cannot read Practice B's directory"
            );
        }

        // ManageMembership: only the Admin membership admits it, even inside
        // the actor's own Firm.
        assert_eq!(
            resolve(
                &db,
                admin_a.role,
                Some(admin_a.id),
                firm_a.id,
                FirmCapability::ManageMembership
            )
            .await
            .unwrap(),
            FirmCapabilityDecision::Allowed
        );
        for actor in [&lawyer_a, &clerk_a] {
            assert_eq!(
                resolve(
                    &db,
                    actor.role,
                    Some(actor.id),
                    firm_a.id,
                    FirmCapability::ManageMembership
                )
                .await
                .unwrap(),
                FirmCapabilityDecision::Forbidden,
                "only Admin membership manages who else joins the Firm"
            );
        }
    }

    #[tokio::test]
    async fn resolve_reports_a_missing_firm_as_not_found_before_any_membership_read() {
        let db = mem_surreal().await;
        let owner = person(&db, "owner", Role::Owner).await;
        let missing_firm = Uuid::now_v7();
        assert_eq!(
            resolve(
                &db,
                Role::Owner,
                Some(owner.id),
                missing_firm,
                FirmCapability::ViewDirectory
            )
            .await
            .unwrap(),
            FirmCapabilityDecision::FirmNotFound
        );
    }

    #[tokio::test]
    async fn resolve_fails_closed_with_no_person_id() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice").await;
        assert_eq!(
            resolve(
                &db,
                Role::Admin,
                None,
                firm.id,
                FirmCapability::ViewDirectory
            )
            .await
            .unwrap(),
            FirmCapabilityDecision::Forbidden
        );
    }

    #[tokio::test]
    async fn allowed_firm_ids_matches_resolve_for_owner_admin_and_client() {
        let db = mem_surreal().await;
        let firm_a = practice(&db, "Practice A").await;
        let firm_b = practice(&db, "Practice B").await;
        let owner = person(&db, "owner", Role::Owner).await;
        let client = person(&db, "client", Role::Client).await;
        let admin_a = person(&db, "admin-a", Role::Admin).await;
        member(&db, admin_a.id, firm_a.id, FirmMembership::Admin).await;

        let owner_ids = allowed_firm_ids(
            &db,
            Role::Owner,
            Some(owner.id),
            FirmCapability::ViewDirectory,
        )
        .await
        .unwrap();
        assert!(owner_ids.contains(&firm_a.id));
        assert!(owner_ids.contains(&firm_b.id));

        let client_ids = allowed_firm_ids(
            &db,
            Role::Client,
            Some(client.id),
            FirmCapability::ViewDirectory,
        )
        .await
        .unwrap();
        assert!(client_ids.is_empty());

        let admin_ids = allowed_firm_ids(
            &db,
            Role::Admin,
            Some(admin_a.id),
            FirmCapability::ViewDirectory,
        )
        .await
        .unwrap();
        assert_eq!(admin_ids, vec![firm_a.id]);

        let admin_manage_ids = allowed_firm_ids(
            &db,
            Role::Admin,
            Some(admin_a.id),
            FirmCapability::ManageMembership,
        )
        .await
        .unwrap();
        assert_eq!(admin_manage_ids, vec![firm_a.id]);
    }

    /// `ManageAdminDri` admits no membership tier: even the Firm's own Admin
    /// member — who does hold `ManageMembership` — is refused it. Only Owner
    /// may appoint or transfer the Admin DRI (ENG-499).
    #[tokio::test]
    async fn manage_admin_dri_admits_no_membership_tier_only_owner() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice").await;
        let owner = person(&db, "owner", Role::Owner).await;
        let admin = person(&db, "admin", Role::Admin).await;
        member(&db, admin.id, firm.id, FirmMembership::Admin).await;

        assert_eq!(
            resolve(
                &db,
                Role::Owner,
                Some(owner.id),
                firm.id,
                FirmCapability::ManageAdminDri
            )
            .await
            .unwrap(),
            FirmCapabilityDecision::Allowed
        );
        assert_eq!(
            resolve(
                &db,
                admin.role,
                Some(admin.id),
                firm.id,
                FirmCapability::ManageAdminDri
            )
            .await
            .unwrap(),
            FirmCapabilityDecision::Forbidden,
            "the Firm's own Admin member does not thereby appoint its DRI"
        );
    }

    #[tokio::test]
    async fn allowed_firm_ids_excludes_a_lawyer_from_manage_membership() {
        let db = mem_surreal().await;
        let firm = practice(&db, "Practice").await;
        let lawyer = person(&db, "lawyer", Role::Lawyer).await;
        member(&db, lawyer.id, firm.id, FirmMembership::Lawyer).await;

        let view_ids = allowed_firm_ids(
            &db,
            Role::Lawyer,
            Some(lawyer.id),
            FirmCapability::ViewDirectory,
        )
        .await
        .unwrap();
        assert_eq!(view_ids, vec![firm.id]);

        let manage_ids = allowed_firm_ids(
            &db,
            Role::Lawyer,
            Some(lawyer.id),
            FirmCapability::ManageMembership,
        )
        .await
        .unwrap();
        assert!(manage_ids.is_empty());
    }
}
