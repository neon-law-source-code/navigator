//! End-to-end coverage for `navigator site sync` and `navigator site pull`
//! against their authorized API.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    store::documents::sha256_hex(bytes)
}

fn navigator() -> Command {
    let mut command = Command::cargo_bin("navigator").unwrap();
    command.env_remove("GITHUB_REPOSITORY");
    command
}

#[cfg(windows)]
fn junction(target: &Path, link: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

fn write_agent_contract(root: &Path) {
    write(
        root,
        "AGENTS.md",
        "# Working in acme\n\n\
         When Navigator's CLI is missing or wrong, open a Linear issue on the Lawyers team rather than documenting a CLI\n\
         workaround here.\n",
    );
}

fn write_layout_for_the_gate(root: &Path) {
    write(
        root,
        "navigator.yaml",
        "project: acme\nhost: staging.neonlaw.com\n",
    );
    write(root, "README.md", "# acme\n\nProject source.\n");
    write(root, ".github/CODEOWNERS", "# CODEOWNERS\n\n* @shicholas\n");
    write_agent_contract(root);
    // The gate identifies a repository root by its `README` and `.git`, and
    // reads the files Git would carry.
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed in {}", root.display());
    write(
        root,
        ".github/workflows/ci.yml",
        r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23
    with:
      project: "acme"
      host: "staging.neonlaw.com"
"#,
    );
    write(
        root,
        ".github/workflows/cd.yml",
        r#"name: cd
on:
  push:
    branches: [main]
  workflow_dispatch:
permissions:
  contents: read
  id-token: write
jobs:
  gate:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23
    with:
      project: "acme"
      host: "staging.neonlaw.com"
  publish:
    needs: gate
    uses: neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.8.23
    with:
      project: "acme"
      host: "staging.neonlaw.com"
"#,
    );
}

fn pointer(asset_id: Uuid) -> serde_json::Value {
    pointer_with_sha(
        asset_id,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        18,
    )
}

fn pointer_with_sha(asset_id: Uuid, sha256: &str, size_bytes: i64) -> serde_json::Value {
    serde_json::json!({
        "kind": "filing",
        "visibility": "internal",
        "current_version": {
            "version": 1,
            "asset_id": asset_id,
            "created_at": "2026-09-05T12:00:00Z",
            "sha256": sha256,
            "size_bytes": size_bytes
        }
    })
}

fn pointer_with_kind_visibility(asset_id: Uuid, kind: &str, visibility: &str) -> serde_json::Value {
    let mut pointer = pointer(asset_id);
    pointer["kind"] = serde_json::Value::String(kind.to_string());
    pointer["visibility"] = serde_json::Value::String(visibility.to_string());
    pointer
}

async fn mount_project_lookup(server: &MockServer, project_id: Uuid) {
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_uploads_through_the_api_writes_a_pointer_and_removes_the_binary() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
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
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            asset_id,
            &sha256(b"synthetic pleading"),
            i64::try_from(b"synthetic pleading".len()).unwrap(),
        )))
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

    assert!(!root.path().join("documents/pleadings/summons.pdf").exists());
    let pointer =
        fs::read_to_string(root.path().join("documents/pleadings/summons.pdf.yaml")).unwrap();
    assert!(pointer.contains(&asset_id.to_string()));
    assert_eq!(
        fs::read_to_string(root.path().join("documents/.gitignore")).unwrap(),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n"
    );

    let pointer_path = root.path().join("documents/pleadings/summons.pdf.yaml");
    fs::write(
        &pointer_path,
        pointer.replace("visibility: internal", "visibility: client"),
    )
    .unwrap();

    // Re-running after a pointer-only visibility edit uploads nothing and
    // reconciles the desired state through the authorized API.
    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 uploaded"));

    write_layout_for_the_gate(root.path());
    navigator()
        .current_dir(root.path())
        .args(["project", "gate"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_preserves_existing_pointer_kind_when_uploading_new_bytes() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write(
        root.path(),
        "documents/pleadings/motion.pdf",
        b"replacement pleading",
    );
    write(
        root.path(),
        "documents/pleadings/motion.pdf.yml",
        serde_yaml::to_string(&pointer_with_kind_visibility(
            asset_id, "pleading", "client",
        ))
        .unwrap(),
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/app/api/projects/{project_id}/documents/{asset_id}"
        )))
        .and(body_json(serde_json::json!({ "visibility": "client" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "changed": false })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "motion.pdf",
            "content_base64": "cmVwbGFjZW1lbnQgcGxlYWRpbmc=",
            "content_type": "application/pdf",
            "kind": "pleading",
            "visibility": "client",
            "slug": "pleadings/motion.pdf"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json({
            let mut pointer = pointer_with_sha(
                asset_id,
                &sha256(b"replacement pleading"),
                i64::try_from(b"replacement pleading".len()).unwrap(),
            );
            pointer["kind"] = serde_json::Value::String("pleading".to_string());
            pointer["visibility"] = serde_json::Value::String("client".to_string());
            pointer
        }))
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

    let pointer = fs::read_to_string(root.path().join("documents/pleadings/motion.pdf.yml"))
        .expect("rewritten pointer");
    assert!(pointer.contains("kind: pleading"));
    assert!(pointer.contains("visibility: client"));
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn sync_refuses_a_symlinked_documents_root_before_network_or_writes() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(outside.path(), "pleadings/outside.pdf", b"outside bytes");
    symlink(outside.path(), root.path().join("documents")).unwrap();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert_eq!(
        fs::read(outside.path().join("pleadings/outside.pdf")).unwrap(),
        b"outside bytes"
    );
    assert!(!outside.path().join(".gitignore").exists());
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn sync_refuses_a_junctioned_documents_root_before_network_or_writes() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(outside.path(), "pleadings/outside.pdf", b"outside bytes");
    junction(outside.path(), &root.path().join("documents"));

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert_eq!(
        fs::read(outside.path().join("pleadings/outside.pdf")).unwrap(),
        b"outside bytes"
    );
    assert!(!outside.path().join(".gitignore").exists());
}

