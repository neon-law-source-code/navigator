//! `general-nag` — the thin `CronJob` entrypoint for the daily `GeneralNag`
//! workflow.
//!
//! Fires one `GeneralNag` invocation against the Restate ingress, then exits.
//! The workflow key is the UTC run date, so a same-day re-fire is a no-op:
//! Restate admits at most one invocation per workflow key. The call is
//! one-way (`/send`): this process does no work beyond accepting the
//! invocation — Restate owns the retry schedule and runs the notify →
//! (optional) counts steps on the `workflows-service` worker. Built from the
//! shared `images/Containerfile.trigger`
//! (`--build-arg CRATE=workflows-service --build-arg BIN=general-nag`).
//!
//! Cadence: daily at 01:11 UTC (the `general-nag` `CronJob` schedule) —
//! the same minute as `dri-digest-trigger`. Both are thin, sub-second calls
//! to the Restate ingress for different workflows, so the shared minute
//! costs nothing beyond the coincidence.
//!
//! Auth: prod Restate Cloud authenticates every ingress call with the tenant
//! bearer (`RESTATE_AUTH_TOKEN`); the in-cluster KIND Operator does not. The
//! shared [`workflows::start_workflow`] helper attaches the header only when
//! the token is present and non-empty, so the same binary works in both.

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-general-nag");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    // Optional bearer — present only when targeting Restate Cloud.
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    // Workflow key = UTC run date. Restate admits at most one invocation per
    // workflow key, so a duplicate nightly fire is a no-op rather than a
    // second jab.
    let run_id = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        "GeneralNag",
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .context("triggering GeneralNag workflow")?;

    tracing::info!(%run_id, "general nag workflow triggered");
    println!("triggered GeneralNag/{run_id}");
    Ok(())
}
