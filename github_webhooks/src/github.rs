//! Typed GitHub App client boundary for `DevX` automation.
//!
//! The concrete GitHub App client lives behind this trait so durable
//! workflows depend on typed operations rather than HTTP details.  The null
//! implementation fails closed: local and unconfigured deployments can never
//! claim to have posted a GitHub comment.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{header, StatusCode, Url};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

const GITHUB_API_BASE: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const ACCEPT: &str = "application/vnd.github+json";
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_mins(5);
const USER_AGENT: &str = concat!("neon-law-navigator/", env!("CARGO_PKG_VERSION"));
const FAILED_LOG_TAIL_LINES: usize = 80;

pub const GITHUB_APP_ID_ENV: &str = "NAVIGATOR_GITHUB_APP_ID";
pub const GITHUB_APP_PRIVATE_KEY_ENV: &str = "NAVIGATOR_GITHUB_APP_PRIVATE_KEY";
pub const GITHUB_INSTALLATION_ID_ENV: &str = "NAVIGATOR_GITHUB_INSTALLATION_ID";
pub const GITHUB_API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";

/// A repository selected from a GitHub App installation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct RepositoryRef {
    /// GitHub organization or user that owns the repository.
    pub owner: String,
    /// Repository name, without the owner prefix.
    pub name: String,
}

/// The source content required to open a pull request.
///
/// This does not implement `Debug`: callers must not accidentally emit a
/// draft's title or body into telemetry.
#[derive(Clone, PartialEq, Eq)]
pub struct PullRequestDraft {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

/// Pull-request metadata used by durable workflow decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub merge_state_status: String,
}

/// One unresolved pull-request review thread.
///
/// Comments are source content, so this type deliberately does not implement
/// `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ReviewThread {
    pub id: String,
    pub comments: Vec<ReviewComment>,
}

/// A review comment returned with its parent thread.
///
/// The body is source content and must never enter telemetry, so this type
/// deliberately does not implement `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ReviewComment {
    pub database_id: Option<u64>,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

/// A check run associated with a particular Git ref.
///
/// This intentionally does not implement `Debug`: a GitHub check name or
/// conclusion is external content and must not enter workflow telemetry by
/// accident.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    pub conclusion: Option<String>,
}

/// The bounded tail from the failed step of a GitHub Actions workflow run.
///
/// This is source content. It intentionally does not implement `Debug` or
/// `Display`, so callers must explicitly choose the tightly scoped runner
/// handoff rather than accidentally emitting it to telemetry.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkflowRunLogTail {
    tail: String,
}

impl WorkflowRunLogTail {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.tail
    }
}

/// An authenticated Git HTTPS URL for a single repository operation.
///
/// This intentionally implements neither `Debug` nor `Display`: it embeds a
/// short-lived installation token and must never be logged.
pub struct CloneUrl(String);

impl CloneUrl {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A short-lived GitHub App installation token.
///
/// This intentionally implements neither `Debug` nor `Display`: a runner
/// may use the token to call the verified-commit API, but must never emit it.
#[derive(Clone)]
pub struct InstallationToken(String);

impl InstallationToken {
    /// Accept a non-empty installation token supplied through a secret-only
    /// runner handoff.
    pub fn from_secret(value: String) -> Result<Self, GitHubClientError> {
        if value.is_empty() {
            Err(GitHubClientError::InvalidConfiguration)
        } else {
            Ok(Self(value))
        }
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// One file written through GitHub's verified-commit mutation.
///
/// File contents are source content, so this type deliberately does not
/// implement `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct FileAddition {
    pub path: String,
    pub contents: Vec<u8>,
}

/// A GitHub-signed commit requested by an isolated worker.
///
/// The mutation's expected head OID makes publication conditional, preventing
/// a stale runner from silently overwriting a branch that has moved.
/// Commit messages and file contents are source content and deliberately do
/// not implement `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedCommit {
    pub branch: String,
    pub expected_head_oid: String,
    pub headline: String,
    pub body: Option<String>,
    pub additions: Vec<FileAddition>,
    pub deletions: Vec<String>,
}

/// Safe metadata returned after GitHub creates a verified commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedCommit {
    pub oid: String,
}

/// An issue retrieved from GitHub.
///
/// The title and body are source content. They deliberately do not implement
/// `Debug` so callers cannot accidentally place them in telemetry.
#[derive(Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
}

/// One source-content-bearing comment from an issue discussion.
///
/// Triage must read the whole thread, not just an issue's opening body. The
/// comment body deliberately does not implement `Debug`, so it cannot leak
/// into workflow telemetry or durable correlation identifiers.
#[derive(Clone, PartialEq, Eq)]
pub struct IssueComment {
    pub body: String,
}

/// Errors exposed by the `DevX` GitHub boundary.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GitHubClientError {
    /// A workflow attempted an external GitHub operation without a configured
    /// GitHub App client.
    #[error("no GitHub App client is configured")]
    Unconfigured,
    #[error("GitHub App configuration is invalid")]
    InvalidConfiguration,
    #[error("GitHub authentication failed with status {status}")]
    Authentication { status: u16 },
    #[error("GitHub resource was not found")]
    NotFound,
    #[error("GitHub rejected the request as invalid")]
    Validation,
    #[error("GitHub request failed with status {status}")]
    Api { status: u16 },
    #[error("GitHub request could not be completed")]
    Transport,
    #[error("GitHub returned an invalid response")]
    InvalidResponse,
    #[error("GitHub GraphQL request failed")]
    GraphQl,
}

/// Explicit configuration for [`GitHubAppClient`].
#[derive(Clone)]
pub struct GitHubAppClientConfig {
    pub api_base: String,
    pub app_id: String,
    pub private_key_pem: String,
    pub installation_id: Option<u64>,
}

#[derive(Serialize)]
struct AppClaims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct PullRequestResponse {
    number: u64,
}

#[derive(Deserialize)]
struct CreateCommitOnBranchData {
    #[serde(rename = "createCommitOnBranch")]
    create_commit_on_branch: Option<CreateCommitOnBranchPayload>,
}

#[derive(Deserialize)]
struct CreateCommitOnBranchPayload {
    commit: Option<CreatedCommitResponse>,
}

