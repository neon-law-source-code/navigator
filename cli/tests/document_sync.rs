//! End-to-end coverage for `navigator site sync` and `navigator site pull`
//! against their authorized API.

use std::fs;
use std::path::Path;

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

fn write_layout_for_validate(root: &Path) {
    write(
        root,
        "navigator.yaml",
        "project: acme\nhost: staging.neonlaw.com\n",
    );
    write(root, "README.md", "# acme\n\nProject source.\n");
    write(
        root,
        ".github/workflows/ci.yml",
        r#"name: ci
on: [pull_request]
jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23
    secrets: inherit
    with:
      version: "26.8.23"
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
        .respond_with(ResponseTemplate::new(201).set_body_json(pointer(asset_id)))
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
        fs::read_to_string(root.path().join("documents/pleadings/summons.pdf.yml")).unwrap();
    assert!(pointer.contains(&asset_id.to_string()));
    assert_eq!(
        fs::read_to_string(root.path().join("documents/.gitignore")).unwrap(),
        "*\n!*/\n!*.yml\n!.gitignore\n"
    );

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
        .env("NAVIGATOR_CREDENTIALS_FILE", &credential_path)
        .args(["site", "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 uploaded"));

    write_layout_for_validate(root.path());
    navigator()
        .current_dir(root.path())
        .args(["validate", "."])
        .assert()
        .success();
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
            19,
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

    // The layout gate scans the filesystem directly and knows nothing about
    // `.gitignore`, so the bytes `pull` just wrote are exactly as refused as
    // any other raw document byte would be — `pull` must never widen that gate.
    write_layout_for_validate(root.path());
    navigator()
        .current_dir(root.path())
        .args(["validate", "."])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "legal documents and raw document bytes must not be committed",
        ));
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
        .failure();

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
        .stderr(predicate::str::contains("sha256 mismatch"));

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
        .stderr(predicate::str::contains("privileged.pdf.yml"));

    assert!(!root
        .path()
        .join("documents/pleadings/privileged.pdf")
        .exists());
}
