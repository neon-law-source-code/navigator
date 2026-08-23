//! GitHub webhook ingress for the `DevX` issue-to-PR loop.
//!
//! The public receiver verifies GitHub's signature over the raw request bytes,
//! applies repository and event trust guards, then submits a compact typed
//! command to Restate. It never logs or forwards issue, review, or comment
//! bodies: downstream workers retrieve the scoped GitHub resource themselves.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use thiserror::Error;

pub mod authority;
pub mod github;
pub mod guardrails;
pub mod harness;
pub mod runner;
pub mod sig;
pub mod triage_job;
pub mod worker;

const SIGNATURE_HEADER: &str = "x-hub-signature-256";
const EVENT_HEADER: &str = "x-github-event";
const DELIVERY_HEADER: &str = "x-github-delivery";

/// Configuration used by the pure event router.
#[derive(Debug, Clone, Copy)]
pub struct RouterConfig<'a> {
    /// The product code repository, always watched.
    pub canonical_repository: &'a str,
    /// The org whose private repositories each back a Project. Every repo it
    /// owns is watched (`neon-law-firm`), so a new Project's repo needs no
    /// config change.
    pub github_org: &'a str,
    pub app_login: &'a str,
}

/// Runtime configuration for the receiver, mounted as a route in `web`.
#[derive(Clone)]
pub struct ReceiverConfig {
    webhook_secret: String,
    canonical_repository: String,
    github_org: String,
    app_login: String,
    restate_ingress_url: String,
    restate_auth_token: String,
}

impl ReceiverConfig {
    /// Load the receiver's routing configuration and secrets.
    ///
    /// # Errors
    ///
    /// Returns every required environment variable that was not set.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(|name| std::env::var(name).ok())
    }

    fn from_values(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let required = [
            "NAVIGATOR_GCP_PROJECT_ID",
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
            "NAVIGATOR_GITHUB_ORG",
            "NAVIGATOR_GITHUB_APP_LOGIN",
            "RESTATE_INGRESS_URL",
            "RESTATE_AUTH_TOKEN",
        ];
        let missing: Vec<&str> = required
            .into_iter()
            .filter(|name| get(name).is_none_or(|value| value.is_empty()))
            .collect();
        if !missing.is_empty() {
            return Err(ConfigError::Missing(missing.join(", ")));
        }

        if !authority::is_automation_home(get("NAVIGATOR_GCP_PROJECT_ID").as_deref()) {
            return Err(ConfigError::NotAutomationHome);
        }

        Ok(Self {
            webhook_secret: get("NAVIGATOR_GITHUB_WEBHOOK_SECRET")
                .expect("checked required webhook secret"),
            canonical_repository: get("NAVIGATOR_GITHUB_CANONICAL_REPOSITORY")
                .expect("checked required canonical repository"),
            github_org: get("NAVIGATOR_GITHUB_ORG").expect("checked required github org"),
            app_login: get("NAVIGATOR_GITHUB_APP_LOGIN").expect("checked required app login"),
            restate_ingress_url: get("RESTATE_INGRESS_URL")
                .expect("checked required Restate ingress URL"),
            restate_auth_token: get("RESTATE_AUTH_TOKEN")
                .expect("checked required Restate auth token"),
        })
    }

    #[must_use]
    pub fn app_state(&self) -> AppState {
        AppState::new(
            self.webhook_secret.as_bytes(),
            RouterSettings::new(
                self.canonical_repository.clone(),
                self.github_org.clone(),
                self.app_login.clone(),
            ),
            Arc::new(RestateSubmitter::new(
                self.restate_ingress_url.clone(),
                self.restate_auth_token.clone(),
            )),
        )
    }
}

/// Startup configuration failures. The webhook secret is never rendered.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("required environment variables are missing: {0}")]
    Missing(String),
    #[error("GitHub engineering automation is only enabled in neon-law-stg")]
    NotAutomationHome,
}

#[derive(Debug, Clone)]
pub struct RouterSettings {
    canonical_repository: String,
    github_org: String,
    app_login: String,
}

impl RouterSettings {
    #[must_use]
    pub fn new(canonical_repository: String, github_org: String, app_login: String) -> Self {
        Self {
            canonical_repository,
            github_org,
            app_login,
        }
    }

    fn as_config(&self) -> RouterConfig<'_> {
        RouterConfig {
            canonical_repository: &self.canonical_repository,
            github_org: &self.github_org,
            app_login: &self.app_login,
        }
    }
}

/// A GitHub event header understood by this ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubEvent {
    Issues,
    IssueComment,
    PullRequestReviewComment,
    PullRequestReview,
    CheckRun,
    WorkflowRun,
    Unknown,
}

impl GithubEvent {
    #[must_use]
    pub fn from_header(value: &str) -> Self {
        match value {
            "issues" => Self::Issues,
            "issue_comment" => Self::IssueComment,
            "pull_request_review_comment" => Self::PullRequestReviewComment,
            "pull_request_review" => Self::PullRequestReview,
            "check_run" => Self::CheckRun,
            "workflow_run" => Self::WorkflowRun,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issues => "issues",
            Self::IssueComment => "issue_comment",
            Self::PullRequestReviewComment => "pull_request_review_comment",
            Self::PullRequestReview => "pull_request_review",
            Self::CheckRun => "check_run",
            Self::WorkflowRun => "workflow_run",
            Self::Unknown => "unknown",
        }
    }
}

