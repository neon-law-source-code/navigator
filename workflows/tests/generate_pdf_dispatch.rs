//! Integration test for the `generate_pdf__*` step's persistence.
//!
//! Drives the dispatch through the shared `workflows::dispatch_step`
//! registry — the same arm the `workflows-service` worker runs inside
//! `ctx.run` — and asserts the rendered PDF is (a) written to object
//! storage where the signature step reads it back, and (b) filed into the
//! matter as a content-addressed document `assets` row via
//! `store::documents::ingest_bytes`. The dispatch returns a
//! `GeneratedPdfRef` the worker journals on the transition's
//! `notation_events.payload`, so the audit trail links to the persisted
//! intermediary PDF. Runs against an embedded, memory-backed store
//! because the side effect writes real rows.

use std::sync::Arc;

use store::test_support::mem_surreal;

use workflows::{dispatch_step, DocumentPayload, GeneratedPdfRef, StateName, StepDeps};

async fn fs_storage() -> Arc<dyn cloud::StorageService> {
    Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-generate-pdf-dispatch-test"))
            .await
            .expect("temp FsStorage"),
    )
}

fn deps(surreal: store::surreal::SurrealDb, storage: Arc<dyn cloud::StorageService>) -> StepDeps {
    // Email is unused by the generate_pdf arm; any EmailService satisfies
    // the struct.
    StepDeps::new(Arc::new(workflows::CapturingEmail::new()), storage).with_surreal(surreal)
}

#[tokio::test]
async fn generate_pdf_persists_the_render_as_a_document_asset() {
    let surreal = mem_surreal().await;
    let notation_id =
        store::test_support::seed_notation_with_kind(&surreal, Some("onboarding")).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;

    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let storage_key = format!("notations/{notation_id}/document.pdf");
    let payload = serde_json::to_string(&DocumentPayload::Typst {
        storage_key: storage_key.clone(),
        typst_source: "Retainer body for the audit trail.".into(),
    })
    .unwrap();

    let recorded = dispatch_step(
        &deps,
        notation_id,
        &StateName::from("generate_pdf__retainer_pdf"),
        Some(&payload),
    )
    .await
    .expect("generate_pdf dispatch renders + persists")
    .expect("a generate_pdf step returns an asset-ref payload when a db is configured");

    // The rendered PDF is at the object-storage key the signature step
    // reads back.
    let served = storage.get(&storage_key).await.expect("object-storage PDF");
    assert!(served.bytes.starts_with(b"%PDF"));

    // A project-scoped document `assets` row files the render into the
    // matter, carrying the notation's pinned *template's* declared kind
    // (not the `generate_pdf__retainer_pdf` state slug) plus `generated`
    // provenance, and holding the same bytes as the served PDF.
    let doc = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.project_id == Some(project_id))
        .expect("a document filed on the notation's project");
    assert_eq!(doc.kind.as_deref(), Some("onboarding"));
    assert_eq!(doc.source.as_deref(), Some("generated"));
    assert_eq!(doc.content_type, "application/pdf");
    let stored = storage.get(&doc.storage_key).await.unwrap();
    assert_eq!(
        stored.bytes, served.bytes,
        "asset bytes match the served PDF"
    );

    // The journal payload links straight to that asset row — the auditable
    // reference the worker records on the generate_pdf transition's
    // `notation_events.payload`.
    let pdf_ref: GeneratedPdfRef =
        serde_json::from_str(&recorded).expect("payload is a GeneratedPdfRef");
    assert_eq!(pdf_ref.asset_id, doc.id);
    assert_eq!(pdf_ref.storage_key, doc.storage_key);
    assert_eq!(pdf_ref.storage_key, format!("blobs/{}", pdf_ref.sha256));
    assert!(pdf_ref.byte_size > 0);
}

