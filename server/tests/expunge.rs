//! Governed-expunge orchestration: admin gate + history rewrite +
//! object-storage deletion + audit record, end to end.
//!
//! Multi-thread runtime: the rewrite shells `git` via `spawn_blocking`.

use std::process::Command;
use std::sync::{Arc, LazyLock};

use cloud::StorageService;
use store::expunge_records;
use store::persons::Role;
use store::test_support::mem_surreal;
use uuid::Uuid;
use workflows::{dispatch_generate_pdf, DocumentPayload, GeneratedPdfRef};

/// One repo root for the whole test binary. `NAVIGATOR_GIT_REPO_ROOT` is
/// process-global, so per-test tempdirs (with their own `set_var` /
/// `remove_var`) would race across the parallel tests — one test unsetting
/// the root mid-rewrite of another. A single stable root sidesteps the race;
/// each test uses its own project code, so repos never collide under it.
static REPO_ROOT: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", dir.path());
    dir
});

// Both tests invoke git under the process-wide repository root. Serializing
// those calls avoids LLVM-instrumented macOS child processes racing over
// inherited file descriptors and intermittently returning EBADF.
static REPO_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn a_person(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    role: Role,
) -> uuid::Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, role),
    )
    .await
    .unwrap()
    .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn governed_expunge_rewrites_deletes_and_records() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let repo_root = &*REPO_ROOT;

    let surreal = mem_surreal().await;
    let storage: Arc<dyn StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-expunge-{}", uuid::Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );

    let admin = a_person(&surreal, "Nick", "nick@neonlaw.com", Role::Admin).await;
    let client = a_person(&surreal, "Aries", "aries@example.com", Role::Client).await;
    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Commit a privileged doc + a kept doc into the repo, and stash the
    // privileged bytes in object storage at a blobs/<sha> key.
    let repo_store = repos::RepoStore::new(repo_root.path());
    repo_store
        .commit_as_code(
            &proj.code,
            repos::Author {
                name: "Aries",
                email: "aries@example.com",
            },
            "file docs",
            &[
                ("privileged.pdf", b"privileged material"),
                ("keep.pdf", b"ordinary doc"),
            ],
        )
        .unwrap();
    let object_key = "blobs/deadbeefdeadbeef";
    storage
        .put(object_key, b"privileged material", "application/pdf")
        .await
        .unwrap();

    // A non-admin may NOT expunge — and nothing is touched.
    let denied = portal::expunge::expunge(
        &surreal,
        &storage,
        portal::expunge::ExpungeRequest {
            project_id: proj.id,
            path: "privileged.pdf",
            category: expunge_records::CATEGORY_PRIVILEGE,
            authorized_by: client,
            storage_keys: vec![object_key.to_string()],
            note: None,
        },
    )
    .await;
    assert!(matches!(
        denied,
        Err(portal::expunge::ExpungeError::NotAdmin)
    ));
    let repo = repo_store.path_for_code(&proj.code);
    assert_eq!(
        git_show(&repo, "show main:privileged.pdf"),
        "privileged material"
    );
    assert!(
        storage.get(object_key).await.is_ok(),
        "no deletion on a denied expunge"
    );

    // The admin expunges: history rewritten, bytes deleted, audit row
    // written.
    let record_id = portal::expunge::expunge(
        &surreal,
        &storage,
        portal::expunge::ExpungeRequest {
            project_id: proj.id,
            path: "privileged.pdf",
            category: expunge_records::CATEGORY_SEALING,
            authorized_by: admin,
            storage_keys: vec![object_key.to_string()],
            note: Some("sealed per docket 24-CV-1"),
        },
    )
    .await
    .expect("admin expunge succeeds");

    // (2) gone from history, kept doc survives.
    let history = git_show(&repo, "log --all --oneline -- privileged.pdf");
    assert!(
        history.is_empty(),
        "privileged doc still in history: {history}"
    );
    assert_eq!(git_show(&repo, "show main:keep.pdf"), "ordinary doc");

    // (3) bytes gone from object storage.
    assert!(matches!(
        storage.get(object_key).await,
        Err(cloud::StorageError::NotFound(_))
    ));

    // (4) audit row records who/when/category, not content.
    let row = expunge_records::by_id(&surreal, record_id)
        .await
        .unwrap()
        .expect("expunge record");
    assert_eq!(row.category, expunge_records::CATEGORY_SEALING);
    assert_eq!(row.authorized_by_person_id, admin);
    assert_eq!(row.project_id, proj.id);
    assert!(row.head_after.is_some());
}