/// The compact, body-free command submitted to a durable `DevX` workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    service: &'static str,
    key: String,
    handler: &'static str,
    body: Value,
}

impl Route {
    #[must_use]
    pub const fn service(&self) -> &'static str {
        self.service
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub const fn handler(&self) -> &'static str {
        self.handler
    }

    #[must_use]
    pub const fn body(&self) -> &Value {
        &self.body
    }
}

/// A safe no-op outcome for an accepted GitHub delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    WrongRepository,
    UntrustedActor,
    OwnApp,
    NotTriage,
    AlreadyTriaged,
    NotPullRequest,
    NotFailure,
    UnhandledAction,
    ForkHeadRepository,
    UnknownEvent,
}

/// The pure router's result.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    Route(Route),
    Ignore(IgnoreReason),
}

impl RouteDecision {
    #[must_use]
    pub fn service(&self) -> &'static str {
        match self {
            Self::Route(route) => route.service(),
            Self::Ignore(_) => "",
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Route(route) => route.key(),
            Self::Ignore(_) => "",
        }
    }

    #[must_use]
    pub fn handler(&self) -> &'static str {
        match self {
            Self::Route(route) => route.handler(),
            Self::Ignore(_) => "",
        }
    }

    #[must_use]
    pub const fn ignore_reason(&self) -> Option<IgnoreReason> {
        match self {
            Self::Route(_) => None,
            Self::Ignore(reason) => Some(*reason),
        }
    }
}

