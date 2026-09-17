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
//! — failed with `project: invalid type: map, expected a string`. LAW-18 named
//! `site document verify --ci`, but `site sync`, `site pull`, `site document
//! log`, and `site document get` all failed identically, because each carried
//! the defect through the same lane rather than through one shared reader.
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

/// The command LAW-18 was filed against. It authenticates through GitHub
/// Actions OIDC rather than a stored login, so the runner environment and both
/// mint hops are served by the same mock — the point is that the manifest is
/// read the same way regardless of which credential the mode uses.
#[tokio::test(flavor = "multi_thread")]
async fn site_document_verify_ci_reads_the_nested_manifest() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    v2_manifest(root.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write_pointer(root.path(), asset_id);

    // The runner's own OIDC endpoint.
    Mock::given(method("GET"))
        .and(path("/actions/oidc"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "value": "github-oidc-token" })),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The deployment's document-token mint.
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

    navigator()
        .current_dir(root.path())
        .env(
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            format!("{host}/actions/oidc"),
        )
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "runner-token")
        .args(["site", "document", "verify", ".", "--ci", "--host", &host])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "1 pointer(s) verified against the live record",
        ));
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
