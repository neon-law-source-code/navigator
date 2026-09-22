use cloud::{DriveService, FakeDrive};

#[tokio::test]
async fn fake_drive_lists_and_downloads_project_files() {
    let drive = FakeDrive::default();
    let folder = drive
        .create_folder("synthetic-matter")
        .await
        .expect("folder creation");
    let file = drive
        .add_file(
            &folder.id,
            "notice.pdf",
            "application/pdf",
            b"synthetic drive bytes".to_vec(),
            Some("2026-09-21T12:00:00Z".to_string()),
        )
        .expect("file creation");

    let files = drive.list_files(&folder.id).await.expect("file listing");
    assert_eq!(files, vec![file.clone()]);

    let downloaded = drive.download_file(&file.id).await.expect("file download");
    assert_eq!(downloaded.content_type, "application/pdf");
    assert_eq!(downloaded.bytes, b"synthetic drive bytes");
}