#[tokio::test]
async fn generate_pdf_dedups_storage_when_the_same_bytes_render_twice() {
    // Content-addressing makes the persistence idempotent: re-rendering the
    // same source (a Restate replay, or a re-approve) files a second
    // document `assets` row but reuses the one content-addressed storage
    // object — the audit trail stays honest without paying storage twice.
    let surreal = mem_surreal().await;
    let notation_id =
        store::test_support::seed_notation_with_kind(&surreal, Some("onboarding")).await;
    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let payload = serde_json::to_string(&DocumentPayload::Typst {
        storage_key: format!("notations/{notation_id}/document.pdf"),
        typst_source: "Identical bytes, rendered twice.".into(),
    })
    .unwrap();
    let state = StateName::from("generate_pdf__retainer_pdf");

    let first: GeneratedPdfRef = serde_json::from_str(
        &dispatch_step(&deps, notation_id, &state, Some(&payload))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let second: GeneratedPdfRef = serde_json::from_str(
        &dispatch_step(&deps, notation_id, &state, Some(&payload))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();

    assert_ne!(
        first.asset_id, second.asset_id,
        "each render files a distinct asset row",
    );
    assert_eq!(
        first.storage_key, second.storage_key,
        "identical bytes dedup to one content-addressed storage object",
    );
    assert_eq!(first.sha256, second.sha256);
}

#[tokio::test]
async fn generate_pdf_asset_kind_is_the_templates_declared_kind_not_the_state_slug() {
    // The step lands on `generate_pdf__nv_articles` — a slug that isn't
    // even a recognized `Kind` — but the template pinned by the notation
    // declares `kind: filing`. The persisted asset must carry the
    // template's kind, proving the slug is never consulted for
    // classification (issue #780).
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation_with_kind(&surreal, Some("filing")).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;
    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let payload = serde_json::to_string(&DocumentPayload::Typst {
        storage_key: format!("notations/{notation_id}/document.pdf"),
        typst_source: "Articles of organization body.".into(),
    })
    .unwrap();

    dispatch_step(
        &deps,
        notation_id,
        &StateName::from("generate_pdf__nv_articles"),
        Some(&payload),
    )
    .await
    .expect("generate_pdf dispatch succeeds");

    let doc = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.project_id == Some(project_id))
        .expect("a document filed on the notation's project");
    assert_eq!(doc.kind.as_deref(), Some("filing"));
}

#[tokio::test]
async fn generate_pdf_on_a_kindless_template_errors_clearly() {
    // A template with no declared `kind:` (legacy data, or a bug in an
    // ingest path — see the sibling `cli` fix in this same issue) must
    // not silently default to `"generated"`; it surfaces as an error the
    // caller can act on.
    let surreal = mem_surreal().await;
    let notation_id = store::test_support::seed_notation_with_kind(&surreal, None).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .unwrap()
        .expect("seeded notation")
        .project_id;
    let storage = fs_storage().await;
    let deps = deps(surreal.clone(), storage.clone());

    let storage_key = format!("notations/{notation_id}/document.pdf");
    let payload = serde_json::to_string(&DocumentPayload::Typst {
        storage_key: storage_key.clone(),
        typst_source: "Body.".into(),
    })
    .unwrap();

    let err = dispatch_step(
        &deps,
        notation_id,
        &StateName::from("generate_pdf__retainer_pdf"),
        Some(&payload),
    )
    .await
    .expect_err("a kindless template must not silently classify its generated asset");
    assert!(
        err.to_string().contains("no declared kind"),
        "expected a MissingKind error, got: {err}"
    );

    // Nothing was filed — this must fail before any `assets` write.
    let filed = store::assets::for_project(&surreal, project_id)
        .await
        .unwrap();
    assert!(
        filed.is_empty(),
        "a rejected generate_pdf dispatch must not file a partial asset"
    );

    // And nothing was written to object storage either: the terminal
    // `MissingKind` never resolves on replay, so persisting the PDF before
    // classifying would strand an orphaned object with no `assets` row or
    // audit-trail entry. Classification must gate the write.
    assert!(
        !storage.exists(&storage_key).await.unwrap(),
        "a rejected generate_pdf dispatch must not leave an orphaned object-storage PDF"
    );
}
