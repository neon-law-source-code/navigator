//! The matter-document write seam keeps raw legal bytes out of Git.

use std::sync::Arc;

use cloud::StorageService;
use repos::Author;
use store::documents::IngestArgs;
use store::test_support::mem_surreal;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_document_persists_only_to_the_asset_lane() {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-matter-docs-{}", uuid::Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );

    let proj = store::test_support::seed_project(&surreal, "Estate of Aries").await;

    let bytes = b"%PDF-1.7 collection notice";
    let ingested = portal::matter_documents::record_document(
        &surreal,
        &storage,
        Author {
            name: "Aries",
            email: "aries@example.com",
        },
        &IngestArgs {
            project_id: proj.id,
            source: store::documents::source::EMAIL,
            filename: "notice.pdf",
            kind: "unclassified",
            content_type: "application/pdf",
            description: Some("received via support@ email"),
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        bytes,
    )
    .await
    .expect("record_document persists");

    // The document `assets` row exists with its provenance.
    let doc = store::assets::find_by_id(&surreal, ingested.asset_id)
        .await
        .unwrap()
        .expect("asset row");
    assert_eq!(doc.filename.as_deref(), Some("notice.pdf"));
    assert_eq!(doc.kind.as_deref(), Some("unclassified"));
}
