//! End-to-end tests for `navigator db list <subject>`. Every list call
//! runs the full canonical seed pass first (idempotent), so a fresh
//! database is enough to see the canonical rows. Imported templates
//! remain on top of the seeded data.
//!
//! A spawned subprocess cannot reach an in-process engine, so these tests
//! run on [`store::test_support::server_surreal`]'s server-mode lane and
//! skip when no endpoint is configured.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use assert_cmd::cargo::cargo_bin;
use store::test_support::ServerSurreal;
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("repo root exists")
}

/// A throwaway object-storage root shared by every subprocess this binary
/// spawns. `site seed` and `db list` open storage before they touch the database,
/// and `cloud`'s selector fails closed on an unset `NAVIGATOR_STORAGE_BACKEND`
/// (#618) — so each invocation names a backend. These tests assert on database
/// rows and never on stored objects, so one scratch root is enough, and
/// pointing at it keeps a `./var/storage` out of the source tree.
fn storage_root() -> &'static Path {
    static ROOT: OnceLock<TempDir> = OnceLock::new();
    ROOT.get_or_init(|| TempDir::new().expect("storage root tempdir"))
        .path()
}

/// The `navigator` binary with object storage configured.
fn navigator() -> Command {
    let mut cmd = Command::new(cargo_bin("navigator"));
    cmd.env("NAVIGATOR_STORAGE_BACKEND", "fs")
        .env("NAVIGATOR_STORAGE_FS_ROOT", storage_root());
    cmd
}

/// The `navigator` binary with object storage configured **and** pointed
/// at this test's person store, so the seed pass `db list` runs writes
/// somewhere the test controls.
fn navigator_with(store: &ServerSurreal) -> Command {
    let mut cmd = navigator();
    cmd.env(store::surreal::ENDPOINT_ENV, &store.config.endpoint)
        .env(store::surreal::NAMESPACE_ENV, &store.config.namespace)
        .env(store::surreal::DATABASE_ENV, &store.config.database);
    if let store::surreal::SurrealAuth::Password {
        scope,
        username,
        password,
    } = &store.config.auth
    {
        cmd.env(store::surreal::USER_ENV, username)
            .env(store::surreal::PASSWORD_ENV, password)
            .env(store::surreal::AUTH_SCOPE_ENV, scope.as_str());
    }
    cmd
}

/// Seed the workspace templates into `store`.
///
/// Both this call and the `db list` that follows must name the *same*
/// store, or the list would read a catalog the seed never wrote.
fn populated_store(store: &ServerSurreal) {
    let out = navigator_with(store)
        .args(["site", "seed"])
        .arg(repo_root().join("templates"))
        .output()
        .expect("run navigator site seed");
    assert!(
        out.status.success(),
        "site seed failed: stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[tokio::test]
async fn list_questions_prints_every_canonical_code() {
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_questions_prints_every_canonical_code")
            .await
    else {
        return;
    };
    populated_store(&store);
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("questions")
        .output()
        .expect("run navigator db list questions");
    assert!(
        out.status.success(),
        "list questions failed: stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for code in [
        "person",
        "people",
        "entity",
        "custom_text",
        "custom_single_choice",
        "lawyer_review",
        "generate_pdf",
    ] {
        assert!(
            stdout.contains(code),
            "expected `{code}` in `list questions` output:\n{stdout}",
        );
    }
}

#[tokio::test]
async fn list_against_fresh_db_auto_seeds() {
    // No prior seed/import — `list` must still produce the full
    // canonical question set on its own.
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_against_fresh_db_auto_seeds").await
    else {
        return;
    };
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("questions")
        .output()
        .expect("run navigator db list questions");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("custom_text"),
        "fresh DB list must contain canonical questions:\n{stdout}"
    );
}

#[tokio::test]
async fn list_templates_prints_imported_titles() {
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_templates_prints_imported_titles").await
    else {
        return;
    };
    populated_store(&store);
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("templates")
        .output()
        .expect("run navigator db list templates");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "onboarding__letter",
        "Retainer Agreement",
        "offboarding__letter",
        "Closing Letter",
        "nv__llc_formation",
        "us__naturalization",
    ] {
        assert!(
            stdout.contains(needle),
            "expected `{needle}` in `list templates` output:\n{stdout}",
        );
    }
}

#[tokio::test]
async fn list_jurisdictions_prints_full_state_set() {
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_jurisdictions_prints_full_state_set")
            .await
    else {
        return;
    };
    populated_store(&store);
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("jurisdictions")
        .output()
        .expect("run navigator db list jurisdictions");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for needle in ["NV", "Nevada", "CA", "California", "DC", "GMBH"] {
        assert!(
            stdout.contains(needle),
            "expected `{needle}` in `list jurisdictions` output:\n{stdout}",
        );
    }
}

#[tokio::test]
async fn list_persons_includes_seeded_emails() {
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_persons_includes_seeded_emails").await
    else {
        return;
    };
    populated_store(&store);
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("persons")
        .output()
        .expect("run navigator db list persons");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let needle = "nick@neonlaw.com";
    assert!(
        stdout.contains(needle),
        "expected `{needle}` in `list persons` output:\n{stdout}",
    );
}

#[tokio::test]
async fn list_entities_includes_seeded_org_names() {
    let Some(store) =
        store::test_support::server_surreal("test_cli_list_entities_includes_seeded_org_names")
            .await
    else {
        return;
    };
    populated_store(&store);
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("entities")
        .output()
        .expect("run navigator db list entities");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for needle in ["Shook Law PLLC", "shook.family"] {
        assert!(
            stdout.contains(needle),
            "expected `{needle}` in `list entities` output:\n{stdout}",
        );
    }
}

#[tokio::test]
async fn list_templates_against_a_seed_only_db_shows_the_bundled_retainer() {
    // The canonical seed pass now bundles the retainer notation
    // template (see `store::seed::seed_templates`), so a fresh
    // seed-only DB carries exactly one row — `onboarding__letter`.
    let Some(store) = store::test_support::server_surreal(
        "test_cli_list_templates_against_a_seed_only_db_shows_the_bundled_retainer",
    )
    .await
    else {
        return;
    };
    let out = navigator_with(&store)
        .args(["db", "list"])
        .arg("templates")
        .output()
        .expect("run navigator db list templates");
    assert!(out.status.success(), "fresh-DB list must still succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("onboarding__letter"),
        "expected the seeded retainer template; got:\n{stdout}",
    );
    assert!(stdout.contains("Retainer Agreement"));
}
