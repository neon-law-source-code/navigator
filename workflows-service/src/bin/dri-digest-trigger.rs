//! `dri-digest-trigger` — the thin `CronJob` entrypoint for the nightly
//! `DriDigest` workflow.
//!
//! Fires one `DriDigest` invocation against the Restate ingress, then exits.
//! The workflow key is the UTC run date, so a same-day re-fire is a no-op:
//! Restate admits at most one invocation per workflow key. The call is
//! one-way (`/send`): this process does no work beyond accepting the
//! invocation — Restate owns the retry schedule and runs the query → notify
//! steps on the `workflows-service` worker. Built from the shared
//! `images/Containerfile.trigger`
//! (`--build-arg CRATE=workflows-service --build-arg BIN=dri-digest-trigger`).
//!
//! Cadence: nightly at 01:11 UTC (the `dri-digest-trigger` `CronJob`
//! schedule) — off the top of the hour and clear of every other nightly
//! trigger (archives 10:00, reconcile-invoices, billing-digest 13:00, canary
//! Sunday 14:00), so a shared-resource hiccup on the hour never lines up with
//! this one.
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
    let _telemetry = telemetry::init("navigator-dri-digest-trigger");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    // Optional bearer — present only when targeting Restate Cloud.
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    // Workflow key = UTC run date. Restate admits at most one invocation per
    // workflow key, so a duplicate nightly fire is a no-op rather than a
    // second digest post.
    let run_id = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        "DriDigest",
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .context("triggering DriDigest workflow")?;

    tracing::info!(%run_id, "dri digest workflow triggered");
    println!("triggered DriDigest/{run_id}");
    Ok(())
}
