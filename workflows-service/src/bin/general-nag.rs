//! `general-nag` — the thin, self-contained `CronJob` for a daily jab at
//! `#general`.
//!
//! Flavor B (see `docs/cronjobs.md`): a one-shot batch that does the whole
//! job itself and exits, rather than a Flavor-A trigger that POSTs to the
//! Restate ingress and lets a durable workflow run the steps. A single
//! static Slack post has no multi-step state to lose or duplicate, so there
//! is nothing for Restate to buy here. Built from the shared
//! `images/Containerfile.trigger`
//! (`--build-arg CRATE=workflows-service --build-arg BIN=general-nag`).
//!
//! Posts through the Slack Web API bot client (`SLACK_BOT_TOKEN`), not the
//! fixed-destination incoming webhook (`SLACK_WEBHOOK_URL`) the ops
//! liveness signals use — that webhook is pinned to the engineering
//! channel, and this message targets `#general` by its channel ID
//! (`SLACK_GENERAL_CHANNEL_ID`), which is the stable posting coordinate.
//! Both env vars are required: a missing one fails the run loudly rather
//! than silently skipping the post, since posting is this job's entire
//! purpose.
//!
//! Cadence: daily at 01:11 UTC (the `general-nag` `CronJob` schedule) —
//! the same minute as `dri-digest-trigger`. Both are thin, sub-second calls
//! to different destinations (Slack Web API vs. the Restate ingress), so
//! the shared minute costs nothing beyond the coincidence.

use anyhow::{Context, Result};
use workflows::{SlackBot, SlackBotClient};

const MESSAGE: &str = "Nobody Cares, Work Harder";

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-general-nag");

    let token = std::env::var("SLACK_BOT_TOKEN")
        .context("SLACK_BOT_TOKEN must be set (the Slack Web API bot token)")?;
    let channel_id = std::env::var("SLACK_GENERAL_CHANNEL_ID")
        .context("SLACK_GENERAL_CHANNEL_ID must be set (the #general channel ID)")?;

    let bot = SlackBotClient::new(token);
    bot.post_message(&channel_id, MESSAGE)
        .await
        .context("posting the daily #general nag")?;

    tracing::info!(%channel_id, "general nag posted");
    println!("posted to {channel_id}: {MESSAGE}");
    Ok(())
}
