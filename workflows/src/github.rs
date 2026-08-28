//! `github_issue__*` step dispatch — open a GitHub issue from a notation.
//!
//! Mirrors [`crate::attest`]: the caller threads a [`GithubIssuePayload`]
//! through the signal `value`, and the worker (the `workflows-service`
//! `NotationService` in prod, the in-process [`crate::DispatchingRuntime`]
//! in dev/tests) opens the issue when a transition lands on a
//! `github_issue__*` state.
//!
//! ## GitHub is isolated behind a trait
//!
//! The HTTP call lives *only* behind the [`IssueOpener`] trait, the same
//! way the chain lives only behind [`crate::attest::Attestor`]. The generic
//! workflow layer knows the provider-neutral `github_issue__` prefix.
//! [`NullIssueOpener`] is the no-token default — it opens *nothing* and
//! returns `None`, so a workflow can never claim an issue that does not
//! exist. [`RestIssueOpener`] is the real one.
//!
//! ## Rust-first, no `gh`
//!
//! [`RestIssueOpener`] calls the GitHub REST API directly with `reqwest`,
//! which the workspace already depends on. It shells out to nothing — the
//! `gh` CLI is not a dependency of the runtime, is not installed in the
//! `workflows-service` image, and would put an unpinned external binary
//! inside a durable step. One `POST /repos/{owner}/{repo}/issues` is the
//! whole surface.
//!
//! ## Telemetry carries identifiers, never content
//!
//! An issue title and body are authored text. Spans and logs here record
//! the repository, the resulting issue number, and the HTTP status — never
//! the title or body, matching the trust boundary the rest of the
//! workspace's telemetry observes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Env var holding the GitHub token the [`RestIssueOpener`] authenticates
/// with. Checked before [`GITHUB_TOKEN_ENV`] so a workspace-specific token
/// can override an ambient one.
pub const NAVIGATOR_GITHUB_TOKEN_ENV: &str = "NAVIGATOR_GITHUB_TOKEN";

/// The conventional GitHub token env var, used when
/// [`NAVIGATOR_GITHUB_TOKEN_ENV`] is unset.
pub const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";

/// Env var overriding the API base, for GitHub Enterprise or a test
/// double. Defaults to [`DEFAULT_API_BASE`].
///
/// Naming GitHub Enterprise is a **feature, not stale narration.** Navigator
/// runs on github.com; this override is how somebody running their own
/// instance points it at their own tenant. See the same note on
/// `webapp::source_repository::GITHUB_API_BASE_ENV`.
pub const GITHUB_API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";

/// Env var naming the default `owner/repo` when a payload omits one.
pub const GITHUB_REPO_ENV: &str = "NAVIGATOR_GITHUB_REPO";

/// Public GitHub's REST API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// The REST API version this client pins, sent as `X-GitHub-Api-Version`.
/// GitHub dates its breaking changes; pinning means a future default does
/// not silently change what a durable step does on replay.
pub const API_VERSION: &str = "2022-11-28";

/// What issue to open, threaded as the JSON `value` of the signal that
/// lands on a `github_issue__*` state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubIssuePayload {
    /// `owner/repo` to open the issue in. Falls back to
    /// [`GITHUB_REPO_ENV`] when absent.
    #[serde(default)]
    pub repo: Option<String>,
    /// The issue title.
    pub title: String,
    /// The issue body — the rendered notation.
    pub body: String,
    /// Labels to apply, if any.
    #[serde(default)]
    pub labels: Vec<String>,
}

/// One issue to open: where it goes and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRequest {
    pub owner: String,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