/// A payload had no required routing field. Its content is never included.
#[derive(Debug, Error)]
pub enum RouterError {
    #[error("malformed GitHub payload: missing or invalid {0}")]
    Malformed(&'static str),
}

/// Route one verified GitHub delivery without performing I/O.
///
/// The command body contains only durable identifiers and event metadata. In
/// particular, it deliberately excludes GitHub's user-authored free-text.
///
/// # Errors
///
/// Returns [`RouterError::Malformed`] if a supported event lacks fields needed
/// to decide its route.
pub fn route(
    config: &RouterConfig<'_>,
    delivery_id: &str,
    event: GithubEvent,
    action: &str,
    payload: &Value,
) -> Result<RouteDecision, RouterError> {
    let repository = string_at(payload, "/repository/full_name", "repository.full_name")?;
    if !repository_is_watched(repository, config) {
        return Ok(RouteDecision::Ignore(IgnoreReason::WrongRepository));
    }
    if event == GithubEvent::Unknown {
        return Ok(RouteDecision::Ignore(IgnoreReason::UnknownEvent));
    }

    let sender = string_at(payload, "/sender/login", "sender.login")?;
    let own_triaged_label = event == GithubEvent::Issues
        && action == "labeled"
        && payload
            .pointer("/label/name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == "triaged");
    // The App's `triaged` label is the explicit triage-to-implementation
    // chain link. Every other self-generated delivery is an echo and must not
    // begin another workflow.
    if sender == config.app_login && !own_triaged_label {
        return Ok(RouteDecision::Ignore(IgnoreReason::OwnApp));
    }

    match event {
        GithubEvent::Issues => route_issue(repository, delivery_id, action, payload),
        GithubEvent::IssueComment => {
            // Only a newly created comment is a signal. `edited`, `deleted`,
            // `pinned`, and `unpinned` mutate an existing comment and must not
            // re-signal `devx-pr`.
            if action != "created" {
                return Ok(RouteDecision::Ignore(IgnoreReason::UnhandledAction));
            }
            if payload.pointer("/issue/pull_request").is_none() {
                return Ok(RouteDecision::Ignore(IgnoreReason::NotPullRequest));
            }
            Ok(pr_route(
                repository,
                delivery_id,
                event,
                number_at(payload, "/issue/number", "issue.number")?,
            ))
        }
        GithubEvent::PullRequestReviewComment => {
            // As with issue comments, only a freshly created review comment is a
            // signal; edits and deletions must not re-signal.
            if action != "created" {
                return Ok(RouteDecision::Ignore(IgnoreReason::UnhandledAction));
            }
            Ok(pr_route(
                repository,
                delivery_id,
                event,
                number_at(payload, "/pull_request/number", "pull_request.number")?,
            ))
        }
        GithubEvent::PullRequestReview => {
            if action != "submitted" {
                return Ok(RouteDecision::Ignore(IgnoreReason::UnhandledAction));
            }
            let state = string_at(payload, "/review/state", "review.state")?;
            if !matches!(state, "changes_requested" | "commented") {
                return Ok(RouteDecision::Ignore(IgnoreReason::NotFailure));
            }
            Ok(pr_route(
                repository,
                delivery_id,
                event,
                number_at(payload, "/pull_request/number", "pull_request.number")?,
            ))
        }
        GithubEvent::CheckRun => failure_route(
            config,
            repository,
            delivery_id,
            event,
            action,
            payload,
            "/check_run/conclusion",
            "/check_run/pull_requests/0/head/repo",
            "/check_run/pull_requests/0/number",
        ),
        GithubEvent::WorkflowRun => failure_route(
            config,
            repository,
            delivery_id,
            event,
            action,
            payload,
            "/workflow_run/conclusion",
            "/workflow_run/head_repository",
            "/workflow_run/pull_requests/0/number",
        ),
        GithubEvent::Unknown => Ok(RouteDecision::Ignore(IgnoreReason::UnknownEvent)),
    }
}

/// A delivery is watched when it targets the canonical product code
/// repository (`neon-law-source-code/navigator`) or any repository owned by the
/// Project org (`github_org` = `neon-law-firm`), each of which backs a Project.
/// The owner match is exact on the `owner/` segment so a look-alike owner
/// (`neon-law-firm-evil/x`) cannot spoof it.
fn repository_is_watched(repository: &str, config: &RouterConfig<'_>) -> bool {
    repository == config.canonical_repository
        || repository
            .split_once('/')
            .is_some_and(|(owner, _)| owner == config.github_org)
}

/// A Restate object-key segment for a repository: the `owner/repo` full name
/// with `/` replaced, so it is safe inside the ingress URL path
/// (`{ingress}/{Service}/{key}/{handler}`).
fn repo_key(repository: &str) -> String {
    repository.replace('/', "__")
}

fn route_issue(
    repository: &str,
    delivery_id: &str,
    action: &str,
    payload: &Value,
) -> Result<RouteDecision, RouterError> {
    let issue = number_at(payload, "/issue/number", "issue.number")?;
    let labels = payload
        .pointer("/issue/labels")
        .and_then(Value::as_array)
        .ok_or(RouterError::Malformed("issue.labels"))?;
    let has_label = |wanted| labels.iter().any(|label| label_name(label) == Some(wanted));

    match action {
        "opened" | "edited" => {
            if has_label("triaged") {
                return Ok(RouteDecision::Ignore(IgnoreReason::AlreadyTriaged));
            }
            if !has_label("triage") {
                return Ok(RouteDecision::Ignore(IgnoreReason::NotTriage));
            }
            let association = string_at(
                payload,
                "/issue/author_association",
                "issue.author_association",
            )?;
            if !trusted(association) {
                return Ok(RouteDecision::Ignore(IgnoreReason::UntrustedActor));
            }
            Ok(issue_route(
                repository,
                "DevxIssueTriage",
                "triage",
                issue,
                delivery_id,
            ))
        }
        // A `labeled` delivery is only sent when someone with write (or triage)
        // access applies the label — GitHub itself refuses the API call for
        // anyone else, and the webhook's `sender` object carries no
        // `author_association` field to re-check. So the delivery already proves
        // a trusted actor; the trust here rides on GitHub's write-access gate.
        "labeled" => {
            let label = string_at(payload, "/label/name", "label.name")?;
            if label == "triaged" {
                return Ok(issue_route(
                    repository,
                    "DevxImplementIssue",
                    "implement",
                    issue,
                    delivery_id,
                ));
            }
            if label != "triage" {
                return Ok(RouteDecision::Ignore(IgnoreReason::NotTriage));
            }
            if has_label("triaged") {
                return Ok(RouteDecision::Ignore(IgnoreReason::AlreadyTriaged));
            }
            Ok(issue_route(
                repository,
                "DevxIssueTriage",
                "triage",
                issue,
                delivery_id,
            ))
        }
        _ => Ok(RouteDecision::Ignore(IgnoreReason::NotTriage)),
    }
}

#[allow(clippy::too_many_arguments)]
fn failure_route(
    config: &RouterConfig<'_>,
    repository: &str,
    delivery_id: &str,
    event: GithubEvent,
    action: &str,
    payload: &Value,
    conclusion_pointer: &str,
    head_repo_pointer: &str,
    pr_pointer: &str,
) -> Result<RouteDecision, RouterError> {
    if action != "completed"
        || payload.pointer(conclusion_pointer).and_then(Value::as_str) != Some("failure")
    {
        return Ok(RouteDecision::Ignore(IgnoreReason::NotFailure));
    }
    let Some(number) = payload.pointer(pr_pointer).and_then(Value::as_u64) else {
        return Ok(RouteDecision::Ignore(IgnoreReason::NotPullRequest));
    };
    // A fork can reference a canonical-repository PR number in a signed
    // failure delivery. The head repository is the field that distinguishes
    // its branch from a canonical branch, so require it before notifying.
    let head_repo = payload
        .pointer(head_repo_pointer)
        .ok_or(RouterError::Malformed("head repository"))?;
    if !head_repo_is_trusted(head_repo, config) {
        return Ok(RouteDecision::Ignore(IgnoreReason::ForkHeadRepository));
    }
    Ok(pr_route(repository, delivery_id, event, number))
}

/// True when a payload's head-repository object identifies a watched repo (the
/// canonical code repo or a `github_org`-owned Project repo).
///
/// GitHub renders a repository's identity two ways across event payloads: the
/// `full_name` (`owner/repo`) on richer objects like `workflow_run`, and only a
/// minimal `{ name, url }` on the pull-request refs inside `check_run`. The API
/// `url` ends with the same `owner/repo`, so it distinguishes a fork (whose
/// `url` tail is `fork-owner/repo`) from a watched repository.
fn head_repo_is_trusted(head_repo: &Value, config: &RouterConfig<'_>) -> bool {
    if let Some(full_name) = head_repo.get("full_name").and_then(Value::as_str) {
        return repository_is_watched(full_name, config);
    }
    head_repo
        .get("url")
        .and_then(Value::as_str)
        .and_then(|url| url.rsplit_once("/repos/"))
        .is_some_and(|(_, tail)| repository_is_watched(tail, config))
}

fn issue_route(
    repository: &str,
    service: &'static str,
    prefix: &str,
    issue: u64,
    delivery_id: &str,
) -> RouteDecision {
    RouteDecision::Route(Route {
        service,
        // Delivery identity makes redeliveries converge, while repository
        // identity keeps future Project-scoped workflows distinct even when
        // GitHub issue numbers happen to match across repositories.
        key: format!("{prefix}-{}-{issue}-{delivery_id}", repo_key(repository)),
        handler: "run",
        body: json!({
            "repository": repository,
            "issue_number": issue,
            "delivery_id": delivery_id,
        }),
    })
}

fn pr_route(repository: &str, delivery_id: &str, event: GithubEvent, number: u64) -> RouteDecision {
    RouteDecision::Route(Route {
        service: "devx-pr",
        // The `devx-pr` object is serialized per key; keying by PR number alone
        // would collide across repositories (every repo has a PR #12), so
        // qualify the key with the repository.
        key: format!("{}-{number}", repo_key(repository)),
        handler: "signal",
        body: json!({
            "repository": repository,
            "pull_request_number": number,
            "delivery_id": delivery_id,
            "event": event.as_str(),
        }),
    })
}

fn string_at<'a>(
    payload: &'a Value,
    pointer: &str,
    field: &'static str,
) -> Result<&'a str, RouterError> {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or(RouterError::Malformed(field))
}

