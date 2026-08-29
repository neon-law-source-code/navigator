//! Row-level visibility helpers.
//!
//! `Role decides the tier; participation decides the scope.` This
//! module is the one place where that mapping turns into SQL:
//! [`visible_projects`] returns the slice of `projects` a given
//! [`Role`] + `person_id` should see, and every project-list / detail
//! handler funnels through it.
//!
//! See [`docs/access-model.md`](../../../docs/access-model.md).

use crate::persons::{self, Person, PersonError, Role};
use crate::projects::{self, Project};
use crate::surreal::SurrealDb;
use uuid::Uuid;

/// A supervisor lookup that failed in the other engine, reported as the
/// `String` every caller of this module already handles.
fn supervisor_lookup_failed(err: &PersonError) -> String {
    format!("resolve the supervising lawyer: {err}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectLens {
    Client,
    Lawyer,
}

impl ProjectLens {
    /// Which side of a matter this tier renders from.
    ///
    /// This is a *render* decision — which fields a page emits, which
    /// `asset.visibility` a download resolves, whether a conversation message
    /// is inbound or outbound. It is never the access decision: that is
    /// [`can_see_project`], which dispatches on the same role and reads the
    /// participation ledger. A caller that reaches a render site has already
    /// passed the gate.
    ///
    /// `Clerk` maps to the client lens so an unforeseen render path fails
    /// closed; the Clerk gate denies the matter surface outright.
    #[must_use]
    pub fn for_role(role: Role) -> Self {
        match role {
            Role::Owner | Role::Admin | Role::Lawyer => Self::Lawyer,
            Role::Clerk | Role::Client => Self::Client,
        }
    }
}

/// Who the caller is *to this matter* — the five distinct renderings the one
/// matter page serves.
///
/// Deliberately separate from [`ProjectLens`], which stays two-valued and keeps
/// answering the only question asset visibility has ever asked: firm side or
/// client side. Folding the two together would make every `asset.visibility`
/// match arm grow from two cases to five, and would mean a variant added for a
/// UI reason silently became a confidentiality decision.
///
/// The order below is the authority order on a matter, not the tier order:
/// a Clerk outranks nobody, and a DRI is the accountable one on their side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatterViewer {
    /// A client-side participant.
    Client,
    /// The client-side participant carrying the accountability marker. Sees the
    /// client page plus the controls only they may fire, such as plan approval.
    ClientDri,
    /// A supervised non-lawyer. Sees the matter's name, status, and supervising
    /// lawyer — never documents, legal work, or any write.
    Clerk,
    /// A firm-side participant.
    Lawyer,
    /// The firm-side participant carrying the accountability marker. Sees the
    /// workbench plus the matter-level accountability actions.
    LawyerDri,
}

impl MatterViewer {
    /// Which side of the matter this viewer is on, for asset visibility.
    #[must_use]
    pub fn lens(self) -> ProjectLens {
        match self {
            Self::Lawyer | Self::LawyerDri => ProjectLens::Lawyer,
            // A Clerk never reaches an asset at all, but if a render path ever
            // asks, the answer must be the narrower one.
            Self::Client | Self::ClientDri | Self::Clerk => ProjectLens::Client,
        }
    }

    /// `true` for the accountable participant on either side.
    #[must_use]
    pub fn is_dri(self) -> bool {
        matches!(self, Self::ClientDri | Self::LawyerDri)
    }

    /// `true` for the firm side of the matter.
    #[must_use]
    pub fn is_firm_side(self) -> bool {
        matches!(self, Self::Lawyer | Self::LawyerDri)
    }
}