#[derive(Deserialize)]
struct CreatedCommitResponse {
    oid: String,
}

#[derive(Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRunResponse>,
}

#[derive(Deserialize)]
struct CheckRunResponse {
    id: u64,
    name: String,
    conclusion: Option<String>,
}

impl From<CheckRunResponse> for CheckRun {
    fn from(response: CheckRunResponse) -> Self {
        Self {
            id: response.id,
            name: response.name,
            conclusion: response.conclusion,
        }
    }
}

#[derive(Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct PullRequestQueryData {
    repository: Option<PullRequestRepository>,
}

#[derive(Deserialize)]
struct PullRequestRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<PullRequestGraphQl>,
}

#[derive(Deserialize)]
struct PullRequestGraphQl {
    number: u64,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: String,
}

#[derive(Deserialize)]
struct ReviewThreadsQueryData {
    repository: Option<ReviewThreadsRepository>,
}

#[derive(Deserialize)]
struct ReviewThreadsRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<ReviewThreadsPullRequest>,
}

#[derive(Deserialize)]
struct ReviewThreadsPullRequest {
    #[serde(rename = "reviewThreads")]
    review_threads: ReviewThreadsConnection,
}

#[derive(Deserialize)]
struct ReviewThreadsConnection {
    nodes: Vec<ReviewThreadResponse>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ReviewThreadResponse {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    comments: ReviewCommentsConnection,
}

#[derive(Deserialize)]
struct ReviewCommentsConnection {
    nodes: Vec<ReviewCommentResponse>,
}

#[derive(Deserialize)]
struct ReviewCommentResponse {
    #[serde(rename = "databaseId")]
    database_id: Option<u64>,
    body: String,
    path: Option<String>,
    line: Option<u64>,
}

impl From<ReviewCommentResponse> for ReviewComment {
    fn from(response: ReviewCommentResponse) -> Self {
        Self {
            database_id: response.database_id,
            body: response.body,
            path: response.path,
            line: response.line,
        }
    }
}

#[derive(Deserialize)]
struct ResolveReviewThreadData {
    #[serde(rename = "resolveReviewThread")]
    resolve_review_thread: Option<ResolveReviewThreadPayload>,
}

#[derive(Deserialize)]
struct ResolveReviewThreadPayload {
    thread: Option<ResolvedReviewThread>,
}

#[derive(Deserialize)]
struct ResolvedReviewThread {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

#[derive(Deserialize)]
struct IssueResponse {
    number: u64,
    title: String,
    body: Option<String>,
}

#[derive(Deserialize)]
struct IssueCommentsQueryData {
    repository: Option<IssueCommentsRepository>,
}

#[derive(Deserialize)]
struct IssueCommentsRepository {
    issue: Option<IssueCommentsIssue>,
}

#[derive(Deserialize)]
struct IssueCommentsIssue {
    comments: IssueCommentsConnection,
}

#[derive(Deserialize)]
struct IssueCommentsConnection {
    nodes: Vec<IssueCommentResponse>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct IssueCommentResponse {
    body: String,
}

impl From<IssueCommentResponse> for IssueComment {
    fn from(response: IssueCommentResponse) -> Self {
        Self {
            body: response.body,
        }
    }
}

impl From<IssueResponse> for Issue {
    fn from(response: IssueResponse) -> Self {
        Self {
            number: response.number,
            title: response.title,
            body: response.body,
        }
    }
}

struct CachedToken {
    value: InstallationToken,
    expires_at: DateTime<Utc>,
}

impl CachedToken {
    fn is_fresh(&self) -> bool {
        (self.expires_at - Utc::now())
            .to_std()
            .is_ok_and(|remaining| remaining > TOKEN_REFRESH_MARGIN)
    }
}

/// A GitHub App authentication client whose credentials stay process-local.
pub struct GitHubAppClient {
    client: reqwest::Client,
    api_base: String,
    app_id: String,
    encoding_key: EncodingKey,
    pinned_installation_id: Option<u64>,
    installations: Mutex<HashMap<RepositoryRef, u64>>,
    tokens: Mutex<HashMap<RepositoryRef, CachedToken>>,
}

/// GitHub's verified-commit API authenticated only with an installation token.
///
/// This is the client passed to an ephemeral runner. It holds no App private
/// key and exposes no shell-based commit operation.
pub struct InstallationTokenClient {
    client: reqwest::Client,
    api_base: String,
    token: InstallationToken,
}

impl std::fmt::Debug for InstallationTokenClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallationTokenClient")
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

impl InstallationTokenClient {
    /// Build the token-only client used by the short-lived runner Job.
    pub fn new(api_base: &str, token: InstallationToken) -> Result<Self, GitHubClientError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::ACCEPT, header::HeaderValue::from_static(ACCEPT));
        headers.insert(
            header::HeaderName::from_static("x-github-api-version"),
            header::HeaderValue::from_static(API_VERSION),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        Ok(Self {
            client,
            api_base: api_base.trim_end_matches('/').to_owned(),
            token,
        })
    }
}

impl std::fmt::Debug for GitHubAppClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubAppClient")
            .field("api_base", &self.api_base)
            .field("app_id", &self.app_id)
            .field("pinned_installation_id", &self.pinned_installation_id)
            .finish_non_exhaustive()
    }
}

