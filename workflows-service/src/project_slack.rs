//! Durable internal Slack notices for one Project.
//!
//! The service is a virtual object keyed by the Project id. That gives each
//! Project one serialized lane: the first client view creates its private
//! channel and later views reuse the channel ID persisted on the Project row.
//! Both the channel-creation and message-post calls are journaled Restate
//! steps, so a transient Slack outage is retried without holding an HTTP page
//! request open.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use workflows::{SlackBot, SlackChannel};

/// Empty request for the client-view event. The server derives the event from
/// its authenticated session and authorization result; callers cannot choose
/// an arbitrary Slack message or recipient.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ClientProjectViewRequest {}

/// The only message currently emitted by this service. It deliberately carries
/// no client name, email, matter name, or portal content; the private channel
/// itself is the Project-specific destination.
pub const CLIENT_PROJECT_VIEW_MESSAGE: &str = "A client viewed this Project in the portal.";

#[derive(Clone)]
pub struct ProjectSlackService {
    surreal: store::surreal::SurrealDb,
    slack: Arc<dyn SlackBot>,
}

impl ProjectSlackService {
    #[must_use]
    pub fn new(surreal: store::surreal::SurrealDb, slack: Arc<dyn SlackBot>) -> Self {
        Self { surreal, slack }
    }
}

fn project_id(key: &str) -> Result<uuid::Uuid, HandlerError> {
    key.parse::<uuid::Uuid>().map_err(|error| {
        HandlerError::from(TerminalError::new(format!(
            "Project Slack object key `{key}` is not a valid project id: {error}"
        )))
    })
}

fn project_store_error(error: store::projects::ProjectStoreError) -> HandlerError {
    if matches!(error, store::projects::ProjectStoreError::Db(_)) {
        HandlerError::from(error)
    } else {
        HandlerError::from(TerminalError::new(error.to_string()))
    }
}

fn project_command_error(error: store::projects::ProjectCommandError) -> HandlerError {
    if matches!(error, store::projects::ProjectCommandError::Db(_)) {
        HandlerError::from(error)
    } else {
        HandlerError::from(TerminalError::new(error.to_string()))
    }
}

/// Find or create the Project's private Slack channel. Extracted from the
/// Restate handler so the store/Slack work is unit-testable without a broker.
async fn ensure_internal_channel(
    surreal: &store::surreal::SurrealDb,
    slack: &Arc<dyn SlackBot>,
    project_id: uuid::Uuid,
) -> Result<String, HandlerError> {
    let project = store::projects::find_by_id(surreal, project_id)
        .await
        .map_err(project_store_error)?
        .ok_or_else(|| {
            HandlerError::from(TerminalError::new(
                "Project no longer exists for Slack notice",
            ))
        })?;
    if let Some(channel_id) = project.internal_slack_channel_id {
        return Ok(channel_id);
    }
    let channel: SlackChannel = slack
        .create_private_channel(&project.code)
        .await
        .map_err(HandlerError::from)?;
    store::projects::set_internal_slack_channel_id(surreal, project_id, &channel.id)
        .await
        .map_err(project_command_error)?
        .ok_or_else(|| {
            HandlerError::from(TerminalError::new(
                "Project disappeared while saving Slack channel",
            ))
        })?;
    Ok(channel.id)
}

async fn post_client_view_notice(
    slack: &Arc<dyn SlackBot>,
    channel_id: &str,
) -> Result<(), HandlerError> {
    slack
        .post_message(channel_id, CLIENT_PROJECT_VIEW_MESSAGE)
        .await
        .map_err(HandlerError::from)
}