fn number_at(payload: &Value, pointer: &str, field: &'static str) -> Result<u64, RouterError> {
    payload
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or(RouterError::Malformed(field))
}

fn label_name(label: &Value) -> Option<&str> {
    label.get("name").and_then(Value::as_str)
}

fn trusted(association: &str) -> bool {
    matches!(association, "OWNER" | "MEMBER" | "COLLABORATOR")
}

/// Submission seam kept separate from HTTP and routing for later worker work.
#[async_trait]
pub trait WorkflowSubmitter: Send + Sync {
    /// Submit an already-authorized route to Restate.
    async fn submit(&self, route: &Route) -> Result<(), SubmissionError>;
}

/// Restate ingress adapter. Carries the Restate Cloud ingress bearer so
/// submissions authenticate — an unauthenticated submit against Cloud is
/// rejected with a 401.
pub struct RestateSubmitter {
    ingress_url: String,
    auth_token: String,
}

impl RestateSubmitter {
    #[must_use]
    pub fn new(ingress_url: String, auth_token: String) -> Self {
        Self {
            ingress_url,
            auth_token,
        }
    }
}

#[async_trait]
impl WorkflowSubmitter for RestateSubmitter {
    async fn submit(&self, route: &Route) -> Result<(), SubmissionError> {
        workflows::start_workflow(
            &self.ingress_url,
            Some(&self.auth_token),
            route.service(),
            route.key(),
            route.handler(),
            route.body(),
            true,
        )
        .await?;
        Ok(())
    }
}

/// The ingress failed to hand a command to Restate.
#[derive(Debug, Error)]
pub enum SubmissionError {
    #[error("workflow ingress submission failed")]
    Trigger(#[from] workflows::TriggerError),
}

/// HTTP state. The webhook secret is kept only in memory and is never logged.
#[derive(Clone)]
pub struct AppState {
    webhook_secret: Arc<[u8]>,
    settings: RouterSettings,
    submitter: Arc<dyn WorkflowSubmitter>,
}

impl AppState {
    #[must_use]
    pub fn new(
        webhook_secret: impl AsRef<[u8]>,
        settings: RouterSettings,
        submitter: Arc<dyn WorkflowSubmitter>,
    ) -> Self {
        Self {
            webhook_secret: Arc::from(webhook_secret.as_ref()),
            settings,
            submitter,
        }
    }
}

/// Build the standalone receiver (used in tests and local runs). In production
/// the webhook route is mounted into `web` via [`webhook_routes`].
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .merge(webhook_routes(state))
}

/// The GitHub webhook route to mount into a host application (`web`). GitHub
/// posts to `https://www.<domain>/webhooks/github/{secret}`.
pub fn webhook_routes(state: AppState) -> Router {
    Router::new()
        .route("/webhooks/github/{secret}", post(github_webhook))
        .with_state(state)
}