/// Resolve who this caller is to this matter, or `None` if they may not see it
/// at all.
///
/// This is the matter surface's single entry point: the gate and the render
/// variant are the same question asked once, so a page cannot render a variant
/// its caller was never admitted to. [`can_see_project`] is this function
/// reduced to a boolean.
///
/// Every tier is scoped by the participation ledger, Owner and Admin included
/// (ENG-81) — there is no privileged short-circuit here.
pub async fn matter_viewer(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
    project_id: Uuid,
) -> Result<Option<MatterViewer>, String> {
    let Some(pid) = person_id else {
        return Ok(None);
    };

    // A Clerk is not a narrower Lawyer: the matter must also name at least one
    // currently licensed lawyer among its lawyer DRIs, a different query.
    if role == Role::Clerk {
        return Ok(
            can_see_project_as_clerk(surreal, person_id, role, project_id)
                .await?
                .then_some(MatterViewer::Clerk),
        );
    }

    let Some(row) = projects::participation_for_person(surreal, pid, project_id)
        .await
        .map_err(|error| project_lookup_failed(&error))?
    else {
        return Ok(None);
    };

    if is_client_participation(&row) {
        // A client-side row is the client lens whatever the tier: a lawyer who
        // is also a client on their own matter reads it as a client.
        return Ok(Some(if row.is_client_dri {
            MatterViewer::ClientDri
        } else {
            MatterViewer::Client
        }));
    }

    // Firm-side rows only reach the workbench from a firm tier. A `client`-tier
    // person holding one is a data error, not a promotion.
    if !role.is_lawyer_tier() {
        return Ok(None);
    }
    Ok(Some(if row.is_lawyer_dri {
        MatterViewer::LawyerDri
    } else {
        MatterViewer::Lawyer
    }))
}

