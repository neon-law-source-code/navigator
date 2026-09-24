//! `navigator project gate --check` through the compiled binary.
//!
//! The comparisons live in `cli/src/projects/document_check.rs`. These tests
//! pin the wiring: `--help` states the auto-fix, a plain `project gate` makes
//! no request, and `--check` reports a missing object.

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

fn pointer(asset_id: Uuid) -> String {
    format!(
        "kind: filing\nvisibility: internal\ncurrent_version:\n  version: 1\n  \
         asset_id: {asset_id}\n  created_at: \"2026-09-05T12:00:00Z\"\n  sha256: {SHA}\n  \
         size_bytes: 18\n"
    )
}

fn root_with_manifest(host: &str) -> TempDir {
    let root = TempDir::new().unwrap();
    write(root.path(), "README.md", "Synthetic repository\n");
    write(root.path(), ".git", "gitdir: /tmp/synthetic\n");
    manifest(root.path(), host);
    root
}

#[test]
fn project_gate_check_help_states_the_autofix() {
    navigator()
        .args(["project", "gate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--check"))
        .stdout(predicate::str::contains("Never writes to the live site"))
        .stdout(predicate::str::contains("--ci"));
}

#[tokio::test(flavor = "multi_thread")]
async fn project_gate_without_check_makes_no_network_calls() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = root_with_manifest(&host);
    let creds = TempDir::new().unwrap();
    let credential_path = credentials(creds.path(), &host);

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["project", "gate"])
        .assert()
        .code(predicate::ne(3));

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "plain project gate requested {:?}",
        requests
            .iter()
            .map(|request| request.url.to_string())
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn project_gate_check_reports_a_missing_object() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = root_with_manifest(&host);
    let creds = TempDir::new().unwrap();
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    let relative = "documents/pleadings/summons.pdf.yaml";
    let yaml = pointer(asset_id);
    write(root.path(), relative, &yaml);
    write(
        root.path(),
        "documents/.gitignore",
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/integrity"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "assets": [{
                "asset_id": asset_id,
                "slug": "pleadings/summons.pdf",
                "exists": false,
                "recorded_size": 18
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/revisions"
        )))
        .and(query_param("slug", "pleadings/summons.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "kind": "filing",
            "revisions": [{
                "version": 1,
                "asset_id": asset_id,
                "created_at": "2026-09-05T12:00:00Z",
                "sha256": SHA,
                "size_bytes": 18,
                "filename": "summons.pdf",
                "visibility": "internal",
                "operative": true
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["project", "gate", "--check"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("storage object missing"));

    assert_eq!(
        fs::read_to_string(root.path().join(relative)).unwrap(),
        yaml
    );
}

/// LAW-62: `--check --ci` used to run the whole offline gate — content
/// rules, YAML, seeds, layout, and, because a Project can declare one, the
/// origin pass reading each application's built `dist/` — before ever
/// reaching the live document check below. `documents` never builds, so a
/// portal with no `dist/` failed every such run with a `Y009` finding even
/// though the live records matched exactly. `--check` must run only the
/// live document check.
#[tokio::test(flavor = "multi_thread")]
async fn project_gate_check_ci_skips_the_offline_gate_and_the_origin_pass() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = root_with_manifest(&host);
    // A portal with no `dist/`: the origin pass would refuse this under
    // `--ci` if `--check` still ran the offline gate first.
    fs::create_dir_all(root.path().join("portal")).unwrap();
    write(
        root.path(),
        "documents/.gitignore",
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    );
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    let relative = "documents/pleadings/summons.pdf.yaml";
    write(root.path(), relative, pointer(asset_id));

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
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/integrity"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "assets": [{
                "asset_id": asset_id,
                "slug": "pleadings/summons.pdf",
                "exists": true,
                "size_bytes": 18,
                "recorded_size": 18
            }],
            "integrations": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/revisions"
        )))
        .and(query_param("slug", "pleadings/summons.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "kind": "filing",
            "revisions": [{
                "version": 1,
                "asset_id": asset_id,
                "created_at": "2026-09-05T12:00:00Z",
                "sha256": SHA,
                "size_bytes": 18,
                "filename": "summons.pdf",
                "visibility": "internal",
                "operative": true
            }]
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
        .success()
        .stdout(predicate::str::contains("documents:"))
        .stdout(predicate::str::contains("Y009").not())
        .stdout(predicate::str::contains("dist/").not());
}
