//! Guard the #navigator deploy narration: every forward-path step of the
//! release pipeline opens with a `slack-progress` post.
//!
//! A release runs ~45 minutes and would otherwise say nothing until it either
//! reported success or paged engineering. The narration closes that window —
//! but only while it stays complete: one step added without a post is one place
//! a failure can hide, and nobody notices a *missing* Slack line. So the
//! completeness is asserted here rather than left to review.
//!
//! `deploy.yml` is the one narrated workflow: the publish run that builds the
//! images and hands them to the operator. Nothing else talks to #navigator.
//!
//! Three categories are deliberately exempt, and this file encodes all three:
//!
//! - steps before the job's `actions/checkout`, because the post is a local
//!   composite action and is not on disk yet;
//! - steps gated on `failure()` or `always()`, which are post-mortem
//!   diagnostics rather than progress — `notify-failure` covers that moment;
//! - `release-version`, the job that DECIDES whether this commit is a release.
//!   Every merge to `main` starts this workflow and almost none of them publish,
//!   so narrating that job's steps would post two lines on every merge about a
//!   release that is not happening. It gets its own guard instead —
//!   `the_decision_job_narrates_once_and_only_for_a_real_release` — which is
//!   stricter than the general rule rather than a hole in it.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

/// The action every narrated step calls.
const PROGRESS_ACTION: &str = "./.github/actions/slack-progress";

/// The release jobs that narrate every forward-path step. `notify` and
/// `notify-failure` are the Slack surface itself — narrating them would post
/// about posting — and `release-version` narrates once rather than per step,
/// because it runs on every merge.
const NARRATED_JOBS: &[&str] = &[
    "build",
    "integration",
    "publish-service",
    "publish-triggers",
    "release-windows-cli-build",
    "release-cli-build-linux",
    "release-cli-build-macos",
    "release-windows-cli-publish",
    "release-homebrew-tap",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root is cli/'s parent")
        .to_path_buf()
}

fn workflow_at(relative: &str) -> Value {
    let path = repo_root().join(relative);
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&raw).unwrap_or_else(|error| panic!("{relative} parses as YAML: {error}"))
}

fn deploy_workflow() -> Value {
    workflow_at(".github/workflows/deploy.yml")
}

/// Every narrated job, paired with the workflow it lives in, so each assertion
/// below reads the same whether one workflow narrates or several do.
fn narrated() -> Vec<(Value, &'static str)> {
    NARRATED_JOBS
        .iter()
        .map(|job| (deploy_workflow(), *job))
        .collect()
}

fn steps<'a>(workflow: &'a Value, job: &str) -> &'a Vec<Value> {
    workflow["jobs"][job]["steps"]
        .as_sequence()
        .unwrap_or_else(|| panic!("job `{job}` has a steps list"))
}

fn uses(step: &Value) -> &str {
    step.get("uses").and_then(Value::as_str).unwrap_or_default()
}

fn name(step: &Value) -> &str {
    step.get("name").and_then(Value::as_str).unwrap_or_default()
}

fn condition(step: &Value) -> &str {
    step.get("if").and_then(Value::as_str).unwrap_or_default()
}

/// A diagnostic runs only once the job has already failed; it reports on a
/// deploy rather than advancing one.
fn is_diagnostic(step: &Value) -> bool {
    matches!(condition(step).trim(), "failure()" | "always()")
}

#[test]
fn every_forward_path_step_opens_with_a_navigator_progress_post() {
    for (workflow, job) in narrated() {
        let steps = steps(&workflow, job);
        let checkout = steps
            .iter()
            .position(|step| uses(step).starts_with("actions/checkout"))
            .unwrap_or_else(|| {
                panic!("job `{job}` narrates, so it must check out the local action")
            });

        for (index, step) in steps.iter().enumerate() {
            if index < checkout || uses(step) == PROGRESS_ACTION || is_diagnostic(step) {
                continue;
            }
            let Some(label) = step.get("name").and_then(Value::as_str) else {
                // An unnamed `uses:` step is setup plumbing (buildx, gcloud);
                // it has no name to narrate.
                continue;
            };
            let preceding = index
                .checked_sub(1)
                .map(|prev| uses(&steps[prev]))
                .unwrap_or_default();
            assert_eq!(
                preceding, PROGRESS_ACTION,
                "`{job}` step `{label}` is not preceded by a #navigator progress post — \
                 a step nobody sees start is a step a failure can hide in"
            );
        }
    }
}

/// The post is a local composite action: it cannot run before the tree it
/// lives in is on disk.
#[test]
fn no_progress_post_runs_before_its_job_checks_the_tree_out() {
    for (workflow, job) in narrated() {
        let mut checked_out = false;
        for step in steps(&workflow, job) {
            if uses(step).starts_with("actions/checkout") {
                checked_out = true;
            }
            if uses(step) == PROGRESS_ACTION {
                assert!(
                    checked_out,
                    "`{job}` posts progress for `{}` before checking out the action",
                    step["with"]["step"].as_str().unwrap_or("?")
                );
            }
        }
    }
}

