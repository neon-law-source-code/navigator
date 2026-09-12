//! `navigator site projects gate` — the Project CI live-status check.
//!
//! Layout and origin already run through `navigator validate`. This verb adds
//! the one question CI can only ask the deployment: whether `navigator.yaml`
//! agrees with the live row (code, host, `repository_url`, status). `--ci`
//! mints through GitHub Actions OIDC at `POST /auth/ci/document-token` and
//! reads the participation-scoped project list. Without the OIDC request URL
//! the door stays shut (exit 2) rather than falling back to a stored login.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::manifest::{self, Manifest};
use super::repository;
use crate::remote;

/// Run layout validation, then the OIDC live-status door when `--ci` is set.
pub async fn run(dir: &Path, ci: bool, host: Option<&str>) -> ExitCode {
    let status = repository::validate_gate(dir, None);
    if status != ExitCode::SUCCESS {
        return status;
    }
    if !ci {
        return ExitCode::SUCCESS;
    }
    if std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_err() {
        eprintln!(
            "navigator: --ci exchanges GitHub Actions OIDC at POST /auth/ci/document-token; \
             ACTIONS_ID_TOKEN_REQUEST_URL is unset — this is not a GitHub Actions job"
        );
        return ExitCode::from(2);
    }
    let Some(host) = host.map(str::trim).filter(|host| !host.is_empty()) else {
        eprintln!("navigator: --ci requires --host");
        return ExitCode::from(2);
    };
    match check_live(dir, host).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Deserialize)]
struct LiveMatter {
    code: String,
    status: String,
    #[serde(default)]
    repository_url: Option<String>,
}

async fn check_live(dir: &Path, host: &str) -> Result<()> {
    let contents = std::fs::read_to_string(dir.join(manifest::FILE))
        .with_context(|| format!("read {}", dir.join(manifest::FILE).display()))?;
    let parsed = manifest::parse(&contents).map_err(|error| anyhow!("{error}"))?;
    let project = parsed
        .project
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| anyhow!("navigator.yaml has no project:"))?;
    if let Some(manifest_host) = parsed
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        if !hosts_agree(manifest_host, host) {
            return Err(anyhow!(
                "manifest host `{manifest_host}` does not match --host `{host}`"
            ));
        }
    }
    let reason = rowless_reason(&parsed);
    let (base, token) = remote::resolve_ci_document(host).await?;
    let response = reqwest::Client::new()
        .get(format!("{base}/app/api/projects"))
        .bearer_auth(&token)
        .send()
        .await
        .context("GET /app/api/projects")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "GET /app/api/projects failed: {status}: {}",
            body.lines().next().unwrap_or(&body)
        ));
    }
    let matters: Vec<LiveMatter> =
        serde_json::from_str(&body).context("parse GET /app/api/projects")?;
    let live = matters.iter().find(|matter| matter.code == project);
    disagreement(project, reason, live, this_repository_url().as_deref())
}

fn rowless_reason(manifest: &Manifest) -> Option<&str> {
    match &manifest.no_live_row {
        Some(serde_yaml::Value::String(reason)) => {
            let trimmed = reason.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        }
        _ => None,
    }
}

fn hosts_agree(manifest_host: &str, flag: &str) -> bool {
    let base = crate::credentials::base_url(flag);
    if base.starts_with("http://") {
        return true;
    }
    let flag_host = base
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(flag);
    manifest_host.eq_ignore_ascii_case(flag_host)
}

fn this_repository_url() -> Option<String> {
    let name = std::env::var("GITHUB_REPOSITORY").ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let server = std::env::var("GITHUB_SERVER_URL").unwrap_or_else(|_| "https://github.com".into());
    Some(format!("{}/{name}", server.trim_end_matches('/')))
}

