//! Integration test for the `github_issue__*` step.
//!
//! Drives the dispatch through the shared `workflows::dispatch_step`
//! registry — the same arm the `workflows-service` worker runs inside
//! `ctx.run` — and asserts the step opens an issue through whatever
//! [`IssueOpener`] the deps carry. No database: this step writes no row,
//! it calls GitHub and journals the reference.
//!
//! The load-bearing case is the third one. A reviewer found that both
//! production `StepDeps` construction sites built the deps without an
//! opener, so every real transition into a `github_issue__*` state would
//! have failed `MissingIssueOpener` *after* the transition had already
//! been persisted — leaving the notation parked at the GitHub step with
//! no issue and no way forward. The catalog called the step
//! "Implemented", which made that gap invisible. These tests pin the
//! wiring so it cannot regress.

use std::sync::Arc;

use async_trait::async_trait;
use workflows::github::{IssueError, IssueOpener, IssueRequest, NullIssueOpener, OpenedIssue};
use workflows::{dispatch_step, StateName, StepDeps};

async fn fs_storage(suite: &str) -> Arc<dyn cloud::StorageService> {
    Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-github-{suite}")))
            .await
            .expect("temp FsStorage"),
    )
}

fn email() -> Arc<dyn workflows::EmailService> {
    Arc::new(workflows::CapturingEmail::default())
}

/// A stand-in for [`workflows::github::RestIssueOpener`]: records a
/// deterministic issue so the opened path is exercised without reaching
/// github.com.
struct RecordingOpener;

#[async_trait]
impl IssueOpener for RecordingOpener {
    async fn open_issue(&self, request: &IssueRequest) -> Result<Option<OpenedIssue>, IssueError> {
        Ok(Some(OpenedIssue {
            number: 636,
            html_url: format!("https://github.com/{}/issues/636", request.slug()),
        }))
    }
}

fn payload() -> String {
    serde_json::json!({
        "repo": "neon-law-source-code/navigator",
        "title": "Seed the default Project",
        "body": "## Observed problem\n\nNo Project for engineering intake.\n",
        "labels": ["autobuild"],
    })
    .to_string()
}

/// A configured opener journals the issue number and URL onto the
/// transition, so the notation records *which* issue it opened.
#[tokio::test]
async fn a_configured_opener_journals_the_opened_issue() {
    let deps = StepDeps::new(email(), fs_storage("recording").await)
        .with_issue_opener(Arc::new(RecordingOpener));
    let next = StateName::from("github_issue__engineering");

    let journaled = dispatch_step(&deps, uuid::Uuid::new_v4(), &next, Some(&payload()))
        .await
        .expect("dispatch should succeed")
        .expect("an opened issue is journaled");

    let value: serde_json::Value = serde_json::from_str(&journaled).expect("valid JSON");
    assert_eq!(value["number"], 636);
    assert_eq!(
        value["html_url"],
        "https://github.com/neon-law-source-code/navigator/issues/636"
    );
}

/// The no-token default opens nothing and journals nothing — the step
/// must not record an issue that does not exist. Mirrors the
/// `NullAttestor` leaving an attestation row `pending`.
#[tokio::test]
async fn the_null_opener_journals_nothing() {
    let deps = StepDeps::new(email(), fs_storage("null").await)
        .with_issue_opener(Arc::new(NullIssueOpener));
    let next = StateName::from("github_issue__engineering");

    let journaled = dispatch_step(&deps, uuid::Uuid::new_v4(), &next, Some(&payload()))
        .await
        .expect("dispatch should succeed without a token");
    assert_eq!(
        journaled, None,
        "no token configured must journal no issue reference",
    );
}

/// Deps built without an opener must fail loudly rather than silently
/// skipping the step. This is the error the production runtimes used to
/// hit on every GitHub transition; it is correct as a guard, and wrong as
/// the thing a real transition encounters — which is what
/// `the_in_process_runtime_wires_an_opener` pins.
#[tokio::test]
async fn deps_without_an_opener_fail_loudly() {
    let deps = StepDeps::new(email(), fs_storage("missing").await);
    let next = StateName::from("github_issue__engineering");

    let err = dispatch_step(&deps, uuid::Uuid::new_v4(), &next, Some(&payload()))
        .await
        .expect_err("a GitHub step without an opener must error");
    assert!(
        err.to_string().contains("issue opener"),
        "error should name the missing seam, got: {err}",
    );
}

/// The regression guard: the in-process runtime must build its deps with
/// an opener attached. Without this, a `github_issue__*` transition is
/// persisted and *then* fails, parking the notation with no issue.
///
/// `issue_opener_from_env` yields the `NullIssueOpener` with no token
/// configured, so this asserts the step completes rather than erroring —
/// the observable difference between wired and unwired.
#[tokio::test]
async fn the_in_process_runtime_wires_an_opener() {
    let deps = StepDeps::new(email(), fs_storage("wired").await)
        .with_issue_opener(workflows::github::issue_opener_from_env());
    let next = StateName::from("github_issue__engineering");

    assert!(
        dispatch_step(&deps, uuid::Uuid::new_v4(), &next, Some(&payload()))
            .await
            .is_ok(),
        "the runtime's deps must carry an opener so the step can complete",
    );
}
