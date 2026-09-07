//! Provision the three handles a Project opens with.
//!
//! Opening a Project records its identity. This module then creates or adopts
//! the three external surfaces that identity names:
//!
//! 1. **Working files** — the documents-bucket prefix
//!    `projects/<code>/documents`. The prefix is a key convention, not a
//!    bucket; nothing here writes an object.
//! 2. **Ingest** — the Drive folder named for the code, recorded as
//!    `drive_folder_id`. Drive is import-only: membership lets people drop
//!    files in; Navigator never treats the folder as a live store.
//! 3. **Source** — one private repository named for the code, recorded as
//!    `repository_url`. Project participation is never copied onto the forge.
//!
//! Each step is idempotent. A folder or repository that already exists is
//! adopted. A column that is already set is left alone, so a retry after a
//! partial external failure cannot duplicate a handle. Absent Drive or forge
//! configuration skips that surface rather than failing the matter open.

use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;

use cloud::drive::{DriveMember, DriveMemberKind, DriveRole, DriveService};
use cloud::forge::{ForgeError, ForgeService};
use cloud::workspace::documents_prefix;
use cloud::DriveError;

use crate::persons;
use crate::projects::{
    self, set_drive_folder_id, set_repository_url, ProjectCommandError, ProjectStoreError,
};
use crate::surreal::{record_id, SurrealDb};

const PROJECT_TABLE: &str = "project";

/// What a reconcile pass did about one provisioned surface (Drive folder or
/// source repository) — distinct from merely reporting the resulting value,
/// because a caller reading only the value cannot tell "this call did the
/// work" from "nothing needed doing" from "nothing was attempted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceStatus {
    /// This call is what recorded the value: the Project row's column was
    /// null coming in, and the call created or adopted the external resource
    /// and wrote it.
    Created,
    /// Already recorded on the Project row before this call; nothing changed.
    Present,
    /// No Drive or forge service is configured for this deployment, so the
    /// surface was never attempted.
    Skipped,
}

/// A Project's repository-provisioning state, read from
/// [`crate::projects::Project::repository_url`],
/// [`crate::projects::Project::forge_provisioned_at`], and
/// [`crate::projects::Project::git_initialized_at`] together rather than one
/// column at a time — a caller reading only `repository_url` cannot tell
/// "never requested" from "requested but the forge failed" from "attached
/// but no source has landed yet."
///
/// Pure derivation ([`source_state`]): no database write, no forge or Drive
/// call, and no timestamp is invented for a state the columns do not already
/// record. A row with every column null reports [`SourceState::NotEnabled`],
/// never [`SourceState::Pending`] or [`SourceState::Failed`] — an absent
/// repository is a legitimate, common resting state, not a stalled or broken
/// one (see `docs/project-repositories.md#an-absent-repository-is-legitimate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// No repository is recorded and reconcile has never stamped a failed
    /// attempt either. The default resting state for a matter that has not
    /// asked for one — [`crate::projects::Project::repository_url`] is
    /// nullable exactly so this can be true forever.
    NotEnabled,
    /// A repository has been requested but reconcile has not yet resolved
    /// it. No writer in this codebase leaves a row in this state today:
    /// [`reconcile`] either records a repository in the same pass that asks
    /// for one, or does not attempt one at all. Reserved for a future
    /// asynchronous provisioning path.
    Pending,
    /// A repository URL is recorded, but not stamped by this deployment's
    /// own provisioning pass — `forge_provisioned_at` is unset, exactly the
    /// shape a direct `PATCH` of `repository_url` or a row written before
    /// that column existed produces. The columns cannot say which, so this
    /// is reported for human reconciliation rather than guessed.
    Unknown,
    /// Recorded and stamped provisioned; no validated source has been
    /// committed or imported yet.
    Attached,
    /// Recorded, provisioned, and carrying validated source.
    Initialized,
    /// Reconcile attempted a repository and the attempt did not succeed.
    /// Today a failed attempt leaves every column exactly as it was —
    /// indistinguishable from never having tried — so no writer produces
    /// this state yet. Reserved for a future persisted failure marker.
    Failed,
}

impl SourceState {
    /// The wire spelling — the same string [`serde::Serialize`] produces,
    /// exposed as a plain method so a CLI table or log line can print it
    /// without round-tripping through JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SourceState::NotEnabled => "not_enabled",
            SourceState::Pending => "pending",
            SourceState::Unknown => "unknown",
            SourceState::Attached => "attached",
            SourceState::Initialized => "initialized",
            SourceState::Failed => "failed",
        }
    }
}