#[cfg(unix)]
#[test]
fn sync_dry_run_refuses_a_nested_symlink_instead_of_reporting_an_empty_tree() {
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    manifest(root.path(), "staging.example.com");
    write(outside.path(), "nested.pdf", b"outside bytes");
    fs::create_dir_all(root.path().join("documents")).unwrap();
    symlink(outside.path(), root.path().join("documents/exhibits")).unwrap();

    navigator()
        .current_dir(root.path())
        .args(["site", "sync", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert!(!root.path().join("documents/.gitignore").exists());
    assert_eq!(
        fs::read(outside.path().join("nested.pdf")).unwrap(),
        b"outside bytes"
    );
}

#[cfg(unix)]
#[test]
fn sync_dry_run_refuses_a_symlinked_source_path() {
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    manifest(root.path(), "staging.example.com");
    write(outside.path(), "source.pdf", b"outside bytes");
    fs::create_dir_all(root.path().join("documents/pleadings")).unwrap();
    symlink(
        outside.path().join("source.pdf"),
        root.path().join("documents/pleadings/source.pdf"),
    )
    .unwrap();

    navigator()
        .current_dir(root.path())
        .args(["site", "sync", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert!(!root.path().join("documents/.gitignore").exists());
    assert_eq!(
        fs::read(outside.path().join("source.pdf")).unwrap(),
        b"outside bytes"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn sync_refuses_a_symlinked_pointer_target_before_network_or_writes() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(
        root.path(),
        "documents/pleadings/source.pdf",
        b"source bytes",
    );
    write(outside.path(), "pointer.yaml", b"existing pointer bytes");
    symlink(
        outside.path().join("pointer.yaml"),
        root.path().join("documents/pleadings/source.pdf.yaml"),
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert_eq!(
        fs::read(root.path().join("documents/pleadings/source.pdf")).unwrap(),
        b"source bytes"
    );
    assert_eq!(
        fs::read(outside.path().join("pointer.yaml")).unwrap(),
        b"existing pointer bytes"
    );
    assert!(!root.path().join("documents/.gitignore").exists());
}

async fn assert_receipt_rejected_without_consuming_source(
    response: ResponseTemplate,
    existing_pointer: bool,
) {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let source = root.path().join("documents/pleadings/receipt.pdf");
    let source_bytes = b"receipt source bytes";
    write(root.path(), "documents/pleadings/receipt.pdf", source_bytes);
    let project_id = Uuid::now_v7();
    let existing_asset = Uuid::now_v7();
    let existing_path = root.path().join("documents/pleadings/receipt.pdf.yaml");
    let before_pointer = serde_yaml::to_string(&pointer_with_sha(
        existing_asset,
        &sha256(b"old receipt bytes"),
        17,
    ))
    .unwrap();
    if existing_pointer {
        write(
            root.path(),
            "documents/pleadings/receipt.pdf.yaml",
            &before_pointer,
        );
    }

    mount_project_lookup(&server, project_id).await;
    if existing_pointer {
        Mock::given(method("PATCH"))
            .and(path(format!(
                "/app/api/projects/{project_id}/documents/{existing_asset}"
            )))
            .and(body_json(serde_json::json!({"visibility": "internal"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "changed": false
            })))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .assert()
        .failure();

    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    if existing_pointer {
        assert_eq!(fs::read(existing_path).unwrap(), before_pointer.as_bytes());
    } else {
        assert!(!existing_path.exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_rejects_a_receipt_with_a_wrong_digest() {
    let source_bytes = b"receipt source bytes";
    assert_receipt_rejected_without_consuming_source(
        ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            Uuid::now_v7(),
            &"0".repeat(64),
            i64::try_from(source_bytes.len()).unwrap(),
        )),
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_rejects_a_receipt_with_a_wrong_byte_count_and_keeps_the_pointer() {
    let source_bytes = b"receipt source bytes";
    assert_receipt_rejected_without_consuming_source(
        ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            Uuid::now_v7(),
            &sha256(source_bytes),
            i64::try_from(source_bytes.len()).unwrap() + 1,
        )),
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_rejects_a_malformed_receipt_and_keeps_the_pointer() {
    assert_receipt_rejected_without_consuming_source(
        ResponseTemplate::new(201).set_body_string("not a pointer"),
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_keeps_changed_source_bytes_when_upload_finishes_late() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let source = root.path().join("documents/pleadings/race.pdf");
    let uploaded_bytes = b"bytes sent before the race";
    write(root.path(), "documents/pleadings/race.pdf", uploaded_bytes);
    let project_id = Uuid::now_v7();
    mount_project_lookup(&server, project_id).await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "race.pdf",
            "content_base64": base64_of(uploaded_bytes),
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/race.pdf"
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_delay(Duration::from_millis(800))
                .set_body_json(pointer_with_sha(
                    Uuid::now_v7(),
                    &sha256(uploaded_bytes),
                    i64::try_from(uploaded_bytes.len()).unwrap(),
                )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_navigator"))
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "sync"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    fs::write(&source, b"changed while upload was in flight").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert_eq!(
        fs::read(&source).unwrap(),
        b"changed while upload was in flight"
    );
    assert!(!root
        .path()
        .join("documents/pleadings/race.pdf.yaml")
        .exists());
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
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            first_asset,
            &sha256(b"one"),
            3,
        )))
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
    assert!(root.path().join("documents/pleadings/a.pdf.yaml").exists());
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
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            second_asset,
            &sha256(b"two"),
            3,
        )))
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
    assert!(root.path().join("documents/pleadings/a.pdf.yaml").exists());
    assert!(!root.path().join("documents/pleadings/b.pdf").exists());
    assert!(root.path().join("documents/pleadings/b.pdf.yaml").exists());
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn pull_round_trips_synced_bytes_and_a_second_pull_writes_nothing() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(
        root.path(),
        "documents/pleadings/a.pdf",
        b"synthetic filing a",
    );
    write(
        root.path(),
        "documents/exhibits/b.png",
        b"synthetic exhibit b",
    );
    let project_id = Uuid::now_v7();
    let asset_a = Uuid::now_v7();
    let asset_b = Uuid::now_v7();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "a.pdf",
            "content_base64": base64_of(b"synthetic filing a"),
            "content_type": "application/pdf",
            "kind": "filing",
            "visibility": "internal",
            "slug": "pleadings/a.pdf"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            asset_a,
            &sha256(b"synthetic filing a"),
            18,
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/projects/{project_id}/documents")))
        .and(body_json(serde_json::json!({
            "filename": "b.png",
            "content_base64": base64_of(b"synthetic exhibit b"),
            "content_type": "image/png",
            "kind": "exhibit",
            "visibility": "internal",
            "slug": "exhibits/b.png"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer_with_sha(
            asset_b,
            &sha256(b"synthetic exhibit b"),
            19,
        )))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 uploaded"));

    // `sync` removes the staged binary after a successful upload, so the
    // checkout now holds only the two committed pointers — the same shape a
    // fresh clone starts from.
    assert!(!root.path().join("documents/pleadings/a.pdf").exists());
    assert!(!root.path().join("documents/exhibits/b.png").exists());

    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_a}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"synthetic filing a".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_b}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"synthetic exhibit b".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 pulled"));

    assert_eq!(
        fs::read(root.path().join("documents/pleadings/a.pdf")).unwrap(),
        b"synthetic filing a"
    );
    assert_eq!(
        fs::read(root.path().join("documents/exhibits/b.png")).unwrap(),
        b"synthetic exhibit b"
    );

    // A second pull finds every local digest already matching the pointer and
    // downloads nothing — the mocks above are `.expect(1)`, so a second
    // network call would fail the test.
    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 pulled"));

    // `pull` writes into `documents/`, whose `.gitignore` keeps the bytes out of
    // Git; the gate judges what a pull request proposes, so it reads them as
    // absent rather than as committed material. The guard that a *tracked* raw
    // byte still fails is `gate_reports_a_tracked_raw_document_byte` in
    // `cli/tests/project_repository.rs`.
    write_layout_for_the_gate(root.path());
    navigator()
        .current_dir(root.path())
        .args(["project", "gate"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
#[cfg(unix)]
async fn pull_rolls_back_publication_when_second_target_fails_after_first_would_change() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    write(
        root.path(),
        "documents/.gitignore",
        b"keep this ignore file\n",
    );
    let project_id = Uuid::now_v7();
    let first_asset = Uuid::now_v7();
    let second_asset = Uuid::now_v7();
    let first_bytes = b"fresh first document";
    let second_bytes = b"fresh second document";
    let first_pointer = root.path().join("documents/pleadings/a.pdf.yml");
    let second_pointer = root.path().join("documents/pleadings/b.pdf.yml");
    let first_target = root.path().join("documents/pleadings/a.pdf");
    let second_target = root.path().join("documents/pleadings/b.pdf");

    write(
        root.path(),
        "documents/pleadings/a.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            first_asset,
            &sha256(first_bytes),
            i64::try_from(first_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );
    write(
        root.path(),
        "documents/pleadings/b.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            second_asset,
            &sha256(second_bytes),
            i64::try_from(second_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );
    write(
        root.path(),
        "documents/pleadings/a.pdf",
        b"old first document",
    );
    write(
        root.path(),
        "documents/pleadings/b.pdf",
        b"old second document",
    );
    fs::set_permissions(&second_target, fs::Permissions::from_mode(0o444)).unwrap();
    let first_pointer_before = fs::read(&first_pointer).unwrap();
    let second_pointer_before = fs::read(&second_pointer).unwrap();

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{first_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(first_bytes.to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{second_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(second_bytes.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "pull"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("document targets are unchanged"));

    assert_eq!(fs::read(&first_target).unwrap(), b"old first document");
    assert_eq!(fs::read(&second_target).unwrap(), b"old second document");
    assert_eq!(fs::read(&first_pointer).unwrap(), first_pointer_before);
    assert_eq!(fs::read(&second_pointer).unwrap(), second_pointer_before);
    assert_eq!(
        fs::read(root.path().join("documents/.gitignore")).unwrap(),
        b"keep this ignore file\n"
    );
    assert!(fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .all(|entry| !entry
            .file_name()
            .to_string_lossy()
            .starts_with(".navigator-pull-")));

    fs::set_permissions(&second_target, fs::Permissions::from_mode(0o644)).unwrap();

    let retry_server = MockServer::start().await;
    let retry_host = retry_server.uri();
    manifest(root.path(), &retry_host);
    let retry_credentials = credentials(creds.path(), &retry_host);
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&retry_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{first_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(first_bytes.to_vec()))
        .expect(1)
        .mount(&retry_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{second_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(second_bytes.to_vec()))
        .expect(1)
        .mount(&retry_server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", retry_credentials)
        .args(["site", "pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 pulled"));

    assert_eq!(fs::read(&first_target).unwrap(), first_bytes);
    assert_eq!(fs::read(&second_target).unwrap(), second_bytes);
    assert_eq!(fs::read(&first_pointer).unwrap(), first_pointer_before);
    assert_eq!(fs::read(&second_pointer).unwrap(), second_pointer_before);
    assert_eq!(
        fs::read(root.path().join("documents/.gitignore")).unwrap(),
        b"keep this ignore file\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_publishes_nothing_when_a_later_download_fails() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let first_asset = Uuid::now_v7();
    let second_asset = Uuid::now_v7();
    let first_bytes = b"fresh first document";
    let second_bytes = b"fresh second document";

    write(
        root.path(),
        "documents/pleadings/a.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            first_asset,
            &sha256(first_bytes),
            i64::try_from(first_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );
    write(
        root.path(),
        "documents/pleadings/b.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            second_asset,
            &sha256(second_bytes),
            i64::try_from(second_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );
    write(
        root.path(),
        "documents/pleadings/b.pdf",
        b"pre-existing document bytes",
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{first_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(first_bytes.to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{second_asset}/download"
        )))
        .respond_with(ResponseTemplate::new(503).set_body_string("late failure"))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "pull"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("document targets are unchanged"));

    assert!(!root.path().join("documents/pleadings/a.pdf").exists());
    assert_eq!(
        fs::read(root.path().join("documents/pleadings/b.pdf")).unwrap(),
        b"pre-existing document bytes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_publishes_nothing_when_a_download_digest_mismatches() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    let expected_bytes = b"expected document bytes";

    write(
        root.path(),
        "documents/pleadings/motion.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            asset_id,
            &sha256(expected_bytes),
            i64::try_from(expected_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_id}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"wrong bytes".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "pull"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("sha256 mismatch"))
        .stderr(predicate::str::contains("document targets are unchanged"));

    assert!(!root.path().join("documents/pleadings/motion.pdf").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_replaces_an_existing_mismatched_target_after_verification() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    let creds = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(creds.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    let expected_bytes = b"current document bytes";
    write(
        root.path(),
        "documents/pleadings/motion.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(
            asset_id,
            &sha256(expected_bytes),
            i64::try_from(expected_bytes.len()).unwrap(),
        ))
        .unwrap(),
    );
    write(
        root.path(),
        "documents/pleadings/motion.pdf",
        b"stale document bytes",
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_id}/download"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(expected_bytes.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 pulled"));

    assert_eq!(
        fs::read(root.path().join("documents/pleadings/motion.pdf")).unwrap(),
        expected_bytes
    );
}

#[test]
fn pull_dry_run_lists_pending_pulls_without_logging_in() {
    let root = TempDir::new().unwrap();
    manifest(root.path(), "staging.example.com");
    let asset_id = Uuid::now_v7();
    write(
        root.path(),
        "documents/pleadings/motion.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(asset_id, &"a".repeat(64), 3)).unwrap(),
    );

    navigator()
        .current_dir(root.path())
        .args(["site", "pull", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would pull documents/pleadings/motion.pdf",
        ))
        .stdout(predicate::str::contains("1 pull(s) planned"));

    assert!(!root.path().join("documents/pleadings/motion.pdf").exists());
}

#[test]
fn a_failure_before_the_guard_is_written_promises_no_gitignore() {
    // LAW-12: every refusal in `pull` claimed "documents/.gitignore may
    // have been created", including the ones reached before `pull` writes
    // that file at all. A dry run returns early, well ahead of the guard,
    // so the claim was simply false — and the operator who went looking
    // for the file found nothing.
    let root = TempDir::new().unwrap();
    write(root.path(), "navigator.yaml", "project: [not, a, scalar]\n");
    write(
        root.path(),
        "documents/pleadings/motion.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(Uuid::now_v7(), &"a".repeat(64), 3)).unwrap(),
    );

    navigator()
        .current_dir(root.path())
        .args(["site", "pull", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("document targets are unchanged"))
        .stderr(predicate::str::contains(".gitignore").not());

    assert!(
        !root.path().join("documents/.gitignore").exists(),
        "the dry run must not write the guard file it was said to write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pointer_the_caller_cannot_read_is_reported_and_no_file_is_written() {
    let server = MockServer::start().await;
    let host = server.uri();
    let root = TempDir::new().unwrap();
    manifest(root.path(), &host);
    let credential_path = credentials(root.path(), &host);
    let project_id = Uuid::now_v7();
    let asset_id = Uuid::now_v7();
    write(
        root.path(),
        "documents/pleadings/privileged.pdf.yml",
        serde_yaml::to_string(&pointer_with_sha(asset_id, &"a".repeat(64), 3)).unwrap(),
    );

    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": project_id, "code": "acme"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/app/projects/acme/documents/{asset_id}/download"
        )))
        .respond_with(ResponseTemplate::new(403).set_body_string("not on your lens"))
        .expect(1)
        .mount(&server)
        .await;

    navigator()
        .current_dir(root.path())
        .env("NAVIGATOR_CREDENTIALS_FILE", credential_path)
        .args(["site", "pull"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("privileged.pdf.yml"))
        .stderr(predicate::str::contains("document targets are unchanged"));

    assert!(!root
        .path()
        .join("documents/pleadings/privileged.pdf")
        .exists());
}
