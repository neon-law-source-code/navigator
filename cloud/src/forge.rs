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
pub const GITHUB_API_BASE_ENV: &str = "NAVIGATOR_GITHUB_API_BASE";
/// Public GitHub's REST API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("neon-law-navigator/", env!("CARGO_PKG_VERSION"));

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
}

/// Create or adopt one private repository named for a Project code.
///
/// The trait is deliberately narrow. Adding a collaborator method here would
/// make Project participation a back door onto the forge, which
/// [`docs/project-repositories.md`] forbids.
#[async_trait]
pub trait ForgeService: Send + Sync {
    async fn find_repository(
        &self,
        project_code: &str,
    ) -> Result<Option<ForgeRepository>, ForgeError>;
    async fn ensure_repository(&self, project_code: &str) -> Result<ForgeRepository, ForgeError>;
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
        let response = self
            .http
            .get(self.repos_url(project_code))
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().as_u16() == 404 => return Ok(None),
            other => Self::checked(other, "finding repository")?,
        };
        Ok(Some(
            parse_repository(response, "finding repository").await?,
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
            return self
                .get_repository(project_code)
                .await?
                .ok_or(ForgeError::Api {
                    action: "creating repository",
                    status: 422,
                });
        }
        let response = Self::checked(created, "creating repository")?;
        parse_repository(response, "creating repository").await
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
}

async fn parse_repository(
    response: reqwest::Response,
    action: &'static str,
) -> Result<ForgeRepository, ForgeError> {
    let body = response
        .json::<RepositoryBody>()
        .await
        .map_err(|source| ForgeError::Response { action, source })?;
    let url = body.html_url.ok_or(ForgeError::MissingUrl { action })?;
    let name = body.name.ok_or(ForgeError::MissingUrl { action })?;
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
        if let Some(existing) = self.get_repository(project_code).await? {
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
}
