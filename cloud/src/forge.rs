//! One Project, one source repository, created through the deployment's forge.
//!
//! A [`ForgeService`] creates or adopts a private repository named for a
//! Project code and returns its URL. It has no collaborator, invite, or
//! membership methods: Project participation never grants source-forge access.
//! Who may clone is governed on the forge itself, not from Navigator's
//! participation ledger.
//!
//! The organization and host come from [`crate::workspace::WorkspaceConfig`].
//! They are the creation target, not a Project's recorded URL — a matter whose
//! source already lives elsewhere keeps that URL and is not moved here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::workspace::{WorkspaceConfig, WorkspaceConfigError, NAVIGATOR_GITHUB_ORG};

/// Env var holding the GitHub token used to create Project repositories.
/// Checked before [`GITHUB_TOKEN_ENV`] so a workspace-specific token can
/// override an ambient one.
pub const NAVIGATOR_GITHUB_TOKEN_ENV: &str = "NAVIGATOR_GITHUB_TOKEN";
/// The conventional GitHub token env var, used when
/// [`NAVIGATOR_GITHUB_TOKEN_ENV`] is unset.
pub const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
/// Override the REST API base, for GitHub Enterprise or a test double.
///
/// Naming GitHub Enterprise is a **feature, not stale narration.** Navigator
/// runs on github.com; this override is how somebody running their own
/// instance points it at their own tenant. See the same note on
/// `webapp::source_repository::GITHUB_API_BASE_ENV`.
pub const GITHUB_API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";
/// Override the GraphQL endpoint independently of the REST API base.
pub const GITHUB_GRAPHQL_API_ENV: &str = "NAVIGATOR_GITHUB_GRAPHQL_API";
/// Public GitHub's REST API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("neon-law-navigator/", env!("CARGO_PKG_VERSION"));
/// Bound on each request. `head_commit_committed_at` now runs on every
/// `/app/projects` render (once per matching row, concurrently), not only
/// from an infrequent, deliberate admin action — so a socket GitHub never
/// answers must not hang a page render indefinitely. Mirrors
/// `webapp::source_repository::REQUEST_TIMEOUT`'s value and reasoning: a
/// hang costs one degraded column, not the request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A Project's source repository as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeRepository {
    /// The clone/browse URL recorded on the Project.
    pub url: String,
    /// The repository name, which is the Project code.
    pub name: String,
}

