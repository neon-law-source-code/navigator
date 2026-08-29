//! End-to-end import tests: walk the `templates/` fixture tree, lint
//! the markdown notation templates, and persist into a per-test
//! store spun up via `store::surreal::test_support::mem`. These
//! tests prove that
//!
//! 1. Every shipped template lints clean against the full Neon Law Navigator
//!    default rule set (so the fixtures stay honest).
//! 2. The import path actually writes templates and questions to the
//!    database — re-running it is idempotent and doesn't duplicate.
//! 3. Question codes referenced by `questionnaire:` and `workflow:`
//!    end up as `questions` rows the application can later resolve.

use std::path::PathBuf;
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at cli; the templates live at the
    // repository root under `templates/<category>/<name>.md`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../templates")
        .canonicalize()
        .expect("templates dir exists")
}

/// Count the notation templates under the fixture tree — every `.md`
/// carrying YAML frontmatter (so `templates/README.md` and other prose
/// are excluded). The import writes one `templates` row per such file,
/// so this is the expected `templates_created` count and tolerates new
/// templates landing without a hard-coded number going stale.
fn fixture_template_count() -> usize {
    fn walk(dir: &std::path::Path, n: &mut usize) {
        for entry in std::fs::read_dir(dir)
            .expect("read templates dir")
            .flatten()
        {
            let path = entry.path();
            // Mirror `import_directory`'s `excluded_directories`: the
            // `github` shelf is engineering intake, never a `templates`
            // row, so it must not count toward what import should create.
            if path
                .file_name()
                .is_some_and(|name| name == rules::GITHUB_SHELF)
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, n);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md")
                && std::fs::read_to_string(&path).is_ok_and(|s| s.starts_with("---\n"))
            {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(&fixtures_dir(), &mut n);
    n
}

async fn fs_storage() -> std::sync::Arc<dyn cloud::StorageService> {
    std::sync::Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-cli-import-test"))
            .await
            .expect("temp FsStorage"),
    )
}

#[tokio::test]
async fn fixture_directory_validates_clean() {
    let bin = assert_cmd::cargo::cargo_bin("navigator");
    // Run from a scratch dir so the `cli` binary's startup `dotenvy`
    // load can't pick up a developer's `.devx/env` (which points
    // `NAVIGATOR_SURREAL_ENDPOINT` at a KIND port-forward). This test asserts the
    // fixtures lint clean against the *default* rule set — a no-DB,
    // structural check — so it must stay hermetic and not flake on
    // whether a local port-forward happens to be up.
    let out = Command::new(&bin)
        .current_dir(std::env::temp_dir())
        .arg("validate")
        .arg(fixtures_dir())
        .output()
        .expect("run navigator validate");
    assert!(
        out.status.success(),
        "navigator validate must succeed on fixtures; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[tokio::test]
async fn catalog_seed_creates_only_shared_templates_and_question_catalog_rows() {
    let surreal = store::surreal::test_support::mem().await;
    let report = cli_import::import_directory(&surreal, &fs_storage().await, &fixtures_dir())
        .await
        .expect("import succeeds");

    assert_eq!(report.files_skipped_due_to_violations, 0);
    assert_eq!(
        report.templates_created,
        fixture_template_count(),
        "expected one templates row per fixture"
    );
    assert!(
        report.questions_created >= 9,
        "each fixture references at least 3 question codes — total ≥ 9 got {}",
        report.questions_created
    );

    let templates = store::templates::list_current(&surreal).await.unwrap();
    assert!(
        templates
            .iter()
            .all(|template| template.project_id.is_none()),
        "catalog seeding must create only workspace-shared templates"
    );
    let codes: Vec<&str> = templates.iter().map(|t| t.code.as_str()).collect();
    assert!(codes.contains(&"onboarding__engagement_letter"));
    // The version now routes through `save_version`, which is the only
    // path that actually carries `kind` through — pin that it does
    // (issue #780).
    let letter = templates
        .iter()
        .find(|t| t.code == "onboarding__engagement_letter")
        .expect("onboarding__engagement_letter seeded");
    assert_eq!(letter.kind.as_deref(), Some("onboarding"));
    assert!(letter.is_current);
    assert!(codes.contains(&"nv__dissolution"));
    assert!(codes.contains(&"nv__annual_report"));
    assert!(codes.contains(&"nv__modified_business_tax"));
    assert!(codes.contains(&"nv__nonprofit_501c3_formation"));
    assert!(codes.contains(&"us__form_990"));
    assert!(codes.contains(&"nv__charitable_solicitation_registration"));
    assert!(codes.contains(&"offboarding__letter"));

    let questions = store::questions::list_all(&surreal).await.unwrap();
    let q_codes: Vec<&str> = questions.iter().map(|q| q.code.as_str()).collect();
    // Spot-check typed question prefixes that come from fixture states such
    // as `person__trustee` and
    // `custom_single_choice__annual_or_amended`.
    assert!(q_codes.contains(&"custom_text"));
    assert!(q_codes.contains(&"custom_yes_no"));
    assert!(q_codes.contains(&"custom_single_choice"));
    assert!(q_codes.contains(&"custom_datetime"));
    // And an aggregate prefix from estate fixtures.
    assert!(q_codes.contains(&"people"));

    assert!(
        store::notations::list_all(&surreal)
            .await
            .unwrap()
            .is_empty(),
        "catalog seeding must not create client-facing notations"
    );
}

#[tokio::test]
async fn db_backed_validate_loads_codes_and_swaps_f104() {
    let surreal = store::surreal::test_support::mem().await;
    cli_import::import_directory(&surreal, &fs_storage().await, &fixtures_dir())
        .await
        .expect("import succeeds");

    let codes = cli_import::load_question_codes(&surreal)
        .await
        .expect("load codes");
    assert!(
        codes.iter().any(|c| c == "custom_text"),
        "loaded registry must contain canonical custom question types; got {codes:?}",
    );
    assert!(
        codes.iter().any(|c| c == "lawyer_review"),
        "loaded registry must preserve imported workflow codes; got {codes:?}",
    );

    let ruleset = rules::navigator_default_rules_with_codes(&codes);
    let n104 = ruleset
        .iter()
        .find(|r| r.code() == "N104")
        .expect("rule set must contain a swapped N104");
    assert_eq!(n104.code(), "N104");
}

#[tokio::test]
async fn re_running_catalog_seed_is_idempotent() {
    let surreal = store::surreal::test_support::mem().await;
    let first = cli_import::import_directory(&surreal, &fs_storage().await, &fixtures_dir())
        .await
        .expect("first import");
    let second = cli_import::import_directory(&surreal, &fs_storage().await, &fixtures_dir())
        .await
        .expect("second import");

    assert_eq!(first.templates_created, fixture_template_count());
    assert_eq!(
        second.templates_created, 0,
        "second pass must not duplicate templates"
    );
    assert_eq!(
        second.questions_created, 0,
        "second pass must not duplicate questions"
    );

    let templates = store::templates::list_current(&surreal).await.unwrap();
    assert_eq!(templates.len(), fixture_template_count());
}

/// A template-like markdown file that omits `kind:` must not silently vanish.
/// Import lints every `.md` with the notation rule set — it does not classify
/// by `kind:` first — so such a file is still processed (imported, or reported
/// as a violation) rather than dropped from a run that then reports success.
#[tokio::test]
async fn import_processes_a_kindless_template_rather_than_dropping_it() {
    let dir = tempfile::TempDir::new().unwrap();
    // Full notation frontmatter minus `kind:`. Under `validate`'s classifier
    // this reads as plain Markdown; import still processes it.
    std::fs::write(
        dir.path().join("no_kind.md"),
        "---\n\
title: Kindless Template\n\
respondent_type: person\n\
code: test__no_kind\n\
questionnaire:\n\
  BEGIN:\n\
    _: person__client\n\
  person__client:\n\
    _: END\n\
  END: {}\n\
---\n\nBody.\n",
    )
    .unwrap();

    let surreal = store::surreal::test_support::mem().await;
    let report = cli_import::import_directory(&surreal, &fs_storage().await, dir.path())
        .await
        .expect("import succeeds");

    // Surfaced, never a silent no-op: it is either imported or reported.
    assert!(
        report.templates_created + report.files_skipped_due_to_violations >= 1,
        "a kindless template must be processed, not silently dropped: {report:?}",
    );
}

/// `persist_template` must route through `store::templates::save_version`
/// rather than a raw "insert if code unseen" — otherwise a changed
/// template body silently no-ops (issue #780) instead of appending a new
/// version, and `kind`/`form_code` never reach the row at all.
#[tokio::test]
async fn catalog_seed_versions_a_changed_template_and_carries_its_kind() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("versioned.md");
    let body_v1 = r"---
title: Versioned Template
respondent_type: person
code: test__versioned
kind: filing
jurisdiction: NV
confidential: false
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: END
  END: {}
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---

Body v1.
";
    std::fs::write(&path, body_v1).unwrap();

    let surreal = store::surreal::test_support::mem().await;
    let storage = fs_storage().await;
    let first = cli_import::import_directory(&surreal, &storage, dir.path())
        .await
        .expect("first import");
    assert_eq!(
        first.templates_created, 1,
        "skipped={} violations={:?}",
        first.files_skipped_due_to_violations, first.violations
    );

    let seeded = store::templates::versions_of(&surreal, None, "test__versioned")
        .await
        .unwrap();
    assert_eq!(seeded.len(), 1);
    assert_eq!(seeded[0].kind.as_deref(), Some("filing"));
    assert!(seeded[0].is_current);

    // Re-import with the identical body: must not churn a new version.
    let unchanged = cli_import::import_directory(&surreal, &storage, dir.path())
        .await
        .expect("re-import succeeds");
    assert_eq!(
        unchanged.templates_created, 0,
        "an unchanged body must not append a new version"
    );

    // Change the body (a new content-addressed asset_id) and re-import:
    // must append a new current version, not silently no-op.
    std::fs::write(
        &path,
        body_v1.replace("Body v1.", "Body v2, materially different."),
    )
    .unwrap();
    let changed = cli_import::import_directory(&surreal, &storage, dir.path())
        .await
        .expect("second import");
    assert_eq!(
        changed.templates_created, 1,
        "a changed body must append a new version, not silently no-op"
    );

    let rows = store::templates::versions_of(&surreal, None, "test__versioned")
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the retired version stays for any notation that pinned it, plus the new current row"
    );
    let current = rows.iter().find(|t| t.is_current).expect("one current row");
    assert_eq!(current.kind.as_deref(), Some("filing"));
    let retired = rows
        .iter()
        .find(|t| !t.is_current)
        .expect("the retired prior version");
    assert_eq!(retired.kind.as_deref(), Some("filing"));
}

/// Module shim so the integration test can call into the binary
/// crate's catalog-seeding function. The cleaner alternative would be a
/// dedicated library crate; for now expose the import API via a
/// path-based module include.
#[path = "../src/import.rs"]
mod cli_import;