/// A `generate_pdf` step dual-writes the same bytes to two object-storage
/// keys — the caller's notation key (`notations/<id>/document.pdf`, what
/// the attest/signature steps and the portal read back) and the
/// content-addressed `blobs/<sha>` the `assets` row points at. A governed
/// expunge must remove **both**, or a copy of the privileged bytes
/// survives outside the asset lifecycle (#470).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn governed_expunge_removes_every_key_of_a_generated_pdf() {
    let _repo_guard = REPO_ENV_LOCK.lock().await;
    let repo_root = &*REPO_ROOT;

    let surreal = mem_surreal().await;
    let storage: Arc<dyn StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-expunge-gen-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );

    let admin = a_person(&surreal, "Nick", "nick@neonlaw.com", Role::Admin).await;
    let client = a_person(&surreal, "Libra", "libra@example.com", Role::Client).await;
    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tmpl = store::templates::save_version(
        &surreal,
        None,
        "retainer",
        store::templates::Version {
            title: "Retainer".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: Some("onboarding".into()),
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();
    let notation = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, client, proj.id, "BEGIN"),
    )
    .await
    .unwrap();

    // Render + persist a generated PDF exactly as a `generate_pdf__*` step
    // does: the caller's notation key plus the content-addressed asset.
    let notation_key = format!("notations/{}/document.pdf", notation.id);
    let ref_json = dispatch_generate_pdf(
        &storage,
        Some(&surreal),
        notation.id,
        &DocumentPayload::Typst {
            storage_key: notation_key.clone(),
            typst_source: "Retainer body.".into(),
        },
    )
    .await
    .expect("dispatch succeeds")
    .expect("db present → an asset is filed");
    let pdf_ref: GeneratedPdfRef = serde_json::from_str(&ref_json).unwrap();
    let blob_key = format!("blobs/{}", pdf_ref.sha256);

    // Both copies are present up front.
    assert!(
        storage.get(&notation_key).await.is_ok(),
        "notation key stored"
    );
    assert!(
        storage.get(&blob_key).await.is_ok(),
        "content-addressed key stored"
    );

    // A worker retry (e.g. the DB blipped after the first ingest committed)
    // re-runs the step: `ingest_bytes` dedups storage by SHA but files a
    // *distinct* asset row. Because the notation key is stamped in the same
    // insert, that second row carries it too — so expunging EITHER row clears
    // every copy, closing the nonatomic-stamp hole (Greptile P1 on #470).
    dispatch_generate_pdf(
        &storage,
        Some(&surreal),
        notation.id,
        &DocumentPayload::Typst {
            storage_key: notation_key.clone(),
            typst_source: "Retainer body.".into(),
        },
    )
    .await
    .expect("retry dispatch succeeds")
    .expect("db present → a second asset is filed");
    let rows = store::assets::list_all(&surreal).await.unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the retry filed a second, distinct asset row"
    );
    assert!(
        rows.iter()
            .all(|r| r.secondary_storage_key.as_deref() == Some(notation_key.as_str())),
        "every asset row for the bytes must carry the notation key"
    );

    // Commit the repo path (the asset filename) so the history rewrite has
    // something to remove.
    repos::RepoStore::new(repo_root.path())
        .commit_as_code(
            &proj.code,
            repos::Author {
                name: "Libra",
                email: "libra@example.com",
            },
            "file generated pdf",
            &[("document.pdf", b"Retainer body.")],
        )
        .unwrap();

    let doc = store::assets::find_by_id(&surreal, pdf_ref.asset_id)
        .await
        .unwrap()
        .expect("asset row");

    portal::expunge::expunge(
        &surreal,
        &storage,
        portal::expunge::ExpungeRequest {
            project_id: proj.id,
            path: "document.pdf",
            category: expunge_records::CATEGORY_CLIENT_REQUEST,
            authorized_by: admin,
            storage_keys: portal::expunge::storage_keys_for_asset(&doc),
            note: None,
        },
    )
    .await
    .expect("admin expunge succeeds");

    // Neither copy of the bytes survives.
    assert!(
        matches!(
            storage.get(&blob_key).await,
            Err(cloud::StorageError::NotFound(_))
        ),
        "content-addressed copy must be gone"
    );
    assert!(
        matches!(
            storage.get(&notation_key).await,
            Err(cloud::StorageError::NotFound(_))
        ),
        "notation-key copy must be gone — else a privileged copy survives"
    );
}