#[derive(Debug, Error)]
pub enum ForgeError {
    #[error("missing required forge configuration: {0}")]
    MissingConfig(&'static str),
    #[error("resolve the deployment forge pair: {0}")]
    Workspace(#[from] WorkspaceConfigError),
    #[error("forge authentication failed")]
    Authentication,
    #[error("forge request failed while {action}")]
    Request {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("forge API returned {status} while {action}")]
    Api { action: &'static str, status: u16 },
    #[error("forge API returned an invalid response while {action}")]
    Response {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("forge API response while {action} did not include a repository URL")]
    MissingUrl { action: &'static str },
    #[error("forge API returned an unexpected response shape while {action}")]
    ResponseShape { action: &'static str },
    #[error("forge GraphQL returned an error while {action}: {message}")]
    Graphql {
        action: &'static str,
        message: String,
    },
    #[error("repository coordinate must be `owner/name`, not `{0}`")]
    InvalidRepository(String),
    #[error(
        "repository {name} is not private after provisioning; a policy regression or a \
         deliberate visibility change happened underneath, and both need a human, not a retry"
    )]
    NotPrivate { name: String },
}

/// A forge pull request opened by the Project-repository delivery lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgePullRequest {
    pub number: u64,
    pub node_id: String,
    pub url: String,
}

/// One check run attached to a pull request's head commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeCheck {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

/// The live facts needed to decide whether delivery merged or stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgePullRequestStatus {
    pub merged: bool,
    pub auto_merge_armed: bool,
    pub merge_state: String,
    pub review_decision: Option<String>,
    pub checks: Vec<ForgeCheck>,
}

/// Pull-request operations kept behind the forge boundary.
#[async_trait]
pub trait PullRequestForge: Send + Sync {
    async fn open_or_adopt_pull_request(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<ForgePullRequest, ForgeError>;
    async fn enable_auto_merge(&self, pull_request_node_id: &str) -> Result<(), ForgeError>;
    async fn pull_request_status(
        &self,
        pull_request_number: u64,
    ) -> Result<ForgePullRequestStatus, ForgeError>;
    async fn required_checks(&self, base: &str) -> Result<Vec<String>, ForgeError>;
}

/// Create or adopt one private repository named for a Project code.
///
/// The trait is deliberately narrow. Adding a collaborator method here would
/// make Project participation a back door onto the forge, which
/// [`docs/project-repositories.md`] forbids. [`Self::delete_repository`] and
/// [`Self::head_commit_sha`] are the exception: they serve a matter's
/// *close*, not its participation, and exist so a closed Project's
/// repository can be archived and safely removed (ENG-481) rather than
/// living forever once the matter has ended.
#[async_trait]
pub trait ForgeService: Send + Sync {
    async fn find_repository(
        &self,
        project_code: &str,
    ) -> Result<Option<ForgeRepository>, ForgeError>;
    async fn ensure_repository(&self, project_code: &str) -> Result<ForgeRepository, ForgeError>;
    /// The current commit SHA at the tip of the repository's default
    /// branch, or `None` if the repository does not exist. Read fresh on
    /// every call — never cached — so a caller comparing it against an
    /// archived document's recorded commit sees whatever has actually been
    /// pushed since.
    async fn head_commit_sha(&self, project_code: &str) -> Result<Option<String>, ForgeError>;
    /// The committer date (RFC 3339) of the commit at the tip of the
    /// repository's default branch, or `None` if the repository does not
    /// exist or the forge reported no date. Same freshness contract as
    /// [`Self::head_commit_sha`]: read fresh, never cached — a lawyer
    /// workbench listing calls this per row on every render rather than
    /// trusting a stale value.
    async fn head_commit_committed_at(
        &self,
        project_code: &str,
    ) -> Result<Option<String>, ForgeError>;
    /// Permanently delete the repository. Irreversible; callers must have
    /// already confirmed an archive exists and matches before calling this.
    async fn delete_repository(&self, project_code: &str) -> Result<(), ForgeError>;
}

/// In-memory forge for store and workflow tests. Idempotent on the Project
/// code: a second `ensure_repository` returns the same URL and does not
/// invent a second repository.
#[derive(Clone)]
pub struct FakeForge {
    host: String,
    organization: String,
    state: Arc<Mutex<FakeForgeState>>,
}

#[derive(Default)]
struct FakeForgeState {
    repositories: BTreeMap<String, ForgeRepository>,
    ensure_calls: usize,
    /// Set only by [`FakeForge::set_head_commit_sha`] — tests control what
    /// the "live" HEAD is rather than this fake deriving one from nothing.
    head_shas: BTreeMap<String, String>,
    /// Set only by [`FakeForge::set_head_commit_committed_at`] — tests
    /// control the "live" HEAD's committer date rather than this fake
    /// deriving one from nothing.
    head_committed_ats: BTreeMap<String, String>,
}

impl FakeForge {
    /// A forge whose URLs live under a synthetic host and organization.
    ///
    /// Which organization a deployment creates Project repositories in is
    /// configuration, so no real organization name is a fixture value.
    #[must_use]
    pub fn new() -> Self {
        Self {
            host: "forge.example".to_string(),
            organization: "an-organization".to_string(),
            state: Arc::new(Mutex::new(FakeForgeState::default())),
        }
    }

    fn url_for(&self, project_code: &str) -> String {
        format!("https://{}/{}/{project_code}", self.host, self.organization)
    }

    /// How many times [`ForgeService::ensure_repository`] ran. A retry that
    /// adopts must not increment a *create* count; this counts the call so
    /// tests can see the method is idempotent on the stored URL, not that
    /// it was never invoked.
    #[must_use]
    pub fn ensure_calls(&self) -> usize {
        self.state
            .lock()
            .expect("fake forge mutex poisoned")
            .ensure_calls
    }

    #[must_use]
    pub fn repository_count(&self) -> usize {
        self.state
            .lock()
            .expect("fake forge mutex poisoned")
            .repositories
            .len()
    }

    /// Test control: declare what [`ForgeService::head_commit_sha`] reports
    /// for `project_code`, standing in for whatever a real forge's default
    /// branch tip would be.
    pub fn set_head_commit_sha(&self, project_code: &str, sha: impl Into<String>) {
        self.state
            .lock()
            .expect("fake forge mutex poisoned")
            .head_shas
            .insert(project_code.to_string(), sha.into());
    }

