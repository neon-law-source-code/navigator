//! The live-status half of `navigator project gate --ci`.
//!
//! Layout, origin, and every content rule run offline in the gate itself. This
//! module asks the one question only the deployment can answer: whether
//! `navigator.yaml` agrees with the live row (code, host, `repository_url`,
//! status). It mints through GitHub Actions OIDC at
//! `POST /auth/ci/document-token`, and never falls back to a stored login: the
//! door opens only where the server would honour it — a push or dispatch on
//! `refs/heads/main`, or a pull request merge ref — and says it skipped
//! everywhere else.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::manifest::{self, Manifest};
use crate::remote;

/// Open the OIDC live-status door for a Project repository.
///
/// The mint honors only a push or dispatch on `refs/heads/main`, so anywhere
/// else the door is shut at the server and asking would be a guaranteed 403.
/// That makes this its own condition rather than a flag a caller has to
/// remember: the gate runs the same way everywhere and the door opens where it
/// can open.
pub(crate) async fn live_status(dir: &Path) -> ExitCode {
    let Some(host) = manifest_host(dir) else {
        return ExitCode::SUCCESS;
    };
    if !can_mint_ci_session() || std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_err() {
        println!("Skipped the live row: this event/ref cannot mint a CI session");
        return ExitCode::SUCCESS;
    }
    match check_live(dir, &host).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(1)
        }
    }
}

/// The deployment `navigator.yaml` names, which is the only host this gate
/// speaks to. A repository that names none has no live row to check.
fn manifest_host(dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(dir.join(manifest::FILE)).ok()?;
    let parsed = manifest::parse(&contents).ok()?;
    parsed
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

fn can_mint_ci_session() -> bool {
    let git_ref = std::env::var("GITHUB_REF").unwrap_or_default();
    let event = std::env::var("GITHUB_EVENT_NAME").unwrap_or_default();
    can_mint_ci_session_for(&git_ref, &event)
}

fn can_mint_ci_session_for(git_ref: &str, event: &str) -> bool {
    (git_ref == "refs/heads/main" && matches!(event, "push" | "workflow_dispatch"))
        || (event == "pull_request" && is_pull_request_merge_ref(git_ref))
}

fn is_pull_request_merge_ref(git_ref: &str) -> bool {
    let Some(number) = git_ref
        .strip_prefix("refs/pull/")
        .and_then(|reference| reference.strip_suffix("/merge"))
    else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number.parse::<u64>().is_ok_and(|number| number > 0)
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
    use super::{can_mint_ci_session_for, disagreement, is_pull_request_merge_ref, LiveMatter};
    use axum::body::Body;
    use axum::http::{header, Request};
    use portal::github_oidc::{GitHubActionsClaims, GitHubOidc};
    use std::sync::LazyLock;
    use tower::ServiceExt;
    use wiremock::matchers::{body_json, header as header_matcher, method, path, query_param};
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
    fn only_a_pull_request_merge_ref_can_open_the_pr_live_check() {
        assert!(is_pull_request_merge_ref("refs/pull/7/merge"));
        assert!(!is_pull_request_merge_ref("refs/pull/0/merge"));
        assert!(!is_pull_request_merge_ref("refs/pull/7/head"));
        assert!(!is_pull_request_merge_ref("refs/heads/main"));
    }

    #[test]
    fn live_check_events_pair_with_their_intended_refs() {
        assert!(can_mint_ci_session_for("refs/pull/7/merge", "pull_request"));
        assert!(!can_mint_ci_session_for(
            "refs/pull/7/merge",
            "pull_request_target"
        ));
        assert!(can_mint_ci_session_for(
            "refs/heads/main",
            "workflow_dispatch"
        ));
        assert!(!can_mint_ci_session_for("refs/heads/topic", "push"));
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
            .and(header_matcher("authorization", "Bearer navigator-ci-token"))
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
            .and(header_matcher("authorization", "Bearer navigator-ci-token"))
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

    /// The regression this module exists to catch: a document-scoped CI
    /// token's real `GET /app/api/projects` response, produced by the actual
    /// `portal::router` handler rather than hand-written JSON, must
    /// deserialize into [`LiveMatter`]. The other tests above mock the
    /// endpoint with a JSON literal that already includes every field this
    /// struct wants, which is exactly why the server's `DocumentProjectLookup`
    /// narrowing (introduced in 26.9.21) could drop `status` and
    /// `repository_url` for two releases without any test here noticing —
    /// see ENG-842. This test fails the same way `ci / verify` does the
    /// moment the two shapes diverge again.
    #[tokio::test]
    async fn document_scoped_projects_response_deserializes_into_live_matter() {
        let surreal = store::test_support::mem_surreal().await;
        let entity_id = store::test_support::seed_entity(&surreal).await;
        let project_id = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: "acme".into(),
                name: "Acme".into(),
                status: "open".into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id;
        store::projects::set_repository_url(
            &surreal,
            project_id,
            Some("https://github.com/org/acme"),
        )
        .await
        .unwrap();
        let lawyer = store::persons::create(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Synthetic Lawyer",
                "ci-lawyer@example.com",
                store::persons::Role::Lawyer,
            ),
        )
        .await
        .unwrap();
        store::projects::designate_dri_in_surreal(
            &surreal,
            project_id,
            lawyer.id,
            store::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();

        let mut state = portal::test_support::app_state(surreal).await;
        state.canonical_host = portal::CanonicalHost::new(Some("staging.neonlaw.com".into()));
        state.github_oidc = GitHubOidc::fixed(GitHubActionsClaims {
            sub: "repo:org/acme:ref:refs/heads/main".into(),
            repository: "org/acme".into(),
            repository_owner: "org".into(),
            git_ref: "refs/heads/main".into(),
            event_name: "push".into(),
            jti: "jti-document-acme".into(),
            exp: 4_000_000_000,
            ..Default::default()
        });
        let app = portal::router(state);

        let mint_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/ci/document-token")
                    .header(header::HOST, "staging.neonlaw.com")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"github-jwt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mint_response.status(), axum::http::StatusCode::OK);
        let mint_body = axum::body::to_bytes(mint_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let minted: serde_json::Value = serde_json::from_slice(&mint_body).unwrap();
        let token = minted["token"].as_str().unwrap();

        let projects_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/app/api/projects")
                    .header(header::HOST, "staging.neonlaw.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(projects_response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(projects_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        let matters: Vec<LiveMatter> =
            serde_json::from_str(&body).expect("the live door's own response must parse");
        let acme = matters
            .iter()
            .find(|matter| matter.code == "acme")
            .expect("the seeded project is in its own lookup");
        assert_eq!(acme.status, "open");
        assert_eq!(
            acme.repository_url.as_deref(),
            Some("https://github.com/org/acme")
        );
    }
}
