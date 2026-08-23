//! The durable Restate services that turn trusted webhook routes into Slack
//! notices. These bind into `workflows-service` alongside the other durable
//! workflows; the public receiver (`crate::app`) never reads `SLACK_WEBHOOK_URL`
//! and only submits identifier-only commands. The repository each command acts
//! on is carried in the request (multi-repo), never held as static config.

use std::sync::Arc;

use async_trait::async_trait;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use workflows::Notifier;

/// Durable, database-resolved target for a trusted GitHub repository.
///
/// The public receiver carries only repository identity. The database-owning
/// worker resolves a private repository to this state before any Project action
/// so future triage and implementation steps cannot confuse a repository name
/// with an authorization grant.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ResolvedRepository {
    /// The public Navigator code repository, which deliberately has no Project.
    Code { repository: String },
    /// One exact private Project repository and its durable Project identity.
    Project {
        repository: String,
        project_id: Uuid,
    },
}

impl ResolvedRepository {
    /// Repository identity as it appeared in the authenticated command.
    #[must_use]
    pub fn repository(&self) -> &str {
        match self {
            Self::Code { repository } | Self::Project { repository, .. } => repository,
        }
    }

    /// Stable key material for a Project-scoped durable workflow.
    ///
    /// Both the external repository and the resolved Project UUID participate:
    /// a redelivery converges, and a stale or corrupt repository mapping cannot
    /// accidentally share a Project workflow journal.
    #[must_use]
    pub fn workflow_key(&self) -> String {
        let repository = self.repository().replace('/', "__");
        match self {
            Self::Code { .. } => format!("code-{repository}"),
            Self::Project { project_id, .. } => format!("project-{repository}-{project_id}"),
        }
    }
}

/// Metadata-only reasons repository-to-Project correlation must stop.
#[derive(Debug, Error)]
pub enum RepositoryResolutionError {
    #[error("malformed repository identifier")]
    Malformed,
    #[error("repository owner is not configured for Project correlation")]
    WrongOwner,
    #[error("no Project matches the repository code")]
    MissingProject,
    #[error("multiple Projects match the repository code")]
    AmbiguousProject,
    #[error("repository correlation query failed")]
    Database,
    #[error("repository correlation is not configured")]
    Unconfigured,
}

/// Database-owning boundary for private-repository correlation.
#[async_trait]
pub trait RepositoryResolver: Send + Sync {
    /// Resolve one authenticated repository without changing authorization data.
    async fn resolve(
        &self,
        repository: &str,
    ) -> Result<ResolvedRepository, RepositoryResolutionError>;
}

/// Fail-closed resolver for deployments where the webhook receiver is absent.
///
/// Local and test workers intentionally run without GitHub webhook variables;
/// no receiver can submit a command in that state. If a command nevertheless
/// reaches this worker, it terminates before any Project action.
#[derive(Default)]
pub struct UnconfiguredRepositoryResolver;

#[async_trait]
impl RepositoryResolver for UnconfiguredRepositoryResolver {
    async fn resolve(
        &self,
        _repository: &str,
    ) -> Result<ResolvedRepository, RepositoryResolutionError> {
        Err(RepositoryResolutionError::Unconfigured)
    }
}

/// Identifier-only request submitted when a trusted issue gains `triage`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IssueTriageRequest {
    pub repository: String,
    pub issue_number: u64,
    pub delivery_id: String,
}

/// Identifier-only request submitted for a pull-request signal.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PullRequestNotice {
    pub repository: String,
    pub pull_request_number: u64,
    pub delivery_id: String,
    pub event: String,
}

/// Render a one-line, body-free Slack message for a triage request.
#[must_use]
pub fn triage_message(repository: &str, issue_number: u64) -> String {
    format!("🏷️ DevX triage requested — https://github.com/{repository}/issues/{issue_number}")
}

/// Render a one-line, body-free Slack message for a pull-request event.
#[must_use]
pub fn pull_request_message(repository: &str, request: &PullRequestNotice) -> String {
    let event = match request.event.as_str() {
        "issue_comment" | "pull_request_review_comment" => "comment",
        "pull_request_review" => "review",
        "check_run" | "workflow_run" => "failed GitHub Actions check",
        _ => "event",
    };
    format!(
        "🔔 DevX pull request {event} — https://github.com/{repository}/pull/{}",
        request.pull_request_number
    )
}

/// The durable triage-notification service. Bound into `workflows-service`.
#[derive(Clone)]
pub struct DevxIssueTriageService {
    notifier: Arc<dyn Notifier>,
    repository_resolver: Arc<dyn RepositoryResolver>,
}

impl DevxIssueTriageService {
    #[must_use]
    pub fn new(
        notifier: Arc<dyn Notifier>,
        repository_resolver: Arc<dyn RepositoryResolver>,
    ) -> Self {
        Self {
            notifier,
            repository_resolver,
        }
    }
}

#[restate_sdk::workflow(name = "DevxIssueTriage")]
impl DevxIssueTriageService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        request: Json<IssueTriageRequest>,
    ) -> Result<(), HandlerError> {
        let repository = request.0.repository.clone();
        let resolver = Arc::clone(&self.repository_resolver);
        let target = ctx
            .run(move || async move { resolve_repository(resolver, repository).await.map(Json) })
            .name("resolve-repository")
            .await?
            .0;
        let message = triage_message(target.repository(), request.0.issue_number);
        let notifier = Arc::clone(&self.notifier);
        ctx.run(move || async move { notifier.notify(message).await.map_err(HandlerError::from) })
            .name("notify-triage")
            .await?;
        Ok(())
    }
}