    /// Test control: declare what
    /// [`ForgeService::head_commit_committed_at`] reports for
    /// `project_code`.
    pub fn set_head_commit_committed_at(
        &self,
        project_code: &str,
        committed_at: impl Into<String>,
    ) {
        self.state
            .lock()
            .expect("fake forge mutex poisoned")
            .head_committed_ats
            .insert(project_code.to_string(), committed_at.into());
    }
}

impl Default for FakeForge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ForgeService for FakeForge {
    async fn find_repository(
        &self,
        project_code: &str,
    ) -> Result<Option<ForgeRepository>, ForgeError> {
        let state = self.state.lock().expect("fake forge mutex poisoned");
        Ok(state.repositories.get(project_code).cloned())
    }

    async fn ensure_repository(&self, project_code: &str) -> Result<ForgeRepository, ForgeError> {
        let mut state = self.state.lock().expect("fake forge mutex poisoned");
        state.ensure_calls += 1;
        if let Some(existing) = state.repositories.get(project_code) {
            return Ok(existing.clone());
        }
        let created = ForgeRepository {
            url: self.url_for(project_code),
            name: project_code.to_string(),
        };
        state
            .repositories
            .insert(project_code.to_string(), created.clone());
        Ok(created)
    }

    async fn head_commit_sha(&self, project_code: &str) -> Result<Option<String>, ForgeError> {
        let state = self.state.lock().expect("fake forge mutex poisoned");
        Ok(state.head_shas.get(project_code).cloned())
    }

    async fn head_commit_committed_at(
        &self,
        project_code: &str,
    ) -> Result<Option<String>, ForgeError> {
        let state = self.state.lock().expect("fake forge mutex poisoned");
        Ok(state.head_committed_ats.get(project_code).cloned())
    }

    async fn delete_repository(&self, project_code: &str) -> Result<(), ForgeError> {
        let mut state = self.state.lock().expect("fake forge mutex poisoned");
        state.repositories.remove(project_code);
        state.head_shas.remove(project_code);
        state.head_committed_ats.remove(project_code);
        Ok(())
    }
}

/// GitHub REST client that creates private repositories in one organization.
pub struct GitHubForge {
    api_base: String,
    organization: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for GitHubForge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubForge")
            .field("api_base", &self.api_base)
            .field("organization", &self.organization)
            .field("token", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl GitHubForge {
    /// Build from an already-resolved organization, token, and API base.
    #[must_use]
    pub fn new(organization: String, token: String, api_base: &str) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            organization,
            token,
            http: reqwest::Client::new(),
        }
    }

    /// Resolve the deployment's forge pair and a token from the environment.
    ///
    /// # Errors
    ///
    /// When the deployment pair is missing or the token is unset.
    pub fn from_env() -> Result<Self, ForgeError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// The lookup form so tests can supply a pair and a token without
    /// mutating process-global environment variables.
    pub fn from_lookup<F>(get: F) -> Result<Self, ForgeError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let config = WorkspaceConfig::from_lookup(&get)?;
        let token = get(NAVIGATOR_GITHUB_TOKEN_ENV)
            .or_else(|| get(GITHUB_TOKEN_ENV))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or(ForgeError::MissingConfig(NAVIGATOR_GITHUB_TOKEN_ENV))?;
        let api_base = get(GITHUB_API_BASE_ENV)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        if config.organization.is_empty() {
            return Err(ForgeError::MissingConfig(NAVIGATOR_GITHUB_ORG));
        }
        Ok(Self::new(config.organization, token, &api_base))
    }

    fn repos_url(&self, project_code: &str) -> String {
        format!(
            "{}/repos/{}/{project_code}",
            self.api_base, self.organization
        )
    }

    fn org_repos_url(&self) -> String {
        format!("{}/orgs/{}/repos", self.api_base, self.organization)
    }

    fn checked(
        response: Result<reqwest::Response, reqwest::Error>,
        action: &'static str,
    ) -> Result<reqwest::Response, ForgeError> {
        let response = response.map_err(|source| ForgeError::Request { action, source })?;
        let status = response.status().as_u16();
        if response.status().is_success() {
            return Ok(response);
        }
        if status == 401 {
            return Err(ForgeError::Authentication);
        }
        Err(ForgeError::Api { action, status })
    }

    async fn get_repository(
        &self,
        project_code: &str,
    ) -> Result<Option<ForgeRepository>, ForgeError> {
        self.get_repository_checked(project_code, false).await
    }

