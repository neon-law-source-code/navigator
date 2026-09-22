use std::sync::Arc;

use cloud::{DriveService, FakeDrive, FsStorage, StorageService};
use serde_json::json;
use store::documents::{source, visibility};
use store::drive_import::{import_project_files, DriveImportArgs};
use store::projects::{create, set_drive_folder_id, NewProject};
use store::test_support::{mem_surreal, seed_entity};

#[tokio::test]
async fn drive_files_become_project_assets_with_object_storage_provenance() {
    let surreal = mem_surreal().await;
    let project = create(
        &surreal,
        &NewProject {
            code: "drive-import".to_string(),
            name: "Drive Import Matter".to_string(),
            status: "open".to_string(),
            entity_id: seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("create matter");
    let drive = FakeDrive::default();
    let folder = drive
        .create_folder(&project.code)
        .await
        .expect("folder creation");
    set_drive_folder_id(&surreal, project.id, Some(&folder.id))
        .await
        .expect("set Drive folder")
        .expect("matter exists");
    let file = drive
        .add_file(
            &folder.id,
            "notice.pdf",
            "application/pdf",
            b"synthetic drive bytes".to_vec(),
            Some("2026-09-21T12:00:00Z".to_string()),
        )
        .expect("file creation");
    let temp = tempfile::tempdir().expect("storage directory");
    let storage: Arc<dyn StorageService> =
        Arc::new(FsStorage::new(temp.path()).await.expect("storage"));

    let imported = import_project_files(
        &surreal,
        &storage,
        &drive,
        project.id,
        &DriveImportArgs {
            kind: "unclassified",
            visibility: visibility::CLIENT,
            description: Some("Dropped in Workspace"),
        },
    )
    .await
    .expect("Drive import");

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].drive_file_id, file.id);
    let assets = store::assets::for_project(&surreal, project.id)
        .await
        .expect("asset listing");
    assert_eq!(assets.len(), 1);
    let asset = &assets[0];
    assert_eq!(asset.source.as_deref(), Some(source::DRIVE));
    assert_eq!(asset.visibility, visibility::CLIENT);
    assert_eq!(asset.description.as_deref(), Some("Dropped in Workspace"));
    assert_eq!(
        asset.metadata,
        Some(json!({
            "drive_file_id": file.id,
            "drive_modified_time": "2026-09-21T12:00:00Z"
        }))
    );
    assert!(asset
        .storage_key
        .starts_with("projects/drive-import/documents/"));
    assert_eq!(
        storage
            .get(&asset.storage_key)
            .await
            .expect("stored bytes")
            .bytes,
        b"synthetic drive bytes"
    );

    let repeated = import_project_files(
        &surreal,
        &storage,
        &drive,
        project.id,
        &DriveImportArgs {
            kind: "unclassified",
            visibility: visibility::CLIENT,
            description: Some("Dropped in Workspace"),
        },
    )
    .await
    .expect("idempotent Drive import");
    assert_eq!(repeated.len(), 1);
    assert_eq!(
        store::assets::for_project(&surreal, project.id)
            .await
            .expect("asset listing")
            .len(),
        1,
        "re-importing unchanged Drive bytes does not duplicate the asset"
    );
}
