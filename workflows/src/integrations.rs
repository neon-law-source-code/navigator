//! Durable integration jobs and mechanism-only Slack notices.
//!
//! Provider calls stay behind traits in `cloud`; a durable worker may replay
//! these jobs, and the provider seams are idempotent on the Project code. The
//! notice surface intentionally carries an event kind only, never client
//! content, document text, or a provider URL.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationProviderKind {
    Notion,
    Slack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationJob {
    pub job_id: String,
    pub project_id: Uuid,
    pub project_code: String,
    pub provider: IntegrationProviderKind,
}

impl IntegrationJob {
    #[must_use]
    pub fn notion(project_id: Uuid, project_code: impl Into<String>) -> Self {
        Self::new(project_id, project_code, IntegrationProviderKind::Notion)
    }

    #[must_use]
    pub fn slack(project_id: Uuid, project_code: impl Into<String>) -> Self {
        Self::new(project_id, project_code, IntegrationProviderKind::Slack)
    }

    fn new(
        project_id: Uuid,
        project_code: impl Into<String>,
        provider: IntegrationProviderKind,
    ) -> Self {
        let project_code = project_code.into();
        let mut digest = Sha256::new();
        digest.update(project_id.as_bytes());
        digest.update([0]);
        digest.update(project_code.as_bytes());
        digest.update([0]);
        digest.update([match provider {
            IntegrationProviderKind::Notion => 1,
            IntegrationProviderKind::Slack => 2,
        }]);
        let job_id = format!("integration-{}", URL_SAFE_NO_PAD.encode(digest.finalize()));
        Self {
            job_id,
            project_id,
            project_code,
            provider,
        }
    }
}

#[derive(Debug, Error)]
pub enum IntegrationJobError {
    #[error("provider did not return a resource")]
    MissingResource,
}

/// Closed event vocabulary for the CLI and durable notification worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackNoticeEvent {
    ProjectOpened,
    ProjectClosed,
    ProjectReconciled,
    IntegrationUnavailable,
}

impl SlackNoticeEvent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectOpened => "project_opened",
            Self::ProjectClosed => "project_closed",
            Self::ProjectReconciled => "project_reconciled",
            Self::IntegrationUnavailable => "integration_unavailable",
        }
    }
}

/// Render a mechanism-only notice. It never accepts arbitrary text, a URL,
/// an email address, a document title, or a client identifier.
#[must_use]
pub fn render_slack_notice(event: SlackNoticeEvent) -> String {
    format!("Navigator integration event: {}.", event.as_str())
}

/// Post one closed-vocabulary, mechanism-only notice to a Firm-private
/// channel. The caller supplies the resolved Firm Slack service and channel
/// coordinate; no deployment token or arbitrary message text is accepted.
pub async fn post_slack_notice<S: cloud::SlackService + ?Sized>(
    service: &S,
    channel_id: &str,
    event: SlackNoticeEvent,
) -> Result<(), cloud::SlackError> {
    service
        .post_message(channel_id, &render_slack_notice(event))
        .await
}

#[cfg(test)]
mod tests {
    use super::{post_slack_notice, render_slack_notice, IntegrationJob, SlackNoticeEvent};
    use cloud::FakeSlack;
    use uuid::Uuid;

    #[test]
    fn job_id_is_stable_for_durable_replay() {
        let project = Uuid::from_u128(7);
        assert_eq!(
            IntegrationJob::notion(project, "sample-project").job_id,
            IntegrationJob::notion(project, "sample-project").job_id
        );
        assert_ne!(
            IntegrationJob::notion(project, "sample-project").job_id,
            IntegrationJob::slack(project, "sample-project").job_id
        );
    }

    #[test]
    fn slack_notice_is_closed_and_mechanism_only() {
        let text = render_slack_notice(SlackNoticeEvent::ProjectReconciled);
        assert_eq!(text, "Navigator integration event: project_reconciled.");
        assert!(!text.contains("http"));
        assert!(!text.contains('@'));
    }

    #[tokio::test]
    async fn notice_helper_posts_only_the_closed_event_text() {
        let slack = FakeSlack::new();
        post_slack_notice(&slack, "C123", SlackNoticeEvent::ProjectOpened)
            .await
            .unwrap();
        assert_eq!(
            slack.messages(),
            vec![(
                "C123".to_string(),
                "Navigator integration event: project_opened.".to_string()
            )]
        );
    }
}
