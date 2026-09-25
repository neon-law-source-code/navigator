//! `navigator ops email-summary quarantined|reconcile-confirmed|authorize-resend`
//! — the operator recovery path out of a `UNKNOWN` Slack delivery (ENG-841).
//!
//! `store::email_deliveries::reconcile_confirmed` and `authorize_resend` are
//! both audited and both tested, but until this module existed nothing
//! called either one: clearing a quarantined delivery required a hand-edit
//! against the database. This module only calls those two functions (and a
//! read-only listing of what is quarantined); it does not change which
//! errors get classified into `UNKNOWN` in the first place — that
//! classification, in `workflows::email_summary_delivery::deliver_summary`,
//! is out of scope here.
//!
//! Diagnostics here are status words and delivery coordinates only — receipt
//! id, channel id, Slack timestamp, state, and the operator-supplied actor —
//! never a summary, a letter, or any client content.

use anyhow::{Context, Result};
use uuid::Uuid;

/// `navigator ops email-summary quarantined` — list every delivery
/// currently sitting in `UNKNOWN`, most-recently-updated first. Not
/// actionable without this: nothing else surfaces which sends are in doubt.
pub fn quarantined() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(quarantined_async())
}

async fn quarantined_async() -> Result<()> {
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    let deliveries =
        store::email_deliveries::list_by_state(&surreal, store::email_deliveries::UNKNOWN)
            .await
            .context("list quarantined deliveries")?;
    if deliveries.is_empty() {
        println!("no quarantined deliveries");
        return Ok(());
    }
    for delivery in deliveries {
        println!(
            "receipt={} channel={} last_error={} updated_at={}",
            delivery.receipt_id,
            delivery.channel_id,
            delivery.last_error.as_deref().unwrap_or("(none)"),
            delivery.updated_at,
        );
    }
    Ok(())
}

/// `navigator ops email-summary reconcile-confirmed --receipt <uuid> --actor
/// <actor> --channel <id> --timestamp <ts>` — record that a quarantined
/// send actually reached Slack, so replay stops treating it as in doubt.
pub fn reconcile_confirmed(
    receipt: Uuid,
    actor: String,
    channel: String,
    timestamp: String,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(reconcile_confirmed_async(
        receipt, actor, channel, timestamp,
    ))
}

async fn reconcile_confirmed_async(
    receipt_id: Uuid,
    actor: String,
    channel_id: String,
    timestamp: String,
) -> Result<()> {
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    let delivery = store::email_deliveries::reconcile_confirmed(
        &surreal,
        receipt_id,
        &actor,
        &channel_id,
        &timestamp,
    )
    .await
    .context("reconcile the delivery as confirmed")?;
    println!("receipt={} state={}", delivery.receipt_id, delivery.state);
    Ok(())
}

/// `navigator ops email-summary authorize-resend --receipt <uuid> --actor
/// <actor>` — record operator authorization to resend a quarantined
/// delivery. Only unblocks the next `admit_attempt`; it does not itself
/// resend anything.
pub fn authorize_resend(receipt: Uuid, actor: String) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(authorize_resend_async(receipt, actor))
}

async fn authorize_resend_async(receipt_id: Uuid, actor: String) -> Result<()> {
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    let delivery = store::email_deliveries::authorize_resend(&surreal, receipt_id, &actor)
        .await
        .context("authorize the resend")?;
    println!("receipt={} state={}", delivery.receipt_id, delivery.state);
    Ok(())
}