async fn github_webhook(
    Path(path_secret): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if !constant_time_eq(path_secret.as_bytes(), &state.webhook_secret) {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(signature) = header(&headers, SIGNATURE_HEADER) else {
        return StatusCode::UNAUTHORIZED;
    };
    if !sig::verify_hmac_sha256_hex(&state.webhook_secret, &body, signature) {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(event) = header(&headers, EVENT_HEADER) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(delivery_id) =
        header(&headers, DELIVERY_HEADER).and_then(|value| uuid::Uuid::parse_str(value).ok())
    else {
        return StatusCode::BAD_REQUEST;
    };
    let delivery_id = delivery_id.to_string();
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let event = GithubEvent::from_header(event);
    // GitHub includes the action in its signed JSON body; it is never trusted
    // until after the raw-body signature has verified above.
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    let Ok(decision) = route(
        &state.settings.as_config(),
        &delivery_id,
        event,
        action,
        &payload,
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    match decision {
        RouteDecision::Route(route) => match state.submitter.submit(&route).await {
            Ok(()) => {
                tracing::info!(
                    delivery_id,
                    event = event.as_str(),
                    service = route.service(),
                    key = route.key(),
                    "GitHub webhook command submitted"
                );
                StatusCode::ACCEPTED
            }
            Err(_) => StatusCode::BAD_GATEWAY,
        },
        RouteDecision::Ignore(IgnoreReason::WrongRepository | IgnoreReason::UntrustedActor) => {
            StatusCode::FORBIDDEN
        }
        RouteDecision::Ignore(_) => StatusCode::NO_CONTENT,
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        app, route, AppState, ConfigError, GithubEvent, IgnoreReason, ReceiverConfig,
        RestateSubmitter, RouteDecision, RouterConfig, RouterSettings,
    };
    use crate::sig::sign_hmac_sha256_hex;

    const SECRET: &str = "test-webhook-secret";
    const CONFIG: RouterConfig = RouterConfig {
        canonical_repository: "neon-law-source-code/navigator",
        github_org: "neon-law-firm",
        app_login: "navigator-devx[bot]",
    };

    fn required_configuration() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg".to_string()),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", SECRET.to_string()),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                CONFIG.canonical_repository.to_string(),
            ),
            ("NAVIGATOR_GITHUB_ORG", CONFIG.github_org.to_string()),
            ("NAVIGATOR_GITHUB_APP_LOGIN", CONFIG.app_login.to_string()),
            ("RESTATE_INGRESS_URL", "http://restate.test".to_string()),
            ("RESTATE_AUTH_TOKEN", "key_test".to_string()),
        ])
    }

    #[test]
    fn configuration_reports_every_missing_required_value() {
        let Err(error) = ReceiverConfig::from_values(|_| None) else {
            panic!("configuration must be incomplete");
        };
        assert!(
            matches!(error, ConfigError::Missing(ref names) if names == "NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_WEBHOOK_SECRET, NAVIGATOR_GITHUB_CANONICAL_REPOSITORY, NAVIGATOR_GITHUB_ORG, NAVIGATOR_GITHUB_APP_LOGIN, RESTATE_INGRESS_URL, RESTATE_AUTH_TOKEN")
        );
    }

    #[test]
    fn receiver_refuses_a_non_authoritative_deployment_even_with_its_secret() {
        let mut values = required_configuration();
        values.insert("NAVIGATOR_GCP_PROJECT_ID", "neon-law".to_string());

        let Err(error) = ReceiverConfig::from_values(|name| values.get(name).cloned()) else {
            panic!("tenant deployments must not consume the singleton webhook stream");
        };

        assert!(matches!(error, ConfigError::NotAutomationHome));
    }

    #[test]
    fn configuration_builds_the_receiver_state() {
        let values = required_configuration();
        let config = ReceiverConfig::from_values(|name| values.get(name).cloned())
            .expect("valid configuration");
        let state = config.app_state();
        assert_eq!(
            state.settings.canonical_repository,
            CONFIG.canonical_repository
        );
        assert_eq!(state.settings.github_org, CONFIG.github_org);
        assert_eq!(state.settings.app_login, CONFIG.app_login);
    }

    fn payload(value: &Value) -> Request<Body> {
        let bytes = serde_json::to_vec(&value).expect("serialize test fixture");
        let signature = sign_hmac_sha256_hex(SECRET.as_bytes(), &bytes);
        Request::builder()
            .method("POST")
            .uri("/webhooks/github/test-webhook-secret")
            .header("x-hub-signature-256", signature)
            .header("x-github-event", "issues")
            .header("x-github-delivery", "11111111-1111-4111-8111-111111111111")
            .body(Body::from(bytes))
            .expect("build test request")
    }

    fn state(ingress_url: String) -> AppState {
        AppState::new(
            SECRET,
            RouterSettings::new(
                CONFIG.canonical_repository.to_string(),
                CONFIG.github_org.to_string(),
                CONFIG.app_login.to_string(),
            ),
            Arc::new(RestateSubmitter::new(ingress_url, "key_test".to_string())),
        )
    }

    #[test]
    fn routes_opened_triage_issues_to_triage() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": {
                "number": 457,
                "author_association": "OWNER",
                "labels": [{ "name": "triage" }]
            },
            "sender": { "login": "contributor" }
        });
        let route = route(
            &CONFIG,
            "delivery-1",
            GithubEvent::Issues,
            "opened",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxIssueTriage");
        assert_eq!(
            route.key(),
            "triage-neon-law-source-code__navigator-457-delivery-1"
        );
        assert_eq!(route.handler(), "run");
    }

    #[test]
    fn routes_a_triage_issue_revision_to_a_new_delivery_key() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": {
                "number": 457,
                "author_association": "OWNER",
                "labels": [{ "name": "triage" }]
            },
            "sender": { "login": "contributor" }
        });
        let route = route(
            &CONFIG,
            "delivery-revision-1",
            GithubEvent::Issues,
            "edited",
            &payload,
        )
        .expect("well-formed payload");

        assert_eq!(route.service(), "DevxIssueTriage");
        assert_eq!(
            route.key(),
            "triage-neon-law-source-code__navigator-457-delivery-revision-1"
        );
    }

    #[test]
    fn ignores_opened_issues_without_triage() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "author_association": "OWNER", "labels": [] },
            "sender": { "login": "owner" }
        });
        let result = route(
            &CONFIG,
            "delivery-1",
            GithubEvent::Issues,
            "opened",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(result.ignore_reason(), Some(IgnoreReason::NotTriage));
    }

    #[test]
    fn routes_trusted_triage_label_to_triage() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [] },
            "label": { "name": "triage" },
            "sender": { "login": "maintainer", "author_association": "MEMBER" }
        });
        let route = route(
            &CONFIG,
            "delivery-2",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxIssueTriage");
        assert_eq!(
            route.key(),
            "triage-neon-law-source-code__navigator-457-delivery-2"
        );
    }

    #[test]
    fn routes_a_triage_label_without_any_author_association() {
        // GitHub's webhook `sender` object carries no `author_association`, and
        // only a write-access user can apply a label at all, so the `labeled`
        // delivery is trusted on its own — the route must not depend on an
        // association field GitHub never sends.
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [] },
            "label": { "name": "triage" },
            "sender": { "login": "maintainer" }
        });
        let route = route(
            &CONFIG,
            "delivery-3",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxIssueTriage");
        assert_eq!(
            route.key(),
            "triage-neon-law-source-code__navigator-457-delivery-3"
        );
    }

    #[test]
    fn routes_a_human_triaged_label_without_any_author_association() {
        // A maintainer applying `triaged` sends a `labeled` delivery whose
        // `sender` has no `author_association`; the implement route must still
        // fire on GitHub's write-access guarantee alone.
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [{ "name": "triaged" }] },
            "label": { "name": "triaged" },
            "sender": { "login": "maintainer" }
        });
        let route = route(
            &CONFIG,
            "delivery-human-triaged",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxImplementIssue");
        assert_eq!(
            route.key(),
            "implement-neon-law-source-code__navigator-457-delivery-human-triaged"
        );
    }

    #[test]
    fn ignores_already_triaged_triage_label() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [{ "name": "triaged" }] },
            "label": { "name": "triage" },
            "sender": { "login": "maintainer", "author_association": "MEMBER" }
        });
        let result = route(
            &CONFIG,
            "delivery-4",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(result.ignore_reason(), Some(IgnoreReason::AlreadyTriaged));
    }

    #[test]
    fn routes_the_apps_triaged_label_to_implementation() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [{ "name": "triaged" }] },
            "label": { "name": "triaged" },
            "sender": { "login": "navigator-devx[bot]", "author_association": "NONE" }
        });
        let route = route(
            &CONFIG,
            "delivery-5",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxImplementIssue");
        assert_eq!(
            route.key(),
            "implement-neon-law-source-code__navigator-457-delivery-5"
        );
    }

    #[test]
    fn routes_a_collaborators_triaged_label_to_implementation() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [{ "name": "triaged" }] },
            "label": { "name": "triaged" },
            "sender": { "login": "maintainer", "author_association": "COLLABORATOR" }
        });
        let route = route(
            &CONFIG,
            "delivery-triaged",
            GithubEvent::Issues,
            "labeled",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(route.service(), "DevxImplementIssue");
    }

    #[test]
    fn ignores_the_apps_other_events() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "labels": [{ "name": "triage" }] },
            "sender": { "login": "navigator-devx[bot]" }
        });
        let result = route(
            &CONFIG,
            "delivery-6",
            GithubEvent::Issues,
            "opened",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(result.ignore_reason(), Some(IgnoreReason::OwnApp));
    }

    #[test]
    fn rejects_another_repository_before_routing() {
        let payload = json!({
            "repository": { "full_name": "fork/navigator" },
            "issue": { "number": 457, "author_association": "OWNER", "labels": [{ "name": "triage" }] },
            "sender": { "login": "owner" }
        });
        let result = route(
            &CONFIG,
            "delivery-7",
            GithubEvent::Issues,
            "opened",
            &payload,
        )
        .expect("well-formed payload");
        assert_eq!(result.ignore_reason(), Some(IgnoreReason::WrongRepository));
    }

    #[test]
    fn routes_each_pr_signal_event_including_failures_on_non_devx_branches() {
        let signals = [
            (
                "delivery-comment",
                GithubEvent::IssueComment,
                "created",
                json!({
                    "repository": { "full_name": "neon-law-source-code/navigator" },
                    "sender": { "login": "reviewer" },
                    "issue": { "number": 99, "pull_request": {} }
                }),
            ),
            (
                "delivery-review-comment",
                GithubEvent::PullRequestReviewComment,
                "created",
                json!({
                    "repository": { "full_name": "neon-law-source-code/navigator" },
                    "sender": { "login": "reviewer" },
                    "pull_request": { "number": 99 }
                }),
            ),
            (
                "delivery-review",
                GithubEvent::PullRequestReview,
                "submitted",
                json!({
                    "repository": { "full_name": "neon-law-source-code/navigator" },
                    "sender": { "login": "reviewer" },
                    "review": { "state": "changes_requested" },
                    "pull_request": { "number": 99 }
                }),
            ),
            (
                "delivery-check",
                GithubEvent::CheckRun,
                "completed",
                json!({
                    "repository": { "full_name": "neon-law-source-code/navigator" },
                    "sender": { "login": "github-actions[bot]" },
                    "check_run": {
                        "conclusion": "failure",
                        "check_suite": { "head_branch": "feature/slack-proof" },
                        "pull_requests": [{
                            "number": 99,
                            "head": { "repo": {
                                "name": "navigator",
                                "url": "https://api.github.com/repos/neon-law-source-code/navigator"
                            } }
                        }]
                    }
                }),
            ),
            (
                "delivery-workflow",
                GithubEvent::WorkflowRun,
                "completed",
                json!({
                    "repository": { "full_name": "neon-law-source-code/navigator" },
                    "sender": { "login": "github-actions[bot]" },
                    "workflow_run": {
                        "conclusion": "failure",
                        "head_branch": "feature/slack-proof",
                        "head_repository": { "full_name": "neon-law-source-code/navigator" },
                        "pull_requests": [{ "number": 99 }]
                    }
                }),
            ),
        ];
        for (delivery, event, action, payload) in signals {
            let result = route(&CONFIG, delivery, event, action, &payload).expect("signal payload");
            assert_eq!(result.service(), "devx-pr");
            assert_eq!(result.key(), "neon-law-source-code__navigator-99");
        }
    }

    #[test]
    fn watches_every_project_repo_in_the_org() {
        // A private Project repo owned by NAVIGATOR_GITHUB_ORG (neon-law-firm) is
        // watched with no per-repo config, and the routed command carries its
        // repository so the worker acts on the right one.
        let payload = json!({
            "repository": { "full_name": "neon-law-firm/arthur" },
            "issue": {
                "number": 12,
                "author_association": "MEMBER",
                "labels": [{ "name": "triage" }]
            },
            "sender": { "login": "attorney" }
        });
        let RouteDecision::Route(routed) = route(
            &CONFIG,
            "delivery-firm",
            GithubEvent::Issues,
            "opened",
            &payload,
        )
        .expect("well-formed payload") else {
            panic!("a Project-org repo must be watched");
        };
        assert_eq!(routed.service(), "DevxIssueTriage");
        assert_eq!(
            routed.body()["repository"].as_str(),
            Some("neon-law-firm/arthur")
        );
    }

    #[test]
    fn pull_request_object_key_is_scoped_per_repository() {
        // The `devx-pr` object is serialized by key; two repositories sharing a
        // PR number must not collide, so the key is repository-qualified.
        let firm = json!({
            "repository": { "full_name": "neon-law-firm/arthur" },
            "sender": { "login": "reviewer" },
            "issue": { "number": 12, "pull_request": {} }
        });
        let code = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "reviewer" },
            "issue": { "number": 12, "pull_request": {} }
        });
        let firm_key = route(
            &CONFIG,
            "d-firm",
            GithubEvent::IssueComment,
            "created",
            &firm,
        )
        .expect("signal")
        .key()
        .to_string();
        let code_key = route(
            &CONFIG,
            "d-code",
            GithubEvent::IssueComment,
            "created",
            &code,
        )
        .expect("signal")
        .key()
        .to_string();
        assert_eq!(firm_key, "neon-law-firm__arthur-12");
        assert_eq!(code_key, "neon-law-source-code__navigator-12");
        assert_ne!(firm_key, code_key);
    }

    #[test]
    fn ignores_edited_and_deleted_comment_mutations() {
        // Editing or deleting an existing comment must not re-signal `devx-pr`
        // the way a freshly created comment does.
        let issue_comment_base = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "reviewer" },
            "issue": { "number": 99, "pull_request": {} }
        });
        let review_comment_base = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "reviewer" },
            "pull_request": { "number": 99 }
        });
        for action in ["edited", "deleted", "pinned", "unpinned"] {
            let issue_comment = route(
                &CONFIG,
                "delivery-comment-mutation",
                GithubEvent::IssueComment,
                action,
                &issue_comment_base,
            )
            .expect("well-formed payload");
            assert_eq!(
                issue_comment.ignore_reason(),
                Some(IgnoreReason::UnhandledAction),
                "issue_comment {action} must not signal"
            );

            let review_comment = route(
                &CONFIG,
                "delivery-review-comment-mutation",
                GithubEvent::PullRequestReviewComment,
                action,
                &review_comment_base,
            )
            .expect("well-formed payload");
            assert_eq!(
                review_comment.ignore_reason(),
                Some(IgnoreReason::UnhandledAction),
                "pull_request_review_comment {action} must not signal"
            );
        }
    }

    #[test]
    fn rejects_a_fork_head_repository_failure_signal() {
        // A fork PR can carry a canonical-repository PR number, but the head
        // repository identifies the fork, so no `devx-pr` signal fires.
        let check_run = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "github-actions[bot]" },
            "check_run": {
                "conclusion": "failure",
                "check_suite": { "head_branch": "devx/issue-457" },
                "pull_requests": [{
                    "number": 99,
                    "head": { "repo": {
                        "name": "navigator",
                        "url": "https://api.github.com/repos/attacker/navigator"
                    } }
                }]
            }
        });
        let check_result = route(
            &CONFIG,
            "delivery-fork-check",
            GithubEvent::CheckRun,
            "completed",
            &check_run,
        )
        .expect("well-formed payload");
        assert_eq!(
            check_result.ignore_reason(),
            Some(IgnoreReason::ForkHeadRepository)
        );

        let workflow_run = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "github-actions[bot]" },
            "workflow_run": {
                "conclusion": "failure",
                "head_branch": "devx/issue-457",
                "head_repository": { "full_name": "attacker/navigator" },
                "pull_requests": [{ "number": 99 }]
            }
        });
        let workflow_result = route(
            &CONFIG,
            "delivery-fork-workflow",
            GithubEvent::WorkflowRun,
            "completed",
            &workflow_run,
        )
        .expect("well-formed payload");
        assert_eq!(
            workflow_result.ignore_reason(),
            Some(IgnoreReason::ForkHeadRepository)
        );
    }

    #[test]
    fn ignores_unknown_events() {
        let payload = json!({ "repository": { "full_name": "neon-law-source-code/navigator" } });
        let result = route(&CONFIG, "delivery-8", GithubEvent::Unknown, "", &payload)
            .expect("unknown events are no-ops");
        assert_eq!(result.ignore_reason(), Some(IgnoreReason::UnknownEvent));
    }

    #[test]
    fn rejects_a_supported_event_without_its_required_routing_fields() {
        let payload = json!({
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "sender": { "login": "maintainer" },
            "issue": { "number": 457, "labels": [{ "name": "triage" }] }
        });
        assert!(route(
            &CONFIG,
            "delivery-malformed",
            GithubEvent::Issues,
            "opened",
            &payload
        )
        .is_err());
    }

    #[test]
    fn valid_and_tampered_hex_signatures_are_distinguished() {
        let body = br#"{"event":"issues"}"#;
        let signature = sign_hmac_sha256_hex(SECRET.as_bytes(), body);
        assert!(crate::sig::verify_hmac_sha256_hex(
            SECRET.as_bytes(),
            body,
            &signature
        ));
        assert!(!crate::sig::verify_hmac_sha256_hex(
            SECRET.as_bytes(),
            br#"{"event":"tampered"}"#,
            &signature
        ));
        assert!(!crate::sig::verify_hmac_sha256_hex(
            SECRET.as_bytes(),
            body,
            "sha256=not-hex"
        ));
    }

    #[tokio::test]
    async fn receiver_submits_the_same_restate_key_for_a_redelivery() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/DevxIssueTriage/triage-neon-law-source-code__navigator-457-11111111-1111-4111-8111-111111111111/run/send"))
            .and(body_partial_json(
                json!({ "issue_number": 457, "delivery_id": "11111111-1111-4111-8111-111111111111" }),
            ))
            .respond_with(ResponseTemplate::new(202))
            .expect(2)
            .mount(&server)
            .await;
        let application = app(state(server.uri()));
        let fixture = json!({
            "action": "opened",
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 457, "author_association": "OWNER", "labels": [{ "name": "triage" }] },
            "sender": { "login": "owner" }
        });
        for _ in 0..2 {
            let response = application
                .clone()
                .oneshot(payload(&fixture))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }
    }

    #[tokio::test]
    async fn receiver_returns_expected_authz_and_noop_statuses() {
        let application = app(state("http://127.0.0.1:9".to_string()));
        let unsigned = Request::builder()
            .method("POST")
            .uri("/webhooks/github/test-webhook-secret")
            .body(Body::from("{}"))
            .expect("build unsigned request");
        assert_eq!(
            application
                .clone()
                .oneshot(unsigned)
                .await
                .expect("response")
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let wrong_repo = json!({
            "action": "opened",
            "repository": { "full_name": "fork/navigator" },
            "issue": { "number": 1, "author_association": "OWNER", "labels": [{ "name": "triage" }] },
            "sender": { "login": "owner" }
        });
        assert_eq!(
            application
                .clone()
                .oneshot(payload(&wrong_repo))
                .await
                .expect("response")
                .status(),
            StatusCode::FORBIDDEN
        );

        let noop = json!({
            "action": "opened",
            "repository": { "full_name": "neon-law-source-code/navigator" },
            "issue": { "number": 1, "author_association": "OWNER", "labels": [] },
            "sender": { "login": "owner" }
        });
        assert_eq!(
            application
                .clone()
                .oneshot(payload(&noop))
                .await
                .expect("response")
                .status(),
            StatusCode::NO_CONTENT
        );

        let malformed_body = b"{";
        let signature = sign_hmac_sha256_hex(SECRET.as_bytes(), malformed_body);
        let malformed = Request::builder()
            .method("POST")
            .uri("/webhooks/github/test-webhook-secret")
            .header("x-hub-signature-256", signature)
            .header("x-github-event", "issues")
            .header("x-github-delivery", "delivery-malformed")
            .body(Body::from(malformed_body.to_vec()))
            .expect("build malformed request");
        assert_eq!(
            application
                .oneshot(malformed)
                .await
                .expect("response")
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}