fn disagreement(
    project: &str,
    reason: Option<&str>,
    live: Option<&LiveMatter>,
    this_repo: Option<&str>,
) -> Result<()> {
    match (live, reason) {
        (None, Some(_)) => Ok(()),
        (None, None) => Err(anyhow!(
            "no live row for project `{project}`; declare no_live_row: <reason> if that is intended"
        )),
        (Some(row), Some(_)) if row.status == "open" => Err(anyhow!(
            "live status is `open` but navigator.yaml declares no_live_row"
        )),
        (Some(row), None) if row.status == "closed" || row.status == "archived" => Err(anyhow!(
            "live status is `{}` but navigator.yaml does not declare no_live_row",
            row.status
        )),
        (Some(row), _) => {
            if let (Some(live_url), Some(here)) = (row.repository_url.as_deref(), this_repo) {
                if !repository_urls_agree(live_url, here) {
                    return Err(anyhow!(
                        "repository_url is `{live_url}` but this job is `{here}`"
                    ));
                }
            }
            Ok(())
        }
    }
}

fn repository_urls_agree(live: &str, here: &str) -> bool {
    fn normalize(url: &str) -> String {
        url.trim()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_ascii_lowercase()
    }
    normalize(live) == normalize(here)
}

#[cfg(test)]
mod tests {
    use super::{disagreement, LiveMatter};
    use std::sync::LazyLock;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn row(status: &str, url: Option<&str>) -> LiveMatter {
        LiveMatter {
            code: "acme".into(),
            status: status.into(),
            repository_url: url.map(str::to_string),
        }
    }

