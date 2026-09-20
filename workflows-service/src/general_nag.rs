//! The `GeneralNag` Restate workflow — the daily jab at `#general`.
//!
//! Two journaled posts, so a retry of one never re-runs the other:
//!
//! 1. `ctx.run("notify")` — post [`nag_message`] through the worker's Slack
//!    Web API bot to `SLACK_GENERAL_CHANNEL_ID`. Persistent staging does not
//!    rewrite this slogan: the Slack adapter already appends a final
//!    `from Staging` line to every real post.
//! 2. `ctx.run("counts")` / `ctx.run("notify_counts")` — a real deployment's
//!    open-matters/pitches follow-up, posted as a **second** Slack message.
//!    Never runs for a simulated-matters deployment: that count is a
//!    real-matter-only signal, so there is nothing to query or post. The
//!    copy is [`crate::dri_digest::open_matters_followup`].
//!
//! **Internal operations signal**, not client correspondence: the jab is a
//! static slogan, and the follow-up is a firm-wide count with no matter
//! names. Posted only to `#general`.
//!
//! The `general-nag` `CronJob` fires nightly at 01:11 UTC, keyed on the UTC
//! run date so a same-day double-fire is a no-op.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use store::surreal::SurrealDb;
use workflows::SlackBot;

use crate::dri_digest::open_matters_followup;

/// Request body for `GeneralNag::run`. Empty — the trigger only starts the
/// workflow — but kept as a struct so fields can be threaded later without
/// changing the handler signature.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct RunRequest {}

/// Invocation output: whether the run posted the open-matters follow-up.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GeneralNagReport {
    pub posted_followup: bool,
}

/// The daily `#general` jab. Staging disclosure is the Slack adapter's
/// last-line `from Staging` mark, not a rewrite of this slogan.
#[must_use]
pub fn nag_message(_simulated: bool) -> String {
    "Nobody Cares, Work Harder".to_string()
}

/// Service registered with the Restate endpoint. Holds a `SurrealDB` clone
/// (the same connection the worker opened at boot), the target-aware Slack
/// bot, `#general`'s channel id, and whether this deployment discloses
/// simulated matters — resolved once at boot in `workflows-service/src/main.rs`.
#[derive(Clone)]
pub struct GeneralNagService {
    surreal: SurrealDb,
    slack: Arc<dyn SlackBot>,
    channel_id: String,
    simulated: bool,
}

impl GeneralNagService {
    #[must_use]
    pub fn new(
        surreal: SurrealDb,
        slack: Arc<dyn SlackBot>,
        channel_id: String,
        simulated: bool,
    ) -> Self {
        Self {
            surreal,
            slack,
            channel_id,
            simulated,
        }
    }
}

#[restate_sdk::workflow(name = "GeneralNag")]
impl GeneralNagService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        _req: Json<RunRequest>,
    ) -> Result<Json<GeneralNagReport>, HandlerError> {
        if self.channel_id.is_empty() {
            return Err(HandlerError::from(TerminalError::new(
                "SLACK_GENERAL_CHANNEL_ID must be set (the #general channel ID)",
            )));
        }

        let channel_id = self.channel_id.clone();
        let message = nag_message(self.simulated);
        let slack = Arc::clone(&self.slack);
        ctx.run(move || async move {
            slack
                .post_message(&channel_id, &message)
                .await
                .map_err(HandlerError::from)
        })
        .name("notify")
        .await?;

        let mut posted_followup = false;
        if !self.simulated {
            let surreal = self.surreal.clone();
            let (open, pitches) = ctx
                .run(move || async move {
                    store::projects::matter_open_pitch_counts(&surreal)
                        .await
                        .map(Json)
                        .map_err(|error| {
                            HandlerError::from(TerminalError::new(format!("counts: {error}")))
                        })
                })
                .name("counts")
                .await?
                .into_inner();

            if let Some(followup) = open_matters_followup(self.simulated, open, pitches) {
                let channel_id = self.channel_id.clone();
                let slack = Arc::clone(&self.slack);
                ctx.run(move || async move {
                    slack
                        .post_message(&channel_id, &followup)
                        .await
                        .map_err(HandlerError::from)
                })
                .name("notify_counts")
                .await?;
                posted_followup = true;
            }
        }

        Ok(Json(GeneralNagReport { posted_followup }))
    }
}

#[cfg(test)]
mod tests {
    use super::nag_message;
    use crate::dri_digest::open_matters_followup;

    #[test]
    fn simulated_and_production_nags_share_one_slogan() {
        assert_eq!(nag_message(true), nag_message(false));
        assert_eq!(nag_message(false), "Nobody Cares, Work Harder");
        assert!(
            !nag_message(true).contains("staging"),
            "the slogan must not carry a staging parenthetical; the adapter marks staging"
        );
    }

    #[test]
    fn a_simulated_deployment_posts_no_open_matters_followup() {
        assert_eq!(open_matters_followup(true, 3, 1), None);
    }

    #[test]
    fn a_real_deployment_posts_the_open_matters_followup() {
        assert_eq!(
            open_matters_followup(false, 3, 1),
            Some("Total open matters: 3, pitches: 1".to_string())
        );
    }
}