#[restate_sdk::object(name = "project-slack")]
impl ProjectSlackService {
    /// Ensure the Project has a private Slack channel, then post the client
    /// portal-view notice into that channel.
    #[restate_sdk::handler]
    async fn client_project_view(
        &self,
        ctx: ObjectContext<'_>,
        _request: Json<ClientProjectViewRequest>,
    ) -> Result<(), HandlerError> {
        let span =
            tracing::info_span!("project_slack.client_project_view", project_id = %ctx.key());
        async move {
            let project_id = project_id(ctx.key())?;
            let surreal = self.surreal.clone();
            let slack = Arc::clone(&self.slack);
            let channel_id: String = ctx
                .run(|| async move {
                    ensure_internal_channel(&surreal, &slack, project_id)
                        .await
                        .map(Json)
                })
                .name("ensure-channel")
                .await?
                .into_inner();

            let slack = Arc::clone(&self.slack);
            ctx.run(move || async move { post_client_view_notice(&slack, &channel_id).await })
                .name("post-client-view")
                .await?;
            Ok(())
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_internal_channel, post_client_view_notice, project_command_error, project_id,
        project_store_error, ProjectSlackService, CLIENT_PROJECT_VIEW_MESSAGE,
    };
    use std::sync::Arc;
    use store::projects::{create, set_internal_slack_channel_id, NewProject, ProjectCommandError};
    use store::test_support::{mem_surreal, seed_entity};
    use uuid::Uuid;
    use workflows::CapturingSlackBot;

    #[test]
    fn object_key_must_be_a_project_uuid() {
        let id = Uuid::now_v7();
        assert_eq!(project_id(&id.to_string()).expect("uuid"), id);
        assert!(project_id("not-a-uuid").is_err());
    }

    #[test]
    fn store_and_command_errors_other_than_db_are_terminal() {
        let store_err = project_store_error(store::projects::ProjectStoreError::CodeTaken);
        assert!(
            format!("{store_err:?}").contains("already in use"),
            "{store_err:?}"
        );
        let command_err = project_command_error(ProjectCommandError::NotFound);
        assert!(
            format!("{command_err:?}").contains("no matter"),
            "{command_err:?}"
        );
        let invalid = project_command_error(ProjectCommandError::Invalid("bad channel"));
        assert!(
            format!("{invalid:?}").contains("bad channel"),
            "{invalid:?}"
        );
    }

    #[test]
    fn client_view_copy_names_no_client_or_matter() {
        assert_eq!(
            CLIENT_PROJECT_VIEW_MESSAGE,
            "A client viewed this Project in the portal."
        );
        let lower = CLIENT_PROJECT_VIEW_MESSAGE.to_ascii_lowercase();
        assert!(!lower.contains("email"));
        assert!(!lower.contains('@'));
    }

    #[tokio::test]
    async fn ensure_creates_a_channel_once_then_reuses_it() {
        let surreal = mem_surreal().await;
        let project = create(
            &surreal,
            &NewProject {
                code: "sample-slack".to_string(),
                name: "Sample Slack".to_string(),
                status: "open".to_string(),
                entity_id: seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("create Project");
        let slack = Arc::new(CapturingSlackBot::new());
        let bot: Arc<dyn workflows::SlackBot> = slack.clone();

        let first = ensure_internal_channel(&surreal, &bot, project.id)
            .await
            .expect("create channel");
        let second = ensure_internal_channel(&surreal, &bot, project.id)
            .await
            .expect("reuse channel");
        assert_eq!(first, second);
        assert_eq!(slack.created_channels(), vec!["sample-slack"]);

        post_client_view_notice(&bot, &first)
            .await
            .expect("post notice");
        assert_eq!(
            slack.posted_messages(),
            vec![(first, CLIENT_PROJECT_VIEW_MESSAGE.to_string())]
        );
    }

    #[tokio::test]
    async fn ensure_reuses_a_channel_id_already_on_the_project() {
        let surreal = mem_surreal().await;
        let project = create(
            &surreal,
            &NewProject {
                code: "sample-existing".to_string(),
                name: "Sample Existing".to_string(),
                status: "open".to_string(),
                entity_id: seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("create Project");
        set_internal_slack_channel_id(&surreal, project.id, "CEXISTING")
            .await
            .expect("persist channel")
            .expect("project exists");
        let slack = Arc::new(CapturingSlackBot::new());
        let bot: Arc<dyn workflows::SlackBot> = slack.clone();
        let channel = ensure_internal_channel(&surreal, &bot, project.id)
            .await
            .expect("reuse stored channel");
        assert_eq!(channel, "CEXISTING");
        assert!(slack.created_channels().is_empty());
    }

    #[tokio::test]
    async fn ensure_fails_closed_when_the_project_is_gone() {
        let surreal = mem_surreal().await;
        let slack: Arc<dyn workflows::SlackBot> = Arc::new(CapturingSlackBot::new());
        let err = ensure_internal_channel(&surreal, &slack, Uuid::now_v7())
            .await
            .expect_err("missing project");
        assert!(format!("{err:?}").contains("no longer exists"), "{err:?}");
    }

    #[tokio::test]
    async fn service_holds_the_store_and_bot() {
        let surreal = mem_surreal().await;
        let slack: Arc<dyn workflows::SlackBot> = Arc::new(CapturingSlackBot::new());
        let _service = ProjectSlackService::new(surreal, slack);
    }
}
