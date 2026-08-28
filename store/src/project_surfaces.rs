//! Provision the three handles a Project opens with.
//!
//! Opening a Project records its identity. This module then creates or adopts
//! the three external surfaces that identity names:
//!
//! 1. **Working files** — the documents-bucket prefix `projects/<code>`. The
//!    prefix is a key convention, not a bucket; nothing here writes an object.
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

/// What one reconcile pass recorded or confirmed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectSurfaces {
    pub code: String,
    /// Always `projects/<code>`. Not stored as its own column: the code is the
    /// prefix, and a second field would be a second spelling to keep in step.
    pub documents_prefix: String,
    pub drive_folder_id: Option<String>,
    pub repository_url: Option<String>,
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

    let drive_folder_id = if let Some(existing) = project.drive_folder_id.as_deref() {
        Some(existing.to_string())
    } else if let Some(drive) = drive {
        let folder = drive.create_folder(&project.code).await?;
        set_drive_folder_id(surreal, project_id, Some(&folder.id))
            .await?
            .ok_or(SurfaceError::NotFound)?;
        Some(folder.id)
    } else {
        None
    };

    if let (Some(drive), Some(folder_id)) = (drive, drive_folder_id.as_deref()) {
        grant_drive_ingest_membership(surreal, project_id, drive, folder_id).await?;
    }

    let repository_url = if let Some(existing) = project.repository_url.as_deref() {
        Some(existing.to_string())
    } else if let Some(forge) = forge {
        let repository = forge.ensure_repository(&project.code).await?;
        set_repository_url(surreal, project_id, Some(&repository.url))
            .await?
            .ok_or(SurfaceError::NotFound)?;
        stamp_forge_provisioned_at(surreal, project_id).await?;
        Some(repository.url)
    } else {
        None
    };

    Ok(ProjectSurfaces {
        code: project.code.clone(),
        documents_prefix: documents_prefix(&project.code),
        drive_folder_id,
        repository_url,
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
/// GitHub collaborator APIs are not called from this module.
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
    use super::{reconcile, ProjectSurfaces, SurfaceError};
    use crate::persons::{self, NewPerson, Role};
    use crate::projects::{self, OpenMatterCommand};
    use crate::test_support::{mem_surreal, seed_entity};
    use cloud::drive::{DriveMemberKind, DriveRole, DriveService, FakeDrive};
    use cloud::forge::FakeForge;
    use cloud::workspace::documents_prefix;

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
                attestation: true,
                acting_person_id: lawyer.id,
            },
        )
        .await
        .expect("open")
    }

    #[test]
    fn the_documents_prefix_is_the_project_code() {
        assert_eq!(documents_prefix("acme"), "projects/acme");
    }

    #[test]
    fn this_module_does_not_read_a_local_drive_mount() {
        let src = include_str!("project_surfaces.rs");
        assert!(
            !src.contains("NAVIGATOR_PROJECTS_DRIVE_MOUNT"),
            "provisioning must not take a workstation mount as input"
        );
        assert!(
            !src.contains("collaborator"),
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
                code: "acme".into(),
                documents_prefix: "projects/acme".into(),
                drive_folder_id: Some("folder-1".into()),
                repository_url: Some("https://forge.example/an-organization/acme".into()),
            }
        );

        let reloaded = projects::find_by_id(&surreal, project.id)
            .await
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.drive_folder_id.as_deref(), Some("folder-1"));
        assert_eq!(
            reloaded.repository_url.as_deref(),
            Some("https://forge.example/an-organization/acme")
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

        assert_eq!(first, second);
        assert_eq!(forge.repository_count(), 1);
        let folders = drive.list_folders().await.expect("list");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "acme");
    }

    #[tokio::test]
    async fn an_existing_drive_folder_is_adopted_not_recreated() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let drive = FakeDrive::default();
        let already = drive.create_folder("acme").await.expect("pre-create");
        let forge = FakeForge::new();

        let surfaces = reconcile(&surreal, project.id, Some(&drive), Some(&forge))
            .await
            .expect("adopt");
        assert_eq!(
            surfaces.drive_folder_id.as_deref(),
            Some(already.id.as_str())
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
        assert_eq!(forge.repository_count(), 0);
    }

    #[tokio::test]
    async fn missing_services_skip_those_surfaces() {
        let surreal = mem_surreal().await;
        let project = open_acme(&surreal).await;
        let surfaces = reconcile(&surreal, project.id, None::<&FakeDrive>, None::<&FakeForge>)
            .await
            .expect("skip");
        assert_eq!(surfaces.documents_prefix, "projects/acme");
        assert_eq!(surfaces.drive_folder_id, None);
        assert_eq!(surfaces.repository_url, None);
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
}
