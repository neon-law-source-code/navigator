//! The asset lane's document identity (#779): the operative-version rule
//! in [`store::assets::current`].
//!
//! The rule under test is deliberately *not* an `is_current` flag. It is
//! derived from insertion order, so these tests pin the two behaviours a
//! flag would have had to keep in sync by hand — "official ≠ latest" for
//! the client, and "latest visible to you" for lawyers — plus the fact that
//! a back-dated `published_at` cannot reorder a chain.

use store::assets::{current, file_revision, revisions, Filed, Lens, RevisionError};
use store::documents::visibility;
use store::surreal::test_support::mem;
use store::surreal::SurrealDb;
use store::test_support::seed_project_surreal;
use uuid::Uuid;

/// File one revision through the real ingest seam. Returns its asset id.
/// Revisions are ordered by `UUIDv7` `id`, which is insertion order, so
/// calling this repeatedly builds a chain in call order.
async fn insert_revision(
    db: &SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    project_id: Uuid,
    slug: Option<&str>,
    bytes: &[u8],
    published_at: Option<&str>,
    vis: &str,
) -> Uuid {
    store::documents::ingest_bytes_as(
        db,
        storage,
        &store::documents::IngestArgs {
            project_id,
            source: store::documents::source::UPLOAD,
            filename: "agreement.pdf",
            kind: "agreement",
            content_type: "application/pdf",
            description: None,
            visibility: vis,
            secondary_storage_key: None,
        },
        &store::documents::DocumentIdentity {
            slug,
            published_at,
            metadata: None,
        },
        bytes,
    )
    .await
    .expect("insert revision")
    .asset_id
}

/// An engine, a filesystem storage, and one matter to file against.
async fn fixtures() -> (
    SurrealDb,
    std::sync::Arc<dyn cloud::StorageService>,
    tempfile::TempDir,
    Uuid,
) {
    let db = mem().await;
    let tmp = tempfile::tempdir().expect("tempdir");
    let storage: std::sync::Arc<dyn cloud::StorageService> = std::sync::Arc::new(
        cloud::FsStorage::new(tmp.path().to_path_buf())
            .await
            .expect("fs storage"),
    );
    let project_id = seed_project_surreal(&db, "identity").await;
    (db, storage, tmp, project_id)
}

/// `(project_id, slug)` is deliberately non-unique: it names a document,
/// and each row under it is a revision. A unique index here would make
/// the second revision an error.
#[tokio::test]
async fn a_slug_holds_many_revisions() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    insert_revision(
        &db,
        &storage,
        project_id,
        Some("msa"),
        b"v1",
        None,
        visibility::CLIENT,
    )
    .await;
    insert_revision(
        &db,
        &storage,
        project_id,
        Some("msa"),
        b"v2",
        None,
        visibility::CLIENT,
    )
    .await;
    insert_revision(
        &db,
        &storage,
        project_id,
        Some("msa"),
        b"v3",
        None,
        visibility::CLIENT,
    )
    .await;

    let history = revisions(&db, project_id, "msa").await.expect("revisions");
    assert_eq!(history.len(), 3, "every row under a slug is a revision");
    assert_eq!(
        history.first().expect("newest").sha256_hex,
        store::documents::sha256_hex(b"v3"),
        "history is newest-first"
    );
}

/// The motivating case: an executed, published, client-visible agreement,
/// then a lawyer files an unpublished redline above it. Lawyers see the redline;
/// the client still sees the executed copy. That is "official ≠ latest"
/// without an `is_current` flag.
#[tokio::test]
async fn an_unpublished_redline_above_an_executed_agreement_changes_nothing_for_the_client() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    let executed = insert_revision(
        &db,
        &storage,
        project_id,
        Some("msa"),
        b"executed",
        Some("2026-08-01T00:00:00Z"),
        visibility::CLIENT,
    )
    .await;
    let redline = insert_revision(
        &db,
        &storage,
        project_id,
        Some("msa"),
        b"redline",
        None,
        visibility::INTERNAL,
    )
    .await;

    let lawyer_view = current(&db, project_id, "msa", Lens::Lawyer)
        .await
        .expect("lawyer current")
        .expect("a lawyer-current revision");
    assert_eq!(
        lawyer_view.id, redline,
        "lawyers see the latest revision, published or not"
    );

    let client_view = current(&db, project_id, "msa", Lens::Client)
        .await
        .expect("client current")
        .expect("a client-current revision");
    assert_eq!(
        client_view.id, executed,
        "the client's operative version stays the executed copy"
    );
}

/// A revision that is published but still `internal`, and one that is
/// client-visible but unpublished, are each invisible to the client. The
/// client lens needs both, not either.
#[tokio::test]
async fn the_client_lens_requires_published_and_client_visible_together() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    insert_revision(
        &db,
        &storage,
        project_id,
        Some("memo"),
        b"published-but-internal",
        Some("2026-08-01T00:00:00Z"),
        visibility::INTERNAL,
    )
    .await;
    insert_revision(
        &db,
        &storage,
        project_id,
        Some("memo"),
        b"client-but-unpublished",
        None,
        visibility::CLIENT,
    )
    .await;

    assert!(
        current(&db, project_id, "memo", Lens::Client)
            .await
            .expect("client current")
            .is_none(),
        "neither gate alone makes a revision operative for the client"
    );
}

