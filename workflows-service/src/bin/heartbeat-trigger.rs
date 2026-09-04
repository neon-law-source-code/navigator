//! `heartbeat-trigger` — the thin `CronJob` entrypoint for a durable
//! heartbeat workflow.
//!
//! Fires one `Heartbeat` invocation against the Restate ingress by default,
//! then exits. The automation home may select the separately bound
//! `GitHubAutomationHeartbeat` authority canary through
//! `HEARTBEAT_WORKFLOW_SERVICE`.
//! Built from the shared `images/Containerfile.trigger`
//! (`--build-arg CRATE=workflows-service --build-arg BIN=heartbeat-trigger`).
//!
//! Cadence is every six hours (the `CronJob` schedule). The workflow key is
//! the UTC **date + hour** (`%Y-%m-%d-%H`), not the date alone: Restate admits
//! at most one invocation per workflow key, so a date-only key would dedupe
//! three of the four daily runs into no-ops. Date+hour gives each six-hour
//! slot its own key while still making a same-hour double-fire idempotent.
//!
//! The call is one-way (`/send`): this process does no work beyond accepting
//! the invocation — Restate owns the retry schedule and runs the beat → notify
//! steps on the `workflows-service` worker.
//!
//! Auth: prod Restate Cloud authenticates every ingress call with the tenant
//! bearer (`RESTATE_AUTH_TOKEN`); the in-cluster KIND Operator does not. The
//! shared [`workflows::start_workflow`] helper attaches the header only when
//! the token is present and non-empty, so the same binary works in both.

use anyhow::{anyhow, Context, Result};

const DEFAULT_WORKFLOW_SERVICE: &str = "Heartbeat";
const GITHUB_AUTOMATION_WORKFLOW_SERVICE: &str = "GitHubAutomationHeartbeat";

fn heartbeat_workflow_service(value: Option<&str>) -> Result<&'static str> {
    match value {
        None | Some(DEFAULT_WORKFLOW_SERVICE) => Ok(DEFAULT_WORKFLOW_SERVICE),
        Some(GITHUB_AUTOMATION_WORKFLOW_SERVICE) => Ok(GITHUB_AUTOMATION_WORKFLOW_SERVICE),
        Some(value) => Err(anyhow!(
            "HEARTBEAT_WORKFLOW_SERVICE must be {DEFAULT_WORKFLOW_SERVICE} or {GITHUB_AUTOMATION_WORKFLOW_SERVICE}, got {value}"
        )),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-heartbeat-trigger");

    let ingress = std::env::var("RESTATE_INGRESS_URL")
        .context("RESTATE_INGRESS_URL must be set (the Restate ingress endpoint)")?;
    // Optional bearer — present only when targeting Restate Cloud.
    let auth_token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    let workflow_service =
        heartbeat_workflow_service(std::env::var("HEARTBEAT_WORKFLOW_SERVICE").ok().as_deref())?;
    // Workflow key = UTC date + hour, so each six-hour slot is a distinct
    // invocation while a duplicate fire within the same hour is a no-op.
    let run_id = chrono::Utc::now().format("%Y-%m-%d-%H").to_string();

    let _response = workflows::start_workflow(
        &ingress,
        auth_token.as_deref(),
        workflow_service,
        &run_id,
        "run",
        &serde_json::json!({}),
        true, // one-way: accept the invocation and exit; Restate runs it.
    )
    .await
    .with_context(|| format!("triggering {workflow_service} workflow"))?;

    tracing::info!(%workflow_service, %run_id, "heartbeat workflow triggered");
    println!("triggered {workflow_service}/{run_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        heartbeat_workflow_service, DEFAULT_WORKFLOW_SERVICE, GITHUB_AUTOMATION_WORKFLOW_SERVICE,
    };

    #[test]
    fn trigger_accepts_only_the_known_heartbeat_workflows() {
        assert_eq!(
            heartbeat_workflow_service(None).unwrap(),
            DEFAULT_WORKFLOW_SERVICE
        );
        assert_eq!(
            heartbeat_workflow_service(Some(GITHUB_AUTOMATION_WORKFLOW_SERVICE)).unwrap(),
            GITHUB_AUTOMATION_WORKFLOW_SERVICE
        );
        assert!(heartbeat_workflow_service(Some("DevxPr")).is_err());
    }
}
