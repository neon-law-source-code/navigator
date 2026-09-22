//! Operator-triggered ingestion from a Project's Drive dropbox.
//!
//! Drive is an ingest source, not a document store. This module lists the
//! files under the Project's recorded folder, copies their bytes through the
//! canonical document asset lane, and leaves the portal's existing
//! object-storage delivery path unchanged. No request handler calls this
//! module.

use std::sync::Arc;

use cloud::{DriveService, StorageService};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::assets::{self, Filed, RevisionError};
use crate::documents::{self, DocumentIdentity, IngestArgs};
use crate::projects;
use crate::surreal::SurrealDb;

/// Maximum number of Drive files one import pass may admit.
pub const MAX_FILES: usize = 50;
/// Maximum size of one Drive file admitted to the buffered ingest seam.
pub const MAX_FILE_BYTES: usize = documents::MAX_DOCUMENT_UPLOAD_BYTES;
/// Maximum total bytes one import pass may buffer and file.
pub const MAX_TOTAL_BYTES: usize = 500 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum DriveImportError {
    #[error("Drive: {0}")]
    Drive(#[from] cloud::DriveError),
    #[error("project: {0}")]
    Project(#[from] projects::ProjectStoreError),
    #[error("asset: {0}")]
    Asset(#[from] RevisionError),
    #[error("Project has no Drive ingest folder")]
    MissingFolder,
    #[error("Drive import contains {actual} files; the maximum is {MAX_FILES}")]
    TooManyFiles { actual: usize },
    #[error("Drive file `{file_id}` is {actual} bytes; the maximum is {MAX_FILE_BYTES}")]
    FileTooLarge { file_id: String, actual: u64 },
    #[error("Drive import exceeds the maximum of {MAX_TOTAL_BYTES} bytes")]
    BatchTooLarge,
}

/// Inputs chosen by the operator for a Drive import pass.
#[derive(Debug, Clone, Copy)]
pub struct DriveImportArgs<'a> {
    pub kind: &'a str,
    pub visibility: &'a str,
    pub description: Option<&'a str>,
}

/// The asset identity produced for one Drive file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDriveFile {
    pub drive_file_id: String,
    pub asset_id: Uuid,
    pub filename: String,
}

/// Import the current direct children of a Project's Drive folder.
///
/// Each file receives a stable Drive-specific document identity, so a
/// changed Drive file becomes a new asset revision while an unchanged retry
/// is a no-op. The stored row carries both `source = "drive"` and the Drive
/// file metadata; subsequent reads use only the asset row and object storage.
pub async fn import_project_files(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    drive: &dyn DriveService,
    project_id: Uuid,
    args: &DriveImportArgs<'_>,
) -> Result<Vec<ImportedDriveFile>, DriveImportError> {
    let project = projects::find_by_id(db, project_id)
        .await?
        .ok_or(projects::ProjectStoreError::NoSuchProject(project_id))?;
    let folder_id = project
        .drive_folder_id
        .as_deref()
        .ok_or(DriveImportError::MissingFolder)?;
    let listed_files = drive.list_files(folder_id).await?;
    if listed_files.len() > MAX_FILES {
        return Err(DriveImportError::TooManyFiles {
            actual: listed_files.len(),
        });
    }

    let mut total_bytes = 0usize;
    let mut imported = Vec::with_capacity(listed_files.len());
    for file in listed_files {
        if let Some(size) = file.size_bytes {
            if size > MAX_FILE_BYTES as u64 {
                return Err(DriveImportError::FileTooLarge {
                    file_id: file.id,
                    actual: size,
                });
            }
            let size = usize::try_from(size).map_err(|_| DriveImportError::BatchTooLarge)?;
            if total_bytes
                .checked_add(size)
                .is_none_or(|total| total > MAX_TOTAL_BYTES)
            {
                return Err(DriveImportError::BatchTooLarge);
            }
        }

        let downloaded = drive.download_file(&file.id).await?;
        if downloaded.bytes.len() > MAX_FILE_BYTES {
            return Err(DriveImportError::FileTooLarge {
                file_id: file.id,
                actual: downloaded.bytes.len() as u64,
            });
        }
        total_bytes = total_bytes
            .checked_add(downloaded.bytes.len())
            .ok_or(DriveImportError::BatchTooLarge)?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(DriveImportError::BatchTooLarge);
        }

        let content_type = if downloaded.content_type == "application/octet-stream"
            && !file.mime_type.trim().is_empty()
        {
            file.mime_type.as_str()
        } else {
            downloaded.content_type.as_str()
        };
        let slug = format!("drive-{}", file.id);
        let identity = DocumentIdentity {
            slug: Some(&slug),
            published_at: None,
            metadata: Some(json!({
                "drive_file_id": file.id,
                "drive_modified_time": file.modified_time,
            })),
        };
        let ingest = IngestArgs {
            project_id,
            source: documents::source::DRIVE,
            filename: &file.name,
            kind: args.kind,
            content_type,
            description: args.description,
            secondary_storage_key: None,
            visibility: args.visibility,
        };
        let revision =
            assets::file_revision(db, storage, &ingest, &identity, &downloaded.bytes).await?;
        let asset_id = match revision {
            Filed::Revision(document) => document.asset_id,
            Filed::Unchanged { asset_id } => asset_id,
        };
        imported.push(ImportedDriveFile {
            drive_file_id: file.id,
            asset_id,
            filename: file.name,
        });
    }
    Ok(imported)
}