    /// [`Self::get_repository`], optionally failing closed when the live
    /// repository is not private. Provisioning uses the checked form both when
    /// it adopts an existing repository and when it adopts after a create
    /// conflict; ordinary lookups — used by callers with no provisioning
    /// stake, such as the reconciler's own drift report — must not start
    /// failing on a repository they are only reading.
    async fn get_repository_checked(
        &self,
        project_code: &str,
        verify_private: bool,
    ) -> Result<Option<ForgeRepository>, ForgeError> {
        let response = self
            .http
            .get(self.repos_url(project_code))
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().as_u16() == 404 => return Ok(None),
            other => Self::checked(other, "finding repository")?,
        };
        Ok(Some(
            parse_repository(response, "finding repository", verify_private).await?,
        ))
    }

    async fn create_repository(&self, project_code: &str) -> Result<ForgeRepository, ForgeError> {
        let created = self
            .http
            .post(self.org_repos_url())
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .json(&CreateRepository {
                name: project_code,
                private: true,
                auto_init: false,
            })
            .send()
            .await;
        if created
            .as_ref()
            .is_ok_and(|response| response.status().as_u16() == 422)
        {
            // Name already taken in the organization: adopt rather than fail.
            // Still verify visibility — adopting a repository that is not
            // private is the same failure a fresh create must catch.
            return self
                .get_repository_checked(project_code, true)
                .await?
                .ok_or(ForgeError::Api {
                    action: "creating repository",
                    status: 422,
                });
        }
        let response = Self::checked(created, "creating repository")?;
        // Fail closed rather than trusting the request was honoured: a policy
        // change underneath (`members_can_create_private_repositories` or a
        // similar org default flipping) can land a repository public even
        // though `private: true` was requested, and that is exactly the
        // failure worth catching before anything else configures it.
        parse_repository(response, "creating repository", true).await
    }

    /// The commit at the tip of the repository's default branch — the shared
    /// two-request fetch (resolve `default_branch`, then read its tip) behind
    /// both [`ForgeService::head_commit_sha`] and
    /// [`ForgeService::head_commit_committed_at`], so a caller wanting either
    /// fact makes the same two calls rather than each growing its own copy.
    async fn head_commit(&self, project_code: &str) -> Result<Option<CommitBody>, ForgeError> {
        let action = "reading repository for HEAD commit";
        let response = self
            .http
            .get(self.repos_url(project_code))
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().as_u16() == 404 => return Ok(None),
            other => Self::checked(other, action)?,
        };
        let detail = response
            .json::<RepositoryDetail>()
            .await
            .map_err(|source| ForgeError::Response { action, source })?;
        let branch = detail
            .default_branch
            .ok_or(ForgeError::MissingUrl { action })?;

        let action = "reading HEAD commit";
        let commit_url = format!(
            "{}/repos/{}/{project_code}/commits/{branch}",
            self.api_base, self.organization
        );
        let response = self
            .http
            .get(commit_url)
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = Self::checked(response, action)?;
        let commit = response
            .json::<CommitBody>()
            .await
            .map_err(|source| ForgeError::Response { action, source })?;
        Ok(Some(commit))
    }
}

/// GitHub's implementation of the Project pull-request delivery boundary.
pub struct GitHubPullRequestForge {
    api_base: String,
    graphql_api: String,
    owner: String,
    repository: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for GitHubPullRequestForge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubPullRequestForge")
            .field("api_base", &self.api_base)
            .field("graphql_api", &self.graphql_api)
            .field("owner", &self.owner)
            .field("repository", &self.repository)
            .field("token", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl GitHubPullRequestForge {
    /// Build a pull-request backend for one `owner/name` repository.
    pub fn new(
        repository: &str,
        token: String,
        api_base: &str,
        graphql_api: &str,
    ) -> Result<Self, ForgeError> {
        let Some((owner, name)) = repository.split_once('/') else {
            return Err(ForgeError::InvalidRepository(repository.to_string()));
        };
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return Err(ForgeError::InvalidRepository(repository.to_string()));
        }
        Ok(Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            graphql_api: graphql_api.trim_end_matches('/').to_string(),
            owner: owner.to_string(),
            repository: name.to_string(),
            token,
            http: reqwest::Client::new(),
        })
    }