    #[test]
    fn missing_row_without_reason_fails() {
        let error = disagreement("acme", None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no live row"), "{error}");
        assert!(error.contains("no_live_row"), "{error}");
    }

    #[test]
    fn missing_row_with_reason_passes() {
        disagreement("acme", Some("the matter closed"), None, None).unwrap();
    }

    #[test]
    fn open_row_with_reason_fails() {
        let error = disagreement("acme", Some("closed"), Some(&row("open", None)), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("live status is `open`"), "{error}");
        assert!(error.contains("no_live_row"), "{error}");
    }

    #[test]
    fn closed_row_without_reason_fails() {
        let error = disagreement("acme", None, Some(&row("closed", None)), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("live status is `closed`"), "{error}");
        assert!(error.contains("no_live_row"), "{error}");
    }

    #[test]
    fn archived_row_without_reason_fails() {
        let error = disagreement("acme", None, Some(&row("archived", None)), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("live status is `archived`"), "{error}");
    }

    #[test]
    fn closed_row_with_reason_passes() {
        disagreement(
            "acme",
            Some("the matter closed"),
            Some(&row("closed", None)),
            None,
        )
        .unwrap();
    }

    #[test]
    fn open_row_without_reason_passes() {
        disagreement("acme", None, Some(&row("open", None)), None).unwrap();
    }

    #[test]
    fn repository_url_mismatch_fails() {
        let error = disagreement(
            "acme",
            None,
            Some(&row("open", Some("https://github.com/org/other"))),
            Some("https://github.com/org/acme"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("repository_url"), "{error}");
        assert!(error.contains("org/other"), "{error}");
    }

    #[test]
    fn repository_url_agrees_ignoring_git_suffix() {
        disagreement(
            "acme",
            None,
            Some(&row("open", Some("https://github.com/org/acme.git"))),
            Some("https://github.com/org/acme"),
        )
        .unwrap();
    }

    #[test]
    fn hosts_agree_on_loopback_even_when_the_manifest_names_staging() {
        assert!(super::hosts_agree(
            "staging.neonlaw.com",
            "http://127.0.0.1:9"
        ));
    }

    #[test]
    fn hosts_disagree_when_the_flag_is_a_different_https_host() {
        assert!(!super::hosts_agree(
            "staging.neonlaw.com",
            "https://other.example"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_open_row_passes_through_oidc() {
        let _lock = ENV_LOCK.lock().await;
        let github = MockServer::start().await;
        let navigator = MockServer::start().await;
        let navigator_uri = navigator.uri();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();

        let previous_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").ok();
        let previous_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok();
        let previous_repo = std::env::var("GITHUB_REPOSITORY").ok();
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_URL", github.uri());
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "github-oidc-request");
        std::env::set_var("GITHUB_REPOSITORY", "org/acme");

        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("audience", navigator_uri.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": "github-jwt"
            })))
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/ci/document-token"))
            .and(body_json(serde_json::json!({ "token": "github-jwt" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "navigator-ci-token",
                "exp": 1,
                "project_code": "acme"
            })))
            .mount(&navigator)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .and(header("authorization", "Bearer navigator-ci-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": "00000000-0000-0000-0000-000000000001",
                    "code": "acme",
                    "status": "open",
                    "repository_url": "https://github.com/org/acme"
                }])),
            )
            .mount(&navigator)
            .await;

        let result = super::check_live(dir.path(), &navigator_uri).await;

        restore_env(previous_url, previous_token, previous_repo);
        result.expect("open live row must pass");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_missing_row_fails_through_oidc() {
        let _lock = ENV_LOCK.lock().await;
        let github = MockServer::start().await;
        let navigator = MockServer::start().await;
        let navigator_uri = navigator.uri();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();

        let previous_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").ok();
        let previous_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok();
        let previous_repo = std::env::var("GITHUB_REPOSITORY").ok();
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_URL", github.uri());
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "github-oidc-request");
        std::env::remove_var("GITHUB_REPOSITORY");

        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("audience", navigator_uri.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": "github-jwt"
            })))
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/ci/document-token"))
            .and(body_json(serde_json::json!({ "token": "github-jwt" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "navigator-ci-token",
                "exp": 1,
                "project_code": "acme"
            })))
            .mount(&navigator)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&navigator)
            .await;

        let error = super::check_live(dir.path(), &navigator_uri)
            .await
            .expect_err("empty list must fail");
        restore_env(previous_url, previous_token, previous_repo);
        assert!(error.to_string().contains("no live row"), "{error:#}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_closed_row_fails_through_oidc() {
        let _lock = ENV_LOCK.lock().await;
        let github = MockServer::start().await;
        let navigator = MockServer::start().await;
        let navigator_uri = navigator.uri();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "host: staging.neonlaw.com\nproject: acme\n",
        )
        .unwrap();

        let previous_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").ok();
        let previous_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok();
        let previous_repo = std::env::var("GITHUB_REPOSITORY").ok();
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_URL", github.uri());
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "github-oidc-request");
        std::env::set_var("GITHUB_REPOSITORY", "org/acme");

        Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("audience", navigator_uri.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "value": "github-jwt"
            })))
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/ci/document-token"))
            .and(body_json(serde_json::json!({ "token": "github-jwt" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "token": "navigator-ci-token",
                "exp": 1,
                "project_code": "acme"
            })))
            .mount(&navigator)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .and(header("authorization", "Bearer navigator-ci-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": "00000000-0000-0000-0000-000000000001",
                    "code": "acme",
                    "status": "closed",
                    "repository_url": "https://github.com/org/acme"
                }])),
            )
            .mount(&navigator)
            .await;

        let error = super::check_live(dir.path(), &navigator_uri)
            .await
            .expect_err("closed live row must fail without no_live_row");
        restore_env(previous_url, previous_token, previous_repo);
        let message = error.to_string();
        assert!(message.contains("live status is `closed`"), "{message}");
        assert!(message.contains("no_live_row"), "{message}");
    }

    fn restore_env(
        previous_url: Option<String>,
        previous_token: Option<String>,
        previous_repo: Option<String>,
    ) {
        match previous_url {
            Some(value) => std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_URL", value),
            None => std::env::remove_var("ACTIONS_ID_TOKEN_REQUEST_URL"),
        }
        match previous_token {
            Some(value) => std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN", value),
            None => std::env::remove_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN"),
        }
        match previous_repo {
            Some(value) => std::env::set_var("GITHUB_REPOSITORY", value),
            None => std::env::remove_var("GITHUB_REPOSITORY"),
        }
    }
}