/// Derive [`SourceState`] from one Project's own columns. See the type's own
/// documentation for why each state is or is not reachable today.
#[must_use]
pub fn source_state(project: &crate::projects::Project) -> SourceState {
    match (
        project.repository_url.is_some(),
        project.forge_provisioned_at.is_some(),
        project.git_initialized_at.is_some(),
    ) {
        (false, _, _) => SourceState::NotEnabled,
        (true, false, _) => SourceState::Unknown,
        (true, true, false) => SourceState::Attached,
        (true, true, true) => SourceState::Initialized,
    }
}

/// What one reconcile pass recorded or confirmed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectSurfaces {
    pub code: String,
    /// Always `projects/<code>/documents`. Not stored as its own column: the
    /// code is the prefix input, and a second field would be a second spelling
    /// to keep in step.
    pub documents_prefix: String,
    pub drive_folder_id: Option<String>,
    pub drive_status: SurfaceStatus,
    pub repository_url: Option<String>,
    pub repository_status: SurfaceStatus,
}

#[derive(Debug, Error)]
pub enum SurfaceError {
    #[error("no matter with that id")]
    NotFound,
    #[error("drive: {0}")]
    Drive(#[from] DriveError),
    #[error("forge: {0}")]
    Forge(#[from] ForgeError),
    #[error("{0}")]
    Command(#[from] ProjectCommandError),
    #[error("database: {0}")]
    Store(#[from] ProjectStoreError),
    #[error("database: {0}")]
    Db(String),
}

/// Create or adopt the Project's Drive ingest folder and source repository,
/// and name its documents-bucket prefix.
///
/// `drive` and `forge` are independently optional. A KIND loop with neither
/// still opens matters; an admin reconcile later fills in whichever service
/// the deployment has configured.
pub async fn reconcile<D, F>(
    surreal: &SurrealDb,
    project_id: Uuid,
    drive: Option<&D>,
    forge: Option<&F>,
) -> Result<ProjectSurfaces, SurfaceError>
where
    D: DriveService + ?Sized,
    F: ForgeService + ?Sized,
{
    let project = projects::find_by_id(surreal, project_id)
        .await
        .map_err(|error| SurfaceError::Db(error.to_string()))?
        .ok_or(SurfaceError::NotFound)?;

    let (drive_folder_id, drive_status) = if let Some(existing) = project.drive_folder_id.as_deref()
    {
        (Some(existing.to_string()), SurfaceStatus::Present)
    } else if let Some(drive) = drive {
        let folder = drive.create_folder(&project.code).await?;
        set_drive_folder_id(surreal, project_id, Some(&folder.id))
            .await?
            .ok_or(SurfaceError::NotFound)?;
        (Some(folder.id), SurfaceStatus::Created)
    } else {
        (None, SurfaceStatus::Skipped)
    };

    if let (Some(drive), Some(folder_id)) = (drive, drive_folder_id.as_deref()) {
        grant_drive_ingest_membership(surreal, project_id, drive, folder_id).await?;
    }

    let (repository_url, repository_status) =
        if let Some(existing) = project.repository_url.as_deref() {
            (Some(existing.to_string()), SurfaceStatus::Present)
        } else if let Some(forge) = forge {
            let repository = forge.ensure_repository(&project.code).await?;
            set_repository_url(surreal, project_id, Some(&repository.url))
                .await?
                .ok_or(SurfaceError::NotFound)?;
            stamp_forge_provisioned_at(surreal, project_id).await?;
            (Some(repository.url), SurfaceStatus::Created)
        } else {
            (None, SurfaceStatus::Skipped)
        };

    Ok(ProjectSurfaces {
        code: project.code.clone(),
        documents_prefix: documents_prefix(&project.code),
        drive_folder_id,
        drive_status,
        repository_url,
        repository_status,
    })
}

/// Resolve Drive and forge from the process environment and reconcile.
///
/// Missing configuration is a skip, not an error: the matter stays open and
/// an operator retries once the deployment's credentials are present.
pub async fn reconcile_from_env(
    surreal: &SurrealDb,
    project_id: Uuid,
) -> Result<ProjectSurfaces, SurfaceError> {
    let drive = cloud::GoogleDrive::from_env().await.ok();
    let forge = cloud::forge::GitHubForge::from_env().ok();
    reconcile(surreal, project_id, drive.as_ref(), forge.as_ref()).await
}

/// Best-effort wrapper for matter-open doors: a Drive or forge fault is
/// logged rather than rolling the open back. Retry is [`reconcile`].
pub async fn reconcile_after_open(surreal: &SurrealDb, project_id: Uuid) {
    if let Err(error) = reconcile_from_env(surreal, project_id).await {
        tracing::error!(
            %error,
            project_id = %project_id,
            "Project surface reconcile failed; retry with projects surfaces reconcile"
        );
    }
}

/// Share the ingest folder with every participant who has an email.
///
/// Drive sharing is how Workspace users drop files in. It is not an
/// authorization decision inside Navigator, and it is not a forge grant —
/// GitHub membership APIs are not called from this module.
async fn grant_drive_ingest_membership<D>(
    surreal: &SurrealDb,
    project_id: Uuid,
    drive: &D,
    folder_id: &str,
) -> Result<(), SurfaceError>
where
    D: DriveService + ?Sized,
{
    let rows = projects::participations_for_project(surreal, project_id).await?;
    for row in rows {
        let Some(person) = persons::find_by_id(surreal, row.person_id)
            .await
            .map_err(|error| SurfaceError::Db(error.to_string()))?
        else {
            continue;
        };
        let email = person.email.trim();
        if email.is_empty() {
            continue;
        }
        drive
            .set_member_permission(
                folder_id,
                &DriveMember {
                    kind: DriveMemberKind::User,
                    email: email.to_string(),
                },
                DriveRole::Writer,
            )
            .await?;
    }
    Ok(())
}

async fn stamp_forge_provisioned_at(
    surreal: &SurrealDb,
    project_id: Uuid,
) -> Result<(), SurfaceError> {
    surreal
        .query("UPDATE $id SET forge_provisioned_at = $now, updated_at = $now")
        .bind(("id", record_id(PROJECT_TABLE, project_id)))
        .bind(("now", Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(|error| SurfaceError::Db(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile, source_state, ProjectSurfaces, SourceState, SurfaceError, SurfaceStatus,
    };
    use crate::persons::{self, NewPerson, Role};
    use crate::projects::{self, OpenMatterCommand};
    use crate::test_support::{mem_surreal, seed_entity};
    use cloud::drive::{DriveMemberKind, DriveRole, DriveService, FakeDrive};
    use cloud::forge::{FakeForge, ForgeError, ForgeRepository, ForgeService};
    use cloud::workspace::documents_prefix;

    /// A forge that always refuses `ensure_repository` — for proving that a
    /// failed provisioning attempt leaves `forge_provisioned_at` (and
    /// `repository_url`) untouched rather than stamping a repository that
    /// was never actually recorded.
    struct FailingForge;

    #[async_trait::async_trait]
    impl ForgeService for FailingForge {
        async fn find_repository(
            &self,
            _project_code: &str,
        ) -> Result<Option<ForgeRepository>, ForgeError> {
            Ok(None)
        }

        async fn ensure_repository(
            &self,
            _project_code: &str,
        ) -> Result<ForgeRepository, ForgeError> {
            Err(ForgeError::Authentication)
        }
    }

    async fn open_acme(surreal: &crate::surreal::SurrealDb) -> projects::Project {
        let entity_id = seed_entity(surreal).await;
        let lawyer = persons::create(
            surreal,
            &NewPerson::with_role("Lawyer", "lawyer@example.com", Role::Lawyer),
        )
        .await
        .expect("lawyer");
        let client = persons::create(
            surreal,
            &NewPerson::with_role("Client", "client@example.com", Role::Client),
        )
        .await
        .expect("client");
        projects::open_matter(
            surreal,
            &OpenMatterCommand {
                name: "Acme matter".into(),
                code: "acme".into(),
                client_id: client.id,
                entity_id,
                description: None,
                brand: "neon".to_string(),
                attestation: true,
                acting_person_id: lawyer.id,
                closed_at: None,
            },
        )
        .await
        .expect("open")
    }

    #[test]
    fn the_documents_prefix_is_the_project_code() {
        assert_eq!(documents_prefix("acme"), "projects/acme/documents");
    }

    #[test]
    fn this_module_does_not_read_a_local_drive_mount() {
        let src = include_str!("project_surfaces.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes the test module");
        assert!(
            !production.contains("DRIVE_MOUNT"),
            "provisioning must not take a workstation mount as input"
        );
        assert!(
            !production.contains("collaborator"),
            "Project participation must not grant forge membership"
        );
    }

    #[tokio::test]
    async fn reconcile_creates_the_drive_folder_and_the_repository() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let drive = FakeDrive::default();
        let forge = FakeForge::new();

        let surfaces = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("reconcile");

        assert_eq!(
            surfaces,
            ProjectSurfaces {
                code: project.code.clone(),
                documents_prefix: format!("projects/{}/documents", project.code),
                drive_folder_id: Some("folder-1".into()),
                drive_status: SurfaceStatus::Created,
                repository_url: Some(format!(
                    "https://forge.example/an-organization/{}",
                    project.code
                )),
                repository_status: SurfaceStatus::Created,
            }
        );

        let reloaded = projects::find_by_id(&surreal, project.id)
            .await
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.drive_folder_id.as_deref(), Some("folder-1"));
        assert_eq!(
            reloaded.repository_url.as_deref(),
            Some(format!("https://forge.example/an-organization/{}", project.code).as_str())
        );
        assert!(reloaded.forge_provisioned_at.is_some());

        let members = drive.members("folder-1");
        let emails: Vec<&str> = members
            .iter()
            .map(|(member, role)| {
                assert_eq!(member.kind, DriveMemberKind::User);
                assert_eq!(*role, DriveRole::Writer);
                member.email.as_str()
            })
            .collect();
        assert!(emails.contains(&"lawyer@example.com"), "{emails:?}");
        assert!(emails.contains(&"client@example.com"), "{emails:?}");
    }

    #[tokio::test]
    async fn a_second_reconcile_adopts_and_does_not_duplicate() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let drive = FakeDrive::default();
        let forge = FakeForge::new();

        let first = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("first");
        let second = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("second");

        assert_eq!(first.drive_folder_id, second.drive_folder_id);
        assert_eq!(first.repository_url, second.repository_url);
        assert_eq!(first.drive_status, SurfaceStatus::Created);
        assert_eq!(first.repository_status, SurfaceStatus::Created);
        assert_eq!(
            second.drive_status,
            SurfaceStatus::Present,
            "the second pass found the folder already recorded rather than creating it again"
        );
        assert_eq!(second.repository_status, SurfaceStatus::Present);
        assert_eq!(forge.repository_count(), 1);
        let folders = drive.list_folders().await.expect("list");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, project.code);
    }

    #[tokio::test]
    async fn an_existing_drive_folder_is_adopted_not_recreated() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let drive = FakeDrive::default();
        let already = drive
            .create_folder(&project.code)
            .await
            .expect("pre-create");
        let forge = FakeForge::new();

        let surfaces = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("adopt");
        assert_eq!(
            surfaces.drive_folder_id.as_deref(),
            Some(already.id.as_str())
        );
        assert_eq!(
            surfaces.drive_status,
            SurfaceStatus::Created,
            "adopting an untracked external folder is this call's own work, \
             so it is reported the same as a fresh create"
        );
        let folders = drive.list_folders().await.expect("list");
        assert_eq!(folders.len(), 1);
    }

    #[tokio::test]
    async fn a_recorded_repository_url_is_left_alone() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        projects::set_repository_url(
            &surreal,
            project.id,
            Some("https://git.example.internal/client-org/acme"),
        )
        .await
        .expect("record a URL on another forge");
        let drive = FakeDrive::default();
        let forge = FakeForge::new();

        let surfaces = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("leave recorded URL");
        assert_eq!(
            surfaces.repository_url.as_deref(),
            Some("https://git.example.internal/client-org/acme")
        );
        assert_eq!(surfaces.repository_status, SurfaceStatus::Present);
        assert_eq!(forge.repository_count(), 0);
    }

    #[tokio::test]
    async fn missing_services_skip_those_surfaces() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let surfaces = reconcile(&surreal, project.id, None::<&FakeDrive>, None::<&FakeForge>)
            .await
            .expect("skip");
        assert_eq!(
            surfaces.documents_prefix,
            format!("projects/{}/documents", project.code)
        );
        assert_eq!(surfaces.drive_folder_id, None);
        assert_eq!(surfaces.drive_status, SurfaceStatus::Skipped);
        assert_eq!(surfaces.repository_url, None);
        assert_eq!(surfaces.repository_status, SurfaceStatus::Skipped);
    }

