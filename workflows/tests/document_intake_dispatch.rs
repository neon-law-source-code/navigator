//! Integration test for the reusable `document_intake__*` step.
//!
//! Drives the dispatch through the shared `workflows::dispatch_step`
//! registry — the same arm the `workflows-service` worker runs inside
//! `ctx.run` — and asserts the provided artifact lands as a
//! content-addressed blob + `documents` row on the notation's project,
//! via `store::documents::ingest_bytes`. Runs against an embedded,
//! memory-backed store because the side effect writes real rows.

use std::sync::Arc;

use store::test_support::mem_surreal;

use workflows::{dispatch_step, IntakeArtifact, IntakePayload, StateName, StepDeps};

async fn fs_storage() -> Arc<dyn cloud::StorageService> {
    Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-intake-dispatch-test"))
            .await
            .expect("temp FsStorage"),
    )
}

fn deps(surreal: store::surreal::SurrealDb, storage: Arc<dyn cloud::StorageService>) -> StepDeps {
    // Email is unused by the intake arm; any EmailService satisfies the
    // struct.
    StepDeps::new(Arc::new(workflows::CapturingEmail::new()), storage).with_surreal(surreal)
}

#[tokio::test]
async fn document_intake_files_a_text_transcript_into_the_matter() {
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation(&surreal).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;

    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let payload = serde_json::to_string(&IntakePayload {
        kind: "transcript".into(), // rules::kind::Kind::Transcript — asset-lane only
        filename: Some("sitting-transcript.txt".into()),
        artifact: IntakeArtifact::Text {
            text: "Consent given. Executor: Aries. Trustee: Capricorn.".into(),
        },
    })
    .unwrap();

    dispatch_step(
        &deps,
        notation_id,
        &StateName::from("document_intake__transcript"),
        Some(&payload),
    )
    .await
    .expect("document_intake dispatch files the transcript");

    // A document `assets` row landed on the notation's project, carrying
    // the intake's kind/filename and the `upload` provenance.
    let docs = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap();
    let doc = docs
        .iter()
        .find(|d| d.project_id == Some(project_id))
        .expect("a document filed on the project");
    assert_eq!(doc.kind.as_deref(), Some("transcript"));
    assert_eq!(doc.filename.as_deref(), Some("sitting-transcript.txt"));
    assert_eq!(doc.source.as_deref(), Some("upload"));

    // And the bytes are retrievable from storage through the asset.
    assert_eq!(doc.content_type, "text/plain");
    let stored = storage.get(&doc.storage_key).await.unwrap();
    assert_eq!(
        stored.bytes,
        b"Consent given. Executor: Aries. Trustee: Capricorn."
    );
}

#[tokio::test]
async fn document_intake_link_artifact_files_a_uri_list_pointer() {
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation(&surreal).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;
    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let payload = serde_json::to_string(&IntakePayload {
        kind: "transcript".into(), // rules::kind::Kind::Transcript — asset-lane only
        filename: Some("zoom-recording.url".into()),
        artifact: IntakeArtifact::Link {
            url: "https://zoom.example/rec/abc123".into(),
        },
    })
    .unwrap();

    dispatch_step(
        &deps,
        notation_id,
        &StateName::from("document_intake__transcript"),
        Some(&payload),
    )
    .await
    .expect("link intake dispatch succeeds");

    let doc = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.filename.as_deref() == Some("zoom-recording.url"))
        .expect("link pointer document filed");
    assert_eq!(doc.content_type, "text/uri-list");
    let stored = storage.get(&doc.storage_key).await.unwrap();
    assert_eq!(stored.bytes, b"https://zoom.example/rec/abc123");
}

#[tokio::test]
async fn document_intake_rejects_a_kind_the_closed_vocabulary_does_not_recognize() {
    // The free-text `IntakePayload.kind` is validated against
    // `rules::kind::Kind::parse` at the dispatch boundary (issue #780) —
    // a typo or an invented classification must not reach `assets.kind`.
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation(&surreal).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;
    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let payload = serde_json::to_string(&IntakePayload {
        kind: "sitting_notes".into(), // not in rules::kind::Kind
        filename: Some("sitting-transcript.txt".into()),
        artifact: IntakeArtifact::Text {
            text: "Consent given.".into(),
        },
    })
    .unwrap();

    let err = dispatch_step(
        &deps,
        notation_id,
        &StateName::from("document_intake__sitting_notes"),
        Some(&payload),
    )
    .await
    .expect_err("an unrecognized document kind must be rejected");
    assert!(
        err.to_string().contains("sitting_notes"),
        "expected an UnknownKind error naming the bad value, got: {err}"
    );

    let filed = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap();
    assert!(
        filed.is_empty(),
        "a rejected document_intake dispatch must not file a partial asset"
    );
}

#[tokio::test]
async fn document_intake_reuses_a_prefiled_immutable_asset() {
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation(&surreal).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;
    let storage = fs_storage().await;
    let original = b"immutable synthetic contract bytes";
    let filed = store::documents::ingest_bytes(
        &surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id,
            source: store::documents::source::UPLOAD,
            filename: "synthetic-contract.docx",
            kind: "inbound_contract",
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        original,
    )
    .await
    .unwrap();
    let deps = deps(surreal.clone(), storage.clone());
    let payload = serde_json::to_string(&IntakePayload {
        kind: "inbound_contract".into(),
        filename: None,
        artifact: IntakeArtifact::Stored {
            asset_id: filed.asset_id,
        },
    })
    .unwrap();

    let output = dispatch_step(
        &deps,
        notation_id,
        &StateName::from("document_intake__inbound_contract"),
        Some(&payload),
    )
    .await
    .expect("prefiled asset dispatch succeeds");
    assert!(output.is_none());
    assert_eq!(
        storage.get(&filed.storage_key).await.unwrap().bytes,
        original
    );
    assert_eq!(
        store::assets::for_project(&surreal, project_id)
            .await
            .unwrap()
            .len(),
        1
    );
}
