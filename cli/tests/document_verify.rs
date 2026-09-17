//! `navigator site document verify` — the three modes an operator reaches it
//! by, driven through the compiled binary.
//!
//! The mode that matters here is `--host` without `--ci` (LAW-12). Before it
//! existed, a lawyer who had just run `site document upload` had no way to ask
//! whether the asset actually landed: `--ci` needs GitHub Actions OIDC, and
//! `--host` on its own was accepted and then ignored, so verify took the
//! offline branch and printed `N pointer(s) valid` for a checkout with the
//! bytes deleted and no network reachable at all. These tests pin the round
//! trip by counting it on the mock, so a regression that silently falls back
//! to the offline branch fails rather than passing quietly.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn navigator() -> Command {
    let mut command = Command::cargo_bin("navigator").unwrap();
    command.env_remove("GITHUB_REPOSITORY");
    command
}

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

/// The nested v2 manifest shape every Project repository carries.
fn manifest(root: &Path, host: &str) {
    write(
        root,
        "navigator.yaml",
        format!("version: \"26.9.17\"\nproject:\n  host: {host}\n  name: acme\n"),
    );
}

fn credentials(root: &Path, host: &str) -> std::path::PathBuf {
    let path = root.join("credentials.json");
    write(
        root,
        "credentials.json",
        serde_json::to_vec(&serde_json::json!({
            "hosts": {
                host: {
                    "token": "test-token",
                    "person_email": "lawyer@example.com",
                    "role": "lawyer",
                    "expires_at": i64::MAX
                }
            }
        }))
        .unwrap(),
    );
    path
}

const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A committed pointer naming `asset_id` as its operative revision.
fn write_pointer(root: &Path, asset_id: Uuid) {
    write(
        root,
        "documents/pleadings/summons.pdf.yml",
        format!(
            "kind: filing\nvisibility: internal\ncurrent_version:\n  version: 1\n  \
             asset_id: {asset_id}\n  created_at: 2026-09-05T12:00:00Z\n  sha256: {SHA}\n  \
             size_bytes: 18\n"
        ),
    );
}

fn revisions_body(asset_id: Uuid, sha256: &str, size_bytes: i64) -> serde_json::Value {
    serde_json::json!({
        "kind": "filing",
        "revisions": [{
            "version": 1,
            "asset_id": asset_id,
            "created_at": "2026-09-05T12:00:00Z",
            "sha256": sha256,
            "size_bytes": size_bytes,
            "filename": "summons.pdf",
            "operative": true
        }]
    })
}

/// Mount the project lookup every authenticated client opens with.
async fn mount_projects(server: &MockServer, project_id: Uuid) {
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(server)
        .await;
}

/// LAW-12: the operator's own login is enough to confirm the upload landed.
/// `.expect(1)` on the revisions route is the assertion that matters — it is
/// what separates a live check from the offline branch's happy print.
#[tokio::test(flavor = "multi_thread")]
async fn verify_host_checks_each_pointer_against_the_live_record_over_a_login() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/revisions"
        )))
        .and(query_param("slug", "pleadings/summons.pdf"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(revisions_body(asset_id, SHA, 18)))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "document", "verify", ".", "--host", &host])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "1 pointer(s) verified against the live record",
        ));
}

/// The same live check has to be able to say no. A pointer whose sha256 no
/// longer matches the live record is drift, and drift is a failure — the
/// confirmation is worthless if it only ever agrees.
#[tokio::test(flavor = "multi_thread")]
async fn verify_host_fails_when_the_live_record_has_drifted_from_the_pointer() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/revisions"
        )))
        .and(query_param("slug", "pleadings/summons.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_json(revisions_body(
            asset_id,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            18,
        )))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "document", "verify", ".", "--host", &host])
        .assert()
        .failure()
        .stderr(predicate::str::contains("sha256 mismatch"));
}

/// Naming no host still means offline, and offline still means no network.
/// This is the behaviour LAW-12 reported as misleading only because `--host`
/// shared it; on its own it is the correct pull-request check, so it is pinned
/// rather than removed.
#[test]
fn verify_without_a_host_checks_shape_only_and_opens_no_connection() {
    let root = TempDir::new().unwrap();
    // A host that would refuse instantly if anything tried to reach it.
    manifest(root.path(), "127.0.0.1:1");
    write_pointer(root.path(), Uuid::now_v7());

    navigator()
        .current_dir(root.path())
        .args(["site", "document", "verify", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 pointer(s) valid"));
}

/// A checkout with no pointers is the common case and stays trivially green in
/// live mode, without reaching for a login it does not need.
#[test]
fn verify_host_succeeds_trivially_when_the_repository_has_no_pointers() {
    let root = TempDir::new().unwrap();
    manifest(root.path(), "127.0.0.1:1");

    navigator()
        .current_dir(root.path())
        .args(["site", "document", "verify", ".", "--host", "127.0.0.1:1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no documents/ pointers to verify"));
}