/// Every post carries the webhook. Losing it turns a narrated deploy silent.
#[test]
fn every_progress_post_is_wired_to_the_webhook() {
    let mut posts = 0;

    for (workflow, job) in narrated() {
        for step in steps(&workflow, job) {
            if uses(step) != PROGRESS_ACTION {
                continue;
            }
            posts += 1;
            let with = &step["with"];
            assert_eq!(
                with["webhook-url"].as_str(),
                Some("${{ secrets.SLACK_WEBHOOK_URL }}"),
                "`{job}` progress post must read the prod ops webhook secret"
            );
            assert!(
                with["stage"].as_str().is_some_and(|s| !s.is_empty()),
                "`{job}` progress post must name its stage"
            );
            assert!(
                with["step"].as_str().is_some_and(|s| !s.is_empty()),
                "`{job}` progress post must name its step"
            );
        }
    }

    assert!(
        posts > 40,
        "the pipeline narrates every forward-path step; got only {posts} posts"
    );
}

/// THE DECISION JOB NARRATES ONCE, AND ONLY WHEN THERE IS A RELEASE.
///
/// `release-version` is the one job that runs on every push to `main`, and
/// almost every push is not a release. A post ahead of its answer would narrate
/// ~95% of merges as releases that then silently stop — so its single post comes
/// AFTER the decision and is gated on it.
///
/// Both halves matter. Without a post the whole release goes unannounced until
/// `build` starts; without the gate #navigator gets two lines per merge and
/// stops being read.
#[test]
fn the_decision_job_narrates_once_and_only_for_a_real_release() {
    let workflow = deploy_workflow();
    let steps = steps(&workflow, "release-version");

    let posts: Vec<&Value> = steps
        .iter()
        .filter(|step| uses(step) == PROGRESS_ACTION)
        .collect();
    assert_eq!(
        posts.len(),
        1,
        "release-version must narrate exactly once: it runs on every merge to `main`"
    );

    assert_eq!(
        condition(posts[0]).trim(),
        "steps.version.outputs.publishable == 'true'",
        "release-version's post must be gated on the decision it just made, or #navigator gets \
         two lines for every merge that publishes nothing"
    );

    // After the decision, not before it — a post ahead of the answer cannot be
    // gated on the answer.
    let decision = steps
        .iter()
        .position(|step| {
            step.get("run")
                .and_then(Value::as_str)
                .is_some_and(|run| run.contains("ops release check"))
        })
        .expect("release-version must run `ops release check`");
    let post = steps
        .iter()
        .position(|step| uses(step) == PROGRESS_ACTION)
        .expect("release-version must narrate once");
    assert!(
        post > decision,
        "the post must follow the decision it reports"
    );
}

/// The Slack surface is the report jobs' own business, in both workflows. A
/// progress post there would narrate the narration.
#[test]
fn the_report_jobs_are_not_narrated() {
    for workflow in [deploy_workflow()] {
        for job in ["notify", "notify-failure"] {
            for step in steps(&workflow, job) {
                assert_ne!(
                    uses(step),
                    PROGRESS_ACTION,
                    "`{job}` is the Slack surface itself and must not narrate: `{}`",
                    name(step)
                );
            }
        }
    }
}

/// PUBLISHING RUNS FROM A PUSH TO `main`, so the gate is a ref test on `main`.
///
/// The gate has to track the trigger exactly, and it fails silently in both
/// directions. Too narrow and a real release goes unnarrated — every post
/// skipped, the whole publish invisible in #navigator, which is exactly what a
/// surviving `refs/tags/*` test would do now that the tag is something this
/// pipeline CREATES rather than runs from. Too wide and a `kind-ci/**` iteration
/// posts a release report for images it never pushed.
///
/// Read from the run rather than from a caller-supplied flag, so a new call site
/// cannot forget it.
#[test]
fn the_progress_gate_admits_exactly_the_publishing_ref() {
    let path = repo_root().join(".github/actions/slack-progress/action.yml");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

    assert!(
        raw.contains("refs/heads/main)"),
        "slack-progress must admit `refs/heads/main`: a landed version bump is the only thing that \
         publishes, and a gate that does not name it silences every release"
    );
    assert!(
        !raw.contains("refs/tags/*)"),
        "the gate must not still admit a tag ref: deploy.yml carries no tag trigger, so a tag arm \
         can only match a ref this pipeline created itself"
    );
    assert!(
        !raw.contains("schedule | workflow_dispatch)"),
        "the event arms must go with the triggers they admitted: nothing publishes on a clock or a \
         dispatch any more, so a gate naming them can only admit a run that publishes nothing"
    );
    assert!(
        !raw.contains("inputs.force") && !raw.contains("inputs.gate"),
        "the gate must be read from the run, not handed in by the caller — a per-call-site flag \
         is a gate a new step can forget"
    );
}

/// A release must never be lost to its own narration. The action reports a
/// failed post as a warning and exits 0 on every path.
#[test]
fn a_failed_progress_post_never_fails_the_deploy() {
    let path = repo_root().join(".github/actions/slack-progress/action.yml");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

    assert!(
        !raw.contains("exit 1"),
        "slack-progress must not exit non-zero: a lost Slack line is not a lost release"
    );
    assert!(
        raw.contains("::warning::"),
        "a failed post must still be visible in the run log as a warning"
    );
    assert!(
        !raw.contains("set -euo"),
        "`set -e` would turn a curl failure into a failed deploy step"
    );
}