impl IssueRequest {
    /// Build a request from a payload, resolving the repository from the
    /// payload or [`GITHUB_REPO_ENV`].
    ///
    /// # Errors
    ///
    /// Returns [`IssueError::MissingRepo`] when neither names a repo, and
    /// [`IssueError::MalformedRepo`] when the value is not `owner/repo`.
    pub fn from_payload(
        payload: &GithubIssuePayload,
        default_repo: Option<&str>,
    ) -> Result<Self, IssueError> {
        let slug = payload
            .repo
            .as_deref()
            .or(default_repo)
            .ok_or(IssueError::MissingRepo)?;
        let (owner, repo) = slug
            .split_once('/')
            .filter(|(owner, repo)| !owner.is_empty() && !repo.is_empty())
            .ok_or_else(|| IssueError::MalformedRepo(slug.to_string()))?;
        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            title: payload.title.clone(),
            body: payload.body.clone(),
            labels: payload.labels.clone(),
        })
    }

    /// `owner/repo`, for diagnostics. Safe to log — a repository slug is an
    /// identifier, not content.
    #[must_use]
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// An issue that now exists on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenedIssue {
    /// The issue number within its repository.
    pub number: u64,
    /// Canonical `https://github.com/owner/repo/issues/N` URL.
    pub html_url: String,
}

/// Failure opening an issue.
#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    /// Neither the payload nor the environment named a repository.
    #[error("no repository: set `repo` on the payload or the `{GITHUB_REPO_ENV}` env var")]
    MissingRepo,
    /// The repository was not `owner/repo`.
    #[error("malformed repository `{0}`: expected `owner/repo`")]
    MalformedRepo(String),
    /// The request never completed (DNS, TLS, timeout).
    #[error("github request to {slug} failed: {source}")]
    Transport {
        slug: String,
        #[source]
        source: reqwest::Error,
    },
    /// GitHub answered with a non-success status.
    #[error("github returned {status} opening an issue in {slug}: {message}")]
    Api {
        slug: String,
        status: u16,
        /// GitHub's own `message` field, which describes the *request*
        /// (bad credentials, not found, validation failed) and carries no
        /// issue content.
        message: String,
    },
    /// The success response did not decode.
    #[error("decode github response for {slug}: {source}")]
    Decode {
        slug: String,
        #[source]
        source: reqwest::Error,
    },
}

/// Opens GitHub issues. The seam the `github_issue__*` step dispatches
/// through.
#[async_trait]
pub trait IssueOpener: Send + Sync {
    /// Open `request`, returning the created issue, or `None` when this
    /// opener is not configured to reach GitHub.
    ///
    /// # Errors
    ///
    /// Returns [`IssueError`] when a configured opener could not create
    /// the issue.
    async fn open_issue(&self, request: &IssueRequest) -> Result<Option<OpenedIssue>, IssueError>;
}

/// The no-token default: opens nothing and returns `None`.
///
/// Selected by [`issue_opener_from_env`] whenever no token is set, so a
/// local KIND run or a test never reaches out to github.com. The workflow
/// still advances; it simply records that no issue was opened, the way
/// [`crate::attest::NullAttestor`] leaves an attestation row `pending`.
pub struct NullIssueOpener;

#[async_trait]
impl IssueOpener for NullIssueOpener {
    async fn open_issue(&self, request: &IssueRequest) -> Result<Option<OpenedIssue>, IssueError> {
        tracing::info!(
            repo = %request.slug(),
            "no github token configured; skipping issue creation"
        );
        Ok(None)
    }
}

