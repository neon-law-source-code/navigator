//! The Postgres connection string is not a Navigator secret.
//!
//! The store is `SurrealDB`. `ops secrets apply` writes the Secret Manager
//! catalog, and that catalog has no `DATABASE_URL`. A connection-string
//! secret kept past the store that read it is a live credential with no
//! reader, so the name is asserted absent rather than remembered.
//!
//! Boot logs are the other half of the same boundary: `SurrealAuth`
//! redacts its password and token, and nothing in the hosting path
//! Debug-formats the store config into stdout (and therefore into Cloud
//! Logging). The covering tests in `store::surreal::config` pin the
//! redaction; this guard pins the call sites.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Files that mention `DATABASE_URL` on purpose: they prove a stray
/// Postgres URL does not become a Surreal endpoint, and that the gate
/// still isolates env.
const DATABASE_URL_ALLOWED_FILES: &[&str] = &[
    "cli/tests/retired_postgres_url.rs",
    "store/src/surreal/config.rs",
    "cli/tests/gate.rs",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn tracked_files() -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["ls-files", "-z"])
        .output()
        .expect("run `git ls-files`");
    assert!(
        output.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    assert!(
        files.len() > 100,
        "expected a tracked file list, got {} entries — this guard would pass vacuously",
        files.len()
    );
    files
}

fn read_tracked(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// `DATABASE_URL` names a Postgres connection string. It must not appear
/// in the Secret Manager catalog, a deployment fixture, or a Kubernetes
/// example — those are the files `ops secrets apply` and `ops ship` read.
#[test]
fn secret_catalogs_do_not_name_database_url() {
    let catalog_prefixes = [
        "examples/deploy/",
        "k8s/",
        "cli/tests/fixtures/deployment-tree/",
        "cli/src/devx/ship.rs",
    ];
    let mut hits = Vec::new();
    for path in tracked_files() {
        if !catalog_prefixes
            .iter()
            .any(|prefix| path == *prefix || path.starts_with(prefix))
        {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line.contains("DATABASE_URL") {
                hits.push(format!("{}:{}: {line}", path, idx + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "Secret Manager catalog and deployment fixtures must not name DATABASE_URL:\n{}",
        hits.join("\n")
    );
}

/// A stray `DATABASE_URL` in application source is not a connection, and
/// this keeps it from becoming one again through a new env read.
#[test]
fn application_source_does_not_read_database_url() {
    let mut hits = Vec::new();
    for path in tracked_files() {
        if !std::path::Path::new(&path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
        {
            continue;
        }
        if DATABASE_URL_ALLOWED_FILES.contains(&path.as_str()) {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line.contains("DATABASE_URL") {
                hits.push(format!("{}:{}: {line}", path, idx + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "application source must not read DATABASE_URL:\n{}",
        hits.join("\n")
    );
}

/// Debug-formatting the store config is how a password reaches Cloud Logging.
#[test]
fn hosting_does_not_debug_format_the_store_config() {
    let hosts = ["portal/src/hosting.rs", "workflows-service/src/main.rs"];
    let needles = [
        "?cfg.db",
        "?cfg.surreal",
        "?surreal",
        "NAVIGATOR_SURREAL_PASSWORD",
        "DATABASE_URL",
    ];
    let mut hits = Vec::new();
    for path in hosts {
        let body = read_tracked(path);
        for (idx, line) in body.lines().enumerate() {
            if needles.iter().any(|needle| line.contains(needle)) {
                hits.push(format!("{}:{}: {line}", path, idx + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "boot logs must not Debug-format store credentials:\n{}",
        hits.join("\n")
    );
}

/// Staging's substrate is GKE plus a hosted `SurrealDB`. Cloud SQL is not
/// a row in that picture, and naming one here would send an operator
/// looking for a password to rotate on an instance that does not exist.
#[test]
fn environments_doc_does_not_name_cloud_sql() {
    let body = read_tracked("docs/environments.md");
    assert!(
        !body.contains("Cloud SQL"),
        "docs/environments.md must not name Cloud SQL as deployment substrate"
    );
    assert!(
        !body.contains("<prefix>-pg"),
        "docs/environments.md must not name a Cloud SQL instance prefix"
    );
}