/// All projects this person is allowed to see.
///
/// - [`Role::Owner`] / [`Role::Admin`] / [`Role::Lawyer`] → only projects with a
///   firm-side `person_project_roles` row for `person_id`. There is no
///   privileged bypass on the matter surface: a lawyer, an admin, and an owner
///   who have not been put on a matter all do not see it.
/// - [`Role::Client`] → only projects with a client-side row. A firm-side
///   assignment does not put the matter in a client's list.
/// - [`Role::Clerk`] → the supervised set only: a firm-side row *and* a matter
///   one of whose flagged lawyer DRIs currently holds the lawyer tier. Narrower
///   than the Lawyer predicate, so a Clerk is never folded into the lawyer lens.
///
/// `person_id` is `Option` because some test paths build sessions
/// without a linked persons row. When it's `None` every tier sees nothing —
/// fail-closed, with no privileged exception.
pub async fn visible_projects(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Vec<Project>, String> {
    match role {
        Role::Client => visible_projects_as_client(surreal, person_id).await,
        // A Clerk now reaches the one matter list like everyone else; the
        // dedicated `/clerk` surface is retired. What they see through it is
        // still the supervised set, which is a different query.
        Role::Clerk => visible_projects_as_clerk(surreal, person_id, role).await,
        Role::Owner | Role::Admin | Role::Lawyer => {
            visible_projects_as_lawyer(surreal, person_id, role).await
        }
    }
}

/// All projects visible through the client portal lens.
///
/// This is intentionally narrower than "has any participation row":
/// firm-side participation such as `attorney` or `paralegal` belongs to
/// `/lawyer`, not the client lens.
pub async fn visible_projects_as_client(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
) -> Result<Vec<Project>, String> {
    let Some(pid) = person_id else {
        return Ok(Vec::new());
    };
    visible_projects_matching(surreal, pid, is_client_participation).await
}

/// All projects visible through the lawyer workbench lens.
///
/// Every firm tier — Owner and Admin included — sees only matters where they
/// hold a firm-side participation row. There is no silent project-scoping
/// bypass on the matter surface; privileged reach is a place you navigate to.
/// The accountable lawyer always has a row, because designating a lawyer DRI
/// flags their membership row.
pub async fn visible_projects_as_lawyer(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Vec<Project>, String> {
    if !role.is_lawyer_tier() {
        return Ok(Vec::new());
    }
    let Some(pid) = person_id else {
        return Ok(Vec::new());
    };
    visible_projects_matching(surreal, pid, is_firm_participation).await
}

/// All projects visible through the supervised Clerk lens.
///
/// Clerk is a non-lawyer role, so its route does not inherit either the
/// client or Lawyer predicates. A project appears only when the Clerk
/// has a firm-side participation row *and* the matter's flagged lawyer-DRI
/// row belongs to a currently licensed lawyer (`lawyer` or `admin`).
///
/// The `/clerk` handlers render a deliberately limited, read-only view.
/// Upload, document-content, drafting, review, approval, Git, MCP, and
/// conversation capabilities each require their own route-level grant.
pub async fn visible_projects_as_clerk(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Vec<Project>, String> {
    if role != Role::Clerk {
        return Ok(Vec::new());
    }
    let Some(pid) = person_id else {
        return Ok(Vec::new());
    };

    Ok(supervised_projects(surreal, pid)
        .await?
        .into_iter()
        .map(|(project, _supervisor)| project)
        .collect())
}

/// The supervised matters a Clerk may see, each paired with the licensed
/// lawyer accountable for it.
///
/// The licensure check and the supervisor lookup are the same query now that
/// the lawyer DRI is a flagged membership row: a matter qualifies only when one
/// of its flagged rows belongs to a currently licensed `owner`, `admin`, or
/// `lawyer` person. Returning the supervisor alongside the project is what lets
/// `webapp::clerk` render "Supervising lawyer" without a second batch load — and
/// makes it impossible to show a Clerk a matter whose supervisor could not be
/// resolved.
///
/// A matter's lawyer DRIs are a set, and supervision reads it as **any**: one
/// licensed lawyer accountable for the matter is what the Clerk needs, so a
/// second DRI who has since lost the tier does not withdraw the matter. The
/// named supervisor is the alphabetically first qualifying one, which keeps the
/// column stable across reads rather than following row order.
pub async fn supervised_projects(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<(Project, Person)>, String> {
    let projects = visible_projects_matching(surreal, person_id, is_firm_participation).await?;
    let mut dri_ids_by_project: std::collections::HashMap<Uuid, Vec<Uuid>> =
        std::collections::HashMap::new();
    for project in &projects {
        let ids: Vec<Uuid> = projects::participations_for_project(surreal, project.id)
            .await
            .map_err(|error| project_lookup_failed(&error))?
            .into_iter()
            .filter(|row| row.is_lawyer_dri)
            .map(|row| row.person_id)
            .collect();
        if !ids.is_empty() {
            dri_ids_by_project.insert(project.id, ids);
        }
    }
    // A Clerk may never be supervised by a non-lawyer, so the DRI has to be a
    // currently licensed `owner`, `admin`, or `lawyer` person. The tier is
    // filtered in Rust because the rows come from the other engine: the DRI
    // designation is a `person_project_role` row and the `role` it
    // has to satisfy is a SurrealDB `person` field.
    let dri_ids: Vec<Uuid> = dri_ids_by_project.values().flatten().copied().collect();
    let licensed: std::collections::HashMap<Uuid, Person> = persons::find_by_ids(surreal, &dri_ids)
        .await
        .map_err(|err| supervisor_lookup_failed(&err))?
        .into_iter()
        .filter(|supervisor| supervisor.role.is_lawyer_tier())
        .map(|supervisor| (supervisor.id, supervisor))
        .collect();
    Ok(projects
        .into_iter()
        .filter_map(|project| {
            let supervisor = dri_ids_by_project
                .get(&project.id)?
                .iter()
                .filter_map(|id| licensed.get(id))
                .min_by(|a, b| a.name.cmp(&b.name))?
                .clone();
            Some((project, supervisor))
        })
        .collect())
}

/// `true` iff the caller may see the given project. Single-row
/// counterpart to [`visible_projects`] — same semantics, one
/// `SELECT 1` instead of loading every membership row.
///
/// Project-detail handlers call this *before* fetching the project
/// itself so an unauthorised caller never even pulls the row into
/// the response.
/// This is **the matter surface's gate** — `/app/projects/{id}` and its
/// children — and it is the one place with no privileged short-circuit: a
/// participation row is required of every tier, Owner and Admin included
/// (ENG-81, decided 2026-08-05).
///
/// It deliberately does *not* delegate the firm arm to
/// [`can_see_project_as_lawyer`], which still carries the documented
/// project-scoping bypass for the surfaces this slice did not move (`/app/api/*`,
/// the notation walker, contract reviews). Those are ENG-83's to
/// collapse. Until then the two functions differ for exactly one input —
/// an Owner or Admin with no row — so reach for this one on the matter
/// surface and that one everywhere else.
pub async fn can_see_project(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
    project_id: Uuid,
) -> Result<bool, String> {
    Ok(matter_viewer(surreal, person_id, role, project_id)
        .await?
        .is_some())
}

/// `true` iff the caller may see the project through the client lens.
pub async fn can_see_project_as_client(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    project_id: Uuid,
) -> Result<bool, String> {
    let Some(pid) = person_id else {
        return Ok(false);
    };
    projects::participation_for_person(surreal, pid, project_id)
        .await
        .map_err(|error| project_lookup_failed(&error))
        .map(|row| row.is_some_and(|row| is_client_participation(&row)))
}

/// `true` iff the caller may see the project through the firm lens, with the
/// Owner/Admin project-scoping bypass still applied.
///
/// This is the gate for the firm surfaces ENG-81 has **not** collapsed yet —
/// `/app/api/*`, the notation walker, and contract reviews. It keeps the
/// behavior documented in `docs/access-model.md` so this slice does not quietly
/// re-authorize surfaces outside its scope. The matter surface uses
/// [`can_see_project`] instead, which has no bypass.
pub async fn can_see_project_as_lawyer(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
    project_id: Uuid,
) -> Result<bool, String> {
    projects::can_access_as_lawyer_in_surreal(surreal, person_id, role, project_id)
        .await
        .map_err(|error| project_lookup_failed(&error))
}

/// `true` iff a supervised Clerk may see the project through `/clerk`.
pub async fn can_see_project_as_clerk(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
    project_id: Uuid,
) -> Result<bool, String> {
    Ok(visible_projects_as_clerk(surreal, person_id, role)
        .await?
        .iter()
        .any(|project| project.id == project_id))
}

fn project_lookup_failed(error: &projects::ProjectStoreError) -> String {
    format!("projects cluster: {error}")
}

fn is_client_participation(row: &projects::PersonProjectRole) -> bool {
    projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str()) || row.is_client_dri
}

fn is_firm_participation(row: &projects::PersonProjectRole) -> bool {
    !projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str()) && !row.is_client_dri
}

async fn visible_projects_matching(
    surreal: &SurrealDb,
    person_id: Uuid,
    predicate: fn(&projects::PersonProjectRole) -> bool,
) -> Result<Vec<Project>, String> {
    let rows = projects::participations_for_person(surreal, person_id)
        .await
        .map_err(|error| project_lookup_failed(&error))?;
    let mut results = Vec::new();
    for row in rows.into_iter().filter(predicate) {
        if let Some(project) = projects::find_by_id(surreal, row.project_id)
            .await
            .map_err(|error| project_lookup_failed(&error))?
        {
            results.push(project);
        }
    }
    results.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::{
        can_see_project, can_see_project_as_clerk, can_see_project_as_client,
        can_see_project_as_lawyer, visible_projects, visible_projects_as_clerk,
        visible_projects_as_client, visible_projects_as_lawyer,
    };
    use crate::persons::{self, NewPerson, Role};
    use crate::surreal::SurrealDb;
    use crate::test_support::mem_surreal;
    use uuid::Uuid;

    async fn seed_project(surreal: &SurrealDb, name: &str) -> Uuid {
        let dri = crate::test_support::dri_person(surreal).await;
        persons::set_role(surreal, dri, Role::Lawyer).await.unwrap();
        let project_id = crate::projects::create(
            surreal,
            &crate::projects::NewProject {
                code: format!("{}-{}", name.replace(' ', "-"), Uuid::now_v7()),
                name: name.into(),
                status: "open".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id;
        // Designate the licensed lawyer person as the matter's lawyer DRI. This
        // is the participation-row equivalent of the retired
        // `lawyer_dri_person_id` column this helper used to set, and it is what
        // gives every seeded matter a disclosed supervising lawyer.
        crate::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            dri,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
        project_id
    }

    async fn seed_person(surreal: &SurrealDb, email: &str) -> Uuid {
        persons::create(surreal, &NewPerson::with_role(email, email, Role::Client))
            .await
            .unwrap()
            .id
    }

    async fn link(surreal: &SurrealDb, person_id: Uuid, project_id: Uuid, participation: &str) {
        crate::projects::add_participation(surreal, project_id, person_id, participation)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn client_lens_ignores_lawyer_only_participation() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let lawyer_side = seed_project(&surreal, "lawyer-side").await;
        let client_side = seed_project(&surreal, "client-side").await;
        link(&surreal, libra, lawyer_side, "paralegal").await;
        link(&surreal, libra, client_side, "client").await;

        let rows = visible_projects_as_client(&surreal, Some(libra))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "client-side");
        assert!(
            can_see_project_as_client(&surreal, Some(libra), client_side)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project_as_client(&surreal, Some(libra), lawyer_side)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn lawyer_lens_ignores_client_only_participation() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let client_side = seed_project(&surreal, "client-side").await;
        let lawyer_side = seed_project(&surreal, "lawyer-side").await;
        link(&surreal, libra, client_side, "client").await;
        link(&surreal, libra, lawyer_side, "paralegal").await;

        let rows = visible_projects_as_lawyer(&surreal, Some(libra), Role::Lawyer)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "lawyer-side");
        assert!(
            can_see_project_as_lawyer(&surreal, Some(libra), Role::Lawyer, lawyer_side)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project_as_lawyer(&surreal, Some(libra), Role::Lawyer, client_side)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn external_lawyer_is_scoped_by_participation_without_admin_bypass() {
        let surreal = mem_surreal().await;
        let lawyer = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::with_role(
                "Outside Legal Aid Counsel",
                "counsel@legalaid.example",
                Role::Lawyer,
            ),
        )
        .await
        .unwrap();
        let assigned = seed_project(&surreal, "assigned legal-aid matter").await;
        let unassigned = seed_project(&surreal, "unassigned matter").await;
        link(&surreal, lawyer.id, assigned, "legal_aid_provider").await;

        let visible = visible_projects_as_lawyer(&surreal, Some(lawyer.id), lawyer.role)
            .await
            .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, assigned);
        assert!(
            can_see_project_as_lawyer(&surreal, Some(lawyer.id), lawyer.role, assigned)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project_as_lawyer(&surreal, Some(lawyer.id), lawyer.role, unassigned)
                .await
                .unwrap(),
            "Lawyer is not an admin bypass"
        );
        assert!(lawyer.role.is_lawyer_tier());
        assert!(!lawyer.role.is_admin_tier());
    }

    #[tokio::test]
    async fn lawyer_lens_requires_lawyer_tier_even_with_lawyer_participation() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let lawyer_side = seed_project(&surreal, "lawyer-side").await;
        link(&surreal, libra, lawyer_side, "paralegal").await;

        let rows = visible_projects_as_lawyer(&surreal, Some(libra), Role::Client)
            .await
            .unwrap();
        assert!(rows.is_empty());
        assert!(
            !can_see_project_as_lawyer(&surreal, Some(libra), Role::Client, lawyer_side)
                .await
                .unwrap()
        );
    }

    /// Designating a lawyer DRI is now the same act as putting them on the
    /// matter, so the old "named but invisible" state is unrepresentable.
    ///
    /// The designee is seeded `lawyer` because the row's participation is
    /// derived from their tier: a matter's lawyer DRI is its disclosed lawyer,
    /// and a client-tier person designated here would take a client-side row
    /// and correctly fail to reach the firm lens.
    #[tokio::test]
    async fn designating_a_lawyer_dri_makes_them_a_matter_person() {
        let surreal = mem_surreal().await;
        let lawyer = persons::create(
            &surreal,
            &NewPerson::with_role("lawyer@example.com", "lawyer@example.com", Role::Lawyer),
        )
        .await
        .unwrap()
        .id;
        let project_id = seed_project(&surreal, "dri-only").await;

        assert!(
            !can_see_project_as_lawyer(&surreal, Some(lawyer), Role::Lawyer, project_id)
                .await
                .unwrap(),
            "a lawyer with no membership row cannot reach the matter"
        );

        crate::projects::designate_dri_in_surreal(
            &surreal,
            project_id,
            lawyer,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();

        assert!(
            can_see_project_as_lawyer(&surreal, Some(lawyer), Role::Lawyer, project_id)
                .await
                .unwrap(),
            "the designation wrote the membership row that grants access"
        );
        assert!(
            crate::projects::participations_for_project(&surreal, project_id)
                .await
                .unwrap()
                .iter()
                .any(|row| row.person_id == lawyer && row.is_lawyer_dri)
        );
    }

    /// The hole #629 correctly identified: an adverse party is on the matter
    /// but must never reach it through the firm lens.
    #[tokio::test]
    async fn a_counterparty_row_does_not_grant_lawyer_lens_visibility() {
        let surreal = mem_surreal().await;
        let adverse = seed_person(&surreal, "adverse@example.com").await;
        let project_id = seed_project(&surreal, "adverse-matter").await;
        link(&surreal, adverse, project_id, "counterparty").await;

        assert!(
            visible_projects_as_lawyer(&surreal, Some(adverse), Role::Lawyer)
                .await
                .unwrap()
                .is_empty(),
            "a counterparty is client-side, never firm-side"
        );
        assert!(
            !can_see_project_as_lawyer(&surreal, Some(adverse), Role::Lawyer, project_id)
                .await
                .unwrap()
        );
    }

    /// The firm lens is the complement of the client-side set, not an
    /// allowlist — so participation kinds the firm has not coined yet still
    /// reach `/lawyer`. Closing this vocabulary would silently drop each new
    /// kind out of the firm lens the day it was coined.
    #[tokio::test]
    async fn firm_side_visibility_survives_an_unforeseen_participation_kind() {
        let surreal = mem_surreal().await;
        let helper = seed_person(&surreal, "helper@example.com").await;
        let project_id = seed_project(&surreal, "open-vocabulary").await;
        link(&surreal, helper, project_id, "guardian_ad_litem").await;

        assert!(
            can_see_project_as_lawyer(&surreal, Some(helper), Role::Lawyer, project_id)
                .await
                .unwrap(),
            "an unlisted firm-side kind still reaches the firm lens"
        );
    }

    #[tokio::test]
    async fn clerk_lens_requires_firm_participation_and_a_licensed_lawyer_dri() {
        let surreal = mem_surreal().await;
        let clerk = persons::create(
            &surreal,
            &NewPerson::with_role("Clerk", "clerk@neonlaw.com", Role::Clerk),
        )
        .await
        .unwrap()
        .id;
        let supervised = seed_project(&surreal, "supervised").await;
        let client_only = seed_project(&surreal, "client-only").await;
        let unassigned = seed_project(&surreal, "unassigned").await;
        link(&surreal, clerk, supervised, "clerk").await;
        link(&surreal, clerk, client_only, "client").await;

        let rows = visible_projects_as_clerk(&surreal, Some(clerk), Role::Clerk)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, supervised);
        assert!(
            can_see_project_as_clerk(&surreal, Some(clerk), Role::Clerk, supervised)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project_as_clerk(&surreal, Some(clerk), Role::Clerk, client_only)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project_as_clerk(&surreal, Some(clerk), Role::Clerk, unassigned)
                .await
                .unwrap(),
            "a Clerk may not see a supervised matter without participation"
        );

        // Supervision reads the lawyer set as *any*: adding a non-lawyer DRI
        // beside the licensed one does not withdraw the matter, because a
        // licensed lawyer is still accountable for it.
        let unlicensed_dri = seed_person(&surreal, "unlicensed@example.com").await;
        crate::projects::designate_dri_in_surreal(
            &surreal,
            supervised,
            unlicensed_dri,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
        assert_eq!(
            visible_projects_as_clerk(&surreal, Some(clerk), Role::Clerk)
                .await
                .unwrap()
                .len(),
            1,
            "one licensed lawyer DRI is enough to supervise"
        );

        // Take the licensed one away and nothing accountable is licensed, so
        // the matter leaves the Clerk's list.
        let licensed = crate::test_support::dri_person(&surreal).await;
        let licensed_row =
            crate::projects::participation_for_person(&surreal, licensed, supervised)
                .await
                .unwrap()
                .expect("the fixture designated a licensed lawyer DRI");
        crate::projects::clear_dri_in_surreal(
            &surreal,
            licensed_row.id,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();

        assert!(
            visible_projects_as_clerk(&surreal, Some(clerk), Role::Clerk)
                .await
                .unwrap()
                .is_empty(),
            "a Clerk may not use a non-lawyer as the lawyer_dri"
        );
    }

    /// The client DRI reaches their own matter through the flagged membership
    /// row — there is no longer a column-level escape hatch beside the ledger.
    #[tokio::test]
    async fn the_client_dri_reaches_their_matter_through_the_flagged_row() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal, "client-dri").await;
        crate::projects::designate_dri_in_surreal(
            &surreal,
            project_id,
            libra,
            crate::projects::DriSide::Client,
        )
        .await
        .unwrap();

        let rows = visible_projects_as_client(&surreal, Some(libra))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "client-dri");
        assert!(can_see_project_as_client(&surreal, Some(libra), project_id)
            .await
            .unwrap());
        assert!(
            crate::projects::participations_for_project(&surreal, project_id)
                .await
                .unwrap()
                .iter()
                .any(|row| row.person_id == libra && row.is_client_dri)
        );
    }

    /// Designation is additive: a second client DRI joins the first rather than
    /// taking the marker from them. Both reach the matter through their own
    /// flagged row, which is what makes two accountable contacts a real state
    /// rather than a race between two writes.
    #[tokio::test]
    async fn designating_a_second_dri_adds_a_flagged_row() {
        let surreal = mem_surreal().await;
        let first = seed_person(&surreal, "first-client@example.com").await;
        let second = seed_person(&surreal, "second-client@example.com").await;
        let project_id = seed_project(&surreal, "reassigned").await;

        crate::projects::designate_dri_in_surreal(
            &surreal,
            project_id,
            first,
            crate::projects::DriSide::Client,
        )
        .await
        .unwrap();
        crate::projects::designate_dri_in_surreal(
            &surreal,
            project_id,
            second,
            crate::projects::DriSide::Client,
        )
        .await
        .unwrap();

        let participations = crate::projects::participations_for_project(&surreal, project_id)
            .await
            .unwrap();
        let flagged: Vec<_> = participations
            .iter()
            .filter(|row| row.is_client_dri)
            .collect();
        assert_eq!(flagged.len(), 2, "both client DRIs carry the marker");
        let mut flagged_ids: Vec<_> = flagged.iter().map(|row| row.person_id).collect();
        flagged_ids.sort();
        let mut expected = vec![first, second];
        expected.sort();
        assert_eq!(flagged_ids, expected);

        // And each of them reaches the matter through the client lens on their
        // own row, rather than one of them holding it on the other's behalf.
        for who in [first, second] {
            assert!(
                visible_projects_as_client(&surreal, Some(who))
                    .await
                    .unwrap()
                    .iter()
                    .any(|p| p.id == project_id),
                "both accountable contacts reach the matter"
            );
        }
    }

    /// The matter surface has no privileged bypass. Owner and Admin are
    /// scoped by the participation ledger exactly like any other firm tier,
    /// so a matter nobody put them on is a matter they do not see. Privileged
    /// reach is a place they navigate to instead, which is what makes a lens
    /// bug distinguishable from an intended widening.
    #[tokio::test]
    async fn owner_and_admin_are_scoped_by_participation_like_every_firm_tier() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let assigned = seed_project(&surreal, "alpha").await;
        let _unassigned = seed_project(&surreal, "bravo").await;
        link(&surreal, libra, assigned, "attorney").await;

        for role in [Role::Owner, Role::Admin] {
            let rows = visible_projects(&surreal, Some(libra), role).await.unwrap();
            assert_eq!(rows.len(), 1, "{role:?} sees only their own matters");
            assert_eq!(rows[0].id, assigned);
        }
    }

    /// The derived-participation write seam (#108) records an Owner as
    /// `owner` and an Admin as `admin`. Neither is in
    /// `PARTICIPATION_CLIENT_SIDE`, so the row a privileged tier gets from the
    /// matter-people form is firm-side by construction — removing the bypass
    /// cannot strand them on the client lens.
    #[tokio::test]
    async fn a_tier_derived_participation_row_reaches_the_firm_lens() {
        let surreal = mem_surreal().await;
        let project_id = seed_project(&surreal, "derived").await;

        for role in [Role::Owner, Role::Admin, Role::Lawyer] {
            let person = persons::create(
                &surreal,
                &NewPerson::with_role(
                    "Derived Participant",
                    format!(
                        "derived-{}@neonlaw.com",
                        crate::projects::participation_for_role(role)
                    ),
                    role,
                ),
            )
            .await
            .unwrap()
            .id;
            link(
                &surreal,
                person,
                project_id,
                crate::projects::participation_for_role(role),
            )
            .await;

            assert!(
                can_see_project(&surreal, Some(person), role, project_id)
                    .await
                    .unwrap(),
                "{role:?}'s derived participation is firm-side"
            );
        }
    }

    #[tokio::test]
    async fn lawyer_sees_only_projects_they_have_a_participation_on() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let visible = seed_project(&surreal, "alpha").await;
        let _hidden = seed_project(&surreal, "bravo").await;
        link(&surreal, libra, visible, "paralegal").await;

        let rows = visible_projects(&surreal, Some(libra), Role::Lawyer)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "alpha");
    }

    /// A Clerk reaches the supervised set and nothing wider. `/clerk` is
    /// retired, so this is what keeps a non-lawyer out of the lawyer lens now
    /// that both read the same path: the Clerk predicate additionally requires
    /// the matter's flagged lawyer DRI to currently hold the lawyer tier, which
    /// `seed_project` provides and a bare `link` does not.
    #[tokio::test]
    async fn a_clerk_reaches_the_supervised_set_and_never_the_lawyer_lens() {
        let surreal = mem_surreal().await;
        let clerk = persons::create(
            &surreal,
            &NewPerson::with_role("Clerk", "clerk@neonlaw.com", Role::Clerk),
        )
        .await
        .unwrap()
        .id;
        let supervised = seed_project(&surreal, "supervised").await;
        let unassigned = seed_project(&surreal, "unassigned").await;
        link(&surreal, clerk, supervised, "clerk").await;

        let rows = visible_projects(&surreal, Some(clerk), Role::Clerk)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "only the supervised matter");
        assert_eq!(rows[0].id, supervised);
        assert!(
            can_see_project(&surreal, Some(clerk), Role::Clerk, supervised)
                .await
                .unwrap()
        );
        assert!(
            !can_see_project(&surreal, Some(clerk), Role::Clerk, unassigned)
                .await
                .unwrap(),
            "a Clerk sees no matter they were not put on"
        );

        // The lawyer lens stays closed to them whatever their participation.
        assert!(
            !can_see_project_as_lawyer(&surreal, Some(clerk), Role::Clerk, supervised)
                .await
                .unwrap(),
            "Clerk is not a narrower Lawyer"
        );
    }

    #[tokio::test]
    async fn client_sees_only_projects_they_have_a_participation_on() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let visible = seed_project(&surreal, "alpha").await;
        let _hidden = seed_project(&surreal, "bravo").await;
        link(&surreal, libra, visible, "client").await;

        let rows = visible_projects(&surreal, Some(libra), Role::Client)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "alpha");
    }

    #[tokio::test]
    async fn a_session_with_no_person_id_sees_nothing() {
        let surreal = mem_surreal().await;
        let _ = seed_project(&surreal, "alpha").await;
        for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Client] {
            let rows = visible_projects(&surreal, None, role).await.unwrap();
            assert!(rows.is_empty(), "{role:?}: missing person_id fails closed");
        }
    }

    /// The guard for the 2026-08-05 decision, and the one assertion that
    /// would otherwise pass by accident: an unassigned privileged tier is
    /// denied the matter, not handed it.
    #[tokio::test]
    async fn owner_and_admin_without_a_participation_row_are_denied_the_matter() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let p = seed_project(&surreal, "alpha").await;
        for role in [Role::Owner, Role::Admin] {
            assert!(
                !can_see_project(&surreal, Some(libra), role, p)
                    .await
                    .unwrap(),
                "{role:?} has no row on this matter"
            );
            assert!(
                !can_see_project(&surreal, None, role, p).await.unwrap(),
                "{role:?} with no person_id fails closed too"
            );
        }
    }

    #[tokio::test]
    async fn can_see_project_client_with_participation() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let p = seed_project(&surreal, "alpha").await;
        link(&surreal, libra, p, "client").await;
        assert!(can_see_project(&surreal, Some(libra), Role::Client, p)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn can_see_project_client_without_participation() {
        let surreal = mem_surreal().await;
        let libra = seed_person(&surreal, "libra@example.com").await;
        let p = seed_project(&surreal, "alpha").await;
        assert!(!can_see_project(&surreal, Some(libra), Role::Client, p)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn can_see_project_lawyer_without_person_id_fails_closed() {
        let surreal = mem_surreal().await;
        let p = seed_project(&surreal, "alpha").await;
        assert!(!can_see_project(&surreal, None, Role::Lawyer, p)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn same_named_projects_order_by_id_so_pagination_is_stable() {
        // Name is not unique, so ordering by name alone leaves same-named rows
        // in an unspecified order the dashboard slices into pages — a repeat or
        // omission across page boundaries. The id tie-breaker pins the order.
        let surreal = mem_surreal().await;
        let admin = seed_person(&surreal, "admin@neonlaw.com").await;
        let first = seed_project(&surreal, "Acme contract review").await;
        let second = seed_project(&surreal, "Acme contract review").await;
        // Every firm tier is participation-scoped now, so the ordering the
        // dashboard pages through is the ordering of the caller's own matters.
        link(&surreal, admin, first, "attorney").await;
        link(&surreal, admin, second, "attorney").await;
        let (lo, hi) = if first < second {
            (first, second)
        } else {
            (second, first)
        };

        for rows in [
            visible_projects_as_lawyer(&surreal, Some(admin), Role::Admin)
                .await
                .unwrap(),
            visible_projects(&surreal, Some(admin), Role::Admin)
                .await
                .unwrap(),
        ] {
            let ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
            let lo_at = ids.iter().position(|id| *id == lo).unwrap();
            let hi_at = ids.iter().position(|id| *id == hi).unwrap();
            assert!(lo_at < hi_at, "same-named rows must sort by ascending id");
        }
    }
}