/// `published_at` is back-datable to a court's file stamp, so it must not
/// decide ordering. Insertion order does. A later revision carrying an
/// *earlier* `published_at` is still the operative one.
#[tokio::test]
async fn a_backdated_published_at_does_not_reorder_the_chain() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    insert_revision(
        &db,
        &storage,
        project_id,
        Some("complaint"),
        b"entered-second",
        Some("2026-08-10T00:00:00Z"),
        visibility::CLIENT,
    )
    .await;
    // Filed later, but stamped by the court as of an earlier date — the
    // CM/ECF "filed on" vs "entered on" split.
    let backdated = insert_revision(
        &db,
        &storage,
        project_id,
        Some("complaint"),
        b"filed-on-an-earlier-date",
        Some("2026-08-02T00:00:00Z"),
        visibility::CLIENT,
    )
    .await;

    let operative = current(&db, project_id, "complaint", Lens::Client)
        .await
        .expect("client current")
        .expect("a client-current revision");
    assert_eq!(
        operative.id, backdated,
        "insertion order decides current; published_at is display metadata"
    );
}

/// An unset slug is a one-off artifact — an inbound attachment, an
/// executed PDF nobody will revise. It never joins a chain.
#[tokio::test]
async fn an_unslugged_asset_is_a_one_off_and_joins_no_chain() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    insert_revision(
        &db,
        &storage,
        project_id,
        None,
        b"attachment",
        None,
        visibility::INTERNAL,
    )
    .await;
    insert_revision(
        &db,
        &storage,
        project_id,
        None,
        b"another",
        None,
        visibility::INTERNAL,
    )
    .await;

    // Scoped to this matter rather than counted table-wide: one engine can
    // outlive one test elsewhere in the workspace, and a table-wide count
    // would then see rows this test never wrote.
    let all = store::assets::for_project(&db, project_id)
        .await
        .expect("project assets");
    assert_eq!(all.len(), 2, "both one-offs exist as their own rows");
    assert!(
        all.iter().all(|row| row.slug.is_none()),
        "a one-off carries no document identity"
    );
}

// --- The write boundary: `store::assets::file_revision` ---------------
//
// The three rules above are enforced in one place so no call site
// reimplements one and drifts. These drive the real boundary rather than
// a helper, because a rule that only a test helper knows is not a rule.

fn args(project_id: Uuid, kind: &str) -> store::documents::IngestArgs<'_> {
    store::documents::IngestArgs {
        project_id,
        source: store::documents::source::UPLOAD,
        filename: "agreement.pdf",
        kind,
        content_type: "application/pdf",
        description: None,
        visibility: visibility::CLIENT,
        secondary_storage_key: None,
    }
}

fn identity(slug: &str) -> store::documents::DocumentIdentity<'_> {
    store::documents::DocumentIdentity {
        slug: Some(slug),
        ..Default::default()
    }
}

/// Rule 1. The asset lane is closed. A teaching page and a dashboard
/// skeleton are Markdown-lane kinds — neither is ever a byte artifact
/// filed on a matter — and a string outside the vocabulary is refused by
/// the same arm.
#[tokio::test]
async fn the_asset_lane_refuses_a_kind_that_is_not_filable() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    for kind in ["workshop", "post", "review_queue_workbench", "not_a_kind"] {
        let err = file_revision(
            &db,
            &storage,
            &args(project_id, kind),
            &identity("doc"),
            b"bytes",
        )
        .await
        .expect_err("a non-filable kind must be refused");
        assert!(
            matches!(err, RevisionError::KindNotFilable(ref k) if k == kind),
            "{kind} should be refused as not filable, got {err:?}"
        );
    }

    // The mirror: a real asset-lane kind is accepted.
    file_revision(
        &db,
        &storage,
        &args(project_id, "transcript"),
        &identity("sitting"),
        b"bytes",
    )
    .await
    .expect("an asset-lane kind is filable");
}

/// Rule 2. Kind is immutable across a chain. A slug holding a `retainer`
/// does not accept an `agreement` revision — that is a different
/// document, and it belongs under its own slug.
#[tokio::test]
async fn a_kind_change_is_a_different_document_not_a_revision() {
    let (db, storage, _tmp, project_id) = fixtures().await;

    file_revision(
        &db,
        &storage,
        &args(project_id, "onboarding"),
        &identity("engagement"),
        b"v1",
    )
    .await
    .expect("first revision");

    let err = file_revision(
        &db,
        &storage,
        &args(project_id, "agreement"),
        &identity("engagement"),
        b"v2",
    )
    .await
    .expect_err("a changed kind must be refused");
    assert!(
        matches!(
            err,
            RevisionError::KindChanged { ref existing, ref attempted, .. }
                if existing == "onboarding" && attempted == "agreement"
        ),
        "expected a KindChanged rejection, got {err:?}"
    );

    // The same kind still appends, so the rule rejects only the change.
    file_revision(
        &db,
        &storage,
        &args(project_id, "onboarding"),
        &identity("engagement"),
        b"v2",
    )
    .await
    .expect("same kind appends");
    assert_eq!(
        revisions(&db, project_id, "engagement")
            .await
            .expect("revisions")
            .len(),
        2,
        "the refused write left no row behind"
    );
}