    /// Resolve credentials and API endpoints from the process environment.
    pub fn from_env(repository: &str) -> Result<Self, ForgeError> {
        let token = std::env::var(NAVIGATOR_GITHUB_TOKEN_ENV)
            .ok()
            .or_else(|| std::env::var(GITHUB_TOKEN_ENV).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or(ForgeError::MissingConfig(NAVIGATOR_GITHUB_TOKEN_ENV))?;
        let api_base = std::env::var(GITHUB_API_BASE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        let graphql_api = std::env::var(GITHUB_GRAPHQL_API_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| graphql_endpoint(&api_base));
        Self::new(repository, token, &api_base, &graphql_api)
    }

    fn pulls_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/pulls",
            self.api_base, self.owner, self.repository
        )
    }

    fn branch_rules_url(&self, base: &str) -> String {
        format!(
            "{}/repos/{}/{}/rules/branches/{base}",
            self.api_base, self.owner, self.repository
        )
    }

    fn checked(
        response: Result<reqwest::Response, reqwest::Error>,
        action: &'static str,
    ) -> Result<reqwest::Response, ForgeError> {
        GitHubForge::checked(response, action)
    }

    async fn graphql(
        &self,
        operation: &'static str,
        query: &'static str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, ForgeError> {
        let response = Self::checked(
            self.http
                .post(&self.graphql_api)
                .bearer_auth(&self.token)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .timeout(REQUEST_TIMEOUT)
                .json(&serde_json::json!({
                    "operationName": operation,
                    "query": query,
                    "variables": variables,
                }))
                .send()
                .await,
            operation,
        )?;
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|source| ForgeError::Response {
                action: operation,
                source,
            })?;
        if let Some(message) = value
            .get("errors")
            .and_then(serde_json::Value::as_array)
            .and_then(|errors| errors.first())
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
        {
            return Err(ForgeError::Graphql {
                action: operation,
                message: message.to_string(),
            });
        }
        Ok(value)
    }
}

fn graphql_endpoint(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    base.strip_suffix("/api/v3").map_or_else(
        || format!("{base}/graphql"),
        |enterprise| format!("{enterprise}/api/graphql"),
    )
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequestBody {
    number: u64,
    node_id: String,
    html_url: String,
}

#[derive(Serialize)]
struct CreatePullRequest<'a> {
    title: &'a str,
    body: &'a str,
    head: &'a str,
    base: &'a str,
}

#[derive(Debug, Deserialize)]
struct RepositoryRule {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    parameters: Option<RequiredStatusCheckParameters>,
}

#[derive(Debug, Deserialize)]
struct RequiredStatusCheckParameters {
    #[serde(default)]
    required_status_checks: Vec<RequiredStatusCheck>,
}

#[derive(Debug, Deserialize)]
struct RequiredStatusCheck {
    context: String,
}

