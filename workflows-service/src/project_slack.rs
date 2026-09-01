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
                    let project = store::projects::find_by_id(&surreal, project_id)
                        .await
                        .map_err(project_store_error)?
                        .ok_or_else(|| {
                            HandlerError::from(TerminalError::new(
                                "Project no longer exists for Slack notice",
                            ))
                        })?;
                    if let Some(channel_id) = project.internal_slack_channel_id {
                        return Ok(Json(channel_id));
                    }
                    let channel: SlackChannel = slack
                        .create_private_channel(&project.code)
                        .await
                        .map_err(HandlerError::from)?;
                    store::projects::set_internal_slack_channel_id(
                        &surreal,
                        project_id,
                        &channel.id,
                    )
                    .await
                    .map_err(project_command_error)?
                    .ok_or_else(|| {
                        HandlerError::from(TerminalError::new(
                            "Project disappeared while saving Slack channel",
                        ))
                    })?;
                    Ok(Json(channel.id))
                })
                .name("ensure-channel")
                .await?
                .into_inner();

            let slack = Arc::clone(&self.slack);
            ctx.run(move || async move {
                slack
                    .post_message(&channel_id, CLIENT_PROJECT_VIEW_MESSAGE)
                    .await
                    .map_err(HandlerError::from)
            })
            .name("post-client-view")
            .await?;
            Ok(())
        }
        .instrument(span)
        .await
    }
}