/// Opens issues against the GitHub REST API with `reqwest`.
pub struct RestIssueOpener {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl RestIssueOpener {
    /// Build an opener for `base_url` (no trailing slash) authenticating
    /// with `token`.
    ///
    /// # Panics
    ///
    /// Panics only if the process cannot build a TLS-capable HTTP client.
    #[must_use]
    pub fn new(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build reqwest client");
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }
}

/// GitHub's error envelope. Only `message` is read — it describes the
/// request, never the issue we tried to create.
#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

/// The subset of GitHub's created-issue response the step records.
#[derive(Debug, Deserialize)]
struct CreatedIssue {
    number: u64,
    html_url: String,
}

#[async_trait]
impl IssueOpener for RestIssueOpener {
    async fn open_issue(&self, request: &IssueRequest) -> Result<Option<OpenedIssue>, IssueError> {
        let slug = request.slug();
        let url = format!(
            "{}/repos/{}/{}/issues",
            self.base_url, request.owner, request.repo
        );
        let body = serde_json::json!({
            "title": request.title,
            "body": request.body,
            "labels": request.labels,
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("User-Agent", "neon-law-navigator")
            .json(&body)
            .send()
            .await
            .map_err(|source| IssueError::Transport {
                slug: slug.clone(),
                source,
            })?;

        let status = response.status();
        if !status.is_success() {
            let message = response
                .json::<ApiError>()
                .await
                .map(|e| e.message)
                .unwrap_or_default();
            tracing::warn!(repo = %slug, status = status.as_u16(), "github issue creation failed");
            return Err(IssueError::Api {
                slug,
                status: status.as_u16(),
                message,
            });
        }

        let created =
            response
                .json::<CreatedIssue>()
                .await
                .map_err(|source| IssueError::Decode {
                    slug: slug.clone(),
                    source,
                })?;
        tracing::info!(repo = %slug, issue = created.number, "opened github issue");
        Ok(Some(OpenedIssue {
            number: created.number,
            html_url: created.html_url,
        }))
    }
}

/// The default repository from [`GITHUB_REPO_ENV`], if set and non-empty.
#[must_use]
pub fn default_repo_from_env() -> Option<String> {
    std::env::var(GITHUB_REPO_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn api_base_from_lookup<F>(get: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    get(GITHUB_API_BASE_ENV)
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

/// Select the opener from the environment: a [`RestIssueOpener`] when a
/// token is set, otherwise the [`NullIssueOpener`].
///
/// Absent configuration is a working default rather than an error, so a
/// KIND run or a test never needs GitHub credentials to walk a workflow
/// that happens to include the step.
#[must_use]
pub fn issue_opener_from_env() -> Arc<dyn IssueOpener> {
    let token = std::env::var(NAVIGATOR_GITHUB_TOKEN_ENV)
        .or_else(|_| std::env::var(GITHUB_TOKEN_ENV))
        .ok()
        .filter(|token| !token.trim().is_empty());
    match token {
        Some(token) => {
            let base = api_base_from_lookup(|key| std::env::var(key).ok());
            Arc::new(RestIssueOpener::new(token, base))
        }
        None => Arc::new(NullIssueOpener),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        api_base_from_lookup, GithubIssuePayload, IssueError, IssueOpener, IssueRequest,
        NullIssueOpener, OpenedIssue, RestIssueOpener, DEFAULT_API_BASE, GITHUB_API_BASE_ENV,
    };
    use wiremock::matchers::{body_json_string, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn payload() -> GithubIssuePayload {
        GithubIssuePayload {
            repo: Some("neon-law-source-code/navigator".to_string()),
            title: "Add the github notation shelf".to_string(),
            body: "## Observed problem\n\nNo intake for engineering work.\n".to_string(),
            labels: vec!["autobuild".to_string()],
        }
    }

    #[test]
    fn a_payload_repo_wins_over_the_environment_default() {
        let request = IssueRequest::from_payload(&payload(), Some("other/repo")).unwrap();
        assert_eq!(request.owner, "neon-law-source-code");
        assert_eq!(request.repo, "navigator");
        assert_eq!(request.slug(), "neon-law-source-code/navigator");
    }

    #[test]
    fn the_environment_default_fills_in_when_the_payload_omits_one() {
        let bare = GithubIssuePayload {
            repo: None,
            ..payload()
        };
        let request = IssueRequest::from_payload(&bare, Some("neon-law-source-code/navigator"))
            .expect("env default should resolve");
        assert_eq!(request.slug(), "neon-law-source-code/navigator");
    }

    #[test]
    fn a_missing_or_malformed_repo_is_a_named_error() {
        let bare = GithubIssuePayload {
            repo: None,
            ..payload()
        };
        assert!(matches!(
            IssueRequest::from_payload(&bare, None),
            Err(IssueError::MissingRepo)
        ));

        for bad in ["navigator", "/navigator", "owner/", ""] {
            let malformed = GithubIssuePayload {
                repo: Some(bad.to_string()),
                ..payload()
            };
            assert!(
                matches!(
                    IssueRequest::from_payload(&malformed, None),
                    Err(IssueError::MissingRepo | IssueError::MalformedRepo(_))
                ),
                "`{bad}` should not resolve to a repository",
            );
        }
    }

    #[test]
    fn the_issue_opener_uses_the_shared_github_api_base() {
        let base = api_base_from_lookup(|key| {
            (key == GITHUB_API_BASE_ENV).then(|| "https://github.example/api/v3".to_string())
        });
        assert_eq!(base, "https://github.example/api/v3");
        assert_eq!(api_base_from_lookup(|_| None), DEFAULT_API_BASE);
    }

    /// The no-token default must reach nothing and claim nothing. If this
    /// ever returned an `OpenedIssue`, a workflow would record an issue
    /// number that does not exist.
    #[tokio::test]
    async fn the_null_opener_opens_nothing() {
        let request = IssueRequest::from_payload(&payload(), None).unwrap();
        assert_eq!(NullIssueOpener.open_issue(&request).await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_rest_opener_posts_the_documented_request_and_reads_the_issue_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/neon-law-source-code/navigator/issues"))
            .and(header("authorization", "Bearer test-token"))
            .and(header("accept", "application/vnd.github+json"))
            .and(header("x-github-api-version", super::API_VERSION))
            .and(body_json_string(
                serde_json::json!({
                    "title": "Add the github notation shelf",
                    "body": "## Observed problem\n\nNo intake for engineering work.\n",
                    "labels": ["autobuild"],
                })
                .to_string(),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 621,
                "html_url": "https://github.com/neon-law-source-code/navigator/issues/621",
            })))
            .mount(&server)
            .await;

        let opener = RestIssueOpener::new("test-token", server.uri());
        let request = IssueRequest::from_payload(&payload(), None).unwrap();
        let created = opener.open_issue(&request).await.unwrap();
        assert_eq!(
            created,
            Some(OpenedIssue {
                number: 621,
                html_url: "https://github.com/neon-law-source-code/navigator/issues/621"
                    .to_string(),
            })
        );
    }

    /// A non-success status surfaces GitHub's own request-level message and
    /// never invents an issue.
    #[tokio::test]
    async fn a_rejected_request_is_an_api_error_carrying_the_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Validation Failed",
            })))
            .mount(&server)
            .await;

        let opener = RestIssueOpener::new("test-token", server.uri());
        let request = IssueRequest::from_payload(&payload(), None).unwrap();
        match opener.open_issue(&request).await {
            Err(IssueError::Api {
                slug,
                status,
                message,
            }) => {
                assert_eq!(slug, "neon-law-source-code/navigator");
                assert_eq!(status, 422);
                assert_eq!(message, "Validation Failed");
            }
            other => panic!("expected an API error, got {other:?}"),
        }
    }

    /// A trailing slash on the configured base must not produce a `//` path
    /// that GitHub 404s on.
    #[tokio::test]
    async fn a_trailing_slash_on_the_api_base_is_normalized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/neon-law-source-code/navigator/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 1,
                "html_url": "https://example.invalid/1",
            })))
            .mount(&server)
            .await;

        let opener = RestIssueOpener::new("test-token", format!("{}/", server.uri()));
        let request = IssueRequest::from_payload(&payload(), None).unwrap();
        assert!(opener.open_issue(&request).await.unwrap().is_some());
    }
}