/// Rule 3. Identical bytes are a no-op **scoped to one slug on one
/// project**. Re-filing the same bytes under a different slug is a
/// genuinely different document, and the probe must never answer across
/// projects — a global "we already have this" is an existence oracle
/// (Harnik et al., IEEE S&P 2010).
#[tokio::test]
async fn identical_bytes_are_a_no_op_scoped_to_one_slug_and_never_global() {
    let (db, storage, _tmp, project_id) = fixtures().await;
    let other_project = seed_project_surreal(&db, "oracle-b").await;
    let bytes = b"the same boilerplate with one low-entropy field";

    let first = match file_revision(
        &db,
        &storage,
        &args(project_id, "agreement"),
        &identity("nda"),
        bytes,
    )
    .await
    .expect("first revision")
    {
        Filed::Revision(doc) => doc.asset_id,
        Filed::Unchanged { .. } => panic!("the first write is always a revision"),
    };

    // Same slug, same bytes — nothing is written, and the caller is
    // pointed at the row it already has.
    match file_revision(
        &db,
        &storage,
        &args(project_id, "agreement"),
        &identity("nda"),
        bytes,
    )
    .await
    .expect("re-file")
    {
        Filed::Unchanged { asset_id } => assert_eq!(asset_id, first),
        Filed::Revision(_) => panic!("identical bytes must not create a revision"),
    }
    assert_eq!(
        revisions(&db, project_id, "nda")
            .await
            .expect("revisions")
            .len(),
        1,
        "the chain did not grow"
    );

    // Same bytes, different slug — a different document, so it is filed.
    assert!(
        matches!(
            file_revision(
                &db,
                &storage,
                &args(project_id, "agreement"),
                &identity("sow"),
                bytes,
            )
            .await
            .expect("different slug"),
            Filed::Revision(_)
        ),
        "the same bytes under a different slug are a different document"
    );

    // Same bytes, different project — the probe must not answer across
    // projects at all.
    assert!(
        matches!(
            file_revision(
                &db,
                &storage,
                &args(other_project, "agreement"),
                &identity("nda"),
                bytes,
            )
            .await
            .expect("different project"),
            Filed::Revision(_)
        ),
        "the probe must never answer across projects — that is an existence oracle"
    );
}

/// A document's identity is `project_id` first — slug and filename are only
/// meaningful inside that scope. Two matters that happen to use the same
/// slug and the same filename must never see each other's revision chain,
/// operative version, or filename lookup.
#[tokio::test]
async fn a_document_is_namespaced_by_project_not_by_slug_or_filename_alone() {
    let (db, storage, _tmp, project_a) = fixtures().await;
    let project_b = seed_project_surreal(&db, "sibling").await;

    let doc_a = insert_revision(
        &db,
        &storage,
        project_a,
        Some("msa"),
        b"project-a-content",
        None,
        visibility::CLIENT,
    )
    .await;
    let doc_b = insert_revision(
        &db,
        &storage,
        project_b,
        Some("msa"),
        b"project-b-content",
        None,
        visibility::CLIENT,
    )
    .await;

    // The revision chain never crosses projects, even under the identical slug.
    let history_a = revisions(&db, project_a, "msa").await.expect("revisions a");
    assert_eq!(
        history_a.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![doc_a],
        "project A's chain holds only its own revision"
    );

    let history_b = revisions(&db, project_b, "msa").await.expect("revisions b");
    assert_eq!(
        history_b.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![doc_b],
        "project B's chain holds only its own revision"
    );

    // `current` resolves within one project only.
    let current_a = current(&db, project_a, "msa", Lens::Lawyer)
        .await
        .expect("current a")
        .expect("a current revision in project a");
    assert_eq!(current_a.id, doc_a, "project A's current stays project A's");

    let current_b = current(&db, project_b, "msa", Lens::Lawyer)
        .await
        .expect("current b")
        .expect("a current revision in project b");
    assert_eq!(current_b.id, doc_b, "project B's current stays project B's");

    // The filename lookup is scoped the same way: both matters filed
    // "agreement.pdf" (see `insert_revision`), and each must resolve to
    // its own row, never the other matter's.
    let by_filename_a =
        store::assets::find_by_project_and_filename(&db, project_a, "agreement.pdf")
            .await
            .expect("find by filename a")
            .expect("a row in project a");
    assert_eq!(
        by_filename_a.id, doc_a,
        "filename lookup in project a must not resolve project b's row"
    );

    let by_filename_b =
        store::assets::find_by_project_and_filename(&db, project_b, "agreement.pdf")
            .await
            .expect("find by filename b")
            .expect("a row in project b");
    assert_eq!(
        by_filename_b.id, doc_b,
        "filename lookup in project b must not resolve project a's row"
    );
}
