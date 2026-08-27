//! The matter-delete referential guard: every table that can point at a
//! matter must refuse the delete rather than let it through.
//!
//! A guard reading an empty table looks exactly like a guard that
//! passed, so a check pointed at the wrong place reports "nothing
//! references this" for a matter that is in fact referenced. These tests
//! assert the refusal itself, one referenced row at a time, and name the
//! table in the assertion — `templates`, `assets`, `communications`,
//! `expunge_requests`, and `expunge_records` — so a check that reads
//! anywhere the rows are not can only ever fail them.

use std::sync::Arc;

use store::projects::ProjectCommandError;
use store::surreal::SurrealDb;
use store::test_support::{mem_surreal, seed_project_surreal};
use uuid::Uuid;

/// Seed a person to hang the expunge fixtures off.
async fn seed_person(surreal: &SurrealDb) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::new(
            "Guard Fixture",
            format!("guard-{}@example.com", Uuid::now_v7()),
        ),
    )
    .await
    .expect("seed person")
    .id
}

/// Ingest one document onto `project_id`, returning its asset id.
async fn seed_asset(surreal: &SurrealDb, project_id: Uuid) -> Uuid {
    let dir = tempfile::tempdir().expect("temp dir");
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(dir.path().to_path_buf())
            .await
            .expect("filesystem storage"),
    );
    store::documents::ingest_bytes(
        surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id,
            source: "upload",
            filename: "exhibit.pdf",
            kind: "unclassified",
            content_type: "application/pdf",
            description: None,
            visibility: store::documents::visibility::INTERNAL,
            secondary_storage_key: None,
        },
        b"guard fixture bytes",
    )
    .await
    .expect("ingest document")
    .asset_id
}

/// Delete the matter and return the table the guard named, panicking if
/// the delete was allowed through — which is the bug this file covers.
async fn refused_table(surreal: &SurrealDb, project_id: Uuid) -> String {
    match store::projects::delete_project_with_surreal(surreal, project_id).await {
        Err(ProjectCommandError::Referenced(table)) => table,
        Ok(_) => panic!("the matter was deleted even though a row still referenced it"),
        Err(other) => panic!("expected a Referenced refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_matter_holding_a_template_is_not_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Template Holder").await;

    store::templates::save_version(
        &surreal,
        Some(project),
        "guard-template",
        store::templates::Version {
            title: "Guard Template".into(),
            respondent_type: "org".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .expect("save a project-scoped template");

    assert_eq!(refused_table(&surreal, project).await, "templates");
}

#[tokio::test]
async fn a_matter_holding_an_asset_is_not_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Asset Holder").await;
    seed_asset(&surreal, project).await;

    assert_eq!(refused_table(&surreal, project).await, "assets");
}

#[tokio::test]
async fn a_matter_holding_a_communication_is_not_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Communication Holder").await;

    store::communications::ingest(
        &surreal,
        &store::communications::IngestArgs {
            project_id: project,
            channel: store::communications::channel::PORTAL_MESSAGE,
            direction: store::communications::direction::INBOUND,
            author_person_id: None,
            counterparty: Some("client@example.com"),
            subject: Some("Guard fixture"),
            body: "One message is enough to refuse the delete.",
            source_ref: None,
            asset_id: None,
            occurred_at: "2026-08-10T00:00:00Z",
        },
    )
    .await
    .expect("ingest a message");

    assert_eq!(refused_table(&surreal, project).await, "communications");
}

#[tokio::test]
async fn a_matter_holding_an_expunge_request_is_not_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Expunge Request Holder").await;
    let asset = seed_asset(&surreal, project).await;
    let person = seed_person(&surreal).await;

    store::expunge_requests::create(
        &surreal,
        &store::expunge_requests::NewExpungeRequest {
            project_id: project,
            asset_id: asset,
            requested_by_person_id: person,
            note: None,
        },
    )
    .await
    .expect("file an expunge request");

    // The asset seeded above is itself a reference, and `assets` is the
    // last check the guard runs — so the request is what this asserts.
    assert_eq!(refused_table(&surreal, project).await, "expunge_requests");
}

#[tokio::test]
async fn a_matter_holding_an_expunge_record_is_not_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Expunge Record Holder").await;
    let person = seed_person(&surreal).await;

    store::expunge_records::record(
        &surreal,
        &store::expunge_records::NewExpunge {
            project_id: project,
            path: "matters/guard/exhibit.pdf",
            category: store::expunge_records::CATEGORY_SEALING,
            authorized_by_person_id: person,
            head_before: None,
            head_after: None,
            note: None,
        },
    )
    .await
    .expect("record an expunge");

    assert_eq!(refused_table(&surreal, project).await, "expunge_records");
}

/// The other half of the guard: with nothing referencing it, the matter
/// really is deleted. Without this, every assertion above would still pass
/// against a guard that refused unconditionally.
#[tokio::test]
async fn an_unreferenced_matter_is_deleted() {
    let surreal = mem_surreal().await;
    let project = seed_project_surreal(&surreal, "Nothing Points Here").await;

    store::projects::delete_project_with_surreal(&surreal, project)
        .await
        .expect("an unreferenced matter deletes");

    assert!(
        store::projects::find_by_id(&surreal, project)
            .await
            .expect("query")
            .is_none(),
        "the matter should be gone"
    );
}
