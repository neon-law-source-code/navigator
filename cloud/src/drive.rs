//! Google Workspace Drive folders for a Project's ingest dropbox.
//!
//! This module deliberately does not store documents: working files and
//! client-facing artifacts remain [`crate::StorageService`] assets. A
//! [`DriveService`] only manages the per-Project ingest folder and its
//! Workspace permissions so people can drop files in. The service is
//! constructed for one Workspace at a time, so callers must select the
//! Project's owning entity before they call it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";
const DRIVE_BASE_URL: &str = "https://www.googleapis.com/drive/v3";
const FOLDER_MIME_TYPE: &str = "application/vnd.google-apps.folder";

/// The two entities that operate an independent Google Workspace.
///
/// The enum is configuration-only. A later Project-to-Workspace resolver owns
/// mapping a database entity to one of these deployments; Drive callers never
/// infer that mapping from a Project code or folder name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveWorkspace {
    NeonLaw,
}

impl DriveWorkspace {
    /// The per-entity prefix of the `NAVIGATOR_DRIVE_*` variables documented
    /// in `.env.example` and pinned by the workshop's Environment Matrix. The
    /// firm's prefix is spelled after its `neonlaw.com` entity zone, matching
    /// the variant; the firm's identifier is `neon`.
    const fn env_prefix(self) -> &'static str {
        match self {
            Self::NeonLaw => "NAVIGATOR_DRIVE_NEON_LAW",
        }
    }
}

/// Credentials and root shared-drive coordinates for one entity Workspace.
///
/// `service_account_json` is kept out of `Debug` so accidental diagnostics do
/// not expose a private key. The service account must be granted domain-wide
/// delegation by the owning Workspace; `delegated_user` is the Workspace user
/// it impersonates for Drive API calls.
#[derive(Clone)]
pub struct DriveWorkspaceConfig {
    pub workspace: DriveWorkspace,
    pub projects_drive_id: String,
    pub delegated_user: String,
    pub service_account_json: String,
}

impl DriveWorkspaceConfig {
    pub fn from_env(workspace: DriveWorkspace) -> Result<Self, DriveError> {
        Self::from_lookup(workspace, |key| std::env::var(key).ok())
    }

    pub fn from_lookup<F>(workspace: DriveWorkspace, get: F) -> Result<Self, DriveError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let prefix = workspace.env_prefix();
        let required = |suffix| {
            let key = format!("{prefix}_{suffix}");
            get(&key)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| DriveError::MissingConfig(key))
        };
        Ok(Self {
            workspace,
            projects_drive_id: required("PROJECTS_DRIVE_ID")?,
            delegated_user: required("DELEGATED_USER")?,
            service_account_json: required("SERVICE_ACCOUNT_JSON")?,
        })
    }
}

impl std::fmt::Debug for DriveWorkspaceConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DriveWorkspaceConfig")
            .field("workspace", &self.workspace)
            .field("projects_drive_id", &self.projects_drive_id)
            .field("delegated_user", &"[redacted]")
            .field("service_account_json", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveFolder {
    pub id: String,
    /// A Project code. The implementation only lists folders directly under
    /// the Projects shared-drive root, never working-file names.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub enum DriveMemberKind {
    User,
    Group,
}

impl DriveMemberKind {
    const fn api_value(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
        }
    }
}

/// A Workspace principal. Its address is intentionally never attached to a
/// tracing span; callers may only emit identifiers and aggregate counts.
#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct DriveMember {
    pub kind: DriveMemberKind,
    pub email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub enum DriveRole {
    Reader,
    Writer,
}

impl DriveRole {
    const fn api_value(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        }
    }
}