impl GitHubAppClient {
    pub fn new(config: GitHubAppClientConfig) -> Result<Self, GitHubClientError> {
        let encoding_key = EncodingKey::from_rsa_pem(config.private_key_pem.as_bytes())
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        let mut headers = header::HeaderMap::new();
        headers.insert(header::ACCEPT, header::HeaderValue::from_static(ACCEPT));
        headers.insert(
            header::HeaderName::from_static("x-github-api-version"),
            header::HeaderValue::from_static(API_VERSION),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        Ok(Self {
            client,
            api_base: config.api_base.trim_end_matches('/').to_string(),
            app_id: config.app_id,
            encoding_key,
            pinned_installation_id: config.installation_id,
            installations: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    pub fn from_env() -> Result<Self, GitHubClientError> {
        let required = |key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or(GitHubClientError::InvalidConfiguration)
        };
        let installation_id = std::env::var(GITHUB_INSTALLATION_ID_ENV)
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| GitHubClientError::InvalidConfiguration)
            })
            .transpose()?;
        Self::new(GitHubAppClientConfig {
            api_base: std::env::var(GITHUB_API_BASE_ENV)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| GITHUB_API_BASE.to_string()),
            app_id: required(GITHUB_APP_ID_ENV)?,
            private_key_pem: required(GITHUB_APP_PRIVATE_KEY_ENV)?,
            installation_id,
        })
    }

    fn app_jwt(&self) -> Result<String, GitHubClientError> {
        let now = jsonwebtoken::get_current_timestamp();
        let claims = AppClaims {
            iat: now.saturating_sub(60),
            exp: now + 9 * 60,
            iss: self.app_id.clone(),
        };
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.encoding_key)
            .map_err(|_| GitHubClientError::InvalidResponse)
    }

    async fn installation_id(&self, repository: &RepositoryRef) -> Result<u64, GitHubClientError> {
        if let Some(id) = self.pinned_installation_id {
            return Ok(id);
        }
        if let Some(id) = self.installations.lock().await.get(repository).copied() {
            return Ok(id);
        }
        let url = format!(
            "{}/repos/{}/{}/installation",
            self.api_base, repository.owner, repository.name
        );
        let response = self
            .client
            .get(url)
            .bearer_auth(self.app_jwt()?)
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let installation: Installation = self.parse_json(response).await?;
        self.installations
            .lock()
            .await
            .insert(repository.clone(), installation.id);
        Ok(installation.id)
    }

    async fn installation_token(
        &self,
        repository: &RepositoryRef,
    ) -> Result<InstallationToken, GitHubClientError> {
        if let Some(cached) = self.tokens.lock().await.get(repository) {
            if cached.is_fresh() {
                return Ok(cached.value.clone());
            }
        }
        let installation_id = self.installation_id(repository).await?;
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.api_base
        );
        let response = self
            .client
            .post(url)
            .bearer_auth(self.app_jwt()?)
            .json(&serde_json::json!({ "repositories": [repository.name] }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let minted: InstallationTokenResponse = self.parse_json(response).await?;
        self.tokens.lock().await.insert(
            repository.clone(),
            CachedToken {
                value: InstallationToken::from_secret(minted.token.clone())?,
                expires_at: minted.expires_at,
            },
        );
        InstallationToken::from_secret(minted.token)
    }

    fn pull_request_url(&self, repository: &RepositoryRef) -> String {
        format!(
            "{}/repos/{}/{}/pulls",
            self.api_base, repository.owner, repository.name
        )
    }

    fn pull_request_review_comment_url(
        &self,
        repository: &RepositoryRef,
        pull_request_number: u64,
        comment_id: u64,
    ) -> String {
        format!(
            "{}/repos/{}/{}/pulls/{pull_request_number}/comments/{comment_id}/replies",
            self.api_base, repository.owner, repository.name
        )
    }

    fn check_runs_url(
        &self,
        repository: &RepositoryRef,
        reference: &str,
    ) -> Result<Url, GitHubClientError> {
        let mut url =
            Url::parse(&self.api_base).map_err(|_| GitHubClientError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| GitHubClientError::InvalidConfiguration)?
            .extend([
                "repos",
                &repository.owner,
                &repository.name,
                "commits",
                reference,
                "check-runs",
            ]);
        Ok(url)
    }

    fn workflow_run_logs_url(
        &self,
        repository: &RepositoryRef,
        workflow_run_id: u64,
    ) -> Result<Url, GitHubClientError> {
        let mut url =
            Url::parse(&self.api_base).map_err(|_| GitHubClientError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| GitHubClientError::InvalidConfiguration)?
            .extend([
                "repos",
                &repository.owner,
                &repository.name,
                "actions",
                "runs",
                &workflow_run_id.to_string(),
                "logs",
            ]);
        Ok(url)
    }

    async fn graph_ql<T: serde::de::DeserializeOwned>(
        &self,
        repository: &RepositoryRef,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        graph_ql_with_token(&self.client, &self.api_base, &token, query, variables).await
    }

    fn issue_url(&self, repository: &RepositoryRef, number: u64) -> String {
        format!(
            "{}/repos/{}/{}/issues/{number}",
            self.api_base, repository.owner, repository.name
        )
    }

    fn issue_label_url(
        &self,
        repository: &RepositoryRef,
        number: u64,
        label: &str,
    ) -> Result<Url, GitHubClientError> {
        let mut url = Url::parse(&format!("{}/labels", self.issue_url(repository, number)))
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| GitHubClientError::InvalidConfiguration)?
            .push(label);
        Ok(url)
    }

    async fn parse_json<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, GitHubClientError> {
        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(|_| GitHubClientError::InvalidResponse)
        } else {
            Err(match response.status() {
                StatusCode::UNAUTHORIZED => GitHubClientError::Authentication { status: 401 },
                StatusCode::NOT_FOUND => GitHubClientError::NotFound,
                StatusCode::UNPROCESSABLE_ENTITY => GitHubClientError::Validation,
                status => GitHubClientError::Api {
                    status: status.as_u16(),
                },
            })
        }
    }

    async fn parse_bytes(&self, response: reqwest::Response) -> Result<Vec<u8>, GitHubClientError> {
        if response.status().is_success() {
            response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|_| GitHubClientError::Transport)
        } else {
            Err(match response.status() {
                StatusCode::UNAUTHORIZED => GitHubClientError::Authentication { status: 401 },
                StatusCode::NOT_FOUND => GitHubClientError::NotFound,
                StatusCode::UNPROCESSABLE_ENTITY => GitHubClientError::Validation,
                status => GitHubClientError::Api {
                    status: status.as_u16(),
                },
            })
        }
    }
}

