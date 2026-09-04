//! `archives trigger` — the thin `CronJob` entrypoint.
//!
//! Fires one `Archives` workflow invocation against the Restate
//! ingress, then exits. The workflow key is the UTC run date, so a
//! same-day re-fire is idempotent: Restate runs a given workflow key
//! at most once. The call is one-way (`/send`): this process does no
//! work beyond accepting the invocation — Restate owns the retry
//! schedule and runs the snapshot → cost → email steps on the
//! `workflows-service` worker (the `email` step now posts a Slack digest).
//!
//! Auth: prod Restate Cloud authenticates every ingress call with the
//! tenant bearer (`RESTATE_AUTH_TOKEN`); the in-cluster KIND Operator
//! does not. The shared [`workflows::start_workflow`] helper attaches
//! the header only when the token is present and non-empty, so the
//! same binary works in both environments.

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-archives-trigger");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    // Optional bearer — present only when targeting Restate Cloud.
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    // Workflow key = UTC run date. Restate admits at most one
    // invocation per workflow key, so a duplicate nightly fire is a
    // no-op rather than a double snapshot.
    let run_id = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        "Archives",
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .context("triggering Archives workflow")?;

    tracing::info!(%run_id, "archives workflow triggered");
    println!("triggered Archives/{run_id}");
    Ok(())
}
