//! End-to-end coverage for `navigator site sync` against its authorized API.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn navigator() -> Command {
    Command::cargo_bin("navigator").unwrap()
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
        format!("project: acme\nhost: {host}\n"),
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

fn pointer(asset_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "kind": "filing",
        "visibility": "internal",
        "current_version": {
            "version": 1,
            "asset_id": asset_id,
            "created_at": "2026-09-05T12:00:00Z",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "size_bytes": 18
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_uploads_through_the_api_writes_a_pointer_and_removes_the_binary() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(root.path(), &host);
    write(
        root.path(),
        "documents/pleadings/summons.pdf",
        b"synthetic pleading",
    );
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/{asset_id}"
        )))
        .and(header("authorization", "Bearer test-token"))
        .and(body_json(serde_json::json!({ "visibility": "client" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "changed": false })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(header("authorization", "Bearer test-token"))
        .and(body_json(serde_json::json!({
            "filename": "summons.pdf",
            "content_base64": "c3ludGhldGljIHBsZWFkaW5n",
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/summons.pdf"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer(asset_id)))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 uploaded"));

    assert!(!root.path().join("documents/pleadings/summons.pdf").exists());
    let pointer =
        fs::read_to_string(root.path().join("documents/pleadings/summons.pdf.yml")).unwrap();
    assert!(pointer.contains(&asset_id.to_string()));
    assert_eq!(
        fs::read_to_string(root.path().join("documents/.gitignore")).unwrap(),
        "*\n!*/\n!*.yml\n!.gitignore\n"
    );
    navigator()
        .current_dir(root.path())
        .args(["validate", "."])
        .assert()
        .success();

    let pointer_path = root.path().join("documents/pleadings/summons.pdf.yml");
    fs::write(
        &pointer_path,
        pointer.replace("visibility: internal", "visibility: client"),
    )
    .unwrap();

    // Re-running after a pointer-only visibility edit uploads nothing and
    // reconciles the desired state through the authorized API.
    navigator()
        .current_dir(root.path())
        .env(
            "NAVIGATOR_CREDENTIALS_FILE",
            root.path().join("credentials.json"),
        )
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 uploaded"));
}

#[test]
fn sync_dry_run_lists_work_without_writing_or_needing_a_login() {
    let root = TempDir::new().unwrap();
    manifest(root.path(), "staging.example.com");
    write(
        root.path(),
        "documents/exhibits/photo.png",
        b"synthetic image",
    );

    navigator()
        .current_dir(root.path())
        .args(["site", "sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would upload documents/exhibits/photo.png",
        ))
        .stdout(predicate::str::contains("1 upload planned"));

    assert!(root.path().join("documents/exhibits/photo.png").is_file());
    assert!(!root
        .path()
        .join("documents/exhibits/photo.png.yml")
        .exists());
    assert!(!root.path().join("documents/.gitignore").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_upload_leaves_the_binary_and_writes_no_pointer() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(root.path(), &host);
    write(
        root.path(),
        "documents/agreements/terms.pdf",
        b"synthetic agreement",
    );
    let project_id = Uuid::now_v7();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .respond_with(ResponseTemplate::new(503).set_body_string("try again"))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .failure();

    assert!(root.path().join("documents/agreements/terms.pdf").is_file());
    assert!(!root
        .path()
        .join("documents/agreements/terms.pdf.yml")
        .exists());
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn an_interrupted_multi_file_sync_resumes_without_refiling_completed_work() {
    let first_server = MockServer::start().await;
    let first_host = first_server.uri();
    let root = TempDir::new().unwrap();
    manifest(root.path(), &first_host);
    let first_credentials = credentials(root.path(), &first_host);
    write(root.path(), "documents/pleadings/a.pdf", b"one");
    write(root.path(), "documents/pleadings/b.pdf", b"two");
    let project_id = Uuid::now_v7();
    let first_asset = Uuid::now_v7();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&first_server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "a.pdf",
            "content_base64": "b25l",
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/a.pdf"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer(first_asset)))
        .expect(1)
        .mount(&first_server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "b.pdf",
            "content_base64": "dHdv",
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/b.pdf"
        })))
        .respond_with(ResponseTemplate::new(503).set_body_string("interrupted"))
        .expect(1)
        .mount(&first_server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", first_credentials)
        .args(["site", "sync"])
        .assert()
        .failure();
    assert!(!root.path().join("documents/pleadings/a.pdf").exists());
    assert!(root.path().join("documents/pleadings/a.pdf.yml").exists());
    assert!(root.path().join("documents/pleadings/b.pdf").exists());

    let second_server = MockServer::start().await;
    let second_host = second_server.uri();
    manifest(root.path(), &second_host);
    let second_credentials = credentials(root.path(), &second_host);
    let second_asset = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&second_server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/{first_asset}"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "changed": false })),
        )
        .expect(1)
        .mount(&second_server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "b.pdf",
            "content_base64": "dHdv",
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/b.pdf"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer(second_asset)))
        .expect(1)
        .mount(&second_server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", second_credentials)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 uploaded"));
    assert!(root.path().join("documents/pleadings/a.pdf.yml").exists());
    assert!(!root.path().join("documents/pleadings/b.pdf").exists());
    assert!(root.path().join("documents/pleadings/b.pdf.yml").exists());
}
