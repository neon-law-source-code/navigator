//! `billing-reconcile trigger` — the thin nightly `CronJob` entrypoint.
//!
//! Fires one `ReconcileInvoices` workflow invocation against the Restate
//! ingress, then exits. The workflow key is the UTC run date, so a
//! same-day re-fire is idempotent: Restate runs a given workflow key at
//! most once. The call is one-way (`/send`) — Restate runs the reconcile
//! on the `workflows-service` worker and owns the retry schedule.
//!
//! Auth + env handling are identical to the canary trigger
//! (`src/bin/trigger.rs`): the shared [`workflows::start_workflow`] helper
//! attaches the `RESTATE_AUTH_TOKEN` bearer only when present, so the same
//! binary works against KIND (no auth) and Restate Cloud (bearer).

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-reconcile-invoices-trigger");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    let run_id = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        "ReconcileInvoices",
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .context("triggering ReconcileInvoices workflow")?;

    tracing::info!(%run_id, "reconcile-invoices workflow triggered");
    println!("triggered ReconcileInvoices/{run_id}");
    Ok(())
}
