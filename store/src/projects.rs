//! Project (matter) lifecycle helpers.
//!
//! A matter's `status` walks `open` → `closed` → `archived`
//! (`Project::status`). Opening is done at retainer
//! intake; this module owns the *close* — flipping a matter to `closed`
//! when the firm signs its closing letter. Archival (the Drive cold
//! store) is a separate downstream step and is left untouched here.

use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::Role;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// Maximum length of a matter code, chosen to stay comfortably within
/// common filesystem and URL segment limits once `.git` is appended.
///
/// The same cap a Project **application** name carries, because they are the
/// same shape — see [`is_valid_code`].
pub const PROJECT_CODE_MAX_LEN: usize = cloud::workspace::SLUG_MAX_LEN;
const SLACK_CHANNEL_ID_INVALID: &str = "Slack channel id is invalid.";

/// A Project read from the SurrealDB projects cluster.
///
/// Every reference is a native link now: `entity_id` became a
/// `record<entity>` when the entities cluster ported (ENG-120), which was
/// the last cross-engine id this cluster carried.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Project {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub status: String,
    /// Which house brand's storefront this matter was opened through — a
    /// closed key from `views::brand::BrandKey` (`"neon"`,
    /// `"delete-your-data"`), written by the server from the resolved
    /// request host and never accepted from a client-submitted form. `store`
    /// does not depend on `views`, so this is a plain validated `String`
    /// rather than the enum itself; the SurrealDB schema's `ASSERT` is the
    /// single source of truth for which values are valid.
    pub brand: String,
    pub entity_id: Uuid,
    /// Owning practice. `None` until the row is pointed at a
    /// [`crate::firms::Firm`].
    pub firm_id: Option<Uuid>,
    pub description: Option<String>,
    pub drive_folder_id: Option<String>,
    /// The full URL of the one source repository holding this Project's
    /// notation templates and its client portal.
    ///
    /// A whole URL rather than a name composed onto one deployment-wide forge
    /// host: a Project's source may live on any forge, in any organization, so
    /// GitHub, GitLab, and a self-hosted remote are all just values here.
    /// `None` means no repository is recorded, and nothing derives one from
    /// [`Self::code`] — a guessed URL would point at a namespace the Firm may
    /// not control.
    pub repository_url: Option<String>,
    pub git_initialized_at: Option<String>,
    pub forge_provisioned_at: Option<String>,
    pub closed_at: Option<String>,
    /// The lawyer-only Slack channel for this matter, shown as a button on the
    /// lawyer workbench. Distinct from [`Self::external_slack_channel_url`]
    /// because a channel shared with the client carries different posting
    /// norms — conflating the two would risk lawyer-only chatter landing in a
    /// client-visible channel by mistake.
    pub internal_slack_channel_url: Option<String>,
    /// The Slack channel shared with the client, if this matter has one.
    /// Optional: most matters never get a client-facing channel.
    pub external_slack_channel_url: Option<String>,
    /// The Slack Web API channel ID for the private firm-side channel. This is
    /// distinct from the optional URL: the bot posts by ID, while a URL is a
    /// human-facing resource link that may not exist for every deployment.
    pub internal_slack_channel_id: Option<String>,
    /// The firm-only Notion page for this matter — the internal write-up,
    /// research, and working notes. Paired with
    /// [`Self::shared_notion_page_url`] for the same reason the two Slack
    /// columns are separate: one page is firm-only work product and the other
    /// is client-visible, and a single column would make a mistaken paste the
    /// difference between them.
    ///
    /// Navigator stores the address only. Who may open the page is governed in
    /// Notion's own sharing, which Navigator neither reads nor enforces — so a
    /// firm-only page must be shared to the firm's Notion group, not left on
    /// its workspace default.
    pub private_notion_page_url: Option<String>,
    /// The Notion page shared with the client, if this matter has one. Optional
    /// in exactly the way [`Self::external_slack_channel_url`] is.
    pub shared_notion_page_url: Option<String>,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(surrealdb::types::SurrealValue)]
struct ProjectRow {
    id: surrealdb::types::RecordId,
    code: String,
    name: String,
    status: String,
    /// `Option` even though the schema types it `string DEFAULT 'neon'`, and
    /// the public [`Project`] carries a plain `String`. `DEFAULT` is a
    /// write-time default: it does not reach rows written before the field
    /// was defined, which hold no value at all. [`ProjectRow::into_project`]
    /// collapses the absent case to `"neon"`, what the default would have
    /// written.
    brand: Option<String>,
    entity_id: surrealdb::types::RecordId,
    firm_id: Option<surrealdb::types::RecordId>,
    description: Option<String>,
    drive_folder_id: Option<String>,
    repository_url: Option<String>,
    git_initialized_at: Option<String>,
    forge_provisioned_at: Option<String>,
    closed_at: Option<String>,
    internal_slack_channel_url: Option<String>,
    external_slack_channel_url: Option<String>,
    internal_slack_channel_id: Option<String>,
    private_notion_page_url: Option<String>,
    shared_notion_page_url: Option<String>,
    inserted_at: String,
    updated_at: String,
}

impl ProjectRow {
    fn into_project(self) -> Option<Project> {
        Some(Project {
            id: record_uuid(&self.id)?,
            code: self.code,
            name: self.name,
            status: self.status,
            brand: self.brand.unwrap_or_else(|| "neon".to_string()),
            entity_id: record_uuid(&self.entity_id)?,
            firm_id: match self.firm_id.as_ref() {
                None => None,
                Some(id) => Some(record_uuid(id)?),
            },
            description: self.description,
            drive_folder_id: self.drive_folder_id,
            repository_url: self.repository_url,
            git_initialized_at: self.git_initialized_at,
            forge_provisioned_at: self.forge_provisioned_at,
            closed_at: self.closed_at,
            internal_slack_channel_url: self.internal_slack_channel_url,
            external_slack_channel_url: self.external_slack_channel_url,
            internal_slack_channel_id: self.internal_slack_channel_id,
            private_notion_page_url: self.private_notion_page_url,
            shared_notion_page_url: self.shared_notion_page_url,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

pub(crate) const PROJECT_TABLE: &str = "project";
/// The entities cluster this table links into (ENG-120).
const ENTITY_TABLE: &str = "entity";
const PERSON_PROJECT_ROLE_TABLE: &str = "person_project_role";
const PROJECT_SELECT: &str = "id, code, name, status, brand, entity_id, firm_id, description, \
                              drive_folder_id, repository_url, git_initialized_at, \
                              forge_provisioned_at, closed_at, \
                              internal_slack_channel_url, external_slack_channel_url, \
                              internal_slack_channel_id, \
                              private_notion_page_url, shared_notion_page_url, \
                              inserted_at, updated_at";

/// Errors from the SurrealDB Project read seam.
#[derive(Debug, thiserror::Error)]
pub enum ProjectStoreError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Person(#[from] crate::persons::PersonError),
    #[error(transparent)]
    Firm(#[from] crate::firms::FirmError),
    #[error("that project code is already in use")]
    CodeTaken,
    /// A direct write tried to change `code` on an existing row. The engine
    /// refuses this itself (`project.code` is `READONLY`); this variant only
    /// gives the refusal a message that points at the rule.
    #[error(
        "a Project code cannot be changed after it is chosen at matter-open — see \
         docs/glossary.md#project"
    )]
    CodeImmutable,
    #[error("writing a project returned no usable row")]
    WriteReturnedNothing,
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    #[error("no project {0}")]
    NoSuchProject(Uuid),
    #[error("no firm {0}")]
    NoSuchFirm(Uuid),
    /// [`matter_lifecycle_sets`]'s batched onboarding/offboarding-artifact
    /// read failed. Kept distinct from [`ProjectStoreError::Db`] because that
    /// read spans four tables, not one.
    #[error("matter lifecycle: {0}")]
    Lifecycle(String),
}

fn classify_project_write(error: surrealdb::Error) -> ProjectStoreError {
    if crate::surreal::retry::unique_violation(&error) == Some("project_code") {
        ProjectStoreError::CodeTaken
    } else {
        let message = error.to_string();
        if message.contains("field `code`") && message.contains("readonly") {
            ProjectStoreError::CodeImmutable
        } else {
            ProjectStoreError::Db(error)
        }
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing_project<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, ProjectStoreError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt)
        .await
        .map_err(classify_project_write)
}

/// Inputs for creating a Project row. Command callers validate the
/// `entity_id` before calling this function: the engine does not validate
/// a `record<>` link, so a dangling one is accepted here and caught above.
#[derive(Debug, Clone)]
pub struct NewProject {
    pub code: String,
    pub name: String,
    pub status: String,
    /// Which house brand's storefront this matter was opened through — see
    /// [`Project::brand`]. Defaults to `"neon"` (below) rather than the
    /// derived empty string, because the schema's `ASSERT` rejects an empty
    /// value: the many internal and fixture callers that build a `NewProject`
    /// with `..Default::default()` never touch this field, and `"neon"` is
    /// the correct value for every one of them. A real client-intake open
    /// (`portal::retainer_walk`) sets it explicitly from the resolved request
    /// host instead.
    pub brand: String,
    pub entity_id: Uuid,
    /// Owning practice. `None` on a row that has not yet been pointed at a
    /// firm — every existing caller that uses [`Default`] keeps that shape.
    pub firm_id: Option<Uuid>,
    pub description: Option<String>,
}

impl Default for NewProject {
    fn default() -> Self {
        Self {
            code: String::new(),
            name: String::new(),
            status: String::new(),
            brand: "neon".to_string(),
            entity_id: Uuid::nil(),
            firm_id: None,
            description: None,
        }
    }
}

/// One person's current participation on a Project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PersonProjectRole {
    pub id: Uuid,
    pub person_id: Uuid,
    pub project_id: Uuid,
    pub participation: String,
    pub is_lawyer_dri: bool,
    pub is_client_dri: bool,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct PersonProjectRoleRow {
    id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    participation: String,
    is_lawyer_dri: bool,
    is_client_dri: bool,
    inserted_at: String,
    updated_at: String,
}

impl PersonProjectRoleRow {
    fn into_role(self) -> Option<PersonProjectRole> {
        Some(PersonProjectRole {
            id: record_uuid(&self.id)?,
            person_id: record_uuid(&self.person_id)?,
            project_id: record_uuid(&self.project_id)?,
            participation: self.participation,
            is_lawyer_dri: self.is_lawyer_dri,
            is_client_dri: self.is_client_dri,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

const PERSON_PROJECT_ROLE_SELECT: &str =
    "id, person_id, project_id, participation, is_lawyer_dri, \
                                           is_client_dri, inserted_at, updated_at";

/// The participation kinds on the client side of a matter. `counterparty`
/// remains listed because a legacy row must keep reading client-side: promoting
/// an adverse party to the firm lens is the one direction that must never
/// happen by omission. Nothing writes it any more.
pub const PARTICIPATION_CLIENT_SIDE: &[&str] = &["client", "counterparty"];

/// The matter-side participation implied by a person's system tier — the only
/// way a participation is ever chosen.
///
/// Which side of a matter someone is on follows from what they are: a `client`
/// lands on the client side, and every firm tier lands firm-side under its own
/// name. There is no second vocabulary to disagree with the tier. The kinds that
/// used to need one are not participants at all — an adverse party never gets a
/// portal row, and co-counsel working the matter is a `lawyer` person.
#[must_use]
pub fn participation_for_role(role: Role) -> &'static str {
    role.as_str()
}

/// All participation rows for a matter, in stable insertion order.
pub async fn participations_for_project(
    surreal: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<PersonProjectRole>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PERSON_PROJECT_ROLE_SELECT} FROM {PERSON_PROJECT_ROLE_TABLE} \
             WHERE project_id = $project_id ORDER BY inserted_at, id"
        ))
        .bind(("project_id", record_id(PROJECT_TABLE, project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonProjectRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonProjectRoleRow::into_role)
        .collect())
}

/// Every participation row, ordered for the lawyer directory.
pub async fn all_participations(
    surreal: &SurrealDb,
) -> Result<Vec<PersonProjectRole>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PERSON_PROJECT_ROLE_SELECT} FROM {PERSON_PROJECT_ROLE_TABLE} \
             ORDER BY project_id, person_id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonProjectRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonProjectRoleRow::into_role)
        .collect())
}