#[derive(Debug, Error)]
pub enum DriveError {
    #[error("missing required Drive configuration: {0}")]
    MissingConfig(String),
    #[error("Drive authentication failed")]
    Authentication,
    #[error("Drive request failed while {action}")]
    Request {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Drive API returned {status} while {action}")]
    Api { action: &'static str, status: u16 },
    #[error("Drive API returned an invalid response while {action}")]
    Response {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Drive API response while {action} did not include a folder id")]
    MissingFolderId { action: &'static str },
    #[error("Drive API response while {action} did not include a permission id")]
    MissingPermissionId { action: &'static str },
    #[error("Drive found {count} folders for one project code; refusing to guess")]
    AmbiguousFolderName { count: usize },
}

/// The internal Workspace-folder contract. It has no document-byte methods:
/// object storage remains the canonical client asset boundary.
#[async_trait]
pub trait DriveService: Send + Sync {
    async fn create_folder(&self, project_code: &str) -> Result<DriveFolder, DriveError>;
    async fn find_folder_by_name(
        &self,
        project_code: &str,
    ) -> Result<Option<DriveFolder>, DriveError>;
    async fn list_folders(&self) -> Result<Vec<DriveFolder>, DriveError>;
    async fn set_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
        role: DriveRole,
    ) -> Result<(), DriveError>;
    async fn remove_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
    ) -> Result<(), DriveError>;
    async fn move_to_archive(&self, folder_id: &str) -> Result<(), DriveError>;
    async fn restore_from_archive(&self, folder_id: &str) -> Result<(), DriveError>;
}

/// Production Drive v3 client authenticated as one Workspace service account
/// with domain-wide delegation.
pub struct GoogleDrive {
    projects_drive_id: String,
    token_source: Arc<dyn google_cloud_token::TokenSource>,
    http: reqwest::Client,
    base_url: String,
}

impl GoogleDrive {
    pub async fn new(config: DriveWorkspaceConfig) -> Result<Self, DriveError> {
        let credentials = google_cloud_auth::credentials::CredentialsFile::new_from_str(
            &config.service_account_json,
        )
        .await
        .map_err(|_| DriveError::Authentication)?;
        let scopes = [DRIVE_SCOPE];
        let auth_config = google_cloud_auth::project::Config::default()
            .with_scopes(&scopes)
            .with_sub(&config.delegated_user);
        let provider = google_cloud_auth::token::DefaultTokenSourceProvider::new_with_credentials(
            auth_config,
            Box::new(credentials),
        )
        .await
        .map_err(|_| DriveError::Authentication)?;
        Ok(Self::from_parts(
            config.projects_drive_id,
            google_cloud_token::TokenSourceProvider::token_source(&provider),
            DRIVE_BASE_URL,
        ))
    }

    /// Construct from `NAVIGATOR_DRIVE_NEON_LAW_*` when a deployment has
    /// configured the Workspace. Missing configuration is an error the
    /// caller treats as "skip Drive this pass".
    pub async fn from_env() -> Result<Self, DriveError> {
        Self::new(DriveWorkspaceConfig::from_env(DriveWorkspace::NeonLaw)?).await
    }

    fn from_parts(
        projects_drive_id: String,
        token_source: Arc<dyn google_cloud_token::TokenSource>,
        base_url: &str,
    ) -> Self {
        Self {
            projects_drive_id,
            token_source,
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    async fn token(&self) -> Result<String, DriveError> {
        let raw = self
            .token_source
            .token()
            .await
            .map_err(|_| DriveError::Authentication)?;
        Ok(raw.strip_prefix("Bearer ").unwrap_or(&raw).to_string())
    }

    fn files_url(&self) -> String {
        format!("{}/files", self.base_url)
    }

    fn file_url(&self, folder_id: &str) -> String {
        format!("{}/files/{folder_id}", self.base_url)
    }

    fn permissions_url(&self, folder_id: &str) -> String {
        format!("{}/permissions", self.file_url(folder_id))
    }

    fn checked(
        response: Result<reqwest::Response, reqwest::Error>,
        action: &'static str,
    ) -> Result<reqwest::Response, DriveError> {
        let response = response.map_err(|source| DriveError::Request { action, source })?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(DriveError::Api {
                action,
                status: response.status().as_u16(),
            })
        }
    }

    async fn archive_folder(&self) -> Result<DriveFolder, DriveError> {
        if let Some(folder) = self.find_folder_by_name("Archive").await? {
            return Ok(folder);
        }
        self.create_folder("Archive").await
    }

    async fn folder_parents(&self, folder_id: &str) -> Result<Vec<String>, DriveError> {
        let token = self.token().await?;
        let response = Self::checked(
            self.http
                .get(self.file_url(folder_id))
                .bearer_auth(token)
                .query(&[("supportsAllDrives", "true"), ("fields", "parents")])
                .send()
                .await,
            "reading folder parents",
        )?;
        response
            .json::<DriveFile>()
            .await
            .map(|file| file.parents.unwrap_or_default())
            .map_err(|source| DriveError::Response {
                action: "reading folder parents",
                source,
            })
    }

    async fn move_folder(&self, folder_id: &str, parent_id: &str) -> Result<(), DriveError> {
        let parents = self.folder_parents(folder_id).await?;
        if parents.iter().any(|parent| parent == parent_id) {
            tracing::info!(
                folder_id,
                outcome = "already_in_parent",
                "Drive folder move completed"
            );
            return Ok(());
        }
        let token = self.token().await?;
        let remove_parents = parents.join(",");
        Self::checked(
            self.http
                .patch(self.file_url(folder_id))
                .bearer_auth(token)
                .query(&[
                    ("supportsAllDrives", "true"),
                    ("addParents", parent_id),
                    ("removeParents", remove_parents.as_str()),
                ])
                .send()
                .await,
            "moving folder",
        )?;
        tracing::info!(folder_id, outcome = "moved", "Drive folder move completed");
        Ok(())
    }

    async fn permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
    ) -> Result<Option<DrivePermission>, DriveError> {
        let token = self.token().await?;
        let response = Self::checked(
            self.http
                .get(self.permissions_url(folder_id))
                .bearer_auth(token)
                .query(&[
                    ("supportsAllDrives", "true"),
                    ("fields", "permissions(id,emailAddress,type,role)"),
                ])
                .send()
                .await,
            "listing folder permissions",
        )?;
        let permissions = response
            .json::<DrivePermissions>()
            .await
            .map_err(|source| DriveError::Response {
                action: "listing folder permissions",
                source,
            })?;
        Ok(permissions.permissions.into_iter().find(|permission| {
            permission.email_address.as_deref() == Some(member.email.as_str())
                && permission.kind.as_deref() == Some(member.kind.api_value())
        }))
    }
}

#[async_trait]
impl DriveService for GoogleDrive {
    async fn create_folder(&self, project_code: &str) -> Result<DriveFolder, DriveError> {
        if let Some(existing) = self.find_folder_by_name(project_code).await? {
            tracing::info!(
                project_code,
                folder_id = existing.id.as_str(),
                outcome = "already_exists",
                "Drive folder provisioning completed"
            );
            return Ok(existing);
        }
        let token = self.token().await?;
        let response = Self::checked(
            self.http
                .post(self.files_url())
                .bearer_auth(token)
                .query(&[("supportsAllDrives", "true"), ("fields", "id,name")])
                .json(&CreateFolder {
                    name: project_code,
                    mime_type: FOLDER_MIME_TYPE,
                    parents: [&self.projects_drive_id],
                })
                .send()
                .await,
            "creating folder",
        )?;
        let folder = response
            .json::<DriveFile>()
            .await
            .map_err(|source| DriveError::Response {
                action: "creating folder",
                source,
            })?;
        let id = folder.id.ok_or(DriveError::MissingFolderId {
            action: "creating folder",
        })?;
        let created = DriveFolder {
            id,
            name: folder.name.unwrap_or_else(|| project_code.to_string()),
        };
        tracing::info!(
            project_code,
            folder_id = created.id.as_str(),
            outcome = "created",
            "Drive folder provisioning completed"
        );
        Ok(created)
    }

    async fn find_folder_by_name(
        &self,
        project_code: &str,
    ) -> Result<Option<DriveFolder>, DriveError> {
        let escaped_name = project_code.replace('\\', "\\\\").replace('\'', "\\'");
        let query = format!(
            "'{}' in parents and name = '{}' and mimeType = '{}' and trashed = false",
            self.projects_drive_id, escaped_name, FOLDER_MIME_TYPE
        );
        let token = self.token().await?;
        let response = Self::checked(
            self.http
                .get(self.files_url())
                .bearer_auth(token)
                .query(&[
                    ("q", query.as_str()),
                    ("corpora", "drive"),
                    ("driveId", self.projects_drive_id.as_str()),
                    ("includeItemsFromAllDrives", "true"),
                    ("supportsAllDrives", "true"),
                    ("fields", "files(id,name)"),
                    ("pageSize", "2"),
                ])
                .send()
                .await,
            "finding folder by name",
        )?;
        let folders = response
            .json::<DriveFiles>()
            .await
            .map_err(|source| DriveError::Response {
                action: "finding folder by name",
                source,
            })?
            .files
            .into_iter()
            .filter_map(DriveFile::into_folder)
            .collect::<Vec<_>>();
        match folders.as_slice() {
            [] => {
                tracing::info!(
                    project_code,
                    outcome = "missing",
                    "Drive folder lookup completed"
                );
                Ok(None)
            }
            [folder] => {
                tracing::info!(
                    project_code,
                    folder_id = folder.id.as_str(),
                    outcome = "found",
                    "Drive folder lookup completed"
                );
                Ok(Some(folder.clone()))
            }
            _ => {
                tracing::warn!(
                    project_code,
                    folder_count = folders.len(),
                    outcome = "ambiguous",
                    "Drive folder lookup refused to guess"
                );
                Err(DriveError::AmbiguousFolderName {
                    count: folders.len(),
                })
            }
        }
    }

    async fn list_folders(&self) -> Result<Vec<DriveFolder>, DriveError> {
        let query = format!(
            "'{}' in parents and mimeType = '{}' and trashed = false",
            self.projects_drive_id, FOLDER_MIME_TYPE
        );
        let token = self.token().await?;
        let response = Self::checked(
            self.http
                .get(self.files_url())
                .bearer_auth(token)
                .query(&[
                    ("q", query.as_str()),
                    ("corpora", "drive"),
                    ("driveId", self.projects_drive_id.as_str()),
                    ("includeItemsFromAllDrives", "true"),
                    ("supportsAllDrives", "true"),
                    ("fields", "files(id,name)"),
                    ("orderBy", "name"),
                ])
                .send()
                .await,
            "listing project folders",
        )?;
        let folders = response
            .json::<DriveFiles>()
            .await
            .map_err(|source| DriveError::Response {
                action: "listing project folders",
                source,
            })
            .map(|files| {
                files
                    .files
                    .into_iter()
                    .filter_map(DriveFile::into_folder)
                    .collect::<Vec<_>>()
            })?;
        tracing::info!(
            folder_count = folders.len(),
            outcome = "listed",
            "Drive project folders listed"
        );
        Ok(folders)
    }

    async fn set_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
        role: DriveRole,
    ) -> Result<(), DriveError> {
        let existing = self.permission(folder_id, member).await?;
        let token = self.token().await?;
        if let Some(permission) = existing {
            if permission.role.as_deref() == Some(role.api_value()) {
                tracing::info!(
                    folder_id,
                    outcome = "already_set",
                    "Drive folder permission reconciliation completed"
                );
                return Ok(());
            }
            let permission_id = permission.id.ok_or(DriveError::MissingPermissionId {
                action: "updating folder permission",
            })?;
            Self::checked(
                self.http
                    .patch(format!(
                        "{}/{}",
                        self.permissions_url(folder_id),
                        permission_id
                    ))
                    .bearer_auth(token)
                    .query(&[("supportsAllDrives", "true")])
                    .json(&PermissionRole {
                        role: role.api_value(),
                    })
                    .send()
                    .await,
                "updating folder permission",
            )?;
            tracing::info!(
                folder_id,
                outcome = "updated",
                "Drive folder permission reconciliation completed"
            );
        } else {
            Self::checked(
                self.http
                    .post(self.permissions_url(folder_id))
                    .bearer_auth(token)
                    .query(&[
                        ("supportsAllDrives", "true"),
                        ("sendNotificationEmail", "false"),
                    ])
                    .json(&CreatePermission {
                        kind: member.kind.api_value(),
                        role: role.api_value(),
                        email_address: &member.email,
                    })
                    .send()
                    .await,
                "creating folder permission",
            )?;
            tracing::info!(
                folder_id,
                outcome = "created",
                "Drive folder permission reconciliation completed"
            );
        }
        Ok(())
    }

    async fn remove_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
    ) -> Result<(), DriveError> {
        let Some(permission) = self.permission(folder_id, member).await? else {
            tracing::info!(
                folder_id,
                outcome = "already_absent",
                "Drive folder permission reconciliation completed"
            );
            return Ok(());
        };
        let permission_id = permission.id.ok_or(DriveError::MissingPermissionId {
            action: "removing folder permission",
        })?;
        let token = self.token().await?;
        Self::checked(
            self.http
                .delete(format!(
                    "{}/{}",
                    self.permissions_url(folder_id),
                    permission_id
                ))
                .bearer_auth(token)
                .query(&[("supportsAllDrives", "true")])
                .send()
                .await,
            "removing folder permission",
        )?;
        tracing::info!(
            folder_id,
            outcome = "removed",
            "Drive folder permission reconciliation completed"
        );
        Ok(())
    }

    async fn move_to_archive(&self, folder_id: &str) -> Result<(), DriveError> {
        let archive = self.archive_folder().await?;
        self.move_folder(folder_id, &archive.id).await
    }

    async fn restore_from_archive(&self, folder_id: &str) -> Result<(), DriveError> {
        self.move_folder(folder_id, &self.projects_drive_id).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: Option<String>,
    name: Option<String>,
    parents: Option<Vec<String>>,
}

impl DriveFile {
    fn into_folder(self) -> Option<DriveFolder> {
        Some(DriveFolder {
            id: self.id?,
            name: self.name?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct DriveFiles {
    #[serde(default)]
    files: Vec<DriveFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePermission {
    id: Option<String>,
    email_address: Option<String>,
    kind: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePermissions {
    #[serde(default)]
    permissions: Vec<DrivePermission>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateFolder<'a> {
    name: &'a str,
    mime_type: &'a str,
    parents: [&'a String; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatePermission<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    role: &'a str,
    email_address: &'a str,
}

#[derive(Serialize)]
struct PermissionRole<'a> {
    role: &'a str,
}

/// In-memory Drive implementation for workflow and store tests. It models the
/// idempotent folder/permission semantics without requiring a Workspace.
#[derive(Clone, Default)]
pub struct FakeDrive {
    state: Arc<Mutex<FakeDriveState>>,
}

#[derive(Default)]
struct FakeDriveState {
    folders: BTreeMap<String, FakeFolder>,
    next_id: usize,
}

#[derive(Default)]
struct FakeFolder {
    id: String,
    archived: bool,
    permissions: BTreeSet<(DriveMember, DriveRole)>,
}

impl FakeDrive {
    #[must_use]
    pub fn members(&self, folder_id: &str) -> Vec<(DriveMember, DriveRole)> {
        let state = self.state.lock().expect("fake drive mutex poisoned");
        state
            .folders
            .values()
            .find(|folder| folder.id == folder_id)
            .map(|folder| folder.permissions.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_archived(&self, folder_id: &str) -> bool {
        let state = self.state.lock().expect("fake drive mutex poisoned");
        state
            .folders
            .values()
            .find(|folder| folder.id == folder_id)
            .is_some_and(|folder| folder.archived)
    }
}

#[async_trait]
impl DriveService for FakeDrive {
    async fn create_folder(&self, project_code: &str) -> Result<DriveFolder, DriveError> {
        let mut state = self.state.lock().expect("fake drive mutex poisoned");
        if let Some(folder) = state.folders.get(project_code) {
            return Ok(DriveFolder {
                id: folder.id.clone(),
                name: project_code.to_string(),
            });
        }
        state.next_id += 1;
        let id = format!("folder-{}", state.next_id);
        state.folders.insert(
            project_code.to_string(),
            FakeFolder {
                id: id.clone(),
                ..Default::default()
            },
        );
        Ok(DriveFolder {
            id,
            name: project_code.to_string(),
        })
    }

    async fn find_folder_by_name(
        &self,
        project_code: &str,
    ) -> Result<Option<DriveFolder>, DriveError> {
        let state = self.state.lock().expect("fake drive mutex poisoned");
        Ok(state.folders.get(project_code).map(|folder| DriveFolder {
            id: folder.id.clone(),
            name: project_code.to_string(),
        }))
    }

    async fn list_folders(&self) -> Result<Vec<DriveFolder>, DriveError> {
        let state = self.state.lock().expect("fake drive mutex poisoned");
        Ok(state
            .folders
            .iter()
            .filter(|(_, folder)| !folder.archived)
            .map(|(name, folder)| DriveFolder {
                id: folder.id.clone(),
                name: name.clone(),
            })
            .collect())
    }

    async fn set_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
        role: DriveRole,
    ) -> Result<(), DriveError> {
        let mut state = self.state.lock().expect("fake drive mutex poisoned");
        if let Some(folder) = state
            .folders
            .values_mut()
            .find(|folder| folder.id == folder_id)
        {
            folder.permissions.retain(|(current, _)| current != member);
            folder.permissions.insert((member.clone(), role));
        }
        Ok(())
    }

    async fn remove_member_permission(
        &self,
        folder_id: &str,
        member: &DriveMember,
    ) -> Result<(), DriveError> {
        let mut state = self.state.lock().expect("fake drive mutex poisoned");
        if let Some(folder) = state
            .folders
            .values_mut()
            .find(|folder| folder.id == folder_id)
        {
            folder.permissions.retain(|(current, _)| current != member);
        }
        Ok(())
    }

    async fn move_to_archive(&self, folder_id: &str) -> Result<(), DriveError> {
        let mut state = self.state.lock().expect("fake drive mutex poisoned");
        if let Some(folder) = state
            .folders
            .values_mut()
            .find(|folder| folder.id == folder_id)
        {
            folder.archived = true;
        }
        Ok(())
    }

    async fn restore_from_archive(&self, folder_id: &str) -> Result<(), DriveError> {
        let mut state = self.state.lock().expect("fake drive mutex poisoned");
        if let Some(folder) = state
            .folders
            .values_mut()
            .find(|folder| folder.id == folder_id)
        {
            folder.archived = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        DriveMember, DriveMemberKind, DriveRole, DriveService, DriveWorkspace,
        DriveWorkspaceConfig, FakeDrive, GoogleDrive,
    };
    use async_trait::async_trait;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Debug)]
    struct StaticToken;

    #[async_trait]
    impl google_cloud_token::TokenSource for StaticToken {
        async fn token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Ok("Bearer test-token".into())
        }
    }

    /// A missing coordinate names itself, prefix included.
    ///
    /// The error text is the whole assertion: an operator reading it has to
    /// learn *which* variable to set, and a message naming only the field would
    /// send them looking through every workspace prefix.
    #[test]
    fn configuration_requires_each_workspace_coordinate() {
        let error =
            DriveWorkspaceConfig::from_lookup(DriveWorkspace::NeonLaw, |_| None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID"),
            "{error}"
        );
    }

    #[test]
    fn configuration_redacts_workspace_credentials() {
        let config = DriveWorkspaceConfig::from_lookup(DriveWorkspace::NeonLaw, |key| match key {
            "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID" => Some("drive-id".into()),
            "NAVIGATOR_DRIVE_NEON_LAW_DELEGATED_USER" => Some("operator@example.com".into()),
            "NAVIGATOR_DRIVE_NEON_LAW_SERVICE_ACCOUNT_JSON" => Some("private-key".into()),
            _ => None,
        })
        .unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("operator@example.com"));
        assert!(!debug.contains("private-key"));
    }

    #[tokio::test]
    async fn fake_drive_converges_folder_membership_and_archive_state() {
        let drive = Arc::new(FakeDrive::default());
        let folder = drive.create_folder("matter-42").await.unwrap();
        assert_eq!(drive.create_folder("matter-42").await.unwrap(), folder);

        let member = DriveMember {
            kind: DriveMemberKind::User,
            email: "lawyer@example.com".into(),
        };
        drive
            .set_member_permission(&folder.id, &member, DriveRole::Reader)
            .await
            .unwrap();
        drive
            .set_member_permission(&folder.id, &member, DriveRole::Writer)
            .await
            .unwrap();
        assert_eq!(
            drive.members(&folder.id),
            vec![(member.clone(), DriveRole::Writer)]
        );

        drive.move_to_archive(&folder.id).await.unwrap();
        assert!(drive.is_archived(&folder.id));
        drive.restore_from_archive(&folder.id).await.unwrap();
        assert!(!drive.is_archived(&folder.id));
        drive
            .remove_member_permission(&folder.id, &member)
            .await
            .unwrap();
        assert!(drive.members(&folder.id).is_empty());
    }

    #[tokio::test]
    async fn google_drive_creates_a_folder_in_the_configured_shared_drive() {
        let server = MockServer::start().await;
        let lookup = "'projects-drive' in parents and name = 'matter-42' and mimeType = 'application/vnd.google-apps.folder' and trashed = false";
        Mock::given(method("GET"))
            .and(path("/files"))
            .and(query_param("q", lookup))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"files": []})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/files"))
            .and(query_param("supportsAllDrives", "true"))
            .and(body_json(serde_json::json!({
                "name": "matter-42",
                "mimeType": "application/vnd.google-apps.folder",
                "parents": ["projects-drive"]
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "folder-42", "name": "matter-42"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let drive = GoogleDrive::from_parts(
            "projects-drive".into(),
            Arc::new(StaticToken),
            &server.uri(),
        );
        assert_eq!(
            drive.create_folder("matter-42").await.unwrap().id,
            "folder-42"
        );
    }

    #[tokio::test]
    async fn google_drive_sets_a_user_permission_without_notification() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files/folder-42/permissions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"permissions": []})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/files/folder-42/permissions"))
            .and(query_param("sendNotificationEmail", "false"))
            .and(body_json(serde_json::json!({
                "type": "user",
                "role": "writer",
                "emailAddress": "lawyer@example.com"
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let drive = GoogleDrive::from_parts(
            "projects-drive".into(),
            Arc::new(StaticToken),
            &server.uri(),
        );
        drive
            .set_member_permission(
                "folder-42",
                &DriveMember {
                    kind: DriveMemberKind::User,
                    email: "lawyer@example.com".into(),
                },
                DriveRole::Writer,
            )
            .await
            .unwrap();
    }
}