/// The serialized per-pull-request notification service. Its keyed object shape
/// is deliberately the same one the later revision loop will extend. Bound into
/// `workflows-service`.
#[derive(Clone)]
pub struct DevxPrService {
    notifier: Arc<dyn Notifier>,
    repository_resolver: Arc<dyn RepositoryResolver>,
}

impl DevxPrService {
    #[must_use]
    pub fn new(
        notifier: Arc<dyn Notifier>,
        repository_resolver: Arc<dyn RepositoryResolver>,
    ) -> Self {
        Self {
            notifier,
            repository_resolver,
        }
    }
}

#[restate_sdk::object(name = "devx-pr")]
impl DevxPrService {
    #[restate_sdk::handler]
    async fn signal(
        &self,
        ctx: ObjectContext<'_>,
        request: Json<PullRequestNotice>,
    ) -> Result<(), HandlerError> {
        let repository = request.0.repository.clone();
        let resolver = Arc::clone(&self.repository_resolver);
        let target = ctx
            .run(move || async move { resolve_repository(resolver, repository).await.map(Json) })
            .name("resolve-repository")
            .await?
            .0;
        let message = pull_request_message(target.repository(), &request.0);
        let notifier = Arc::clone(&self.notifier);
        ctx.run(move || async move { notifier.notify(message).await.map_err(HandlerError::from) })
            .name("notify-pull-request")
            .await?;
        Ok(())
    }
}

async fn resolve_repository(
    resolver: Arc<dyn RepositoryResolver>,
    repository: String,
) -> Result<ResolvedRepository, HandlerError> {
    resolver.resolve(&repository).await.map_err(|error| {
        // A correlation query failure is transient: a temporary store
        // outage must stay retryable so Restate recovers the notification
        // durably instead of permanently dropping it. Every other variant is a
        // permanent correlation failure that can never succeed on retry, so it
        // terminates before any Project action.
        if matches!(error, RepositoryResolutionError::Database) {
            tracing::warn!(repository, reason = %error, "retrying GitHub command after a repository correlation query failure");
            HandlerError::from(error)
        } else {
            tracing::warn!(repository, reason = %error, "rejecting GitHub command before Project action");
            HandlerError::from(TerminalError::new(error.to_string()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        pull_request_message, resolve_repository, triage_message, PullRequestNotice,
        RepositoryResolutionError, RepositoryResolver, ResolvedRepository,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Always rejects with a fixed correlation error so the worker's
    /// terminal-vs-retryable mapping can be observed in isolation.
    struct RejectingResolver(RepositoryResolutionError);

    #[async_trait]
    impl RepositoryResolver for RejectingResolver {
        async fn resolve(
            &self,
            _repository: &str,
        ) -> Result<ResolvedRepository, RepositoryResolutionError> {
            Err(match self.0 {
                RepositoryResolutionError::Malformed => RepositoryResolutionError::Malformed,
                RepositoryResolutionError::WrongOwner => RepositoryResolutionError::WrongOwner,
                RepositoryResolutionError::MissingProject => {
                    RepositoryResolutionError::MissingProject
                }
                RepositoryResolutionError::AmbiguousProject => {
                    RepositoryResolutionError::AmbiguousProject
                }
                RepositoryResolutionError::Database => RepositoryResolutionError::Database,
                RepositoryResolutionError::Unconfigured => RepositoryResolutionError::Unconfigured,
            })
        }
    }

    async fn resolution_error_debug(kind: RepositoryResolutionError) -> String {
        let error = resolve_repository(
            Arc::new(RejectingResolver(kind)),
            "neon-law-firm/arthur".into(),
        )
        .await
        .expect_err("resolution must fail");
        // `HandlerError` is Debug-only; its inner variant is `Retryable(..)`
        // for a retried invocation and `Terminal(..)` for a permanent failure.
        format!("{error:?}")
    }

    #[tokio::test]
    async fn transient_database_failure_stays_retryable() {
        assert!(
            resolution_error_debug(RepositoryResolutionError::Database)
                .await
                .contains("Retryable"),
            "a correlation query failure must remain retryable so a temporary store outage recovers durably"
        );
    }

    #[tokio::test]
    async fn permanent_correlation_failures_are_terminal() {
        for kind in [
            RepositoryResolutionError::Malformed,
            RepositoryResolutionError::WrongOwner,
            RepositoryResolutionError::MissingProject,
            RepositoryResolutionError::AmbiguousProject,
            RepositoryResolutionError::Unconfigured,
        ] {
            let debug = resolution_error_debug(kind).await;
            assert!(
                debug.contains("Terminal"),
                "permanent correlation failures must not be retried, got {debug}"
            );
        }
    }

    #[test]
    fn triage_message_carries_only_the_issue_link() {
        assert_eq!(
            triage_message("neon-law-source-code/navigator", 457),
            "🏷️ DevX triage requested — https://github.com/neon-law-source-code/navigator/issues/457"
        );
    }

    #[test]
    fn pull_request_messages_identify_comments_and_failures_without_content() {
        let comment = PullRequestNotice {
            repository: "neon-law-firm/arthur".into(),
            pull_request_number: 557,
            delivery_id: "delivery-comment".into(),
            event: "issue_comment".into(),
        };
        assert_eq!(
            pull_request_message(&comment.repository, &comment),
            "🔔 DevX pull request comment — https://github.com/neon-law-firm/arthur/pull/557"
        );
        let failure = PullRequestNotice {
            event: "check_run".into(),
            ..comment
        };
        assert_eq!(
            pull_request_message(&failure.repository, &failure),
            "🔔 DevX pull request failed GitHub Actions check — https://github.com/neon-law-firm/arthur/pull/557"
        );
    }
}
