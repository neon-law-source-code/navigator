//! The `DriDigest` Restate workflow — a nightly Slack notice listing every
//! project with its lawyer DRI and client DRI.
//!
//! Two durable steps, journaled independently — a notify retry must never
//! re-run the query, and a query retry must never re-post the notice:
//!
//! 1. `ctx.run("query")` — read every project's [`store::projects::dri_digest`]
//!    summary: both DRI sides, so an unassigned side on either is visible
//!    from the notice itself rather than requiring a separate admin-tier
//!    directory read.
//! 2. `ctx.run("notify")` — render the summary as mrkdwn and post it to firm
//!    ops through the worker's [`Notifier`]. Posts to the notifier directly,
//!    like `Archives` and `Heartbeat`, rather than through the
//!    `SlackOpsDelivery` code-fence adapter: the digest names people and
//!    projects as a bulleted list, not a fixed-width table, so it needs no
//!    fencing.
//!
//! **Internal operations signal**, not client correspondence: it names
//! matters and the firm's own people by their already-internal DRI markers,
//! posted only to the firm ops Slack channel — never emailed to a client or
//! matter contact.
//!
//! The `dri-digest-trigger` `CronJob` fires nightly at 01:11 UTC, keyed on
//! the UTC run date so a same-day double-fire is a no-op.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use store::projects::ProjectDriSummary;
use store::surreal::SurrealDb;
use workflows::Notifier;

/// Request body for `DriDigest::run`. Empty — the trigger only starts the
/// workflow — but kept as a struct so fields can be threaded later without
/// changing the handler signature.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct RunRequest {}

/// Invocation output: how many projects the digest covered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DriDigestReport {
    pub projects: usize,
}

/// Render the nightly digest: a bold header naming the project count, then
/// one bulleted line per project — code, name, and each DRI side, "none" when
/// a side is unassigned so the gap reads as a gap rather than a blank. Pure
/// and exposed so the message is unit-tested without a workflow context.
#[must_use]
pub fn dri_digest_message(projects: &[ProjectDriSummary]) -> String {
    use std::fmt::Write as _;

    let mut out = format!("*Project DRI digest — {} projects*\n", projects.len());
    for project in projects {
        let lawyer = names_or_none(&project.lawyer_dris);
        let client = names_or_none(&project.client_dris);
        let _ = writeln!(
            out,
            "• *{}* ({}) — Lawyer DRI: {lawyer}; Client DRI: {client}",
            project.code, project.status
        );
    }
    out.trim_end().to_string()
}

fn names_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// Service registered with the Restate endpoint. Holds a `SurrealDB` clone (the
/// same connection the worker opened at boot, same shape as
/// `ReconcileInvoicesService`) and the worker-side Slack [`Notifier`].
#[derive(Clone)]
pub struct DriDigestService {
    surreal: SurrealDb,
    notifier: Arc<dyn Notifier>,
}

impl DriDigestService {
    #[must_use]
    pub fn new(surreal: SurrealDb, notifier: Arc<dyn Notifier>) -> Self {
        Self { surreal, notifier }
    }
}

#[restate_sdk::workflow(name = "DriDigest")]
impl DriDigestService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        _req: Json<RunRequest>,
    ) -> Result<Json<DriDigestReport>, HandlerError> {
        // Step 1 — query every project's DRIs. A database error surfaces as a
        // retryable `HandlerError` so Restate replays just this step.
        let surreal = self.surreal.clone();
        let projects: Vec<ProjectDriSummary> = ctx
            .run(move || async move { Ok(Json(store::projects::dri_digest(&surreal).await?)) })
            .name("query")
            .await?
            .into_inner();

        // Step 2 — render + post, journaled separately so a query retry never
        // re-posts and a notify retry never re-reads the database.
        let message = dri_digest_message(&projects);
        let notifier = Arc::clone(&self.notifier);
        ctx.run(move || async move { notifier.notify(message).await.map_err(HandlerError::from) })
            .name("notify")
            .await?;

        Ok(Json(DriDigestReport {
            projects: projects.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{dri_digest_message, ProjectDriSummary};

    fn project(
        code: &str,
        status: &str,
        lawyer_dris: &[&str],
        client_dris: &[&str],
    ) -> ProjectDriSummary {
        ProjectDriSummary {
            code: code.into(),
            name: code.into(),
            status: status.into(),
            lawyer_dris: lawyer_dris.iter().map(ToString::to_string).collect(),
            client_dris: client_dris.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn digest_names_both_dri_sides_per_project() {
        let msg = dri_digest_message(&[project(
            "sample-litigation",
            "open",
            &["Jane Roe"],
            &["Cruller Client"],
        )]);
        assert!(msg.starts_with("*Project DRI digest — 1 projects*"));
        assert!(
            msg.contains(
                "• *sample-litigation* (open) — Lawyer DRI: Jane Roe; Client DRI: Cruller Client"
            ),
            "{msg}"
        );
    }

    #[test]
    fn an_unassigned_side_reads_as_none_rather_than_blank() {
        let msg = dri_digest_message(&[project("sample-estate", "open", &[], &[])]);
        assert!(
            msg.contains("Lawyer DRI: none; Client DRI: none"),
            "an empty side must say so: {msg}"
        );
    }

    #[test]
    fn multiple_dris_on_one_side_are_comma_joined() {
        let msg = dri_digest_message(&[project(
            "sample-transactional",
            "open",
            &["Jane Roe", "John Doe"],
            &[],
        )]);
        assert!(msg.contains("Lawyer DRI: Jane Roe, John Doe"), "{msg}");
    }

    #[test]
    fn header_counts_every_project_and_body_lists_each_one() {
        let msg = dri_digest_message(&[
            project("a", "open", &["Lawyer A"], &[]),
            project("b", "closed", &[], &["Client B"]),
        ]);
        assert!(msg.starts_with("*Project DRI digest — 2 projects*"));
        assert!(msg.contains("• *a* (open)"));
        assert!(msg.contains("• *b* (closed)"));
    }

    #[test]
    fn no_projects_still_renders_a_clean_header() {
        let msg = dri_digest_message(&[]);
        assert_eq!(msg, "*Project DRI digest — 0 projects*");
    }
}