fn validate_verified_commit(commit: &VerifiedCommit) -> Result<(), GitHubClientError> {
    if commit.branch.is_empty()
        || commit.branch.starts_with('-')
        || commit.branch.starts_with('/')
        || commit.branch.contains("..")
        || !is_commit_oid(&commit.expected_head_oid)
        || commit.headline.trim().is_empty()
        || (commit.additions.is_empty() && commit.deletions.is_empty())
    {
        return Err(GitHubClientError::InvalidConfiguration);
    }

    let mut paths = std::collections::HashSet::new();
    for path in commit
        .additions
        .iter()
        .map(|addition| &addition.path)
        .chain(commit.deletions.iter())
    {
        if !is_git_tree_path(path) || !paths.insert(path) {
            return Err(GitHubClientError::InvalidConfiguration);
        }
    }
    Ok(())
}

fn is_commit_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_git_tree_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

async fn graph_ql_with_token<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    api_base: &str,
    token: &InstallationToken,
    query: &str,
    variables: serde_json::Value,
) -> Result<T, GitHubClientError> {
    let response = client
        .post(format!("{api_base}/graphql"))
        .bearer_auth(token.as_str())
        .json(&serde_json::json!({
            "query": query,
            "variables": variables,
        }))
        .send()
        .await
        .map_err(|_| GitHubClientError::Transport)?;
    let response: GraphQlResponse<T> = parse_json(response).await?;
    if response.errors.is_empty() {
        response.data.ok_or(GitHubClientError::InvalidResponse)
    } else {
        Err(GitHubClientError::GraphQl)
    }
}

async fn parse_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, GitHubClientError> {
    if response.status().is_success() {
        response
            .json()
            .await
            .map_err(|_| GitHubClientError::InvalidResponse)
    } else {
        Err(match response.status() {
            StatusCode::UNAUTHORIZED => GitHubClientError::Authentication { status: 401 },
            StatusCode::NOT_FOUND => GitHubClientError::NotFound,
            StatusCode::UNPROCESSABLE_ENTITY => GitHubClientError::Validation,
            status => GitHubClientError::Api {
                status: status.as_u16(),
            },
        })
    }
}