/// One participation row by its id.
pub async fn participation_by_id(
    surreal: &SurrealDb,
    id: Uuid,
) -> Result<Option<PersonProjectRole>, ProjectStoreError> {
    let mut response = surreal
        .query(format!("SELECT {PERSON_PROJECT_ROLE_SELECT} FROM ONLY $id"))
        .bind(("id", record_id(PERSON_PROJECT_ROLE_TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<PersonProjectRoleRow> = response.take(0)?;
    Ok(row.and_then(PersonProjectRoleRow::into_role))
}

/// Add one person to a Project, validating both record links at the command
/// seam. The unique pair index is the concurrency backstop.
pub async fn add_participation(
    surreal: &SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    participation: &str,
) -> Result<PersonProjectRole, ProjectStoreError> {
    if crate::persons::find_by_id(surreal, person_id)
        .await?
        .is_none()
    {
        return Err(ProjectStoreError::NoSuchPerson(person_id));
    }
    if find_by_id(surreal, project_id).await?.is_none() {
        return Err(ProjectStoreError::NoSuchProject(project_id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing_project(|| {
        surreal
            .query(format!(
                "CREATE $id SET person_id = $person_id, project_id = $project_id, \
                 participation = $participation, inserted_at = $now, updated_at = $now \
                 RETURN {PERSON_PROJECT_ROLE_SELECT}"
            ))
            .bind(("id", record_id(PERSON_PROJECT_ROLE_TABLE, Uuid::now_v7())))
            .bind(("person_id", record_id("person", person_id)))
            .bind(("project_id", record_id(PROJECT_TABLE, project_id)))
            .bind(("participation", participation.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<PersonProjectRoleRow> = response.take(0)?;
    row.and_then(PersonProjectRoleRow::into_role)
        .ok_or(ProjectStoreError::WriteReturnedNothing)
}

/// Replace a participation's person and label without disturbing its DRI
/// markers. The caller enforces the DRI-side rule before invoking this write.
pub async fn update_participation(
    surreal: &SurrealDb,
    role_id: Uuid,
    person_id: Uuid,
    participation: &str,
) -> Result<Option<PersonProjectRole>, ProjectStoreError> {
    if crate::persons::find_by_id(surreal, person_id)
        .await?
        .is_none()
    {
        return Err(ProjectStoreError::NoSuchPerson(person_id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing_project(|| {
        surreal
            .query(format!(
                "UPDATE $id SET person_id = $person_id, participation = $participation, \
                 updated_at = $now RETURN {PERSON_PROJECT_ROLE_SELECT}"
            ))
            .bind(("id", record_id(PERSON_PROJECT_ROLE_TABLE, role_id)))
            .bind(("person_id", record_id("person", person_id)))
            .bind(("participation", participation.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<PersonProjectRoleRow> = response.take(0)?;
    Ok(row.and_then(PersonProjectRoleRow::into_role))
}

/// Remove one participation row. The caller checks the lawyer-DRI invariant
/// before deletion; this low-level command is intentionally idempotent.
pub async fn remove_participation(
    surreal: &SurrealDb,
    role_id: Uuid,
) -> Result<(), ProjectStoreError> {
    writing_project(|| {
        surreal
            .query("DELETE $id")
            .bind(("id", record_id(PERSON_PROJECT_ROLE_TABLE, role_id)))
    })
    .await?;
    Ok(())
}

/// Read the participation that gives `person_id` scope on `project_id`.
pub async fn participation_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
    project_id: Uuid,
) -> Result<Option<PersonProjectRole>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PERSON_PROJECT_ROLE_SELECT} FROM ONLY {PERSON_PROJECT_ROLE_TABLE} \
             WHERE person_id = $person_id AND project_id = $project_id LIMIT 1"
        ))
        .bind(("person_id", record_id("person", person_id)))
        .bind(("project_id", record_id(PROJECT_TABLE, project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<PersonProjectRoleRow> = response.take(0)?;
    Ok(row.and_then(PersonProjectRoleRow::into_role))
}

/// All participation rows held by one person, in stable insertion order.
pub async fn participations_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<PersonProjectRole>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PERSON_PROJECT_ROLE_SELECT} FROM {PERSON_PROJECT_ROLE_TABLE} \
             WHERE person_id = $person_id ORDER BY inserted_at, id"
        ))
        .bind(("person_id", record_id("person", person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonProjectRoleRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonProjectRoleRow::into_role)
        .collect())
}

/// Whether this person holds a firm-side participation row on the matter.
///
/// The membership question on its own, with no tier policy folded in:
/// client-side participations and client DRIs never enter the lawyer lens,
/// while unrecognized participation strings remain firm-side. Callers layer
/// their own tier rule over it — `store::access` requires the row of every
/// tier, while [`can_access_as_lawyer_in_surreal`] keeps the Owner/Admin
/// short-circuit its own callers still depend on.
pub async fn has_firm_participation(
    surreal: &SurrealDb,
    person_id: Uuid,
    project_id: Uuid,
) -> Result<bool, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT VALUE id FROM ONLY {PERSON_PROJECT_ROLE_TABLE} \
             WHERE person_id = $person_id AND project_id = $project_id \
             AND participation NOT IN ['client', 'counterparty'] \
             AND is_client_dri = false LIMIT 1"
        ))
        .bind(("person_id", record_id("person", person_id)))
        .bind(("project_id", record_id(PROJECT_TABLE, project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let result: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(result.is_some())
}

/// Whether a lawyer-tier actor may read a Project through the firm lens,
/// with the Owner/Admin project-scoping bypass applied.
///
/// The matter surface no longer comes through here — `store::access` requires
/// a participation row of every tier, so privileged reach is an explicit
/// place you navigate to rather than an invisible widening. What remains are
/// the callers that legitimately act as the deployment rather than as a
/// person on a matter: the MCP *write* tools, `aida_create_notation` and
/// `aida_create_project`, which check that the matter they are about to write
/// to is one the acting principal may write to.
///
/// Not the MCP reads. Since ENG-216 those answer through the caller's own
/// lens — `store::access::visible_projects` for the membership tiers and
/// [`matter_directory`] for Owner and Admin — so the admin-tier
/// short-circuit below is deliberately not on that path. It grants *full*
/// access to a matter, which is the opposite of what oversight gets.
pub async fn can_access_as_lawyer_in_surreal(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
    project_id: Uuid,
) -> Result<bool, ProjectStoreError> {
    if !role.is_lawyer_tier() {
        return Ok(false);
    }
    if role.is_admin_tier() {
        return Ok(true);
    }
    let Some(person_id) = person_id else {
        return Ok(false);
    };
    has_firm_participation(surreal, person_id, project_id).await
}

/// Whether a person may read a Project through the client lens.
///
/// The projects cluster is authoritative for both the project row and its
/// participation rows. The client-side predicate lives here beside the
/// lawyer predicate so that every access decision reads one membership
/// table.
pub async fn can_access_as_client_in_surreal(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    project_id: Uuid,
) -> Result<bool, ProjectStoreError> {
    let Some(person_id) = person_id else {
        return Ok(false);
    };
    let mut response = surreal
        .query(format!(
            "SELECT VALUE id FROM ONLY {PERSON_PROJECT_ROLE_TABLE} \
             WHERE person_id = $person_id AND project_id = $project_id \
             AND (participation IN ['client', 'counterparty'] OR is_client_dri = true) LIMIT 1"
        ))
        .bind(("person_id", record_id("person", person_id)))
        .bind(("project_id", record_id(PROJECT_TABLE, project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let result: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(result.is_some())
}

/// One matter as the Owner/Admin directory lens sees it: what the matter is,
/// and who is accountable for it.
///
/// Deliberately four fields. The lens answers "which matters exist, and who
/// owns each" and nothing further, so there is no project id here to hang a
/// detail link on: [`code`](Self::code) is the stable handle (`project_code`
/// is `UNIQUE`), and anything a matter *contains* — notations, deadlines,
/// documents, communications, the rest of the participation ledger — is
/// membership's to disclose, not oversight's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MatterDirectoryEntry {
    pub code: String,
    pub name: String,
    pub status: String,
    /// The names on the matter's `is_lawyer_dri` rows, alphabetical, or empty
    /// when no lawyer holds the marker. An unassigned matter is what this lens
    /// exists to surface, so an empty set is an ordinary value here rather than
    /// an error.
    pub lawyer_dris: Vec<String>,
}

/// The Owner/Admin directory over every matter.
///
/// The third shape beside the two predicates above, and neither of them:
/// [`can_access_as_lawyer_in_surreal`] short-circuits the admin tier to full
/// access to a matter, and `store::access` requires a participation row of
/// every tier and denies without one. This one is oversight rather than
/// membership — see that a matter exists and who owns it, without seeing what
/// is in it — so it returns the projection instead of a bool, and Owner and
/// Admin reach it holding no `person_project_roles` row at all.
///
/// Admin tier only. A `lawyer` caller gets an empty directory, the same
/// answer they get from every other firm-wide read they are not entitled to;
/// the tier is checked before the read runs, so a `lawyer` call touches
/// nothing.
pub async fn matter_directory(
    surreal: &SurrealDb,
    role: Role,
) -> Result<Vec<MatterDirectoryEntry>, ProjectStoreError> {
    if !role.is_admin_tier() {
        return Ok(Vec::new());
    }
    let projects = all(surreal).await?;
    if projects.is_empty() {
        return Ok(Vec::new());
    }

    let dris_by_project = dri_names_by_project(surreal, "is_lawyer_dri").await?;
    Ok(projects
        .into_iter()
        .map(|project| MatterDirectoryEntry {
            // A matter with no flagged row reads as unassigned rather than
            // an error.
            lawyer_dris: dris_by_project
                .get(&project.id)
                .cloned()
                .unwrap_or_default(),
            code: project.code,
            name: project.name,
            status: project.status,
        })
        .collect())
}

/// Owner sees every matter. Admin sees only matters owned by a firm they
/// belong to. An Admin with no `person_firm_role` row gets an empty
/// directory rather than the deployment-wide listing.
pub async fn matter_directory_for(
    surreal: &SurrealDb,
    role: Role,
    viewer_person_id: Option<Uuid>,
) -> Result<Vec<MatterDirectoryEntry>, ProjectStoreError> {
    let entries = matter_directory(surreal, role).await?;
    if role == Role::Owner {
        return Ok(entries);
    }
    if role != Role::Admin {
        return Ok(entries);
    }
    let Some(person_id) = viewer_person_id else {
        return Ok(Vec::new());
    };
    let firm_ids = crate::firms::firm_ids_for_person(surreal, person_id).await?;
    if firm_ids.is_empty() {
        return Ok(Vec::new());
    }
    let owned: std::collections::BTreeSet<_> = all(surreal)
        .await?
        .into_iter()
        .filter(|project| {
            project
                .firm_id
                .is_some_and(|firm_id| firm_ids.contains(&firm_id))
        })
        .map(|project| project.code)
        .collect();
    Ok(entries
        .into_iter()
        .filter(|entry| owned.contains(&entry.code))
        .collect())
}

/// Every project's names on one DRI side, keyed by project id, for every
/// project that has at least one. One round trip per table, not one per
/// matter: every flagged row, then the persons they name — the same batching
/// `matter_lifecycle_sets` uses. `flag_column` is always one of this module's
/// own two DRI column names, never external input.
///
/// A flagged row naming a person who is no longer there drops out rather than
/// becoming a name the caller cannot produce. The set has no inherent order,
/// so each project's names come back sorted rather than left in row-scan
/// order.
async fn dri_names_by_project(
    surreal: &SurrealDb,
    flag_column: &str,
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PERSON_PROJECT_ROLE_SELECT} FROM {PERSON_PROJECT_ROLE_TABLE} \
             WHERE {flag_column} = true"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<PersonProjectRoleRow> = response.take(0)?;
    let mut ids_by_project: std::collections::HashMap<Uuid, Vec<Uuid>> =
        std::collections::HashMap::new();
    for row in rows.into_iter().filter_map(PersonProjectRoleRow::into_role) {
        ids_by_project
            .entry(row.project_id)
            .or_default()
            .push(row.person_id);
    }
    let dri_ids: Vec<Uuid> = ids_by_project.values().flatten().copied().collect();
    let names: std::collections::HashMap<Uuid, String> =
        crate::persons::find_by_ids(surreal, &dri_ids)
            .await?
            .into_iter()
            .map(|person| (person.id, person.name))
            .collect();
    Ok(ids_by_project
        .into_iter()
        .map(|(project_id, ids)| {
            let mut names: Vec<String> = ids
                .into_iter()
                .filter_map(|id| names.get(&id).cloned())
                .collect();
            names.sort();
            (project_id, names)
        })
        .collect())
}

/// One project's accountable people on both DRI sides — the shape the nightly
/// `DriDigest` Slack notice renders one line per project from.
///
/// `Deserialize` too (unlike [`MatterDirectoryEntry`]): this is the journaled
/// output of the workflow's `query` step, and Restate must be able to replay
/// it back out of the journal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectDriSummary {
    pub code: String,
    pub name: String,
    pub status: String,
    /// The names on the project's `is_lawyer_dri` rows, alphabetical, or
    /// empty when no lawyer holds the marker.
    pub lawyer_dris: Vec<String>,
    /// The names on the project's `is_client_dri` rows, alphabetical, or
    /// empty when no client holds the marker.
    pub client_dris: Vec<String>,
}

/// Every project's DRIs on both sides, for the nightly digest.
///
/// Unlike [`matter_directory`], this has no role gate: the caller is a
/// headless workflow with no session, not a person, and the digest's whole
/// purpose is to surface a side left unassigned — on either side — so both
/// are read here, where `matter_directory` deliberately reads only the
/// lawyer side for its Owner/Admin oversight view.
pub async fn dri_digest(surreal: &SurrealDb) -> Result<Vec<ProjectDriSummary>, ProjectStoreError> {
    let projects = all(surreal).await?;
    if projects.is_empty() {
        return Ok(Vec::new());
    }
    let lawyer_by_project = dri_names_by_project(surreal, "is_lawyer_dri").await?;
    let client_by_project = dri_names_by_project(surreal, "is_client_dri").await?;
    Ok(projects
        .into_iter()
        .map(|project| ProjectDriSummary {
            lawyer_dris: lawyer_by_project
                .get(&project.id)
                .cloned()
                .unwrap_or_default(),
            client_dris: client_by_project
                .get(&project.id)
                .cloned()
                .unwrap_or_default(),
            code: project.code,
            name: project.name,
            status: project.status,
        })
        .collect())
}

/// Add a person to a Project's lawyer or client DRI set.
///
/// Designation is **additive**: a matter carries as many accountable people per
/// side as the firm has put there, so taking the marker displaces nobody. Giving
/// it up is [`clear_dri_in_surreal`], and that is the only way a person leaves a
/// set.
///
/// The transaction writes the Project itself first: concurrent DRI changes to
/// the same Project conflict on that row and retry through the typed conflict
/// loop, so the set-emptiness rule `store::participation` checks beforehand
/// cannot be raced by two removals that each saw a second holder.
pub async fn designate_dri_in_surreal(
    surreal: &SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    side: DriSide,
) -> Result<(), ProjectStoreError> {
    let Some(person) = crate::persons::find_by_id(surreal, person_id).await? else {
        return Err(ProjectStoreError::NoSuchPerson(person_id));
    };
    if find_by_id(surreal, project_id).await?.is_none() {
        return Err(ProjectStoreError::NoSuchProject(project_id));
    }
    let flag = match side {
        DriSide::Lawyer => "is_lawyer_dri",
        DriSide::Client => "is_client_dri",
    };
    // A DRI's first row is a participation like any other: derived from the
    // person being designated, never from a word this function picks.
    let participation = participation_for_role(person.role);
    let now = chrono::Utc::now().to_rfc3339();
    writing_project(|| {
        surreal
            .query(format!(
                "BEGIN; \
                 UPDATE $project SET updated_at = $now; \
                 LET $existing = (SELECT VALUE id FROM {PERSON_PROJECT_ROLE_TABLE} \
                    WHERE project_id = $project AND person_id = $person LIMIT 1)[0]; \
                 IF $existing != NONE {{ \
                    UPDATE $existing SET {flag} = true, updated_at = $now; \
                 }} ELSE {{ \
                    CREATE $role SET person_id = $person, project_id = $project, \
                    participation = $participation, {flag} = true, \
                    inserted_at = $now, updated_at = $now; \
                 }}; \
                 COMMIT;"
            ))
            .bind(("project", record_id(PROJECT_TABLE, project_id)))
            .bind(("person", record_id("person", person_id)))
            .bind(("role", record_id(PERSON_PROJECT_ROLE_TABLE, Uuid::now_v7())))
            .bind(("participation", participation.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    Ok(())
}

/// Drop one participation row's DRI marker for `side`.
///
/// The inverse of [`designate_dri_in_surreal`], and the only way a person leaves
/// a DRI set. It takes one row out and leaves the rest of the side alone; that
/// the lawyer set never empties is `store::participation`'s rule, checked above
/// this call.
pub async fn clear_dri_in_surreal(
    surreal: &SurrealDb,
    role_id: Uuid,
    side: DriSide,
) -> Result<(), ProjectStoreError> {
    let flag = match side {
        DriSide::Lawyer => "is_lawyer_dri",
        DriSide::Client => "is_client_dri",
    };
    let now = chrono::Utc::now().to_rfc3339();
    writing_project(|| {
        surreal
            .query(format!(
                "UPDATE $role SET {flag} = false, updated_at = $now"
            ))
            .bind(("role", record_id(PERSON_PROJECT_ROLE_TABLE, role_id)))
            .bind(("now", now.clone()))
    })
    .await?;
    Ok(())
}

/// Create a Project under a fresh UUID record key.
///
/// Writes retry only typed Surreal transaction conflicts; a code collision is
/// a caller-correctable conflict and is never retried as if it were transient.
pub async fn create(surreal: &SurrealDb, input: &NewProject) -> Result<Project, ProjectStoreError> {
    if let Some(firm_id) = input.firm_id {
        if crate::firms::find_by_id(surreal, firm_id).await?.is_none() {
            return Err(ProjectStoreError::NoSuchFirm(firm_id));
        }
    }
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing_project(|| {
        surreal
            .query(format!(
                "CREATE $id SET code = $code, name = $name, status = $status, \
                 brand = $brand, \
                 entity_id = $entity_id, \
                 firm_id = $firm_id, \
                 description = $description, inserted_at = $inserted_at, \
                 updated_at = $updated_at RETURN {PROJECT_SELECT}"
            ))
            .bind(("id", record_id(PROJECT_TABLE, id)))
            .bind(("code", input.code.clone()))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
            .bind(("brand", input.brand.clone()))
            .bind(("entity_id", record_id(ENTITY_TABLE, input.entity_id)))
            .bind((
                "firm_id",
                input
                    .firm_id
                    .map(|firm_id| record_id(crate::firms::TABLE, firm_id)),
            ))
            .bind(("description", input.description.clone()))
            .bind(("inserted_at", now.clone()))
            .bind(("updated_at", now.clone()))
    })
    .await?;
    let row: Option<ProjectRow> = response.take(0)?;
    row.and_then(ProjectRow::into_project)
        .ok_or(ProjectStoreError::WriteReturnedNothing)
}

/// Find the Project carrying `code`, whose UNIQUE index makes it at most one.
///
/// Fixtures resolve by code rather than id: the code is the stable name a seed
/// knows, while the id is whatever the engine that first created the record
/// assigned.
pub async fn find_by_code(
    surreal: &SurrealDb,
    code: &str,
) -> Result<Option<Project>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PROJECT_SELECT} FROM ONLY {PROJECT_TABLE} WHERE code = $code LIMIT 1"
        ))
        .bind(("code", code.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ProjectRow> = response.take(0)?;
    Ok(row.and_then(ProjectRow::into_project))
}

/// Resolve the Project carrying `code`, creating it under `id` if absent.
///
/// Racing callers settle on one record rather than one of them failing: `code`
/// is UNIQUE, so the loser of a create race re-reads and adopts the winner's
/// record. The cucumber suites need this — they run scenarios concurrently
/// against one shared engine, each re-seeding the same fixture codes. Mirrors
/// [`crate::persons::find_or_create`].
pub async fn find_or_create_by_code(
    surreal: &SurrealDb,
    id: Uuid,
    input: &NewProject,
) -> Result<Project, ProjectStoreError> {
    if let Some(existing) = find_by_code(surreal, &input.code).await? {
        return Ok(existing);
    }
    match upsert_with_id(surreal, id, input).await {
        Err(ProjectStoreError::CodeTaken) => find_by_code(surreal, &input.code)
            .await?
            .ok_or(ProjectStoreError::WriteReturnedNothing),
        other => other,
    }
}

/// Create or update the Project carrying `id`, rather than minting a new one.
///
/// [`create`] owns id generation, which is right for a real matter open. A
/// fixture that must exist in both engines under a single id cannot use it:
/// the caller already holds the id and needs this cluster to agree with it.
/// Idempotent, so re-running a seed over a persisted database reconciles the
/// record instead of duplicating it.
pub async fn upsert_with_id(
    surreal: &SurrealDb,
    id: Uuid,
    input: &NewProject,
) -> Result<Project, ProjectStoreError> {
    if let Some(firm_id) = input.firm_id {
        if crate::firms::find_by_id(surreal, firm_id).await?.is_none() {
            return Err(ProjectStoreError::NoSuchFirm(firm_id));
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    let firm_set = if input.firm_id.is_some() {
        "firm_id = $firm_id,"
    } else {
        ""
    };
    let mut response = writing_project(|| {
        surreal
            .query(format!(
                "UPSERT $id SET code = $code, name = $name, status = $status, \
                 brand = $brand, \
                 entity_id = $entity_id, \
                 {firm_set} \
                 description = $description, \
                 inserted_at = IF inserted_at THEN inserted_at ELSE $inserted_at END, \
                 updated_at = $updated_at RETURN {PROJECT_SELECT}"
            ))
            .bind(("id", record_id(PROJECT_TABLE, id)))
            .bind(("code", input.code.clone()))
            .bind(("name", input.name.clone()))
            .bind(("status", input.status.clone()))
            .bind(("brand", input.brand.clone()))
            .bind(("entity_id", record_id(ENTITY_TABLE, input.entity_id)))
            .bind((
                "firm_id",
                input
                    .firm_id
                    .map(|firm_id| record_id(crate::firms::TABLE, firm_id)),
            ))
            .bind(("description", input.description.clone()))
            .bind(("inserted_at", now.clone()))
            .bind(("updated_at", now.clone()))
    })
    .await?;
    let row: Option<ProjectRow> = response.take(0)?;
    row.and_then(ProjectRow::into_project)
        .ok_or(ProjectStoreError::WriteReturnedNothing)
}

/// Find the Project identified by `id` in the projects cluster.
///
/// This is the validation read a writer uses to prove a project id names a
/// real row before storing a reference to it.
pub async fn find_by_id(
    surreal: &SurrealDb,
    id: Uuid,
) -> Result<Option<Project>, ProjectStoreError> {
    let mut response = surreal
        .query(format!("SELECT {PROJECT_SELECT} FROM ONLY $id"))
        .bind(("id", record_id(PROJECT_TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ProjectRow> = response.take(0)?;
    Ok(row.and_then(ProjectRow::into_project))
}

/// Find a Project by its display name. Seeds use this only as their natural
/// key lookup; application commands address Projects by UUID.
pub async fn find_by_name(
    surreal: &SurrealDb,
    name: &str,
) -> Result<Option<Project>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PROJECT_SELECT} FROM ONLY {PROJECT_TABLE} WHERE name = $name LIMIT 1"
        ))
        .bind(("name", name.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ProjectRow> = response.take(0)?;
    Ok(row.and_then(ProjectRow::into_project))
}

/// Every project in stable lawyer-list order.
pub async fn all(surreal: &SurrealDb) -> Result<Vec<Project>, ProjectStoreError> {
    let mut response = surreal
        .query(format!(
            "SELECT {PROJECT_SELECT} FROM {PROJECT_TABLE} ORDER BY name, id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ProjectRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(ProjectRow::into_project)
        .collect())
}

/// A Google Drive resource id is opaque, but its documented wire form is an
/// ASCII identifier. Accept only that form at this boundary so an operator
/// cannot accidentally persist a copied Drive URL in the address column.
const DRIVE_FOLDER_ID_INVALID: &str =
    "Drive folder id must contain only letters, digits, hyphens, or underscores.";

/// Which side of a matter a DRI designation applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriSide {
    Lawyer,
    Client,
}

/// Normalize free text into the alphanumeric kebab-case base of a matter code.
/// The generated code appends a stable letter suffix for uniqueness.
///
/// Digits survive. A name carrying a numeral — a company name ending in one, a
/// statute or form number, a matter named for a section — keeps it, rather than
/// having it collapsed into a separator: the code is what the matter's route,
/// portal mount, and documents-bucket prefix use, so a numbered filing has to
/// be expressible as a code.
#[must_use]
pub fn code_base_from_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !out.is_empty() && !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

fn letter_suffix(project_id: Uuid) -> String {
    let mut n = project_id.as_u128();
    let mut suffix = ['a'; 8];
    for ch in suffix.iter_mut().rev() {
        *ch = char::from(b'a' + u8::try_from(n % 26).expect("modulo 26 fits u8"));
        n /= 26;
    }
    suffix.into_iter().collect()
}

/// Generate a unique default matter code from its display name and UUID.
#[must_use]
pub fn code_from_name(name: &str, project_id: Uuid) -> String {
    let suffix = letter_suffix(project_id);
    let max_base = PROJECT_CODE_MAX_LEN - suffix.len() - 1;
    let mut base: String = code_base_from_name(name).chars().take(max_base).collect();
    while base.ends_with('-') {
        base.pop();
    }
    format!("{base}-{suffix}")
}

/// Normalize a manually supplied matter code. Returns `None` when the
/// input is blank.
#[must_use]
pub fn normalize_code(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

/// Whether a matter code is safe as a URL segment, a bare-repo directory name,
/// and an object-storage key prefix.
///
/// Lowercase letters, digits, and single hyphens; alphanumeric at both ends,
/// and not a segment Navigator routes on its own.
///
/// `cloud::workspace` holds the single definition of that shape and this calls
/// it, so a Project code and its repository's directory name cannot drift
/// apart. The rationale for each shape restriction lives there. The
/// `repository_url` *column*, in contrast, is not composed from the code — it
/// is a whole URL stored on the row, validated by [`is_valid_repository_url`].
///
/// The reserved-code refusal is the second half. A code is a route segment —
/// `/app/projects/{code}/portal` — and `/app/projects/new` is the matter-open
/// form, so `new` is well-formed and still refused. Which side of a genuine
/// collision would win depends on Axum registration order, so the code is
/// refused rather than the precedence reasoned about. The engine carries the
/// same refusal as an `ASSERT` on `project.code`, because this function only
/// guards the write paths that call it.
#[must_use]
pub fn is_valid_code(code: &str) -> bool {
    cloud::workspace::is_valid_slug(code)
        && !cloud::workspace::RESERVED_PROJECT_CODES.contains(&code)
}

/// Resolve a `{project_code}` segment to its Project's internal id.
///
/// `None` covers every miss the same way — a malformed code, a code no Project
/// carries, or a store error — because a caller must not be able to tell them
/// apart. Each handler turns that into its own non-disclosing refusal, which is
/// 404 everywhere below `/app`: a 403 would confirm to a stranger that a matter
/// with this code exists.
///
/// The code is validated before the store is asked. That refuses `new` — the
/// matter-open form rather than a matter — and keeps a malformed segment from
/// reaching a query at all.
///
/// A caller that owes its reader a distinct answer for a store failure calls
/// [`find_by_code`] directly and maps the error itself: an outage must read as
/// `500`, not as "your matter is gone".
pub async fn id_for_code(surreal: &SurrealDb, project_code: &str) -> Option<Uuid> {
    if !is_valid_code(project_code) {
        return None;
    }
    find_by_code(surreal, project_code)
        .await
        .ok()
        .flatten()
        .map(|project| project.id)
}

/// Human-readable reason [`is_valid_repository_url`] refuses a value, used as
/// the caller-correctable message on the command boundary.
pub const REPOSITORY_URL_INVALID: &str =
    "A repository URL must be an http(s):// URL naming a host and a path.";

/// Whether a value is usable as a Project's source repository URL.
///
/// Deliberately permissive about *where*: any forge, any organization, any
/// self-hosted host. It is strict about *what*, because this URL is both shown
/// to a lawyer as a link and handed to `git clone`:
///
/// - **`http://` or `https://` only.** A `file://`, `ssh://`, or `javascript:`
///   value would either read the serving host's own disk or render as a live
///   link, and neither is a repository the Firm meant to record.
/// - **A non-empty host and path.** `https://github.com` names a forge, not a
///   repository, so cloning it could never succeed.
/// - **No whitespace and no credentials.** A `user:token@host` URL would put a
///   secret in a column that is rendered into a page and logged.
#[must_use]
pub fn is_valid_repository_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(("http" | "https", rest)) = trimmed.split_once("://") else {
        return false;
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return false;
    };
    // `@` in the authority is an embedded credential; a bare host has none.
    !authority.is_empty() && !authority.contains('@') && !path.trim_matches('/').is_empty()
}

/// Human-readable reason [`is_valid_resource_url`] refuses a value, used as
/// the caller-correctable message on the command boundary.
pub const RESOURCE_URL_INVALID: &str =
    "A resource link must be an http(s):// URL naming a host and a path.";

/// Whether a value is usable as one of the matter's collaboration resource
/// links — the two Slack channels and the two Notion pages.
///
/// Every one of these is rendered as an `href` on the matter page, to lawyer
/// and client alike, which is the whole reason they are validated rather than
/// merely trimmed. The rule is [`is_valid_repository_url`]'s: `http(s)` only,
/// a real host, a non-empty path, no whitespace, and no embedded credential.
/// A `javascript:` or `data:` value would execute instead of navigating, and a
/// `user:token@host` value would put a secret into a rendered page.
///
/// It is deliberately the *same* rule rather than a looser one. These columns
/// hold third-party addresses the firm pastes in, so there is no shape to
/// check beyond "this is a link that navigates somewhere" — and a resource
/// panel that trusted one column more than another would be a gap waiting for
/// the next field to be added to the wrong half.
#[must_use]
pub fn is_valid_resource_url(url: &str) -> bool {
    is_valid_repository_url(url)
}

/// The notation id of the person's **sole open matter**, for auto-routing an
/// inbound message to a matter without manual triage. Returns `Some` only
/// when the person is the client (`notations.person_id`) on exactly one
/// matter whose project is still `open`; `None` when they have none, or more
/// than one (the ambiguous case — fall back to manual `@link`).
///
/// This is the seam the email loop uses so a known client's reply lands on
/// their matter's conversation log on its own.
///
/// # Errors
///
/// Propagates any database error.
pub async fn sole_open_matter_for_person(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Option<Uuid>, String> {
    let notations = crate::notations::list_by_person(surreal, person_id)
        .await
        .map_err(|error| error.to_string())?;

    let mut open: Vec<Uuid> = Vec::new();
    for n in notations {
        if let Some(p) = find_by_id(surreal, n.project_id)
            .await
            .map_err(|error| error.to_string())?
        {
            if p.status == "open" {
                open.push(n.id);
            }
        }
    }
    Ok((open.len() == 1).then(|| open[0]))
}

/// Flip the matter that `notation_id` belongs to from `open` to
/// `closed`. Returns the closed project's id, or `None` if the notation
/// (or its project) no longer exists.
///
/// Idempotent and monotonic: a matter already `closed` or `archived` is
/// left as-is — re-running never re-opens it, and a replay of the
/// firm-signature side effect is a no-op. `inserted_at`/`updated_at` are
/// maintained by the entity's active-model behavior.
pub async fn close_for_notation(
    surreal: &SurrealDb,
    notation_id: Uuid,
) -> Result<Option<Uuid>, String> {
    let Some(n) = crate::notations::find_by_id(surreal, notation_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(p) = find_by_id(surreal, n.project_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let project_id = p.id;
    // Monotonic: don't walk backwards out of `archived`, and don't
    // churn an already-`closed` row.
    if p.status == "closed" || p.status == "archived" {
        return Ok(Some(project_id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    surreal
        .query("UPDATE $id SET status = 'closed', closed_at = $closed_at, updated_at = $closed_at")
        .bind(("id", record_id(PROJECT_TABLE, project_id)))
        .bind(("closed_at", now))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| error.to_string())?;
    Ok(Some(project_id))
}

/// The matter lifecycle: `open` → `closed` → `archived`.
///
/// A matter's `status` is a **lifecycle** field, not a descriptive one.
/// It is coupled to `closed_at`, and the two must never disagree: open
/// matter routing keys off `status` while the ten-year retention purge
/// keys off `closed_at`, so a contradiction routes a matter as open
/// while retention treats it as closed, or the reverse.
///
/// That invariant lives in these three transitions and nowhere else.
/// [`update_project`] owns the genuinely descriptive fields and refuses
/// to touch either column — `status` and `closed_at` are rejected outright
/// by a PATCH, not silently accepted and forwarded here. Deserializes from
/// the lowercase wire form (`"close"`, `"reopen"`, `"archive"`) so
/// `POST /app/api/projects/{id}/lifecycle`, the CLI, and the
/// `aida_close_project` MCP tool all name the same three transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transition {
    /// Close a matter directly, without the closing-letter ceremony.
    /// Stamps `closed_at` if it is not already stamped.
    Close,
    /// Reopen a closed matter. Clears `closed_at`, which restarts the
    /// retention window if it is closed again later.
    Reopen,
    /// Archive a matter. Terminal.
    Archive,
}

impl Transition {
    /// The status this transition lands the matter in.
    #[must_use]
    pub fn target_status(self) -> &'static str {
        match self {
            Transition::Close => "closed",
            Transition::Reopen => "open",
            Transition::Archive => "archived",
        }
    }
}

/// Move a matter through its lifecycle, maintaining the `status` /
/// `closed_at` invariant.
///
/// - Landing on `open` clears `closed_at`.
/// - Landing on `closed` or `archived` guarantees exactly one
///   `closed_at`, preserving an existing stamp rather than restarting
///   the retention window.
///
/// **`archived` is terminal.** An archived matter refuses every
/// transition except a no-op re-archive; reopening one would resurrect a
/// matter whose retention clock is already running. Re-applying a
/// transition the matter has already made is a no-op rather than an
/// error, so a double-submitted lawyer form does not churn the row.
///
/// This is the direct lawyer path. [`close_for_notation`] remains the
/// *ceremonial* path — the closing-letter workflow side effect — and is
/// unchanged.
///
/// # Errors
/// [`ProjectCommandError::NotFound`] when no matter has that id, and
/// [`ProjectCommandError::Invalid`] for a transition out of `archived`.
pub async fn transition_project(
    surreal: &SurrealDb,
    id: Uuid,
    transition: Transition,
) -> Result<Project, ProjectCommandError> {
    let row = find_by_id(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .ok_or(ProjectCommandError::NotFound)?;

    // Terminal means terminal. Re-archiving is the one no-op allowed,
    // because a repeated request should not fail.
    if row.status == "archived" && transition != Transition::Archive {
        return Err(ProjectCommandError::Invalid(
            "This matter is archived; archiving is terminal.",
        ));
    }

    let target = transition.target_status();
    let existing_close = row.closed_at.clone();

    // Already there: nothing to write. Guarded *after* the terminal
    // check so reopening an archived matter still reports why.
    if row.status == target {
        return Ok(row);
    }

    let closed_at = match transition {
        // Open matters carry no close date at all.
        Transition::Reopen => None,
        // Preserve an existing stamp: a matter closed, reopened, and
        // closed again starts a fresh retention window, but one merely
        // archived after closing keeps the date retention already knows.
        Transition::Close | Transition::Archive => {
            Some(existing_close.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()))
        }
    };
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET status = $status, closed_at = $closed_at, updated_at = $updated_at \
             RETURN {PROJECT_SELECT}"
        ))
        .bind(("id", record_id(PROJECT_TABLE, id)))
        .bind(("status", target.to_string()))
        .bind(("closed_at", closed_at))
        .bind(("updated_at", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    let updated: Option<ProjectRow> = response
        .take(0)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    updated
        .and_then(ProjectRow::into_project)
        .ok_or(ProjectCommandError::NotFound)
}

/// Set or clear the opaque Drive folder address for a matter.
///
/// `Some` records a Google Drive resource id; `None` deliberately clears the
/// address, for example after a failed provisioning attempt is reconciled.
/// The value is not a URL, name, or sync cursor. The database's partial unique
/// index is the final guard that prevents one folder from being assigned to
/// two matters while allowing any number of rollout-state `NULL`s.
///
/// Returns `Ok(None)` when the matter no longer exists.
///
/// # Errors
/// Returns [`ProjectCommandError::Invalid`] for an empty or non-identifier
/// value, and propagates a database error (including a duplicate assignment).
pub async fn set_drive_folder_id(
    surreal: &SurrealDb,
    project_id: Uuid,
    drive_folder_id: Option<&str>,
) -> Result<Option<Project>, ProjectCommandError> {
    let drive_folder_id = drive_folder_id
        .map(str::trim)
        .map(|id| {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(ProjectCommandError::Invalid(DRIVE_FOLDER_ID_INVALID));
            }
            Ok(id.to_string())
        })
        .transpose()?;

    if find_by_id(surreal, project_id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_none()
    {
        return Ok(None);
    }
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET drive_folder_id = $drive_folder_id, updated_at = $updated_at \
             RETURN {PROJECT_SELECT}"
        ))
        .bind(("id", record_id(PROJECT_TABLE, project_id)))
        .bind(("drive_folder_id", drive_folder_id))
        .bind(("updated_at", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    let updated: Option<ProjectRow> = response
        .take(0)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    Ok(updated.and_then(ProjectRow::into_project))
}

/// Record the Slack Web API channel ID provisioned for a matter's private
/// firm-side channel. This is a system-managed coordinate, not a free-form
/// resource URL, so only non-empty ASCII channel IDs are accepted here.
pub async fn set_internal_slack_channel_id(
    surreal: &SurrealDb,
    project_id: Uuid,
    channel_id: &str,
) -> Result<Option<Project>, ProjectCommandError> {
    let channel_id = channel_id.trim();
    if channel_id.is_empty() || !channel_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(ProjectCommandError::Invalid(SLACK_CHANNEL_ID_INVALID));
    }
    if find_by_id(surreal, project_id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_none()
    {
        return Ok(None);
    }
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET internal_slack_channel_id = $channel_id, updated_at = $updated_at \
             RETURN {PROJECT_SELECT}"
        ))
        .bind(("id", record_id(PROJECT_TABLE, project_id)))
        .bind(("channel_id", channel_id.to_string()))
        .bind(("updated_at", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    let updated: Option<ProjectRow> = response
        .take(0)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    Ok(updated.and_then(ProjectRow::into_project))
}

/// Request body for updating a matter's **descriptive** fields through the
/// command boundary — its name, entity, and scope narrative. Deliberately
/// narrow: it is neither the matter-open path (no conflict check, no repo
/// provisioning) nor a lifecycle transition. `status` and its coupled
/// `closed_at` are intentionally absent — moving a matter through
/// open/closed/archived is a lifecycle change whose retention semantics are a
/// firm-policy determination, so it belongs to [`transition_project`] through
/// `POST /app/api/projects/{id}/lifecycle`, not this general edit.
///
/// `#[serde(deny_unknown_fields)]` is load-bearing here, not decorative: a
/// caller posting `status` or `closed_at` (or any other field this struct
/// does not name) must be told so with a `400`, rather than have the field
/// silently dropped while a `200` implies it was honored.
///
/// # This is always a patch
///
/// **Every field is optional, and an absent field is never written.** A caller
/// sends the fields it wants to change and nothing else; the columns it omits
/// keep the values they had. There is no field whose absence means "blank this
/// out", because that is how a partial update silently destroys data a caller
/// never mentioned — a client that reads a matter, edits one field, and sends
/// its own narrow body should not be able to erase the four collaboration
/// links by not knowing about them.
///
/// So the three states a column can be in are:
///
/// | The body | Effect |
/// | --- | --- |
/// | the field is absent (or `null`) | the column is left exactly as it was |
/// | the field is `""` | the column is cleared |
/// | the field has a value | the column is set to it |
///
/// `null` and absent are deliberately the same. `serde` cannot distinguish them
/// on an `Option` without a nested `Option`, and collapsing them toward *leave
/// alone* is the safe direction: the failure mode of the other choice is a
/// caller wiping a column it did not intend to touch. Clearing is therefore
/// always explicit, and always the empty string — the same value an HTML form
/// posts for a text input a person emptied, so the form and the JSON caller
/// converge on one rule rather than each getting its own.
///
/// `name` is the one field that refuses `""`, because a matter with no name is
/// not a state a patch may produce. Omitting it leaves the name alone.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProjectCommand {
    /// The matter's name. Absent leaves it alone, like every other field here;
    /// present-but-blank is refused, because a matter with no name is not a
    /// state a patch may produce.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entity_id: Option<Uuid>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub internal_slack_channel_url: Option<String>,
    #[serde(default)]
    pub external_slack_channel_url: Option<String>,
    /// The Project's source repository as a whole URL, on any forge. A blank
    /// submission clears it; an omitted one leaves it untouched.
    #[serde(default)]
    pub repository_url: Option<String>,
    /// The firm-only Notion page. A blank submission clears it; an omitted one
    /// leaves it untouched.
    #[serde(default)]
    pub private_notion_page_url: Option<String>,
    /// The client-shared Notion page, on the same blank-clears terms.
    #[serde(default)]
    pub shared_notion_page_url: Option<String>,
}

/// Set or clear the Project's source repository URL.
///
/// The provisioning-side counterpart to the `repository_url` field on
/// [`UpdateProjectCommand`], for a caller that holds only the matter id — the
/// dev seed, or a reconciler recording where a Project's source landed.
/// `None` clears the column.
///
/// Returns `Ok(None)` when the matter no longer exists.
///
/// # Errors
/// Returns [`ProjectCommandError::Invalid`] when the URL is not one
/// [`is_valid_repository_url`] accepts, and propagates a database error.
pub async fn set_repository_url(
    surreal: &SurrealDb,
    project_id: Uuid,
    repository_url: Option<&str>,
) -> Result<Option<Project>, ProjectCommandError> {
    let repository_url = repository_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| {
            if is_valid_repository_url(url) {
                Ok(url.to_string())
            } else {
                Err(ProjectCommandError::Invalid(REPOSITORY_URL_INVALID))
            }
        })
        .transpose()?;

    if find_by_id(surreal, project_id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_none()
    {
        return Ok(None);
    }
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET repository_url = $repository_url, updated_at = $updated_at \
             RETURN {PROJECT_SELECT}"
        ))
        .bind(("id", record_id(PROJECT_TABLE, project_id)))
        .bind(("repository_url", repository_url))
        .bind(("updated_at", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    let updated: Option<ProjectRow> = response
        .take(0)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    Ok(updated.and_then(ProjectRow::into_project))
}

/// A matter's descriptive update could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum ProjectCommandError {
    /// The request is malformed in a way the caller can correct.
    #[error("{0}")]
    Invalid(&'static str),
    /// No matter with the requested id.
    #[error("no matter with that id")]
    NotFound,
    /// The database refused a delete because other rows still reference this
    /// matter (participations, notations, price events, …). Carries the
    /// database's own detail, which names the referencing table so an
    /// operator sees *why*. Caller-correctable (detach/close those first), so
    /// an adapter renders it as a conflict, not a server fault.
    #[error("this matter is still referenced by other records ({0})")]
    Referenced(String),
    #[error("database: {0}")]
    Db(String),
}

/// Refuse any collaboration resource link that would not navigate.
///
/// Split out of [`update_project`] so the command body stays about *what* it
/// writes. Each of the four is rendered as an `href` on the matter page, to
/// lawyer and client alike, so the gate is the same for all of them — a panel
/// that trusted one column more than another would be a gap waiting for the
/// next field to be added to the wrong half. A blank value is the documented
/// way to clear a column, so it passes.
/// The optional text columns [`update_project`] may set, each paired with the
/// command field it reads.
///
/// One table drives both halves of the sparse update — which assignments the
/// `SET` clause carries and which parameters are bound — so a new optional
/// column is one row here instead of two `if` blocks that must agree. `None`
/// means "leave this column alone"; `Some("")` means "clear it".
fn optional_text_columns(input: &UpdateProjectCommand) -> [(&'static str, Option<&String>); 6] {
    [
        ("description", input.description.as_ref()),
        (
            "internal_slack_channel_url",
            input.internal_slack_channel_url.as_ref(),
        ),
        (
            "external_slack_channel_url",
            input.external_slack_channel_url.as_ref(),
        ),
        ("repository_url", input.repository_url.as_ref()),
        (
            "private_notion_page_url",
            input.private_notion_page_url.as_ref(),
        ),
        (
            "shared_notion_page_url",
            input.shared_notion_page_url.as_ref(),
        ),
    ]
}

fn validate_resource_links(input: &UpdateProjectCommand) -> Result<(), ProjectCommandError> {
    let links = [
        &input.internal_slack_channel_url,
        &input.external_slack_channel_url,
        &input.private_notion_page_url,
        &input.shared_notion_page_url,
    ];
    for url in links.into_iter().flatten() {
        if !url.trim().is_empty() && !is_valid_resource_url(url) {
            return Err(ProjectCommandError::Invalid(RESOURCE_URL_INVALID));
        }
    }
    Ok(())
}

/// Update a matter's descriptive fields — name, entity, scope narrative, its
/// Slack channels, and its source repository URL. Behind both the JSON
/// `PATCH /app/api/projects/{id}` command and the `/app/projects/{id}` edit
/// form, so neither door re-implements the write. Name is required; a submitted
/// `entity_id` or `description` is applied and an omitted one is left
/// untouched, with a blank description clearing the column.
///
/// `repository_url` is the one Project field that is validated rather than
/// merely trimmed ([`is_valid_repository_url`]): it is handed to `git clone`
/// and rendered as a link, so a bad scheme or an embedded credential is
/// refused here rather than stored.
///
/// Scope is deliberately narrow. It is not the matter-open path (no conflict
/// check), it does not move the agreed price (the append-only price-events
/// command), and it does not change `status`/`closed_at` — a lifecycle
/// transition whose retention semantics are a firm-policy determination owned
/// by [`transition_project`].
///
/// Because every written value comes wholly from the request (never from a
/// read of the row), there is no read-modify-write to serialize: the sparse
/// `Unchanged(id)` update leaves `status`, `closed_at`, and every other
/// column a concurrent lifecycle write may be changing entirely untouched.
pub async fn update_project(
    surreal: &SurrealDb,
    id: Uuid,
    input: &UpdateProjectCommand,
) -> Result<Project, ProjectCommandError> {
    if let Some(name) = &input.name {
        if name.trim().is_empty() {
            return Err(ProjectCommandError::Invalid("Name is required."));
        }
    }
    // A blank submission clears the column; anything else must be a URL that
    // could actually be cloned and safely rendered as a link.
    if let Some(url) = &input.repository_url {
        if !url.trim().is_empty() && !is_valid_repository_url(url) {
            return Err(ProjectCommandError::Invalid(REPOSITORY_URL_INVALID));
        }
    }
    validate_resource_links(input)?;
    if find_by_id(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_none()
    {
        return Err(ProjectCommandError::NotFound);
    }
    if let Some(entity_id) = input.entity_id {
        // A `record<entity>` link is not validated by the engine, so this
        // read-back is what keeps a matter from being repointed at an
        // entity that does not exist.
        if crate::entities::find_by_id(surreal, entity_id)
            .await
            .map_err(|error| ProjectCommandError::Db(error.to_string()))?
            .is_none()
        {
            return Err(ProjectCommandError::Invalid("That entity does not exist."));
        }
    }
    // The optional text columns are handled from one table, so a column is set
    // and bound from the same row rather than from two `if` blocks a hundred
    // lines apart that have to be kept in step.
    let text_columns = optional_text_columns(input);
    let mut assignments = vec!["updated_at = $updated_at".to_string()];
    if input.name.is_some() {
        assignments.push("name = $name".to_string());
    }
    if input.entity_id.is_some() {
        assignments.push("entity_id = $entity_id".to_string());
    }
    for (column, value) in text_columns {
        if value.is_some() {
            assignments.push(format!("{column} = ${column}"));
        }
    }
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET {} RETURN {PROJECT_SELECT}",
            assignments.join(", ")
        ))
        .bind(("id", record_id(PROJECT_TABLE, id)))
        .bind((
            "name",
            input
                .name
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string(),
        ))
        .bind(("updated_at", chrono::Utc::now().to_rfc3339()));
    if let Some(entity_id) = input.entity_id {
        response = response.bind(("entity_id", record_id(ENTITY_TABLE, entity_id)));
    }
    for (column, value) in text_columns {
        if let Some(value) = value {
            // A blank submission clears the column rather than storing "".
            response = response.bind((column, crate::people_commands::none_if_blank(Some(value))));
        }
    }
    let mut response = response
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    let updated: Option<ProjectRow> = response
        .take(0)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    updated
        .and_then(ProjectRow::into_project)
        .ok_or(ProjectCommandError::NotFound)
}

/// A reference probe could not be answered. The matter is left alone: a
/// guard that cannot read is not a guard that passed.
fn probe_failed(error: impl std::fmt::Display) -> ProjectCommandError {
    ProjectCommandError::Db(error.to_string())
}

/// Delete a matter after validating the records in the projects cluster.
/// Nothing in the engine refuses a delete that strands a reference, so the
/// check belongs at this command seam.
pub async fn delete_project_with_surreal(
    surreal: &SurrealDb,
    id: Uuid,
) -> Result<Project, ProjectCommandError> {
    let project = find_by_id(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .ok_or(ProjectCommandError::NotFound)?;
    if !participations_for_project(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_empty()
    {
        return Err(ProjectCommandError::Referenced(
            "person_project_roles".into(),
        ));
    }
    // `notations` is Surreal-resident, so its reference check runs against
    // that engine.
    if crate::notations::exists_for_project(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
    {
        return Err(ProjectCommandError::Referenced("notations".into()));
    }
    if !crate::xero_invoices::for_projects(surreal, &[id])
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?
        .is_empty()
    {
        return Err(ProjectCommandError::Referenced("xero_invoice".into()));
    }
    // The remaining five reference checks, each asked of the module that
    // owns the table. All five moved to SurrealDB — `templates` and
    // `assets` with ENG-121, `communications`, `expunge_requests`, and
    // `expunge_records` with ENG-160 — so each answer comes from the engine
    // that actually holds the rows.
    if crate::templates::exists_for_project(surreal, id)
        .await
        .map_err(probe_failed)?
    {
        return Err(ProjectCommandError::Referenced("templates".into()));
    }
    if crate::expunge_requests::exists_for_project(surreal, id)
        .await
        .map_err(probe_failed)?
    {
        return Err(ProjectCommandError::Referenced("expunge_requests".into()));
    }
    if crate::communications::exists_for_project(surreal, id)
        .await
        .map_err(probe_failed)?
    {
        return Err(ProjectCommandError::Referenced("communications".into()));
    }
    if crate::expunge_records::exists_for_project(surreal, id)
        .await
        .map_err(probe_failed)?
    {
        return Err(ProjectCommandError::Referenced("expunge_records".into()));
    }
    if crate::assets::exists_for_project(surreal, id)
        .await
        .map_err(probe_failed)?
    {
        return Err(ProjectCommandError::Referenced("assets".into()));
    }
    // Surreal cascades nothing, so the
    // `authority_uses`/`citations`/`verifications` chain is walked
    // explicitly before the project row itself is removed below.
    crate::authorities::delete_for_project(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    // `cases` and its discovery chain moved to SurrealDB with wave six
    // (ENG-160); the cascade walks that chain explicitly, the same way the
    // `authorities` sweep above does.
    crate::cases::delete_for_project(surreal, id)
        .await
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    surreal
        .query("DELETE $id")
        .bind(("id", record_id(PROJECT_TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| ProjectCommandError::Db(error.to_string()))?;
    Ok(project)
}

/// Everything an adapter has resolved from its caller's session and form to
/// open a matter. The session-dependent parts — which client, which entity,
/// and *who is attesting* — are resolved by the adapter (web
/// form, CLI, or `POST /app/api/projects`) and handed here as ids, so the command
/// itself stays free of session and HTTP types.
#[derive(Debug)]
pub struct OpenMatterCommand {
    /// The matter's display name. Required.
    pub name: String,
    /// The matter's code, stored exactly as supplied (after normalizing case
    /// and whitespace — see [`normalize_code`]). This *is* the stored code:
    /// [`open_matter`] does not generate or append anything to it. A code is
    /// chosen once and never changes — `project.code` is `READONLY` (see
    /// [`docs/glossary.md#project`](../../docs/glossary.md#project)) — and it
    /// is a coordinate shared with three systems Navigator does not own (a
    /// repository's `navigator.yaml`, the matter's Drive folder name, and its
    /// Notion `Project code` URL), so the caller — not Navigator — must own
    /// picking it. A collision with an already-open matter is refused with
    /// [`OpenMatterError::CodeConflict`] rather than silently resolved. The
    /// code must still pass [`is_valid_code`]: lowercase letters, digits, and
    /// single hyphens, alphanumeric at both ends, and not a reserved word.
    pub code: String,
    /// The client of record — a pre-existing `Role::Client` person, never a
    /// firm attorney (the firm-as-its-own-client default is a loyalty problem
    /// both councils flagged).
    pub client_id: Uuid,
    /// The pre-existing entity the matter opens against (`projects.entity_id`
    /// is NOT NULL).
    pub entity_id: Uuid,
    /// The matter's scope narrative.
    pub description: Option<String>,
    /// Which house brand's storefront this matter was opened through — see
    /// [`Project::brand`]. The caller resolves this from the request's
    /// `Host:` header (or, for a door with no request — the CLI, an MCP
    /// tool — the firm's own default brand); it is never read from form or
    /// JSON input the caller deserialized, so a spoofed field cannot reach
    /// here.
    pub brand: String,
    /// The opening attorney's conflict attestation. Required on **every** open
    /// — at this firm `lawyer` is an attorney, so a lawyer/admin session opening
    /// a Project is an attorney attesting they have checked for conflicts, and
    /// that either none prevent the open or the Project is not legal advice.
    /// A missing attestation is refused; it is never defaulted true.
    pub attestation: bool,
    /// The attesting attorney — the opening `lawyer` (=attorney) person, who
    /// becomes the matter's accountable lawyer DRI and the actor on the
    /// attestation audit row.
    pub acting_person_id: Uuid,
}

/// A matter could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenMatterError {
    /// The request is malformed in a way the caller can correct.
    #[error("{0}")]
    Invalid(&'static str),
    /// The named client of record is not a `Role::Client` person — a matter
    /// cannot be opened for a firm attorney as its own client.
    #[error("the client of record must be a client, not a firm attorney")]
    ClientNotAllowed,
    /// The attester (`acting_person_id`) is not a firm lawyer. At this firm the
    /// `lawyer`/`admin` tier is the attorney, and the attester becomes the
    /// accountable lawyer DRI, so a `client` or `clerk` can never attest or hold
    /// that DRI.
    #[error("the attesting attorney must be a firm lawyer")]
    AttesterNotAllowed,
    /// A referenced row (client, entity, or attester) does not exist.
    /// Carries which one so the adapter can point the caller at it.
    #[error("no such {0}")]
    NotFound(&'static str),
    /// No conflict attestation was supplied. Every Project open requires the
    /// opening attorney to attest; there is no default.
    #[error("a matter open requires the attorney's conflict attestation")]
    AttestationRequired,
    /// The conflict check found the matter adverse to a current client. This
    /// is a hard stop no attestation overrides; a waiver is a separate flow.
    #[error("conflict check blocked this matter: adverse to a current client")]
    BlockingConflict(Vec<String>),
    /// The supplied project code is already in use by another matter. The
    /// caller picks a different code; Navigator never resolves this itself.
    #[error("that project code is already in use")]
    CodeConflict,
    /// Appending the conflict attestation to the Relationship Log
    /// failed. The matter is already open at that point — the attestation
    /// is the last step — so this reports a matter whose audit entry is
    /// missing rather than a matter that did not open.
    #[error("record the conflict attestation: {0}")]
    Attestation(crate::relationship_logs::RelationshipLogError),
    /// A database failure, carrying the engine's own message.
    #[error("database: {0}")]
    Db(String),
}

/// Open a new matter. The single write behind the lawyer `POST /app/projects`
/// form, the CLI `project create`, and the JSON `POST /app/api/projects` command,
/// so every door runs the same conflict check, the same required attestation,
/// and writes the same audit trail — no door can open a matter without them.
///
/// The order is load-bearing. Cheap caller-correctable input checks run first;
/// then **one** `SERIALIZABLE` transaction reads every reference, runs the
/// conflict check, and writes the project, the attestation audit row, both DRI
/// designations, and the repository — so a failure at any step rolls the whole
/// open back and no read the open acted on can have changed underneath it.
///
/// The attestation is required on **every** open and audited on **every**
/// open, not just flagged ones (see [`OpenMatterCommand::attestation`]). The
/// conflict findings, if any, are recorded in the audit `detail` alongside the
/// attestation, so the durable record shows what the attorney attested over.
/// Confirm the matter-open references resolve before any write: the client of
/// record exists and is a `Role::Client` (never a firm attorney), and the
/// entity, and attester exist. A missing reference names which one so
/// the adapter can point the caller at it.
///
/// Read one person's row for the two `role` checks the open depends on.
///
/// # This takes no lock, and that is a narrowing on the record
///
/// This is a plain read, so it cannot hold the row against a concurrent
/// admin `UPDATE` demoting the attester between the lawyer-tier check below
/// and the attestation write (navigator#790).
///
/// What holds the invariant is the door: a session carries the role it
/// authenticated with, and a role change forces re-authentication, so a
/// demoted attester cannot drive a *new* open. The residual window is one
/// already-in-flight open by a person demoted mid-request. Nick accepted
/// that narrowing on 2026-08-02.
async fn lock_person(
    surreal: &SurrealDb,
    person_id: Uuid,
) -> Result<Option<crate::persons::Person>, OpenMatterError> {
    crate::persons::find_by_id(surreal, person_id)
        .await
        .map_err(|err| person_lookup_failed(&err))
}

/// A failed person lookup, reported as the `String` this command already
/// carries.
fn person_lookup_failed(err: &crate::persons::PersonError) -> OpenMatterError {
    OpenMatterError::Db(format!("resolve a matter-open reference: {err}"))
}

async fn validate_open_references(
    surreal: &SurrealDb,
    input: &OpenMatterCommand,
) -> Result<(), OpenMatterError> {
    let client = lock_person(surreal, input.client_id)
        .await?
        .ok_or(OpenMatterError::NotFound("client"))?;
    if client.role != Role::Client {
        return Err(OpenMatterError::ClientNotAllowed);
    }
    // The engine does not validate a `record<entity>` link, so the matter
    // a client opens against is read back rather than trusted.
    if crate::entities::find_by_id(surreal, input.entity_id)
        .await
        .map_err(|error| OpenMatterError::Db(error.to_string()))?
        .is_none()
    {
        return Err(OpenMatterError::NotFound("entity"));
    }
    let attester = lock_person(surreal, input.acting_person_id)
        .await?
        .ok_or(OpenMatterError::NotFound("attester"))?;
    // The attester becomes the accountable lawyer DRI and the actor on the
    // attestation audit row, so the shared command enforces the "lawyer is the
    // attorney" invariant here rather than trusting each adapter's own session
    // gate — a `client` or `clerk` can never be recorded as the attesting
    // attorney, whichever door (web form, CLI, or API) drove the open.
    if !attester.role.is_lawyer_tier() {
        return Err(OpenMatterError::AttesterNotAllowed);
    }
    Ok(())
}

pub async fn open_matter(
    surreal: &SurrealDb,
    input: &OpenMatterCommand,
) -> Result<Project, OpenMatterError> {
    // The attorney's attestation gates every open, checked first: no amount of
    // valid form data opens a Project the attorney hasn't attested to.
    if !input.attestation {
        return Err(OpenMatterError::AttestationRequired);
    }
    let name = input.name.trim();
    if name.is_empty() {
        return Err(OpenMatterError::Invalid("Name is required."));
    }
    let project_id = Uuid::now_v7();
    // The code is required, not blank, and is stored exactly as supplied —
    // normalized for case and whitespace, never generated or appended to. It
    // is a coordinate the caller already committed to in a repository's
    // `navigator.yaml`, a Drive folder name, or a Notion URL, so Navigator
    // storing anything else would be unfixable once `project.code` (READONLY)
    // is written. A collision is refused below as `CodeConflict`, not
    // resolved by generation.
    let code = normalize_code(&input.code).ok_or(OpenMatterError::Invalid("Code is required."))?;
    if !is_valid_code(&code) {
        return Err(OpenMatterError::Invalid(
            "Project code must use lowercase letters, digits, and single hyphens; \
             it must start and end with a letter or digit.",
        ));
    }

    // Validate every reference before opening. The project and both
    // participations commit in the explicit SurrealDB transaction below;
    // transaction conflicts retry with jitter. `lock_person` remains the
    // accepted narrowing.
    validate_open_references(surreal, input).await?;

    // Conflict check, before any write. The relationship graph is advisory to
    // clear but authoritative to block: a confident adverse link to a current
    // client hard-stops the open, and no attestation overrides it (a waiver is
    // a separate, heavier flow). A clear or softly-flagged check proceeds on
    // the attorney's attestation, which the audit row below records.
    let conflict = crate::conflicts::check_new_matter(surreal, input.client_id, input.entity_id)
        .await
        .map_err(OpenMatterError::Db)?;
    if conflict.has_blocking() {
        return Err(OpenMatterError::BlockingConflict(conflict.summary_lines()));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let description = crate::people_commands::none_if_blank(input.description.as_deref());
    let mut response = writing_project(|| {
        surreal
            .query(format!(
                r"BEGIN;
                 CREATE $project SET code = $code, name = $name, status = 'open',
                    brand = $brand,
                    entity_id = $entity_id, description = $description,
                    inserted_at = $now, updated_at = $now RETURN {PROJECT_SELECT};
                 CREATE $lawyer_role SET person_id = $attester, project_id = $project, participation = 'attorney',
                    is_lawyer_dri = true, inserted_at = $now, updated_at = $now;
                 CREATE $client_role SET person_id = $client, project_id = $project, participation = 'client',
                    is_client_dri = true, inserted_at = $now, updated_at = $now;
                 COMMIT;"
            ))
            .bind(("project", record_id(PROJECT_TABLE, project_id)))
            .bind(("lawyer_role", record_id(PERSON_PROJECT_ROLE_TABLE, Uuid::now_v7())))
            .bind(("client_role", record_id(PERSON_PROJECT_ROLE_TABLE, Uuid::now_v7())))
            .bind(("code", code.clone()))
            .bind(("name", name.to_string()))
            .bind(("brand", input.brand.clone()))
            .bind(("entity_id", record_id(ENTITY_TABLE, input.entity_id)))
            .bind(("description", description.clone()))
            .bind(("attester", record_id("person", input.acting_person_id)))
            .bind(("client", record_id("person", input.client_id)))
            .bind(("now", now.clone()))
    })
    .await
    .map_err(|error| match error {
        ProjectStoreError::CodeTaken => OpenMatterError::CodeConflict,
        other => OpenMatterError::Db(other.to_string()),
    })?;
    let created: Option<ProjectRow> = response
        .take(1)
        .map_err(|error| OpenMatterError::Db(error.to_string()))?;
    let created = created
        .and_then(ProjectRow::into_project)
        .ok_or_else(|| OpenMatterError::Db("opening a matter returned no project".into()))?;

    // The attestation audit row — written on every open, clear or flagged. It
    // names who attested and, in the detail, what the conflict check found, so
    // the durable record shows exactly what the attorney attested over.
    let detail = if conflict.is_clear() {
        "Conflict attestation at matter open. No conflicts found.".to_string()
    } else {
        format!(
            "Conflict attestation at matter open over these findings:\n{}",
            conflict.summary_lines().join("\n"),
        )
    };
    // The attestation and the matter it attests to land in one engine, so
    // an opened matter cannot exist without the record that the conflict
    // check was run and cleared — the record the firm's `@cleared`
    // discipline rests on.
    crate::relationship_logs::record(
        surreal,
        &crate::relationship_logs::NewRelationshipLog {
            actor_person_id: Some(input.acting_person_id),
            subject_type: "project".to_string(),
            subject_id: project_id,
            action: "conflict_attestation".to_string(),
            detail,
        },
    )
    .await
    .map_err(OpenMatterError::Attestation)?;

    Ok(created)
}

/// Whether a template's declared `kind` makes its notation the engagement that
/// opens a matter — the same classifier the engagement-first gate uses. Keyed
/// off the declared kind, never the template `code`, so a retainer template
/// named otherwise still counts as the engagement.
#[must_use]
pub fn template_opens_a_matter(kind: Option<&str>) -> bool {
    kind.and_then(rules::kind::Kind::parse)
        .is_some_and(rules::kind::Kind::opens_a_matter)
}

/// Whether a template's declared `kind` makes its notation the letter that
/// closes a matter — the mirror of [`template_opens_a_matter`]. Keyed off the
/// declared kind, never the template `code`, so a bespoke offboarding letter
/// named otherwise still counts as the closing letter.
#[must_use]
pub fn template_closes_a_matter(kind: Option<&str>) -> bool {
    kind.and_then(rules::kind::Kind::parse)
        .is_some_and(rules::kind::Kind::closes_a_matter)
}

/// From a matter's onboarding/offboarding facts, the two lifecycle warning
/// flags: `missing_onboarding` (no matter-opening engagement on file) and
/// `missing_offboarding_letter` (a `closed` matter with no offboarding letter
/// on file).
#[must_use]
pub fn matter_flags(has_engagement: bool, status: &str, has_closing: bool) -> (bool, bool) {
    let missing_onboarding = !has_engagement;
    let missing_offboarding_letter = status == "closed" && !has_closing;
    (missing_onboarding, missing_offboarding_letter)
}

/// The lawyer-facing traffic-light summary of a matter's lifecycle: the one
/// state a reader scans for on the Projects list, distinct from the finer
/// diligence badges [`matter_flags`] computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatterLifecycle {
    /// Open, with no onboarding artifact on file — including a matter whose
    /// onboarding walk was started and abandoned before it produced one.
    /// That walk is deliberately *not* a fourth state: for the one question
    /// this indicator asks — is the matter papered? — the answer on an
    /// abandoned walk is no, and the row belongs in the queue that gets
    /// chased.
    NeedsOnboarding,
    /// Open, with an onboarding artifact on file: a classified document, or a
    /// walk that produced its instruments.
    ///
    /// Presence, not execution: [`matter_lifecycle_sets`] matches an artifact
    /// by its declared kind and reads no signature state, so this state says
    /// the onboarding paperwork is *filed*, never that it was executed. The
    /// label and title say so too — see [`MatterLifecycle::label`].
    OnboardingOnFile,
    /// Closed. A closed matter is always this variant, whether or not it is
    /// missing its offboarding letter — that finer-grained gap stays the
    /// separate `missing_offboarding_letter` badge's job, not a fourth colour.
    Closed,
}

impl MatterLifecycle {
    /// The CSS class this state renders as. `matter-lifecycle` carries the
    /// shared pill shape; the modifier carries the colour.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            MatterLifecycle::NeedsOnboarding => "matter-lifecycle matter-lifecycle--yellow",
            MatterLifecycle::OnboardingOnFile => "matter-lifecycle matter-lifecycle--green",
            MatterLifecycle::Closed => "matter-lifecycle matter-lifecycle--red",
        }
    }

    /// The visible text label — colour is never the only signal, so this
    /// (not just the class) is what a colour-blind or screen-reader reader
    /// gets.
    ///
    /// The green state reads "active": short, and the word a lawyer scanning
    /// the list actually wants — whether papering is *filed*, not merely
    /// executed, stays [`MatterLifecycle::title`]'s job, spelled out in full
    /// on hover rather than carried by the pill's own word.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            // "Pitch" is the firm's own term for this stage: the matter is
            // open but not yet papered, which is the state a prospective
            // client is in before they sign — the word a lawyer already uses
            // for it, and shorter than the description it stands in for.
            MatterLifecycle::NeedsOnboarding => "pitch",
            MatterLifecycle::OnboardingOnFile => "active",
            MatterLifecycle::Closed => "closed",
        }
    }

    /// The hover/assistive title, one per state. Each spells out what the
    /// indicator did and did not verify, so the short pill label never has to
    /// carry the caveat alone.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            MatterLifecycle::NeedsOnboarding => {
                "No onboarding document is on file for this matter. An onboarding walk that \
                 was started but produced no document reads this way too."
            }
            MatterLifecycle::OnboardingOnFile => {
                "An onboarding document is on file for this matter: filed, not verified as \
                 executed."
            }
            MatterLifecycle::Closed => "This matter is closed.",
        }
    }
}

/// The lifecycle state a matter row renders, from its `status` and whether
/// it is missing its onboarding letter. `missing_offboarding_letter` is taken
/// but deliberately does not branch the result: a closed matter is
/// [`MatterLifecycle::Closed`] whether or not it still owes its offboarding
/// letter, and that gap keeps surfacing through the existing "no offboarding
/// letter" badge alongside this indicator, not folded into a fourth colour.
///
/// Deliberately an exhaustive match on `(status == "closed", missing_onboarding)`
/// so the offboarding-letter parameter's non-effect on the outcome is visible at
/// the call site, not just asserted in a doc comment.
#[must_use]
pub fn matter_lifecycle(
    status: &str,
    missing_onboarding: bool,
    missing_offboarding_letter: bool,
) -> MatterLifecycle {
    match (
        status == "closed",
        missing_onboarding,
        missing_offboarding_letter,
    ) {
        (true, _, _) => MatterLifecycle::Closed,
        (false, true, _) => MatterLifecycle::NeedsOnboarding,
        (false, false, _) => MatterLifecycle::OnboardingOnFile,
    }
}

/// For the given matters, return `(project_ids with a matter-opening engagement,
/// project_ids with a matter-closing offboarding letter)` in four batched
/// queries (notations for these projects, the templates they bind, the drafted
/// instruments those notations produced, and the assets filed on these
/// projects).
///
/// **An artifact clears a side; a walk never does.** A matter clears either by
/// an `assets` row carrying a classifying `kind`, or by a notation whose
/// template classifies that way *and* which actually produced a drafted
/// instrument. Both keys go through [`template_opens_a_matter`] and
/// [`template_closes_a_matter`] — never a template's or asset's `code` — so a
/// bespoke engagement or closing letter named otherwise still counts.
///
/// # Why a notation's existence is not evidence
///
/// A notation row means somebody *opened* a questionnaire walk on the matter.
/// It does not mean the walk produced anything: a lawyer who opens an
/// onboarding walk and abandons it at `BEGIN` has generated no document,
/// signed nothing, and filed nothing. Clearing the flag on that row reported a
/// never-papered matter as papered — a silent false positive, invisible
/// precisely because the document that would contradict it does not exist.
///
/// A notation's `state` is *not* the fix, and this is the subtle part. The
/// state records where the walk is standing, never what it has produced, and
/// the retainer walk's own "honest" state is entered *before* its artifact
/// exists: it parks *at* `generate_pdf__*` and the worker renders the PDF
/// afterwards. So this reads the artifacts themselves.
///
/// # The two artifact lanes
///
/// A walk that reaches document generation files its rendered PDF into the
/// **asset lane** under the pinned template's declared `kind`
/// (`workflows::document::dispatch_generate_pdf`), so the asset query already
/// sees every generated engagement and offboarding letter at no extra cost.
/// A walk that drafts its instruments instead files into
/// **`review_documents`**, which the asset lane cannot see. That lane is
/// keyed off the *notation's* opening or closing kind, never the draft row's
/// own kind, because the drafts of an onboarding walk are the instruments it
/// opens the matter with, not copies of it.
///
/// **This answers "is it on file", not "is it executed".** An asset row
/// carries no signature state, and an upload commonly arrives from DocuSign
/// already signed — inventing a signature marker would assert a fact
/// Navigator does not have. A lawyer classifying an upload as `onboarding` or
/// `offboarding` is the firm's own assertion that the document does that job,
/// and this flag trusts that assertion. The trade-off is deliberate: a lawyer
/// who files a draft under the wrong kind clears the badge on a matter with
/// no executed letter — a misclassification by a licensed professional on a
/// firm-side worklist, not a hole in the rule, and the document is right
/// there on the matter to inspect and correct.
///
/// Where the evidence is thin the flag stays *set*: an under-claiming badge
/// costs a lawyer one look at a matter that is fine, while an over-claiming
/// one leaves an unpapered representation off the worklist entirely.
///
/// `notations`, `templates`, `review_documents`, and `assets` are all
/// Surreal-resident since ENG-121, but this stays four batched queries either
/// way — one round trip per table, not one round trip per Project.
///
/// Errors propagate rather than collapsing to an empty set: an empty set is
/// indistinguishable from "this matter has no engagement", so swallowing a
/// failed query would badge every matter as missing the retainer it actually
/// has — a flag that lies about a legal artifact is worse than a page that
/// admits it could not tell.
pub async fn matter_lifecycle_sets(
    surreal: &crate::surreal::SurrealDb,
    projects: &[Project],
) -> Result<
    (
        std::collections::HashSet<Uuid>,
        std::collections::HashSet<Uuid>,
    ),
    String,
> {
    use std::collections::{HashMap, HashSet};
    let project_ids: Vec<Uuid> = projects.iter().map(|p| p.id).collect();
    let mut has_engagement = HashSet::new();
    let mut has_closing = HashSet::new();
    if project_ids.is_empty() {
        return Ok((has_engagement, has_closing));
    }
    let notations = crate::notations::list_by_projects(surreal, &project_ids)
        .await
        .map_err(|error| error.to_string())?;
    let template_ids: Vec<Uuid> = notations.iter().map(|n| n.template_id).collect();
    let templates_by_id: HashMap<Uuid, crate::templates::Template> =
        crate::templates::find_by_ids(surreal, &template_ids)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
    // Only the notations whose template classifies as opening or closing a
    // matter can ever clear a flag, so only those need the drafts probe.
    let lifecycle_notations: Vec<&crate::notations::Notation> = notations
        .iter()
        .filter(|n| {
            templates_by_id.get(&n.template_id).is_some_and(|t| {
                template_opens_a_matter(t.kind.as_deref())
                    || template_closes_a_matter(t.kind.as_deref())
            })
        })
        .collect();
    let drafted = crate::review_documents::notations_with_drafts(
        surreal,
        &lifecycle_notations
            .iter()
            .map(|n| n.id)
            .collect::<Vec<Uuid>>(),
    )
    .await
    .map_err(|error| error.to_string())?;
    for n in lifecycle_notations {
        // The walk produced an instrument, so the notation is evidence. A
        // walk with no drafts is only evidence that somebody started.
        if !drafted.contains(&n.id) {
            continue;
        }
        if let Some(t) = templates_by_id.get(&n.template_id) {
            if template_opens_a_matter(t.kind.as_deref()) {
                has_engagement.insert(n.project_id);
            }
            if template_closes_a_matter(t.kind.as_deref()) {
                has_closing.insert(n.project_id);
            }
        }
    }
    let asset_kinds = crate::assets::kinds_by_projects(surreal, &project_ids)
        .await
        .map_err(|error| error.to_string())?;
    for (project_id, kind) in &asset_kinds {
        if template_opens_a_matter(kind.as_deref()) {
            has_engagement.insert(*project_id);
        }
        if template_closes_a_matter(kind.as_deref()) {
            has_closing.insert(*project_id);
        }
    }
    Ok((has_engagement, has_closing))
}

/// From each matter's `(brand, status, missing_onboarding)`, how many
/// `"neon"`-brand matters are open (`status != "closed"`), and how many of
/// those are still pitches — the [`MatterLifecycle::NeedsOnboarding`]
/// subset, no onboarding artifact on file yet. Scoped to the firm's own root
/// brand: never another house brand, and never another firm's matters on a
/// licensed fork.
///
/// Pure and exposed so the nightly `DriDigest` follow-up post's counts are
/// unit-tested without a database — see [`matter_open_pitch_counts`] for the
/// database-backed caller that supplies `missing_onboarding` from
/// [`matter_lifecycle_sets`].
#[must_use]
pub fn open_pitch_counts(rows: &[(&str, &str, bool)]) -> (usize, usize) {
    rows.iter().fold(
        (0, 0),
        |(open, pitches), (brand, status, missing_onboarding)| {
            if *brand != "neon" || *status == "closed" {
                return (open, pitches);
            }
            (open + 1, pitches + usize::from(*missing_onboarding))
        },
    )
}

/// Every `"neon"`-brand matter's open/pitch counts, for the nightly
/// `DriDigest` follow-up post. Reads every project, then
/// [`matter_lifecycle_sets`] for the onboarding-artifact facts
/// [`open_pitch_counts`] needs — the same two-query shape
/// `webapp::project_list::get_project_list` already uses to badge the
/// lawyer matter list.
pub async fn matter_open_pitch_counts(
    surreal: &SurrealDb,
) -> Result<(usize, usize), ProjectStoreError> {
    let projects = all(surreal).await?;
    let (has_engagement, _has_closing) = matter_lifecycle_sets(surreal, &projects)
        .await
        .map_err(ProjectStoreError::Lifecycle)?;
    let rows: Vec<(&str, &str, bool)> = projects
        .iter()
        .map(|project| {
            (
                project.brand.as_str(),
                project.status.as_str(),
                !has_engagement.contains(&project.id),
            )
        })
        .collect();
    Ok(open_pitch_counts(&rows))
}

#[cfg(test)]
mod surreal_read_tests {
    use super::{
        can_access_as_client_in_surreal, can_access_as_lawyer_in_surreal, classify_project_write,
        create, designate_dri_in_surreal, dri_digest, find_by_id, matter_directory,
        matter_directory_for, open_pitch_counts, record_id, DriSide, NewProject, ProjectStoreError,
        ENTITY_TABLE,
    };
    use crate::persons::Role;
    use crate::schema::apply;
    use crate::surreal::test_support::unmigrated;
    use crate::test_support::mem_surreal;

    #[tokio::test]
    async fn find_by_id_reads_the_projects_cluster_row() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let id = uuid::Uuid::now_v7();
        let entity_id = uuid::Uuid::now_v7();
        db.query(
            "CREATE $id SET code = 'matter', name = 'Matter', status = 'open', \
             entity_id = $entity_id, inserted_at = '2026-08-04T00:00:00Z', \
             updated_at = '2026-08-04T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .unwrap()
        .check()
        .unwrap();

        let project = find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(project.id, id);
        assert_eq!(project.entity_id, entity_id);
        assert_eq!(project.code, "matter");
    }

    /// `brand` round-trips through `create` for every key the registry
    /// serves today, and a row written with no explicit `brand` (`create`
    /// always writes one via `NewProject::default()`, but a caller may pass
    /// through that default) reads back as `"neon"` — see
    /// [`reads_a_project_row_written_before_brand_was_defined`] for the
    /// separate case of a row that predates the field entirely.
    #[tokio::test]
    async fn create_round_trips_brand_for_every_registered_key() {
        let db = mem_surreal().await;
        for (label, brand) in [
            ("neon", "neon"),
            ("delete-your-data", "delete-your-data"),
            ("default", "neon"),
        ] {
            let entity_id = crate::test_support::seed_entity(&db).await;
            let input = if label == "default" {
                NewProject {
                    code: format!("brand-{label}"),
                    name: format!("Brand {label}"),
                    status: "open".to_string(),
                    entity_id,
                    ..Default::default()
                }
            } else {
                NewProject {
                    code: format!("brand-{label}"),
                    name: format!("Brand {label}"),
                    status: "open".to_string(),
                    brand: brand.to_string(),
                    entity_id,
                    description: None,
                    ..Default::default()
                }
            };
            let created = create(&db, &input).await.unwrap();
            assert_eq!(created.brand, brand, "{label}: create response");
            let reloaded = find_by_id(&db, created.id).await.unwrap().unwrap();
            assert_eq!(reloaded.brand, brand, "{label}: reread from the store");
        }
    }

    /// The faithful reproduction of a historical row, mirroring
    /// `persons::reads_a_person_row_written_before_email_confirmed_was_defined`:
    /// drop the definition, write the row, put the definition back. `DEFAULT`
    /// is a write-time default and does not reach this row's absent value, so
    /// a bare `String` on `ProjectRow` would fail here with `Expected string,
    /// got none` — on the same seeding-boot path `email_confirmed` crashed
    /// staging in #331 — which is why `ProjectRow.brand` is `Option<String>`
    /// and [`ProjectRow::into_project`] collapses the absent case to `"neon"`.
    #[tokio::test]
    async fn reads_a_project_row_written_before_brand_was_defined() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        db.query("REMOVE FIELD brand ON project").await.unwrap();
        let id = uuid::Uuid::now_v7();
        let entity_id = uuid::Uuid::now_v7();
        db.query(
            "CREATE $id SET code = 'pre-brand-matter', name = 'Pre-Brand Matter', \
             status = 'open', entity_id = $entity_id, \
             inserted_at = '2026-08-25T00:00:00Z', updated_at = '2026-08-25T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(
            "DEFINE FIELD OVERWRITE brand ON project TYPE string \
             ASSERT $value IN ['neon', 'delete-your-data'] DEFAULT 'neon'",
        )
        .await
        .unwrap();

        let project = find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(
            project.brand, "neon",
            "an absent value reads as the default the schema would have written"
        );
    }

    /// The write-side counterpart to
    /// [`reads_a_project_row_written_before_brand_was_defined`]: a project
    /// row that predates `brand` reads fine (the tolerant `Option<String>`
    /// collapse), but any later partial `UPDATE` against that same row makes
    /// SurrealDB re-validate the whole record against the schema, including
    /// the untouched `brand` field, whose stored value is genuinely absent
    /// rather than defaulted. `designate_dri_in_surreal` is exactly such an
    /// update — it only sets `updated_at` on the Project row — and this is
    /// the shape of the coercion error a live DRI assignment hit against a
    /// pre-brand Project.
    #[tokio::test]
    async fn designating_a_dri_on_a_project_written_before_brand_was_defined() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        db.query("REMOVE FIELD brand ON project").await.unwrap();
        let project_id = uuid::Uuid::now_v7();
        let entity_id = uuid::Uuid::now_v7();
        db.query(
            "CREATE $id SET code = 'pre-brand-dri', name = 'Pre-Brand DRI Matter', \
             status = 'open', entity_id = $entity_id, \
             inserted_at = '2026-08-25T00:00:00Z', updated_at = '2026-08-25T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", project_id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .unwrap()
        .check()
        .unwrap();
        // Re-apply the shipped schema, exactly as a boot would: the
        // Project's `brand` was never written, and re-applying converges the
        // field definition without touching the row.
        apply(&db).await.unwrap();

        let lawyer = crate::persons::create(
            &db,
            &crate::persons::NewPerson::with_role(
                "Pre-Brand DRI Lawyer",
                "dri-lawyer@example.com",
                Role::Lawyer,
            ),
        )
        .await
        .unwrap();

        designate_dri_in_surreal(&db, project_id, lawyer.id, DriSide::Lawyer)
            .await
            .expect("designating a DRI must not fail on a project row that predates `brand`");
    }

    /// An empty portfolio counts nothing.
    #[test]
    fn open_pitch_counts_of_an_empty_slice_is_zero_and_zero() {
        assert_eq!(open_pitch_counts(&[]), (0, 0));
    }

    /// Every matter closed: none of them are open, so there is nothing left
    /// to call a pitch either.
    #[test]
    fn open_pitch_counts_with_every_matter_closed_is_zero_and_zero() {
        let rows = [("neon", "closed", true), ("neon", "closed", false)];
        assert_eq!(open_pitch_counts(&rows), (0, 0));
    }

    /// Only `"neon"`-brand rows count, whatever their status or onboarding
    /// state — a `"delete-your-data"` matter (or any other house brand) never
    /// moves either number.
    #[test]
    fn open_pitch_counts_ignores_every_brand_but_neon() {
        let rows = [
            ("neon", "open", true),  // open + pitch
            ("neon", "open", false), // open, not a pitch
            ("delete-your-data", "open", true),
            ("delete-your-data", "closed", false),
        ];
        assert_eq!(open_pitch_counts(&rows), (2, 1));
    }

    /// A repository URL may name any forge, in any organization.
    ///
    /// This is the whole point of storing a URL rather than composing one from
    /// a deployment-wide host: two Projects can legitimately live on different
    /// forges, so nothing here privileges one.
    #[test]
    fn a_repository_url_may_name_any_forge_and_any_organization() {
        for url in [
            "https://github.com/neon-law-source-code/navigator-sample-project",
            "https://gitlab.com/some-group/some-subgroup/a-project",
            "https://git.example.internal/an-org/a-project.git",
            // A self-hosted forge on a port, and a plain-http intranet remote.
            "https://forge.example:8443/an-org/a-project",
            "http://forge.internal/an-org/a-project",
        ] {
            assert!(
                super::is_valid_repository_url(url),
                "{url} must be accepted — any forge, any organization"
            );
        }
    }

    /// The shapes that are refused, and why each one matters.
    #[test]
    fn a_repository_url_refuses_unclonable_and_unsafe_values() {
        for (url, why) in [
            ("", "blank is not a URL"),
            ("   ", "whitespace is not a URL"),
            ("github.com/an-org/a-project", "no scheme"),
            ("ssh://git@forge.example/an-org/a", "ssh is not http(s)"),
            ("file:///etc/passwd", "file:// would read the serving host"),
            (
                "javascript:alert(1)//x/y",
                "a live-link scheme must never render",
            ),
            ("https://github.com", "a forge root is not a repository"),
            ("https://github.com/", "an empty path is not a repository"),
            (
                "https://user:token@forge.example/an-org/a",
                "an embedded credential must not enter a rendered, logged column",
            ),
            (
                "https://forge.example/an org/a",
                "whitespace cannot survive into a clone argument",
            ),
        ] {
            assert!(
                !super::is_valid_repository_url(url),
                "{url:?} must be refused: {why}"
            );
        }
    }

    /// `new` is well-formed and still refused, in Rust *and* in the engine.
    ///
    /// `/app/projects/new` is the matter-open form and `/app/projects/{code}/…`
    /// is a Project's own surface, so a Project coded `new` collides with a
    /// literal route. Refusing it in [`super::is_valid_code`] alone would leave
    /// a direct write able to create the colliding row, which is why the
    /// `ASSERT` on `project.code` carries the same refusal.
    #[tokio::test]
    async fn the_project_code_new_is_refused_in_rust_and_in_the_engine() {
        assert!(
            cloud::workspace::is_valid_slug("new"),
            "`new` is a well-formed slug — the shape check is not what rejects it"
        );
        assert!(!super::is_valid_code("new"));
        // Every other well-formed code still passes, so the refusal is one
        // code and not a widening.
        assert!(super::is_valid_code("new-matter"));
        assert!(super::is_valid_code("renew"));

        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let direct = db
            .query(
                "CREATE $id SET code = 'new', name = 'New', status = 'open', \
                 entity_id = $entity_id, inserted_at = '2026-08-11T00:00:00Z', \
                 updated_at = '2026-08-11T00:00:00Z'",
            )
            .bind((
                "id",
                crate::surreal::record_id("project", uuid::Uuid::now_v7()),
            ))
            .bind(("entity_id", record_id(ENTITY_TABLE, uuid::Uuid::now_v7())))
            .await
            .and_then(surrealdb::IndexedResults::check);
        assert!(
            direct.is_err(),
            "the engine must refuse a Project coded `new` written around is_valid_code"
        );
    }

    /// Immutability is structural: an `UPDATE` that rewrites `code` on an
    /// existing row is refused by the engine itself, not only by the absence
    /// of a handler that offers to change it (`UpdateProjectCommand` has no
    /// `code` field at all — this test is the direct write no handler stands
    /// between).
    #[tokio::test]
    async fn a_direct_write_cannot_change_an_existing_projects_code() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let id = uuid::Uuid::now_v7();
        let entity_id = uuid::Uuid::now_v7();
        db.query(
            "CREATE $id SET code = 'alpha-matter', name = 'Alpha', status = 'open', \
             entity_id = $entity_id, inserted_at = '2026-08-25T00:00:00Z', \
             updated_at = '2026-08-25T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, entity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .expect("seed the project row");

        let attempt = db
            .query("UPDATE $id SET code = 'beta-matter'")
            .bind(("id", crate::surreal::record_id("project", id)))
            .await
            .and_then(surrealdb::IndexedResults::check);

        let error = attempt.expect_err("the engine must refuse a direct write that changes `code`");
        assert!(
            matches!(
                classify_project_write(error),
                ProjectStoreError::CodeImmutable
            ),
            "the refusal should classify as CodeImmutable, pointing callers at the glossary rule"
        );
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_project_code() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let input = NewProject {
            code: "matter".into(),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: uuid::Uuid::now_v7(),
            ..NewProject::default()
        };

        let created = create(&db, &input).await.unwrap();
        assert_eq!(created.code, "matter");
        assert!(matches!(
            create(&db, &input).await,
            Err(ProjectStoreError::CodeTaken)
        ));
    }

    #[tokio::test]
    async fn lawyer_lens_excludes_client_side_participation() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let lawyer_id = uuid::Uuid::now_v7();
        let client_id = uuid::Uuid::now_v7();
        let project_id = uuid::Uuid::now_v7();
        for (id, email, role) in [
            (lawyer_id, "lawyer@example.com", "lawyer"),
            (client_id, "client@example.com", "client"),
        ] {
            db.query("CREATE $id SET name = $email, email = $email, role = $role")
                .bind(("id", crate::surreal::record_id("person", id)))
                .bind(("email", email.to_string()))
                .bind(("role", role.to_string()))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        db.query(
            "CREATE $id SET code = 'matter', name = 'Matter', status = 'open', \
             entity_id = $entity_id, inserted_at = '2026-08-04T00:00:00Z', \
             updated_at = '2026-08-04T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", project_id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, uuid::Uuid::now_v7())))
        .await
        .unwrap()
        .check()
        .unwrap();
        for (person_id, participation) in [(lawyer_id, "attorney"), (client_id, "client")] {
            db.query(
                "CREATE $id SET person_id = $person_id, project_id = $project_id, \
                 participation = $participation, inserted_at = '2026-08-04T00:00:00Z', \
                 updated_at = '2026-08-04T00:00:00Z'",
            )
            .bind((
                "id",
                crate::surreal::record_id("person_project_role", uuid::Uuid::now_v7()),
            ))
            .bind(("person_id", crate::surreal::record_id("person", person_id)))
            .bind((
                "project_id",
                crate::surreal::record_id("project", project_id),
            ))
            .bind(("participation", participation.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
        }

        assert!(
            can_access_as_lawyer_in_surreal(&db, Some(lawyer_id), Role::Lawyer, project_id)
                .await
                .unwrap()
        );
        assert!(
            !can_access_as_lawyer_in_surreal(&db, Some(client_id), Role::Lawyer, project_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn client_lens_matches_client_side_participation_and_dri_marker() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let client_id = uuid::Uuid::now_v7();
        let lawyer_id = uuid::Uuid::now_v7();
        let project_id = uuid::Uuid::now_v7();
        for (id, email) in [
            (client_id, "client@example.com"),
            (lawyer_id, "lawyer@example.com"),
        ] {
            db.query("CREATE $id SET name = $email, email = $email, role = 'client'")
                .bind(("id", crate::surreal::record_id("person", id)))
                .bind(("email", email.to_string()))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        db.query(
            "CREATE $id SET code = 'matter', name = 'Matter', status = 'open', \
             entity_id = $entity_id, inserted_at = '2026-08-04T00:00:00Z', \
             updated_at = '2026-08-04T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", project_id)))
        .bind(("entity_id", record_id(ENTITY_TABLE, uuid::Uuid::now_v7())))
        .await
        .unwrap()
        .check()
        .unwrap();
        for (person_id, participation, is_client_dri) in
            [(client_id, "client", false), (lawyer_id, "attorney", true)]
        {
            db.query(
                "CREATE $id SET person_id = $person_id, project_id = $project_id, \
                 participation = $participation, is_client_dri = $is_client_dri, \
                 inserted_at = '2026-08-04T00:00:00Z', updated_at = '2026-08-04T00:00:00Z'",
            )
            .bind((
                "id",
                crate::surreal::record_id("person_project_role", uuid::Uuid::now_v7()),
            ))
            .bind(("person_id", crate::surreal::record_id("person", person_id)))
            .bind((
                "project_id",
                crate::surreal::record_id("project", project_id),
            ))
            .bind(("participation", participation.to_string()))
            .bind(("is_client_dri", is_client_dri))
            .await
            .unwrap()
            .check()
            .unwrap();
        }
        assert!(
            can_access_as_client_in_surreal(&db, Some(client_id), project_id)
                .await
                .unwrap()
        );
        assert!(
            can_access_as_client_in_surreal(&db, Some(lawyer_id), project_id)
                .await
                .unwrap()
        );
        assert!(!can_access_as_client_in_surreal(&db, None, project_id)
            .await
            .unwrap());
    }

    /// Read at the SurrealQL level, where the exclusivity used to live: the
    /// transaction no longer clears anyone else's flag, so both rows keep it.
    #[tokio::test]
    async fn designating_a_second_dri_adds_to_the_side() {
        let db = unmigrated().await;
        apply(&db).await.unwrap();
        let first = uuid::Uuid::now_v7();
        let second = uuid::Uuid::now_v7();
        let project = uuid::Uuid::now_v7();
        for (id, email) in [(first, "first@example.com"), (second, "second@example.com")] {
            db.query("CREATE $id SET name = $email, email = $email, role = 'lawyer'")
                .bind(("id", crate::surreal::record_id("person", id)))
                .bind(("email", email.to_string()))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        db.query(
            "CREATE $id SET code = 'matter', name = 'Matter', status = 'open', \
             entity_id = $entity_id, inserted_at = '2026-08-04T00:00:00Z', \
             updated_at = '2026-08-04T00:00:00Z'",
        )
        .bind(("id", crate::surreal::record_id("project", project)))
        .bind(("entity_id", record_id(ENTITY_TABLE, uuid::Uuid::now_v7())))
        .await
        .unwrap()
        .check()
        .unwrap();

        designate_dri_in_surreal(&db, project, first, DriSide::Lawyer)
            .await
            .unwrap();
        designate_dri_in_surreal(&db, project, second, DriSide::Lawyer)
            .await
            .unwrap();
        let lawyer_dris: Vec<uuid::Uuid> = db
            .query(
                "SELECT VALUE person_id.id() FROM person_project_role \
                 WHERE project_id = $project AND is_lawyer_dri = true",
            )
            .bind(("project", crate::surreal::record_id("project", project)))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        let mut got = lawyer_dris;
        got.sort();
        let mut expected = vec![first, second];
        expected.sort();
        assert_eq!(got, expected, "designation adds rather than replaces");
    }

    /// The lens over every matter: what it is, and who is accountable. The
    /// matter with no flagged row is the case the view exists to surface, so
    /// it appears with an empty DRI rather than being dropped or erroring.
    #[tokio::test]
    async fn the_directory_names_every_matter_and_its_accountable_lawyer() {
        let surreal = mem_surreal().await;
        let lawyer = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Accountable Lawyer", "dri@neonlaw.com"),
        )
        .await
        .unwrap();
        let assigned = create(
            &surreal,
            &NewProject {
                code: "assigned-matter".into(),
                name: "Assigned matter".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewProject {
                code: "unassigned-matter".into(),
                name: "Unassigned matter".into(),
                status: "closed".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        designate_dri_in_surreal(&surreal, assigned.id, lawyer.id, DriSide::Lawyer)
            .await
            .unwrap();

        // Owner and Admin read the same directory, and neither holds a
        // participation row on either matter — oversight is not membership.
        for role in [Role::Owner, Role::Admin] {
            let directory = matter_directory(&surreal, role).await.unwrap();
            let entries: Vec<(&str, &str, Vec<&str>)> = directory
                .iter()
                .map(|entry| {
                    (
                        entry.code.as_str(),
                        entry.status.as_str(),
                        entry.lawyer_dris.iter().map(String::as_str).collect(),
                    )
                })
                .collect();
            assert_eq!(
                entries,
                vec![
                    ("assigned-matter", "open", vec!["Accountable Lawyer"]),
                    ("unassigned-matter", "closed", vec![]),
                ],
                "{role:?} directory"
            );
            assert_eq!(directory[0].name, "Assigned matter");
        }
    }

    /// The lens is admin-tier only. A `lawyer` caller gets nothing from
    /// it — the firm-wide directory is not a wider `/app/lawyer` read.
    #[tokio::test]
    async fn the_directory_is_closed_to_every_tier_below_admin() {
        let surreal = mem_surreal().await;
        create(
            &surreal,
            &NewProject {
                code: "firm-wide".into(),
                name: "Firm wide".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        for role in [Role::Lawyer, Role::Clerk, Role::Client] {
            assert!(
                matter_directory(&surreal, role).await.unwrap().is_empty(),
                "{role:?} must read no directory"
            );
        }
        assert_eq!(
            matter_directory(&surreal, Role::Admin).await.unwrap().len(),
            1
        );
    }

    /// Owner still reads every matter. Admin reads only matters whose
    /// `firm_id` is a firm they belong to, and an Admin with no membership
    /// reads nothing — including matters that still have no owner.
    #[tokio::test]
    async fn the_directory_scopes_an_admin_to_their_firms() {
        let surreal = mem_surreal().await;
        let firm_a = crate::firms::create(
            &surreal,
            &crate::firms::NewFirm {
                name: "Practice A".into(),
                status: "active".into(),
                entity_id: crate::test_support::seed_entity(&surreal).await,
            },
        )
        .await
        .unwrap();
        let firm_b = crate::firms::create(
            &surreal,
            &crate::firms::NewFirm {
                name: "Practice B".into(),
                status: "active".into(),
                entity_id: crate::test_support::seed_entity(&surreal).await,
            },
        )
        .await
        .unwrap();
        let admin_a = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::with_role("Admin A", "admin-a@example.com", Role::Admin),
        )
        .await
        .unwrap();
        crate::firms::add_membership(
            &surreal,
            &crate::firms::NewPersonFirmRole {
                person_id: admin_a.id,
                firm_id: firm_a.id,
                membership: crate::firms::FirmMembership::Admin,
                is_dri: true,
            },
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewProject {
                code: "matter-a".into(),
                name: "Matter A".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                firm_id: Some(firm_a.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewProject {
                code: "matter-b".into(),
                name: "Matter B".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                firm_id: Some(firm_b.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewProject {
                code: "unowned".into(),
                name: "Unowned".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let owner = matter_directory_for(&surreal, Role::Owner, None)
            .await
            .unwrap();
        assert_eq!(
            owner.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec!["matter-a", "matter-b", "unowned"]
        );

        let scoped = matter_directory_for(&surreal, Role::Admin, Some(admin_a.id))
            .await
            .unwrap();
        assert_eq!(
            scoped.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec!["matter-a"]
        );

        let unscoped = matter_directory_for(&surreal, Role::Admin, None)
            .await
            .unwrap();
        assert!(unscoped.is_empty());
    }

    /// A flagged row naming a person who is no longer in the table reads as
    /// unassigned. The directory's job is to keep listing the matter.
    #[tokio::test]
    async fn a_dri_row_naming_a_missing_person_reads_as_unassigned() {
        let surreal = mem_surreal().await;
        let project = create(
            &surreal,
            &NewProject {
                code: "dangling-dri".into(),
                name: "Dangling DRI".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        surreal
            .query(
                "CREATE $id SET person_id = $person, project_id = $project, \
                 participation = 'lawyer', is_lawyer_dri = true, is_client_dri = false, \
                 inserted_at = '2026-08-11T00:00:00Z', updated_at = '2026-08-11T00:00:00Z'",
            )
            .bind((
                "id",
                record_id(super::PERSON_PROJECT_ROLE_TABLE, uuid::Uuid::now_v7()),
            ))
            .bind(("person", record_id("person", uuid::Uuid::now_v7())))
            .bind(("project", record_id(super::PROJECT_TABLE, project.id)))
            .await
            .unwrap()
            .check()
            .unwrap();

        let directory = matter_directory(&surreal, Role::Owner).await.unwrap();
        assert_eq!(directory.len(), 1);
        assert_eq!(directory[0].code, "dangling-dri");
        assert!(directory[0].lawyer_dris.is_empty());
    }

    /// Unlike `matter_directory`, the digest carries both DRI sides and no
    /// role gate at all — the nightly workflow that reads it runs headless,
    /// with no session to check.
    #[tokio::test]
    async fn dri_digest_reports_both_sides_for_every_project() {
        let surreal = mem_surreal().await;
        let lawyer = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Accountable Lawyer", "digest-lawyer@neonlaw.com"),
        )
        .await
        .unwrap();
        let client = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Represented Client", "digest-client@neonlaw.com"),
        )
        .await
        .unwrap();
        let assigned = create(
            &surreal,
            &NewProject {
                code: "digest-assigned".into(),
                name: "Digest assigned".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewProject {
                code: "digest-unassigned".into(),
                name: "Digest unassigned".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        designate_dri_in_surreal(&surreal, assigned.id, lawyer.id, DriSide::Lawyer)
            .await
            .unwrap();
        designate_dri_in_surreal(&surreal, assigned.id, client.id, DriSide::Client)
            .await
            .unwrap();

        let digest = dri_digest(&surreal).await.unwrap();
        let entries: Vec<(&str, Vec<&str>, Vec<&str>)> = digest
            .iter()
            .map(|entry| {
                (
                    entry.code.as_str(),
                    entry.lawyer_dris.iter().map(String::as_str).collect(),
                    entry.client_dris.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            vec![
                (
                    "digest-assigned",
                    vec!["Accountable Lawyer"],
                    vec!["Represented Client"]
                ),
                ("digest-unassigned", vec![], vec![]),
            ]
        );
    }

    #[tokio::test]
    async fn participation_write_readbacks_both_native_links() {
        let surreal = mem_surreal().await;
        let person = crate::persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Participant", "participant-port@example.com"),
        )
        .await
        .unwrap();
        let project = create(
            &surreal,
            &NewProject {
                code: "participation-port".into(),
                name: "Participation port".into(),
                status: "open".into(),
                entity_id: uuid::Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let created = super::add_participation(&surreal, project.id, person.id, "attorney")
            .await
            .unwrap();
        assert_eq!(created.person_id, person.id);
        assert_eq!(
            super::participations_for_project(&surreal, project.id)
                .await
                .unwrap(),
            vec![created]
        );
    }
}

#[cfg(test)]
mod project_code_resolution_tests {
    use super::id_for_code;

    /// A code resolves; everything else is the same `None`.
    #[tokio::test]
    async fn a_code_resolves_and_every_miss_is_indistinguishable() {
        let surreal = crate::test_support::mem_surreal().await;
        let project = crate::projects::create(
            &surreal,
            &crate::projects::NewProject {
                code: "libra-formation".into(),
                name: "Libra formation".into(),
                status: "open".into(),
                entity_id: crate::test_support::seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("a seeded Project");

        assert_eq!(
            id_for_code(&surreal, "libra-formation").await,
            Some(project.id)
        );

        for miss in [
            // A well-formed code no Project carries.
            "aries-eviction",
            // `new` is the matter-open form, not a matter.
            "new",
            // Malformed, so the store is never asked.
            "Not_A_Code",
            "",
        ] {
            assert_eq!(
                id_for_code(&surreal, miss).await,
                None,
                "`{miss}` must not resolve a matter"
            );
        }

        // The row id is not a way to name the matter, which is the whole point
        // of keying these routes on the code.
        assert_eq!(
            id_for_code(&surreal, &project.id.to_string()).await,
            None,
            "a row id must not resolve the matter it belongs to"
        );
    }
}