/// Two matters can legitimately hold the same bytes — a shared exhibit, a
/// filed order, a blank government form. Before dedup was scoped to a matter
/// (`store::documents::ingest_bytes_as`), one content-addressed object backed
/// asset rows across matters, and expunge deleted it unconditionally. A
/// sealing order on one matter therefore emptied an unrelated matter's
/// document; if that matter was under a preservation duty, the deletion was
/// spoliation caused by a case with no connection to it.
///
/// Scoping ingest stops new rows from sharing an object. Rows that already
/// share one must still be safe, so expunge retains an object that an asset
/// row on **another** matter still references.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_expunge_retains_an_object_another_matter_still_references() {
    let repo_root = &*REPO_ROOT;

    let surreal = mem_surreal().await;
    let storage: Arc<dyn StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-expunge-shared-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );

    let admin = a_person(&surreal, "Nick", "nick@neonlaw.com", Role::Admin).await;
    let mut projects = Vec::new();
    for name in ["Sealed Matter", "Unrelated Matter"] {
        projects.push(
            store::projects::create(
                &surreal,
                &store::projects::NewProject {
                    code: format!("matter-{}", Uuid::now_v7()),
                    name: name.into(),
                    status: "open".into(),
                    entity_id: store::test_support::seed_entity(&surreal).await,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        );
    }
    let (sealed, unrelated) = (&projects[0], &projects[1]);

    let repo_store = repos::RepoStore::new(repo_root.path());
    repo_store
        .commit_as_code(
            &sealed.code,
            repos::Author {
                name: "Aries",
                email: "aries@example.com",
            },
            "file the exhibit",
            &[("exhibit.pdf", b"an exhibit on two matters")],
        )
        .unwrap();

    // One object, two asset rows — the pre-scoping corpus shape. Both
    // ingests hash the same bytes, so both rows point at the same
    // content-addressed key, which is exactly the shape the retain guard
    // has to recognize.
    let shared_bytes: &[u8] = b"an exhibit on two matters";
    let shared_key = format!("blobs/{}", store::documents::sha256_hex(shared_bytes));
    for project in [sealed, unrelated] {
        store::documents::ingest_bytes(
            &surreal,
            &storage,
            &store::documents::IngestArgs {
                project_id: project.id,
                source: store::documents::source::UPLOAD,
                filename: "exhibit.pdf",
                kind: "unclassified",
                content_type: "application/pdf",
                description: None,
                secondary_storage_key: None,
                visibility: store::documents::visibility::INTERNAL,
            },
            shared_bytes,
        )
        .await
        .unwrap();
    }

    portal::expunge::expunge(
        &surreal,
        &storage,
        portal::expunge::ExpungeRequest {
            project_id: sealed.id,
            path: "exhibit.pdf",
            category: expunge_records::CATEGORY_SEALING,
            authorized_by: admin,
            storage_keys: vec![shared_key.clone()],
            note: Some("sealed per docket 24-CV-2"),
        },
    )
    .await
    .expect("admin expunge succeeds");

    // The sealed matter no longer serves the document ...
    let repo = repo_store.path_for_code(&sealed.code);
    assert!(
        git_show(&repo, "log --all --oneline -- exhibit.pdf").is_empty(),
        "the sealed matter's history must not still carry the path"
    );

    // ... but the unrelated matter's copy survives.
    assert!(
        storage.get(&shared_key).await.is_ok(),
        "an object another matter's asset row still references must survive"
    );
}

fn git_show(repo: &std::path::Path, args: &str) -> String {
    let split: Vec<&str> = args.split(' ').collect();
    let out = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .arg("-C")
        .arg(repo)
        .args(&split)
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