#[async_trait]
impl PullRequestForge for GitHubPullRequestForge {
    async fn open_or_adopt_pull_request(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<ForgePullRequest, ForgeError> {
        let action = "finding pull request";
        let head = format!("{}:{branch}", self.owner);
        let response = Self::checked(
            self.http
                .get(self.pulls_url())
                .bearer_auth(&self.token)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .timeout(REQUEST_TIMEOUT)
                .query(&[("state", "open"), ("head", head.as_str()), ("base", base)])
                .send()
                .await,
            action,
        )?;
        let mut existing = response
            .json::<Vec<GitHubPullRequestBody>>()
            .await
            .map_err(|source| ForgeError::Response { action, source })?;
        let pull_request = if let Some(existing) = existing.pop() {
            existing
        } else {
            let action = "opening pull request";
            let response = Self::checked(
                self.http
                    .post(self.pulls_url())
                    .bearer_auth(&self.token)
                    .header(reqwest::header::USER_AGENT, USER_AGENT)
                    .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                    .timeout(REQUEST_TIMEOUT)
                    .json(&CreatePullRequest {
                        title,
                        body,
                        head: branch,
                        base,
                    })
                    .send()
                    .await,
                action,
            )?;
            response
                .json::<GitHubPullRequestBody>()
                .await
                .map_err(|source| ForgeError::Response { action, source })?
        };
        Ok(ForgePullRequest {
            number: pull_request.number,
            node_id: pull_request.node_id,
            url: pull_request.html_url,
        })
    }

    async fn enable_auto_merge(&self, pull_request_node_id: &str) -> Result<(), ForgeError> {
        const QUERY: &str = "mutation EnableAutoMerge($id: ID!) { enablePullRequestAutoMerge(input: { pullRequestId: $id, mergeMethod: SQUASH }) { pullRequest { id } } }";
        self.graphql(
            "EnableAutoMerge",
            QUERY,
            serde_json::json!({ "id": pull_request_node_id }),
        )
        .await?;
        Ok(())
    }

    async fn pull_request_status(
        &self,
        pull_request_number: u64,
    ) -> Result<ForgePullRequestStatus, ForgeError> {
        const ACTION: &str = "PullRequestStatus";
        const QUERY: &str = "query PullRequestStatus($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { merged autoMergeRequest { enabledAt } mergeStateStatus reviewDecision commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes { __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } } } }";
        let value = self
            .graphql(
                ACTION,
                QUERY,
                serde_json::json!({
                    "owner": self.owner,
                    "name": self.repository,
                    "number": pull_request_number,
                }),
            )
            .await?;
        let pull_request = value
            .pointer("/data/repository/pullRequest")
            .ok_or(ForgeError::ResponseShape { action: ACTION })?;
        let merged = pull_request
            .get("merged")
            .and_then(serde_json::Value::as_bool)
            .ok_or(ForgeError::ResponseShape { action: ACTION })?;
        let merge_state = pull_request
            .get("mergeStateStatus")
            .and_then(serde_json::Value::as_str)
            .ok_or(ForgeError::ResponseShape { action: ACTION })?
            .to_string();
        let review_decision = pull_request
            .get("reviewDecision")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let auto_merge_armed = pull_request
            .get("autoMergeRequest")
            .is_some_and(|request| !request.is_null());
        let mut checks = Vec::new();
        if let Some(nodes) = pull_request
            .pointer("/commits/nodes/0/commit/statusCheckRollup/contexts/nodes")
            .and_then(serde_json::Value::as_array)
        {
            for node in nodes {
                match node.get("__typename").and_then(serde_json::Value::as_str) {
                    Some("CheckRun") => {
                        if let (Some(name), Some(status)) = (
                            node.get("name").and_then(serde_json::Value::as_str),
                            node.get("status").and_then(serde_json::Value::as_str),
                        ) {
                            checks.push(ForgeCheck {
                                name: name.to_string(),
                                status: status.to_string(),
                                conclusion: node
                                    .get("conclusion")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string),
                            });
                        }
                    }
                    Some("StatusContext") => {
                        if let (Some(name), Some(state)) = (
                            node.get("context").and_then(serde_json::Value::as_str),
                            node.get("state").and_then(serde_json::Value::as_str),
                        ) {
                            checks.push(ForgeCheck {
                                name: name.to_string(),
                                status: state.to_string(),
                                conclusion: Some(state.to_string()),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(ForgePullRequestStatus {
            merged,
            auto_merge_armed,
            merge_state,
            review_decision,
            checks,
        })
    }

    async fn required_checks(&self, base: &str) -> Result<Vec<String>, ForgeError> {
        let action = "reading required checks";
        let response = Self::checked(
            self.http
                .get(self.branch_rules_url(base))
                .bearer_auth(&self.token)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await,
            action,
        )?;
        let rules = response
            .json::<Vec<RepositoryRule>>()
            .await
            .map_err(|source| ForgeError::Response { action, source })?;
        let mut checks = rules
            .into_iter()
            .filter(|rule| rule.kind == "required_status_checks")
            .filter_map(|rule| rule.parameters)
            .flat_map(|parameters| parameters.required_status_checks)
            .map(|check| check.context)
            .collect::<Vec<_>>();
        checks.sort();
        checks.dedup();
        Ok(checks)
    }
}

#[derive(Serialize)]
struct CreateRepository<'a> {
    name: &'a str,
    private: bool,
    auto_init: bool,
}

#[derive(Deserialize)]
struct RepositoryBody {
    html_url: Option<String>,
    name: Option<String>,
    #[serde(default)]
    private: Option<bool>,
}

#[derive(Deserialize)]
struct RepositoryDetail {
    default_branch: Option<String>,
}

#[derive(Deserialize)]
struct CommitBody {
    sha: String,
    #[serde(default)]
    commit: Option<CommitDetail>,
}

#[derive(Deserialize)]
struct CommitDetail {
    #[serde(default)]
    committer: Option<CommitPerson>,
}

#[derive(Deserialize)]
struct CommitPerson {
    #[serde(default)]
    date: Option<String>,
}

async fn parse_repository(
    response: reqwest::Response,
    action: &'static str,
    verify_private: bool,
) -> Result<ForgeRepository, ForgeError> {
    let body = response
        .json::<RepositoryBody>()
        .await
        .map_err(|source| ForgeError::Response { action, source })?;
    let url = body.html_url.ok_or(ForgeError::MissingUrl { action })?;
    let name = body.name.ok_or(ForgeError::MissingUrl { action })?;
    if verify_private && body.private != Some(true) {
        return Err(ForgeError::NotPrivate { name });
    }
    Ok(ForgeRepository { url, name })
}

#[async_trait]
impl ForgeService for GitHubForge {
    async fn find_repository(
        &self,
        project_code: &str,
    ) -> Result<Option<ForgeRepository>, ForgeError> {
        self.get_repository(project_code).await
    }

    async fn ensure_repository(&self, project_code: &str) -> Result<ForgeRepository, ForgeError> {
        if let Some(existing) = self.get_repository_checked(project_code, true).await? {
            tracing::info!(
                project_code,
                repository_url = existing.url.as_str(),
                outcome = "already_exists",
                "Project repository provisioning completed"
            );
            return Ok(existing);
        }
        let created = self.create_repository(project_code).await?;
        tracing::info!(
            project_code,
            repository_url = created.url.as_str(),
            outcome = "created",
            "Project repository provisioning completed"
        );
        Ok(created)
    }

    async fn head_commit_sha(&self, project_code: &str) -> Result<Option<String>, ForgeError> {
        Ok(self
            .head_commit(project_code)
            .await?
            .map(|commit| commit.sha))
    }

    async fn head_commit_committed_at(
        &self,
        project_code: &str,
    ) -> Result<Option<String>, ForgeError> {
        Ok(self
            .head_commit(project_code)
            .await?
            .and_then(|commit| commit.commit)
            .and_then(|detail| detail.committer)
            .and_then(|person| person.date))
    }

    async fn delete_repository(&self, project_code: &str) -> Result<(), ForgeError> {
        let response = self
            .http
            .delete(self.repos_url(project_code))
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        Self::checked(response, "deleting repository")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FakeForge, ForgeService, GitHubForge, GITHUB_API_BASE_ENV, GITHUB_TOKEN_ENV,
        NAVIGATOR_GITHUB_TOKEN_ENV,
    };
    use crate::workspace::{NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_ORG, NAVIGATOR_GIT_HOST};
    use serde_json::json;
    use std::collections::HashMap;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[tokio::test]
    async fn fake_forge_is_idempotent_on_the_project_code() {
        let forge = FakeForge::new();
        let first = forge.ensure_repository("acme").await.unwrap();
        let second = forge.ensure_repository("acme").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first.name, "acme");
        assert_eq!(first.url, "https://forge.example/an-organization/acme");
        assert_eq!(forge.repository_count(), 1);
        assert_eq!(forge.ensure_calls(), 2);
    }

    #[tokio::test]
    async fn fake_forge_reports_no_head_sha_until_one_is_set() {
        let forge = FakeForge::new();
        forge.ensure_repository("acme").await.unwrap();
        assert_eq!(forge.head_commit_sha("acme").await.unwrap(), None);
        forge.set_head_commit_sha("acme", "deadbeef");
        assert_eq!(
            forge.head_commit_sha("acme").await.unwrap(),
            Some("deadbeef".to_string())
        );
    }

    #[tokio::test]
    async fn fake_forge_reports_no_committed_at_until_one_is_set() {
        let forge = FakeForge::new();
        forge.ensure_repository("acme").await.unwrap();
        assert_eq!(forge.head_commit_committed_at("acme").await.unwrap(), None);
        forge.set_head_commit_committed_at("acme", "2026-09-01T12:00:00Z");
        assert_eq!(
            forge.head_commit_committed_at("acme").await.unwrap(),
            Some("2026-09-01T12:00:00Z".to_string())
        );
    }

    #[tokio::test]
    async fn fake_forge_delete_removes_the_repository_and_its_head_sha() {
        let forge = FakeForge::new();
        forge.ensure_repository("acme").await.unwrap();
        forge.set_head_commit_sha("acme", "deadbeef");
        assert_eq!(forge.repository_count(), 1);

        forge.delete_repository("acme").await.unwrap();

        assert_eq!(forge.repository_count(), 0);
        assert_eq!(forge.head_commit_sha("acme").await.unwrap(), None);
        assert_eq!(forge.find_repository("acme").await.unwrap(), None);
    }

    #[test]
    fn github_forge_requires_a_token_and_a_deployment_pair() {
        let error = GitHubForge::from_lookup(lookup(&[(NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg")]))
            .unwrap_err();
        assert!(error.to_string().contains(NAVIGATOR_GITHUB_ORG), "{error}");

        let error = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
        ]))
        .unwrap_err();
        assert!(
            error.to_string().contains(NAVIGATOR_GITHUB_TOKEN_ENV),
            "{error}"
        );
    }

    #[tokio::test]
    async fn github_forge_adopts_an_existing_private_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": true
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GIT_HOST, "forge.example"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let repo = forge.ensure_repository("acme").await.unwrap();
        assert_eq!(repo.name, "acme");
        assert_eq!(repo.url, "https://forge.example/an-organization/acme");
    }

    #[tokio::test]
    async fn github_forge_fails_closed_when_an_existing_repository_is_not_private() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": false
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");

        let error = forge.ensure_repository("acme").await.unwrap_err();
        assert!(error.to_string().contains("acme"), "{error}");
        assert!(error.to_string().contains("not private"), "{error}");
    }

    #[tokio::test]
    async fn github_forge_creates_a_private_repository_when_none_exists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/an-organization/repos"))
            .and(body_json(json!({
                "name": "acme",
                "private": true,
                "auto_init": false
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": true
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let repo = forge.ensure_repository("acme").await.unwrap();
        assert_eq!(repo.url, "https://forge.example/an-organization/acme");
    }

    #[tokio::test]
    async fn github_forge_fails_closed_when_a_created_repository_is_not_private() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/an-organization/repos"))
            .and(body_json(json!({
                "name": "acme",
                "private": true,
                "auto_init": false
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": false
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");

        let error = forge.ensure_repository("acme").await.unwrap_err();
        assert!(error.to_string().contains("acme"), "{error}");
        assert!(error.to_string().contains("not private"), "{error}");
    }

    #[tokio::test]
    async fn github_forge_fails_closed_when_an_adopted_repository_is_not_private() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/an-organization/repos"))
            .respond_with(ResponseTemplate::new(422))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": false
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");

        let error = forge.ensure_repository("acme").await.unwrap_err();
        assert!(error.to_string().contains("acme"), "{error}");
        assert!(error.to_string().contains("not private"), "{error}");
    }

    #[tokio::test]
    async fn github_forge_adopts_when_create_reports_the_name_is_taken() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/an-organization/repos"))
            .respond_with(ResponseTemplate::new(422))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "private": true
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let repo = forge.ensure_repository("acme").await.unwrap();
        assert_eq!(repo.url, "https://forge.example/an-organization/acme");
    }

    #[tokio::test]
    async fn github_forge_reads_the_default_branchs_head_sha() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "default_branch": "main"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme/commits/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "deadbeefcafe"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let sha = forge.head_commit_sha("acme").await.unwrap();
        assert_eq!(sha, Some("deadbeefcafe".to_string()));
    }

    #[tokio::test]
    async fn github_forge_reads_the_default_branchs_head_commit_committed_at() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "html_url": "https://forge.example/an-organization/acme",
                "name": "acme",
                "default_branch": "main"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/acme/commits/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "deadbeefcafe",
                "commit": { "committer": { "date": "2026-09-01T12:00:00Z" } }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let committed_at = forge.head_commit_committed_at("acme").await.unwrap();
        assert_eq!(committed_at, Some("2026-09-01T12:00:00Z".to_string()));
    }

    #[tokio::test]
    async fn github_forge_head_sha_is_none_for_a_repository_that_does_not_exist() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/an-organization/ghost"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        let sha = forge.head_commit_sha("ghost").await.unwrap();
        assert_eq!(sha, None);
    }

    #[tokio::test]
    async fn github_forge_deletes_the_repository() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/repos/an-organization/acme"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let forge = GitHubForge::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "an-organization"),
            (NAVIGATOR_GITHUB_TOKEN_ENV, "test-token"),
            (GITHUB_API_BASE_ENV, server.uri().as_str()),
        ]))
        .expect("configured forge");
        forge.delete_repository("acme").await.unwrap();
    }
}