fn workflow_run_log_tail(bytes: &[u8]) -> Result<WorkflowRunLogTail, GitHubClientError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| GitHubClientError::InvalidResponse)?;
    let mut tail = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| GitHubClientError::InvalidResponse)?;
        if entry.is_dir() {
            continue;
        }
        let mut log = String::new();
        entry
            .read_to_string(&mut log)
            .map_err(|_| GitHubClientError::InvalidResponse)?;
        if let Some(failed_step) = log.rfind("##[error]") {
            tail = Some(
                log[failed_step..]
                    .lines()
                    .rev()
                    .take(FAILED_LOG_TAIL_LINES)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    tail.map(|tail| WorkflowRunLogTail { tail })
        .ok_or(GitHubClientError::InvalidResponse)
}

/// The single GitHub mutation an ephemeral worker may use to publish code.
///
/// Implementations must create the commit through GitHub's
/// `createCommitOnBranch` API. Shelling out to `git commit` is deliberately
/// outside this boundary so a runner never needs a signing key.
#[async_trait]
pub trait VerifiedCommitClient: Send + Sync {
    /// Append a GitHub-signed commit when the branch still points at the
    /// requested immutable head.
    async fn create_verified_commit(
        &self,
        repository: &RepositoryRef,
        commit: &VerifiedCommit,
    ) -> Result<CreatedCommit, GitHubClientError>;
}

#[async_trait]
impl VerifiedCommitClient for InstallationTokenClient {
    async fn create_verified_commit(
        &self,
        repository: &RepositoryRef,
        commit: &VerifiedCommit,
    ) -> Result<CreatedCommit, GitHubClientError> {
        create_verified_commit(
            &self.client,
            &self.api_base,
            &self.token,
            repository,
            commit,
        )
        .await
    }
}

async fn create_verified_commit(
    client: &reqwest::Client,
    api_base: &str,
    token: &InstallationToken,
    repository: &RepositoryRef,
    commit: &VerifiedCommit,
) -> Result<CreatedCommit, GitHubClientError> {
    const MUTATION: &str = "mutation CreateCommitOnBranch($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid } } }";

    validate_verified_commit(commit)?;
    let data: CreateCommitOnBranchData = graph_ql_with_token(
        client,
        api_base,
        token,
        MUTATION,
        serde_json::json!({
            "input": {
                "branch": {
                    "repositoryNameWithOwner": format!("{}/{}", repository.owner, repository.name),
                    "branchName": commit.branch,
                },
                "expectedHeadOid": commit.expected_head_oid,
                "message": {
                    "headline": commit.headline,
                    "body": commit.body,
                },
                "fileChanges": {
                    "additions": commit.additions.iter().map(|addition| serde_json::json!({
                        "path": addition.path,
                        "contents": STANDARD.encode(&addition.contents),
                    })).collect::<Vec<_>>(),
                    "deletions": commit.deletions.iter().map(|path| serde_json::json!({
                        "path": path,
                    })).collect::<Vec<_>>(),
                },
            }
        }),
    )
    .await?;
    let commit = data
        .create_commit_on_branch
        .and_then(|payload| payload.commit)
        .ok_or(GitHubClientError::InvalidResponse)?;
    Ok(CreatedCommit { oid: commit.oid })
}

#[async_trait]
impl GitHubClient for GitHubAppClient {
    async fn create_pull_request(
        &self,
        repository: &RepositoryRef,
        draft: &PullRequestDraft,
    ) -> Result<u64, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .post(self.pull_request_url(repository))
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({
                "title": draft.title,
                "body": draft.body,
                "head": draft.head,
                "base": draft.base,
                "draft": draft.draft,
            }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let pull_request: PullRequestResponse = self.parse_json(response).await?;
        Ok(pull_request.number)
    }

    async fn get_pull_request(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<PullRequest, GitHubClientError> {
        const QUERY: &str = "query PullRequest($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { number mergeStateStatus } } }";
        let number = i64::try_from(number).map_err(|_| GitHubClientError::InvalidConfiguration)?;
        let data: PullRequestQueryData = self
            .graph_ql(
                repository,
                QUERY,
                serde_json::json!({
                    "owner": repository.owner,
                    "name": repository.name,
                    "number": number,
                }),
            )
            .await?;
        let pull_request = data
            .repository
            .and_then(|repository| repository.pull_request)
            .ok_or(GitHubClientError::NotFound)?;
        Ok(PullRequest {
            number: pull_request.number,
            merge_state_status: pull_request.merge_state_status,
        })
    }

    async fn update_pull_request_branch(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .put(format!(
                "{}/{number}/update-branch",
                self.pull_request_url(repository)
            ))
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn list_unresolved_review_threads(
        &self,
        repository: &RepositoryRef,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThread>, GitHubClientError> {
        const QUERY: &str = "query ReviewThreads($owner: String!, $name: String!, $number: Int!, $cursor: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $cursor) { nodes { id isResolved comments(first: 100) { nodes { databaseId body path line } } } pageInfo { hasNextPage endCursor } } } } }";
        let number = i64::try_from(pull_request_number)
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        let mut cursor = None;
        let mut threads = Vec::new();
        loop {
            let data: ReviewThreadsQueryData = self
                .graph_ql(
                    repository,
                    QUERY,
                    serde_json::json!({
                        "owner": repository.owner,
                        "name": repository.name,
                        "number": number,
                        "cursor": cursor,
                    }),
                )
                .await?;
            let connection = data
                .repository
                .and_then(|repository| repository.pull_request)
                .map(|pull_request| pull_request.review_threads)
                .ok_or(GitHubClientError::NotFound)?;
            threads.extend(connection.nodes.into_iter().filter_map(|thread| {
                (!thread.is_resolved).then(|| ReviewThread {
                    id: thread.id,
                    comments: thread.comments.nodes.into_iter().map(Into::into).collect(),
                })
            }));
            if !connection.page_info.has_next_page {
                return Ok(threads);
            }
            cursor = Some(
                connection
                    .page_info
                    .end_cursor
                    .ok_or(GitHubClientError::InvalidResponse)?,
            );
        }
    }

    async fn reply_to_review_comment(
        &self,
        repository: &RepositoryRef,
        pull_request_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .post(self.pull_request_review_comment_url(repository, pull_request_number, comment_id))
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn resolve_review_thread(
        &self,
        repository: &RepositoryRef,
        thread_id: &str,
    ) -> Result<(), GitHubClientError> {
        const MUTATION: &str = "mutation ResolveReviewThread($threadId: ID!) { resolveReviewThread(input: {threadId: $threadId}) { thread { isResolved } } }";
        let data: ResolveReviewThreadData = self
            .graph_ql(
                repository,
                MUTATION,
                serde_json::json!({ "threadId": thread_id }),
            )
            .await?;
        if data
            .resolve_review_thread
            .and_then(|payload| payload.thread)
            .is_some_and(|thread| thread.is_resolved)
        {
            Ok(())
        } else {
            Err(GitHubClientError::InvalidResponse)
        }
    }

    async fn get_issue(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Issue, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .get(self.issue_url(repository, number))
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let issue: IssueResponse = self.parse_json(response).await?;
        Ok(issue.into())
    }

    async fn list_issue_comments(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<IssueComment>, GitHubClientError> {
        const QUERY: &str = "query IssueComments($owner: String!, $name: String!, $number: Int!, $cursor: String) { repository(owner: $owner, name: $name) { issue(number: $number) { comments(first: 100, after: $cursor) { nodes { body } pageInfo { hasNextPage endCursor } } } } }";
        let number = i64::try_from(number).map_err(|_| GitHubClientError::InvalidConfiguration)?;
        let mut cursor = None;
        let mut comments = Vec::new();
        loop {
            let data: IssueCommentsQueryData = self
                .graph_ql(
                    repository,
                    QUERY,
                    serde_json::json!({
                        "owner": repository.owner,
                        "name": repository.name,
                        "number": number,
                        "cursor": cursor,
                    }),
                )
                .await?;
            let connection = data
                .repository
                .and_then(|repository| repository.issue)
                .map(|issue| issue.comments)
                .ok_or(GitHubClientError::NotFound)?;
            comments.extend(connection.nodes.into_iter().map(Into::into));
            if !connection.page_info.has_next_page {
                return Ok(comments);
            }
            cursor = Some(
                connection
                    .page_info
                    .end_cursor
                    .ok_or(GitHubClientError::InvalidResponse)?,
            );
        }
    }

    async fn update_issue_body(
        &self,
        repository: &RepositoryRef,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .patch(self.issue_url(repository, number))
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn create_comment(
        &self,
        repository: &RepositoryRef,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let url = format!("{}/comments", self.issue_url(repository, number));
        let response = self
            .client
            .post(url)
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn add_label(
        &self,
        repository: &RepositoryRef,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let url = format!("{}/labels", self.issue_url(repository, number));
        let response = self
            .client
            .post(url)
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({ "labels": [label] }))
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn remove_label(
        &self,
        repository: &RepositoryRef,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let url = self.issue_label_url(repository, number, label)?;
        let response = self
            .client
            .delete(url)
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let _: serde_json::Value = self.parse_json(response).await?;
        Ok(())
    }

    async fn list_check_runs(
        &self,
        repository: &RepositoryRef,
        reference: &str,
    ) -> Result<Vec<CheckRun>, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .get(self.check_runs_url(repository, reference)?)
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        let response: CheckRunsResponse = self.parse_json(response).await?;
        Ok(response.check_runs.into_iter().map(Into::into).collect())
    }

    async fn download_workflow_run_log_tail(
        &self,
        repository: &RepositoryRef,
        workflow_run_id: u64,
    ) -> Result<WorkflowRunLogTail, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let response = self
            .client
            .get(self.workflow_run_logs_url(repository, workflow_run_id)?)
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| GitHubClientError::Transport)?;
        workflow_run_log_tail(&self.parse_bytes(response).await?)
    }

    async fn clone_url(&self, repository: &RepositoryRef) -> Result<CloneUrl, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        let mut url = Url::parse("https://github.com")
            .map_err(|_| GitHubClientError::InvalidConfiguration)?;
        url.set_username("x-access-token")
            .map_err(|()| GitHubClientError::InvalidConfiguration)?;
        url.set_password(Some(token.as_str()))
            .map_err(|()| GitHubClientError::InvalidConfiguration)?;
        url.path_segments_mut()
            .map_err(|()| GitHubClientError::InvalidConfiguration)?
            .extend([&repository.owner, &format!("{}.git", repository.name)]);
        Ok(CloneUrl(url.into()))
    }
}

#[async_trait]
impl VerifiedCommitClient for GitHubAppClient {
    async fn create_verified_commit(
        &self,
        repository: &RepositoryRef,
        commit: &VerifiedCommit,
    ) -> Result<CreatedCommit, GitHubClientError> {
        let token = self.installation_token(repository).await?;
        create_verified_commit(&self.client, &self.api_base, &token, repository, commit).await
    }
}

/// GitHub operations available to `DevX` workflows.
#[async_trait]
pub trait GitHubClient: Send + Sync {
    /// Open a pull request from the requested head branch against its base.
    async fn create_pull_request(
        &self,
        repository: &RepositoryRef,
        draft: &PullRequestDraft,
    ) -> Result<u64, GitHubClientError>;

    /// Read the merge state used to decide whether a pull request needs a
    /// branch update before the revision loop continues.
    async fn get_pull_request(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<PullRequest, GitHubClientError>;

    /// Ask GitHub to update a pull request branch from its base branch.
    async fn update_pull_request_branch(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<(), GitHubClientError>;

    /// List every unresolved review thread, following GitHub's pagination.
    async fn list_unresolved_review_threads(
        &self,
        repository: &RepositoryRef,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThread>, GitHubClientError>;

    /// Reply to an inline pull-request comment with source-safe body content.
    async fn reply_to_review_comment(
        &self,
        repository: &RepositoryRef,
        pull_request_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<(), GitHubClientError>;

    /// Resolve a GraphQL review-thread identifier after replying to it.
    async fn resolve_review_thread(
        &self,
        repository: &RepositoryRef,
        thread_id: &str,
    ) -> Result<(), GitHubClientError>;

    /// Read an issue's source content.
    async fn get_issue(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Issue, GitHubClientError>;

    /// Read every comment in an issue discussion, following GitHub pagination.
    ///
    /// The returned text is source content and must remain inside the
    /// short-lived runner or one journaled side effect.
    async fn list_issue_comments(
        &self,
        repository: &RepositoryRef,
        number: u64,
    ) -> Result<Vec<IssueComment>, GitHubClientError>;

    /// Replace an issue body.
    async fn update_issue_body(
        &self,
        repository: &RepositoryRef,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubClientError>;

    /// Add a comment to an issue or pull request.
    ///
    /// Callers must treat `body` as client-content-bearing data: it must not
    /// be included in telemetry, errors, or durable correlation identifiers.
    async fn create_comment(
        &self,
        repository: &RepositoryRef,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubClientError>;

    /// Attach a label to an issue or pull request.
    async fn add_label(
        &self,
        repository: &RepositoryRef,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubClientError>;

    /// Remove a label from an issue or pull request.
    async fn remove_label(
        &self,
        repository: &RepositoryRef,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubClientError>;

    /// List check runs attached to a commit or branch ref.
    async fn list_check_runs(
        &self,
        repository: &RepositoryRef,
        reference: &str,
    ) -> Result<Vec<CheckRun>, GitHubClientError>;

    /// Download and bound the failure tail from a GitHub Actions workflow run.
    async fn download_workflow_run_log_tail(
        &self,
        repository: &RepositoryRef,
        workflow_run_id: u64,
    ) -> Result<WorkflowRunLogTail, GitHubClientError>;

    /// Mint a short-lived, repository-scoped Git HTTPS credential URL.
    async fn clone_url(&self, repository: &RepositoryRef) -> Result<CloneUrl, GitHubClientError>;
}

/// A fail-closed client for local and unconfigured deployments.
#[derive(Debug, Default)]
pub struct NullGitHubClient;

#[async_trait]
impl GitHubClient for NullGitHubClient {
    async fn create_pull_request(
        &self,
        _repository: &RepositoryRef,
        _draft: &PullRequestDraft,
    ) -> Result<u64, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn get_pull_request(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
    ) -> Result<PullRequest, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn update_pull_request_branch(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn list_unresolved_review_threads(
        &self,
        _repository: &RepositoryRef,
        _pull_request_number: u64,
    ) -> Result<Vec<ReviewThread>, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn reply_to_review_comment(
        &self,
        _repository: &RepositoryRef,
        _pull_request_number: u64,
        _comment_id: u64,
        _body: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn resolve_review_thread(
        &self,
        _repository: &RepositoryRef,
        _thread_id: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn get_issue(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
    ) -> Result<Issue, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn list_issue_comments(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
    ) -> Result<Vec<IssueComment>, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn update_issue_body(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
        _body: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn create_comment(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
        _body: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn add_label(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
        _label: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn remove_label(
        &self,
        _repository: &RepositoryRef,
        _number: u64,
        _label: &str,
    ) -> Result<(), GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn list_check_runs(
        &self,
        _repository: &RepositoryRef,
        _reference: &str,
    ) -> Result<Vec<CheckRun>, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn download_workflow_run_log_tail(
        &self,
        _repository: &RepositoryRef,
        _workflow_run_id: u64,
    ) -> Result<WorkflowRunLogTail, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }

    async fn clone_url(&self, _repository: &RepositoryRef) -> Result<CloneUrl, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }
}

#[async_trait]
impl VerifiedCommitClient for NullGitHubClient {
    async fn create_verified_commit(
        &self,
        _repository: &RepositoryRef,
        _commit: &VerifiedCommit,
    ) -> Result<CreatedCommit, GitHubClientError> {
        Err(GitHubClientError::Unconfigured)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    use chrono::{Duration, Utc};
    use jsonwebtoken::EncodingKey;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zip::write::SimpleFileOptions;

    use super::{
        validate_verified_commit, CachedToken, FileAddition, GitHubAppClient, GitHubClient,
        GitHubClientError, InstallationToken, InstallationTokenClient, NullGitHubClient,
        PullRequestDraft, RepositoryRef, VerifiedCommit, VerifiedCommitClient,
    };

    fn repository() -> RepositoryRef {
        RepositoryRef {
            owner: "neon-law-source-code".to_string(),
            name: "navigator".to_string(),
        }
    }

    fn client_with_cached_token(server: &MockServer) -> GitHubAppClient {
        let repository = repository();
        GitHubAppClient {
            client: reqwest::Client::new(),
            api_base: server.uri(),
            app_id: "test-app".to_string(),
            // The cached installation token isolates each operation's HTTP
            // contract from the App-JWT exchange.
            encoding_key: EncodingKey::from_secret(b"unused in operation tests"),
            pinned_installation_id: None,
            installations: tokio::sync::Mutex::new(HashMap::new()),
            tokens: tokio::sync::Mutex::new(HashMap::from([(
                repository,
                CachedToken {
                    value: InstallationToken::from_secret("ghs_test".to_string()).unwrap(),
                    expires_at: Utc::now() + Duration::hours(1),
                },
            )])),
        }
    }

    fn installation_token_client(server: &MockServer) -> InstallationTokenClient {
        InstallationTokenClient::new(
            &server.uri(),
            InstallationToken::from_secret("ghs_test".to_string()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn null_client_never_claims_to_post_a_comment() {
        let result = NullGitHubClient
            .create_comment(
                &RepositoryRef {
                    owner: "neon-law-source-code".to_string(),
                    name: "navigator".to_string(),
                },
                455,
                "This must never reach GitHub.",
            )
            .await;

        assert_eq!(result, Err(GitHubClientError::Unconfigured));
    }

    #[tokio::test]
    async fn reads_an_issue_with_its_source_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/neon-law-source-code/navigator/issues/455"))
            .and(header("authorization", "Bearer ghs_test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 455,
                "title": "Improve intake",
                "body": "Synthetic issue content."
            })))
            .mount(&server)
            .await;

        let issue = client_with_cached_token(&server)
            .get_issue(&repository(), 455)
            .await
            .unwrap();

        assert_eq!(issue.number, 455);
        assert_eq!(issue.title, "Improve intake");
        assert_eq!(issue.body.as_deref(), Some("Synthetic issue content."));
    }

    #[tokio::test]
    async fn reads_every_issue_comment_page_without_exposing_the_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "variables": {
                    "owner": "neon-law-source-code",
                    "name": "navigator",
                    "number": 455,
                    "cursor": null,
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "issue": {
                            "comments": {
                                "nodes": [{ "body": "Synthetic opening discussion." }],
                                "pageInfo": { "hasNextPage": true, "endCursor": "page-two" }
                            }
                        }
                    }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "variables": {
                    "owner": "neon-law-source-code",
                    "name": "navigator",
                    "number": 455,
                    "cursor": "page-two",
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "issue": {
                            "comments": {
                                "nodes": [{ "body": "Synthetic final discussion." }],
                                "pageInfo": { "hasNextPage": false, "endCursor": null }
                            }
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let comments = client_with_cached_token(&server)
            .list_issue_comments(&repository(), 455)
            .await
            .unwrap();

        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].body, "Synthetic opening discussion.");
        assert_eq!(comments[1].body, "Synthetic final discussion.");
    }

    #[tokio::test]
    async fn updates_an_issue_body() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/repos/neon-law-source-code/navigator/issues/455"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "body": "Updated synthetic body."
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        client_with_cached_token(&server)
            .update_issue_body(&repository(), 455, "Updated synthetic body.")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn comments_on_an_issue() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/neon-law-source-code/navigator/issues/455/comments",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "body": "Synthetic status comment."
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        client_with_cached_token(&server)
            .create_comment(&repository(), 455, "Synthetic status comment.")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn labels_an_issue_and_removes_the_label() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/neon-law-source-code/navigator/issues/455/labels",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({ "labels": ["devx"] })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(
                "/repos/neon-law-source-code/navigator/issues/455/labels/devx",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = client_with_cached_token(&server);
        client.add_label(&repository(), 455, "devx").await.unwrap();
        client
            .remove_label(&repository(), 455, "devx")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn label_urls_escape_path_delimiters() {
        let server = MockServer::start().await;
        let client = client_with_cached_token(&server);

        let url = client
            .issue_label_url(&repository(), 455, "needs/review")
            .unwrap();

        assert_eq!(
            url.as_str(),
            format!(
                "{}/repos/neon-law-source-code/navigator/issues/455/labels/needs%2Freview",
                server.uri()
            )
        );
    }

    #[tokio::test]
    async fn creates_a_pull_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/neon-law-source-code/navigator/pulls"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "title": "Synthetic pull request",
                "body": "Synthetic body.",
                "head": "devx/issue-455",
                "base": "main",
                "draft": true,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 991
            })))
            .mount(&server)
            .await;

        let number = client_with_cached_token(&server)
            .create_pull_request(
                &repository(),
                &PullRequestDraft {
                    title: "Synthetic pull request".to_string(),
                    body: "Synthetic body.".to_string(),
                    head: "devx/issue-455".to_string(),
                    base: "main".to_string(),
                    draft: true,
                },
            )
            .await
            .unwrap();

        assert_eq!(number, 991);
    }

    #[tokio::test]
    async fn reads_merge_state_with_graphql() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "variables": {
                    "owner": "neon-law-source-code",
                    "name": "navigator",
                    "number": 991,
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "number": 991,
                            "mergeStateStatus": "CLEAN"
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let pull_request = client_with_cached_token(&server)
            .get_pull_request(&repository(), 991)
            .await
            .unwrap();

        assert_eq!(pull_request.number, 991);
        assert_eq!(pull_request.merge_state_status, "CLEAN");
    }

    #[tokio::test]
    async fn rejects_graphql_errors_without_exposing_their_messages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": null,
                "errors": [{ "message": "Synthetic internal detail" }]
            })))
            .mount(&server)
            .await;

        let error = client_with_cached_token(&server)
            .get_pull_request(&repository(), 991)
            .await
            .unwrap_err();

        assert_eq!(error, GitHubClientError::GraphQl);
        assert!(!error.to_string().contains("Synthetic internal detail"));
    }

    #[tokio::test]
    async fn updates_a_pull_request_branch() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(
                "/repos/neon-law-source-code/navigator/pulls/991/update-branch",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        client_with_cached_token(&server)
            .update_pull_request_branch(&repository(), 991)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn lists_unresolved_review_threads_across_pages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_partial_json(serde_json::json!({
                "variables": { "cursor": null }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "reviewThreads": {
                                "nodes": [
                                    {
                                        "id": "PRRT_open",
                                        "isResolved": false,
                                        "comments": {
                                            "nodes": [{
                                                "databaseId": 123,
                                                "body": "Synthetic actionable review.",
                                                "path": "github_webhooks/src/github.rs",
                                                "line": 42
                                            }]
                                        }
                                    },
                                    {
                                        "id": "PRRT_resolved",
                                        "isResolved": true,
                                        "comments": { "nodes": [] }
                                    }
                                ],
                                "pageInfo": { "hasNextPage": true, "endCursor": "next" }
                            }
                        }
                    }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_partial_json(serde_json::json!({
                "variables": { "cursor": "next" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "reviewThreads": {
                                "nodes": [{
                                    "id": "PRRT_second",
                                    "isResolved": false,
                                    "comments": { "nodes": [] }
                                }],
                                "pageInfo": { "hasNextPage": false, "endCursor": null }
                            }
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let threads = client_with_cached_token(&server)
            .list_unresolved_review_threads(&repository(), 991)
            .await
            .unwrap();

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, "PRRT_open");
        assert_eq!(threads[0].comments.len(), 1);
        assert_eq!(threads[0].comments[0].database_id, Some(123));
        assert_eq!(threads[0].comments[0].body, "Synthetic actionable review.");
        assert_eq!(threads[1].id, "PRRT_second");
    }

    #[tokio::test]
    async fn replies_to_a_review_comment() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/neon-law-source-code/navigator/pulls/991/comments/123/replies",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "body": "Fixed in the current commit."
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        client_with_cached_token(&server)
            .reply_to_review_comment(&repository(), 991, 123, "Fixed in the current commit.")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolves_a_review_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "variables": { "threadId": "PRRT_open" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "resolveReviewThread": {
                        "thread": { "isResolved": true }
                    }
                }
            })))
            .mount(&server)
            .await;

        client_with_cached_token(&server)
            .resolve_review_thread(&repository(), "PRRT_open")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn lists_check_runs_for_an_encoded_ref() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/neon-law-source-code/navigator/commits/devx%2Fissue-455/check-runs",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "check_runs": [{
                    "id": 44,
                    "name": "workspace tests",
                    "conclusion": "failure"
                }]
            })))
            .mount(&server)
            .await;

        let checks = client_with_cached_token(&server)
            .list_check_runs(&repository(), "devx/issue-455")
            .await
            .unwrap();

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, 44);
        assert_eq!(checks[0].name, "workspace tests");
        assert_eq!(checks[0].conclusion.as_deref(), Some("failure"));
    }

    #[tokio::test]
    async fn extracts_a_bounded_tail_from_the_failed_workflow_step() {
        let mut bytes = Vec::new();
        {
            let mut archive = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            archive
                .start_file(
                    "workspace tests/Run tests.txt",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writeln!(archive, "running synthetic suite").unwrap();
            writeln!(archive, "##[error]first synthetic failure").unwrap();
            writeln!(archive, "detail after the failure").unwrap();
            archive.finish().unwrap();
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/neon-law-source-code/navigator/actions/runs/123/logs",
            ))
            .and(header("authorization", "Bearer ghs_test"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;

        let tail = client_with_cached_token(&server)
            .download_workflow_run_log_tail(&repository(), 123)
            .await
            .unwrap();

        assert_eq!(
            tail.as_str(),
            "##[error]first synthetic failure\ndetail after the failure"
        );
    }

    #[tokio::test]
    async fn clone_url_is_credential_bearing_but_client_debug_is_not() {
        let client = client_with_cached_token(&MockServer::start().await);

        let clone_url = client.clone_url(&repository()).await.unwrap();

        assert_eq!(
            clone_url.as_str(),
            "https://x-access-token:ghs_test@github.com/neon-law-source-code/navigator.git"
        );
        assert!(!format!("{client:?}").contains("ghs_test"));
    }

    #[tokio::test]
    async fn installation_token_client_creates_a_verified_commit_through_graphql() {
        let server = MockServer::start().await;
        let expected_head = "a".repeat(40);
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer ghs_test"))
            .and(body_partial_json(serde_json::json!({
                "variables": {
                    "input": {
                        "branch": {
                            "repositoryNameWithOwner": "neon-law-source-code/navigator",
                            "branchName": "devx/issue-463",
                        },
                        "expectedHeadOid": expected_head,
                        "message": {
                            "headline": "feat(devx): publish verified runner changes",
                            "body": "Closes #463",
                        },
                        "fileChanges": {
                            "additions": [{
                                "path": "github_webhooks/src/runner.rs",
                                "contents": "cHViIG1vZGU7",
                            }],
                            "deletions": [{ "path": "old.rs" }],
                        },
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "createCommitOnBranch": {
                        "commit": { "oid": "b".repeat(40) }
                    }
                }
            })))
            .mount(&server)
            .await;

        let commit = VerifiedCommit {
            branch: "devx/issue-463".to_string(),
            expected_head_oid: expected_head,
            headline: "feat(devx): publish verified runner changes".to_string(),
            body: Some("Closes #463".to_string()),
            additions: vec![FileAddition {
                path: "github_webhooks/src/runner.rs".to_string(),
                contents: b"pub mode;".to_vec(),
            }],
            deletions: vec!["old.rs".to_string()],
        };
        let created = installation_token_client(&server)
            .create_verified_commit(&repository(), &commit)
            .await
            .unwrap();

        assert_eq!(created.oid, "b".repeat(40));
    }

    #[test]
    fn verified_commit_rejects_invalid_paths_and_duplicate_changes() {
        let commit = VerifiedCommit {
            branch: "devx/issue-463".to_string(),
            expected_head_oid: "a".repeat(40),
            headline: "feat(devx): publish verified runner changes".to_string(),
            body: None,
            additions: vec![FileAddition {
                path: "../escape.rs".to_string(),
                contents: Vec::new(),
            }],
            deletions: vec!["../escape.rs".to_string()],
        };

        assert_eq!(
            validate_verified_commit(&commit),
            Err(GitHubClientError::InvalidConfiguration)
        );
    }
}
