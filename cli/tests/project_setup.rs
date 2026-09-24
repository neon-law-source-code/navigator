//! Actual `navigator project setup` outcomes through the authenticated HTTP
//! doors. The CLI is intentionally tested as a process: a unit test of the
//! composition would not prove clap parsing, stored-login resolution, or
//! exit-code behavior.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn credentials(dir: &TempDir, host: &str) -> std::path::PathBuf {
    let path = dir.path().join("navigator.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "hosts": {
                host: {
                    "token": "test-token",
                    "person_email": "admin@example.com",
                    "role": "admin",
                    "expires_at": i64::MAX
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

async fn mount_setup_doors(
    server: &MockServer,
    project_id: Uuid,
    code: &str,
    integration_outcomes: [(&str, &str); 2],
) {
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "id": project_id, "code": code }
        ])))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/project-surfaces/{project_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": code,
            "documents_prefix": format!("projects/{code}/documents"),
            "drive_folder_id": "drive-1",
            "drive_status": "created",
            "repository_url": "https://forge.example/acme",
            "repository_status": "present"
        })))
        .mount(server)
        .await;
    for (provider, outcome) in integration_outcomes {
        Mock::given(method("POST"))
            .and(path(format!("/app/api/integrations/{provider}/ensure")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [{ "project_code": code, "outcome": outcome }]
            })))
            .mount(server)
            .await;
    }
}

fn command() -> Command {
    let mut command = Command::cargo_bin("navigator").unwrap();
    command.env_remove("GITHUB_REPOSITORY");
    command
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_runs_every_door_and_repeat_execution_is_successful() {
    let server = MockServer::start().await;
    let host = server.uri();
    let credentials_dir = TempDir::new().unwrap();
    let credentials_path = credentials(&credentials_dir, &host);
    let project_id = Uuid::now_v7();
    mount_setup_doors(
        &server,
        project_id,
        "acme",
        [("slack", "adopted"), ("notion", "adopted")],
    )
    .await;

    for _ in 0..2 {
        let output = command()
            .env("NAVIGATOR_CREDENTIALS_FILE", &credentials_path)
            .args(["project", "setup", "acme", "--host", &host, "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["results"][0]["project_code"], "acme");
        assert_eq!(
            report["results"][0]["resources"]["drive"]["outcome"],
            "created"
        );
        assert_eq!(
            report["results"][0]["resources"]["repository"]["outcome"],
            "present"
        );
        assert_eq!(
            report["results"][0]["resources"]["slack"]["outcome"],
            "adopted"
        );
        assert_eq!(
            report["results"][0]["resources"]["notion"]["outcome"],
            "adopted"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_all_uses_the_visible_project_list() {
    let server = MockServer::start().await;
    let host = server.uri();
    let credentials_dir = TempDir::new().unwrap();
    let credentials_path = credentials(&credentials_dir, &host);
    let project_id = Uuid::now_v7();
    mount_setup_doors(
        &server,
        project_id,
        "acme",
        [("slack", "created"), ("notion", "created")],
    )
    .await;

    let output = command()
        .env("NAVIGATOR_CREDENTIALS_FILE", &credentials_path)
        .args(["project", "setup", "--all", "--host", &host, "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["results"].as_array().unwrap().len(), 1);
    assert_eq!(report["results"][0]["project_code"], "acme");
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_reports_partial_failure_and_returns_nonzero() {
    let server = MockServer::start().await;
    let host = server.uri();
    let credentials_dir = TempDir::new().unwrap();
    let credentials_path = credentials(&credentials_dir, &host);
    let project_id = Uuid::now_v7();
    mount_setup_doors(
        &server,
        project_id,
        "acme",
        [("slack", "credential_missing"), ("notion", "created")],
    )
    .await;

    let output = command()
        .env("NAVIGATOR_CREDENTIALS_FILE", &credentials_path)
        .args(["project", "setup", "acme", "--host", &host, "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["results"][0]["resources"]["drive"]["outcome"],
        "created"
    );
    assert_eq!(
        report["results"][0]["resources"]["slack"]["outcome"],
        "credential_missing"
    );
    assert_eq!(
        report["results"][0]["resources"]["notion"]["outcome"],
        "created"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_keeps_running_after_an_authorization_refusal() {
    let server = MockServer::start().await;
    let host = server.uri();
    let credentials_dir = TempDir::new().unwrap();
    let credentials_path = credentials(&credentials_dir, &host);
    let project_id = Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/app/api/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "id": project_id, "code": "acme" }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/app/api/project-surfaces/{project_id}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": "forbidden",
            "message": "admin tier required"
        })))
        .mount(&server)
        .await;
    for provider in ["slack", "notion"] {
        Mock::given(method("POST"))
            .and(path(format!("/app/api/integrations/{provider}/ensure")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [{ "project_code": "acme", "outcome": "created" }]
            })))
            .mount(&server)
            .await;
    }

    let output = command()
        .env("NAVIGATOR_CREDENTIALS_FILE", &credentials_path)
        .args(["project", "setup", "acme", "--host", &host, "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["results"][0]["resources"]["drive"]["outcome"],
        "request_failed"
    );
    assert_eq!(
        report["results"][0]["resources"]["repository"]["outcome"],
        "request_failed"
    );
    for resource in ["drive", "repository"] {
        let detail = report["results"][0]["resources"][resource]["detail"]
            .as_str()
            .unwrap_or_default();
        assert!(detail.contains("403"), "{resource}: {detail}");
        assert!(
            detail.contains("admin tier required"),
            "{resource}: {detail}"
        );
    }
    assert_eq!(
        report["results"][0]["resources"]["slack"]["outcome"],
        "created"
    );
    assert_eq!(
        report["results"][0]["resources"]["notion"]["outcome"],
        "created"
    );
}
