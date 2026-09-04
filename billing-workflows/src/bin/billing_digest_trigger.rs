//! `billing-digest-trigger` — the thin `CronJob` entrypoint for the daily
//! GCP-cost digest.
//!
//! Fires one `BillingDigest` workflow invocation against the Restate ingress,
//! then exits. The workflow key is the UTC run **date** (`YYYY-MM-DD`), so a
//! same-day re-fire is a no-op: Restate runs a given workflow key at most once.
//! A daily `CronJob` therefore sends exactly one digest per day. The call is
//! one-way (`/send`): Restate runs the query → email steps on the
//! `workflows-service` worker and owns the retry schedule.
//!
//! Built from the shared `images/Containerfile.trigger`
//! (`--build-arg CRATE=billing-workflows --build-arg BIN=billing-digest-trigger`).
//!
//! Auth is the shared [`workflows::start_workflow`] bearer handling — attached
//! only when `RESTATE_AUTH_TOKEN` is present (Restate Cloud); absent in KIND.
//! Identical shape to the `billing-canary` trigger.

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-billing-digest-trigger");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    // Workflow key = UTC run date. Restate admits at most one invocation per
    // key, so a duplicate same-day fire is a free no-op — exactly one digest
    // per day.
    let run_id = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        "BillingDigest",
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .context("triggering BillingDigest workflow")?;

    tracing::info!(%run_id, "billing digest workflow triggered");
    println!("triggered BillingDigest/{run_id}");
    Ok(())
}