    #[tokio::test]
    async fn unknown_project_is_not_found() {
        let surreal = mem_surreal().await;
        let error = reconcile(
            &surreal,
            uuid::Uuid::now_v7(),
            None::<&FakeDrive>,
            None::<&FakeForge>,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SurfaceError::NotFound));
    }

    #[tokio::test]
    async fn a_failed_forge_attempt_stamps_nothing() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let forge = FailingForge;

        let error = reconcile(&surreal, project.id, None::<&FakeDrive>, Some(&forge))
            .await
            .unwrap_err();
        assert!(matches!(error, SurfaceError::Forge(_)));

        let reloaded = projects::find_by_id(&surreal, project.id)
            .await
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.repository_url, None);
        assert_eq!(
            reloaded.forge_provisioned_at, None,
            "a failed create/adopt attempt must never stamp forge_provisioned_at"
        );
    }

    #[tokio::test]
    async fn a_retry_never_moves_the_first_success_stamp() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let forge = FakeForge::new();

        reconcile(&surreal, project.id, None::<&FakeDrive>, Some(&forge))
            .await
            .expect("first reconcile stamps forge_provisioned_at");
        let first_stamp = projects::find_by_id(&surreal, project.id)
            .await
            .expect("load")
            .expect("exists")
            .forge_provisioned_at
            .expect("stamped on first success");

        // A second reconcile finds `repository_url` already recorded and
        // takes the "present" branch, never the forge, so nothing re-stamps.
        reconcile(&surreal, project.id, None::<&FakeDrive>, Some(&forge))
            .await
            .expect("second reconcile is a no-op adopt");
        let second_stamp = projects::find_by_id(&surreal, project.id)
            .await
            .expect("load")
            .expect("exists")
            .forge_provisioned_at
            .expect("still stamped");

        assert_eq!(first_stamp, second_stamp);
    }

    #[test]
    fn source_state_reports_no_repository_as_not_enabled() {
        let project = test_project(None, None, None);
        assert_eq!(source_state(&project), SourceState::NotEnabled);
    }

    #[test]
    fn source_state_reports_an_unstamped_url_as_unknown() {
        let project = test_project(Some("https://forge.example/acme"), None, None);
        assert_eq!(source_state(&project), SourceState::Unknown);
    }

    #[test]
    fn source_state_reports_a_stamped_url_with_no_source_as_attached() {
        let project = test_project(
            Some("https://forge.example/acme"),
            Some("2026-09-01T00:00:00Z"),
            None,
        );
        assert_eq!(source_state(&project), SourceState::Attached);
    }

    #[test]
    fn source_state_reports_a_stamped_url_with_source_as_initialized() {
        let project = test_project(
            Some("https://forge.example/acme"),
            Some("2026-09-01T00:00:00Z"),
            Some("2026-09-02T00:00:00Z"),
        );
        assert_eq!(source_state(&project), SourceState::Initialized);
    }

    /// `as_str` and `serde::Serialize` must agree, or a CLI table and the
    /// JSON a caller parses would name the same state two different ways.
    #[test]
    fn as_str_matches_the_serialized_wire_spelling() {
        for state in [
            SourceState::NotEnabled,
            SourceState::Pending,
            SourceState::Unknown,
            SourceState::Attached,
            SourceState::Initialized,
            SourceState::Failed,
        ] {
            let serialized = serde_json::to_value(state).expect("serialize");
            assert_eq!(serialized, state.as_str(), "{state:?}");
        }
    }

    /// A minimal in-memory [`projects::Project`] carrying only the three
    /// columns [`source_state`] reads — the other fields do not vary
    /// across these table-driven cases, so this stays a plain constructor
    /// rather than dragging every test through [`open_acme`] and a real
    /// database.
    fn test_project(
        repository_url: Option<&str>,
        forge_provisioned_at: Option<&str>,
        git_initialized_at: Option<&str>,
    ) -> projects::Project {
        projects::Project {
            id: uuid::Uuid::now_v7(),
            code: "acme".to_string(),
            name: "Acme matter".to_string(),
            status: "open".to_string(),
            brand: "neon".to_string(),
            entity_id: uuid::Uuid::now_v7(),
            firm_id: None,
            description: None,
            drive_folder_id: None,
            repository_url: repository_url.map(str::to_string),
            git_initialized_at: git_initialized_at.map(str::to_string),
            forge_provisioned_at: forge_provisioned_at.map(str::to_string),
            closed_at: None,
            internal_slack_channel_url: None,
            external_slack_channel_url: None,
            internal_slack_channel_id: None,
            private_notion_page_url: None,
            shared_notion_page_url: None,
            inserted_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }
}
