//! Every document command reads the nested `navigator.yaml`, not just the one
//! that was reported (LAW-18).
//!
//! At 26.9.16 `project` was deserialized as a scalar on the document lane, so
//! the versioned manifest shape every Project repository carries —
//!
//! ```yaml
//! version: "26.9.17"
//! project:
//!   host: staging.neonlaw.com
//!   name: acme
//! ```
//!
//! — failed with `project: invalid type: map, expected a string`. `site sync`,
//! `site pull`, `site document log`, `site document get`, and
//! `project gate --check` all read that manifest through one parser.
//!
//! `navigator#596` routed the lane through `projects::manifest::parse`, and
//! that reader has its own unit coverage. What it did not have is a test that
//! reaches the reader *the way a lawyer does* — every end-to-end document test
//! in `cli/tests/document_sync.rs` writes the deprecated flat manifest, so the
//! nested shape was exercised nowhere above the parser. A command that
//! reintroduced a private `project` field would leave that suite green.
//!
//! So this file is deliberately one test per command, each driving the
//! compiled binary against a v2 manifest and asserting the command reached its
//! API. The assertion is the successful round trip, not the absence of a parse
//! message: a command that failed to parse never reaches the mock, and the
//! mounted `.expect(1)` fails the test on drop.

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

/// The nested manifest shape. `version` travels with it because that is the
/// document a Project repository actually commits; a reader that accepts the
/// map but refuses the sibling key would still block the gate.
fn v2_manifest(root: &Path, host: &str) {
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

const BYTES: &[u8] = b"synthetic pleading";
const SLUG: &str = "pleadings/summons.pdf";
const POINTER: &str = "documents/pleadings/summons.pdf.yml";

fn sha256(bytes: &[u8]) -> String {
    store::documents::sha256_hex(bytes)
}

fn write_pointer(root: &Path, asset_id: Uuid) {
    write(
        root,
        POINTER,
        format!(
            "kind: filing\nvisibility: internal\ncurrent_version:\n  version: 1\n  \
             asset_id: {asset_id}\n  created_at: 2026-09-05T12:00:00Z\n  sha256: {}\n  \
             size_bytes: {}\n",
            sha256(BYTES),
            BYTES.len()
        ),
    );
}

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

async fn mount_revisions(server: &MockServer, project_id: Uuid, asset_id: Uuid) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/revisions"
        )))
        .and(query_param("slug", SLUG))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "kind": "filing",
            "revisions": [{
                "version": 1,
                "asset_id": asset_id,
                "created_at": "2026-09-05T12:00:00Z",
                "sha256": sha256(BYTES),
                "size_bytes": BYTES.len(),
                "filename": "summons.pdf",
                "visibility": "internal",
                "operative": true
            }]
        })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn site_sync_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(root.path(), "documents/pleadings/summons.pdf", BYTES);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();

    mount_projects(&server, project_id).await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "kind": "filing",
            "visibility": "internal",
            "current_version": {
                "version": 1,
                "asset_id": asset_id,
                "created_at": "2026-09-05T12:00:00Z",
                "sha256": sha256(BYTES),
                "size_bytes": BYTES.len()
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 uploaded"));
}

#[tokio::test(flavor = "multi_thread")]
async fn site_pull_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_id}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BYTES))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "pull"])
        .assert()
        .success();

    assert_eq!(
        fs::read(root.path().join("documents/pleadings/summons.pdf")).unwrap(),
        BYTES
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn site_document_log_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    mount_revisions(&server, project_id, asset_id).await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "document", "log", POINTER])
        .assert()
        .success()
        .stdout(predicate::str::contains(SLUG));
}

#[tokio::test(flavor = "multi_thread")]
async fn site_document_get_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    let out = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    mount_revisions(&server, project_id, asset_id).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_id}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BYTES))
        .expect(1)
        .mount(&server)
        .await;

    let destination = out.path().join("summons.pdf");
    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args([
            "site",
            "document",
            "get",
            POINTER,
            "--out",
            destination.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert_eq!(fs::read(&destination).unwrap(), BYTES);
}

/// `project gate --check --ci` reads the nested manifest and mints through
/// GitHub Actions OIDC. The host is the manifest's, not a flag.
#[tokio::test(flavor = "multi_thread")]
async fn project_gate_check_ci_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    write(root.path(), "README.md", "Synthetic repository\n");
    write(root.path(), ".git", "gitdir: /tmp/synthetic\n");
    write(
        root.path(),
        "documents/.gitignore",
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    );
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    Mock::given(method("GET"))
        .and(path("/actions/oidc"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "value": "github-oidc-token" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/ci/document-token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "token": "test-token", "project_code": "acme" }),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_projects(&server, project_id).await;
    mount_revisions(&server, project_id, asset_id).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/integrity"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "assets": [{
                "asset_id": asset_id,
                "slug": SLUG,
                "exists": true,
                "size_bytes": BYTES.len(),
                "recorded_size": BYTES.len()
            }],
            "integrations": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env(
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            format!("{host}/actions/oidc"),
        )
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "runner-token")
        .args(["project", "gate", "--check", "--ci"])
        .assert()
        .stdout(predicate::str::contains("documents:"));
}

/// The deprecated flat shape has to keep working while released binaries and
/// unmigrated repositories are still in the field — the v2 support is an
/// addition, not a swap.
#[tokio::test(flavor = "multi_thread")]
async fn the_deprecated_flat_manifest_still_reads_on_the_same_lane() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    write(
        root.path(),
        "navigator.yaml",
        format!("project: acme\nhost: {host}\n"),
    );
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    mount_projects(&server, project_id).await;
    mount_revisions(&server, project_id, asset_id).await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "document", "log", POINTER])
        .assert()
        .success()
        .stdout(predicate::str::contains(SLUG));
}
