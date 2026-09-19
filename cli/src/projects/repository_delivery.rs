//! Gate, publish, and watch one Project-repository pull request.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use cloud::forge::{ForgeCheck, ForgePullRequestStatus, GitHubPullRequestForge, PullRequestForge};

const BASE_BRANCH: &str = "main";

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeliveryVerdict {
    Merged,
    AutoMergeNotArmed,
    FailedRequiredChecks(Vec<String>),
    RequiredReview(String),
    OutdatedBranch,
    TimedOut(Vec<String>),
}

/// Deliver the current clean Project-repository checkout to a bounded verdict.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    root: &Path,
    branch: &str,
    title: &str,
    body_file: Option<&Path>,
    repository: Option<&str>,
    timeout: Duration,
    poll_interval: Duration,
) -> ExitCode {
    match deliver(
        root,
        branch,
        title,
        body_file,
        repository,
        timeout,
        poll_interval,
    )
    .await
    {
        Ok((url, DeliveryVerdict::Merged)) => {
            println!("merged     {url}");
            ExitCode::SUCCESS
        }
        Ok((url, DeliveryVerdict::AutoMergeNotArmed)) => {
            eprintln!("navigator: auto-merge was reported successful but is not armed for {url}");
            ExitCode::from(1)
        }
        Ok((url, DeliveryVerdict::FailedRequiredChecks(checks))) => {
            eprintln!(
                "navigator: required check(s) failed for {url}: {}",
                checks.join(", ")
            );
            ExitCode::from(1)
        }
        Ok((url, DeliveryVerdict::RequiredReview(decision))) => {
            eprintln!("navigator: required review holds {url}: {decision}");
            ExitCode::from(1)
        }
        Ok((url, DeliveryVerdict::OutdatedBranch)) => {
            eprintln!("navigator: branch is behind `{BASE_BRANCH}` for {url}");
            ExitCode::from(1)
        }
        Ok((url, DeliveryVerdict::TimedOut(pending))) => {
            let detail = if pending.is_empty() {
                "forge has not merged it".to_string()
            } else {
                format!("pending required check(s): {}", pending.join(", "))
            };
            eprintln!("navigator: delivery timed out for {url}: {detail}");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn deliver(
    root: &Path,
    branch: &str,
    title: &str,
    body_file: Option<&Path>,
    repository: Option<&str>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(String, DeliveryVerdict)> {
    validate_branch_name(root, branch)?;
    require_clean(root)?;
    let coordinate = match repository {
        Some(repository) => validate_coordinate(repository)?,
        None => repository_coordinate(root)?,
    };
    run_gate(root, &coordinate)?;
    require_clean(root)?;
    select_topic_branch(root, branch)?;
    git_status(root, &["fetch", "origin", BASE_BRANCH], "fetch origin/main")?;
    require_signed_topic_commits(root)?;
    git_status(
        root,
        &[
            "push",
            "--set-upstream",
            "origin",
            &format!("HEAD:{branch}"),
        ],
        "push topic branch",
    )?;

    let body = match body_file {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("read pull-request body {}", path.display()))?,
        None => String::new(),
    };
    let forge = GitHubPullRequestForge::from_env(&coordinate)?;
    let required_checks = forge.required_checks(BASE_BRANCH).await?;
    if required_checks.is_empty() {
        bail!("`{BASE_BRANCH}` has no required checks, so auto-merge has nothing to wait on");
    }
    let pull_request = forge
        .open_or_adopt_pull_request(branch, BASE_BRANCH, title, &body)
        .await?;
    println!("pull request {}", pull_request.url);
    forge.enable_auto_merge(&pull_request.node_id).await?;

    let started = Instant::now();
    loop {
        let status = forge.pull_request_status(pull_request.number).await?;
        if let Some(verdict) = classify(&status, &required_checks) {
            return Ok((pull_request.url, verdict));
        }
        if started.elapsed() >= timeout {
            return Ok((
                pull_request.url,
                DeliveryVerdict::TimedOut(pending_required_checks(&status, &required_checks)),
            ));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

fn classify(
    status: &ForgePullRequestStatus,
    required_checks: &[String],
) -> Option<DeliveryVerdict> {
    if status.merged {
        return Some(DeliveryVerdict::Merged);
    }
    if !status.auto_merge_armed {
        return Some(DeliveryVerdict::AutoMergeNotArmed);
    }
    if status.merge_state.eq_ignore_ascii_case("BEHIND") {
        return Some(DeliveryVerdict::OutdatedBranch);
    }
    let failed = failed_required_checks(status, required_checks);
    if !failed.is_empty() {
        return Some(DeliveryVerdict::FailedRequiredChecks(failed));
    }
    if let Some(decision) = status
        .review_decision
        .as_deref()
        .filter(|decision| matches!(*decision, "REVIEW_REQUIRED" | "CHANGES_REQUESTED"))
    {
        return Some(DeliveryVerdict::RequiredReview(decision.to_string()));
    }
    None
}

fn failed_required_checks(
    status: &ForgePullRequestStatus,
    required_checks: &[String],
) -> Vec<String> {
    let required = required_checks.iter().collect::<BTreeSet<_>>();
    status
        .checks
        .iter()
        .filter(|check| required.contains(&check.name))
        .filter(|check| check_failed(check))
        .map(|check| check.name.clone())
        .collect()
}

fn check_failed(check: &ForgeCheck) -> bool {
    let conclusion = check.conclusion.as_deref().unwrap_or_default();
    matches!(
        conclusion,
        "ACTION_REQUIRED"
            | "CANCELLED"
            | "FAILURE"
            | "STALE"
            | "STARTUP_FAILURE"
            | "TIMED_OUT"
            | "ERROR"
    )
}

fn pending_required_checks(
    status: &ForgePullRequestStatus,
    required_checks: &[String],
) -> Vec<String> {
    required_checks
        .iter()
        .filter(|required| {
            status
                .checks
                .iter()
                .find(|check| check.name.as_str() == required.as_str())
                .is_none_or(|check| {
                    !check.status.eq_ignore_ascii_case("COMPLETED")
                        && !matches!(check.status.as_str(), "SUCCESS" | "FAILURE" | "ERROR")
                })
        })
        .cloned()
        .collect()
}

fn validate_branch_name(root: &Path, branch: &str) -> Result<()> {
    if matches!(branch, "main" | "master") {
        bail!("delivery requires a topic branch, not `{branch}`");
    }
    git_status(
        root,
        &["check-ref-format", "--branch", branch],
        "validate topic branch",
    )
}

fn require_clean(root: &Path) -> Result<()> {
    let status = git_stdout(root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        bail!("working tree is not clean; commit the gated change before delivery");
    }
    Ok(())
}

fn run_gate(root: &Path, coordinate: &str) -> Result<()> {
    let executable = std::env::current_exe().context("resolve navigator executable")?;
    let status = Command::new(executable)
        .current_dir(root)
        .env("GITHUB_REPOSITORY", coordinate)
        .args(["project", "gate", "--ci"])
        .status()
        .context("run navigator project gate --ci")?;
    if !status.success() {
        bail!("project gate failed");
    }
    Ok(())
}

fn select_topic_branch(root: &Path, branch: &str) -> Result<()> {
    let current = git_optional_stdout(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    match current.as_deref() {
        Some(current) if current == branch => Ok(()),
        Some(BASE_BRANCH) | None => {
            let exists = git_command(
                root,
                &["show-ref", "--verify", &format!("refs/heads/{branch}")],
            )?
            .status
            .success();
            if exists {
                git_status(root, &["switch", branch], "switch to topic branch")
            } else {
                git_status(root, &["switch", "-c", branch], "create topic branch")
            }
        }
        Some(current) => bail!(
            "current branch is `{current}`; switch to `{branch}` or choose that branch explicitly"
        ),
    }
}

fn require_signed_topic_commits(root: &Path) -> Result<()> {
    let commits = git_stdout(root, &["rev-list", &format!("origin/{BASE_BRANCH}..HEAD")])?;
    if commits.trim().is_empty() {
        bail!("topic branch has no commits ahead of `origin/{BASE_BRANCH}`");
    }
    let unsigned = crate::devx::signed_commits::unsigned_commits(
        root,
        &format!("origin/{BASE_BRANCH}"),
        "HEAD",
    )?;
    if !unsigned.is_empty() {
        bail!(
            "topic branch contains unsigned commit(s): {}",
            unsigned.join(", ")
        );
    }
    Ok(())
}

fn repository_coordinate(root: &Path) -> Result<String> {
    let remote = git_stdout(root, &["remote", "get-url", "origin"])?;
    let remote = remote.trim().trim_end_matches(".git");
    if let Some(path) = remote.strip_prefix("git@github.com:") {
        return validate_coordinate(path);
    }
    let url = url::Url::parse(remote).context("parse origin URL")?;
    if url.host_str() != Some("github.com") {
        bail!(
            "cannot infer a GitHub repository from origin `{remote}`; pass --repository owner/name"
        );
    }
    validate_coordinate(url.path().trim_start_matches('/'))
}

fn validate_coordinate(coordinate: &str) -> Result<String> {
    let Some((owner, name)) = coordinate.split_once('/') else {
        bail!("repository coordinate must be `owner/name`");
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        bail!("repository coordinate must be `owner/name`");
    }
    Ok(format!("{owner}/{name}"))
}

fn git_optional_stdout(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = git_command(root, args)?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .context("git stdout is UTF-8")
            .map(|stdout| Some(stdout.trim().to_string()));
    }
    Ok(None)
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<String> {
    let output = git_command(root, args)?;
    if !output.status.success() {
        return Err(git_error(args, &output));
    }
    String::from_utf8(output.stdout).context("git stdout is UTF-8")
}

fn git_status(root: &Path, args: &[&str], action: &str) -> Result<()> {
    let output = git_command(root, args)?;
    if !output.status.success() {
        return Err(git_error(args, &output)).with_context(|| action.to_string());
    }
    Ok(())
}

fn git_command(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))
}

fn git_error(args: &[&str], output: &std::process::Output) -> anyhow::Error {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if detail.is_empty() {
        anyhow!("git {} failed", args.join(" "))
    } else {
        anyhow!("git {} failed: {detail}", args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        armed: bool,
        merge_state: &str,
        review: Option<&str>,
        checks: Vec<ForgeCheck>,
    ) -> ForgePullRequestStatus {
        ForgePullRequestStatus {
            merged: false,
            auto_merge_armed: armed,
            merge_state: merge_state.to_string(),
            review_decision: review.map(str::to_string),
            checks,
        }
    }

    #[test]
    fn optional_failures_do_not_replace_the_actual_required_check() {
        let snapshot = status(
            true,
            "CLEAN",
            None,
            vec![ForgeCheck {
                name: "publish".into(),
                status: "COMPLETED".into(),
                conclusion: Some("FAILURE".into()),
            }],
        );
        assert_eq!(classify(&snapshot, &["ci / ci".into()]), None);
        assert_eq!(
            pending_required_checks(&snapshot, &["ci / ci".into()]),
            vec!["ci / ci"]
        );
    }

    #[test]
    fn each_terminal_stop_condition_has_its_own_verdict() {
        assert_eq!(
            classify(&status(false, "CLEAN", None, vec![]), &["ci / ci".into()]),
            Some(DeliveryVerdict::AutoMergeNotArmed)
        );
        assert_eq!(
            classify(&status(true, "BEHIND", None, vec![]), &["ci / ci".into()]),
            Some(DeliveryVerdict::OutdatedBranch)
        );
        assert_eq!(
            classify(
                &status(true, "BLOCKED", Some("REVIEW_REQUIRED"), vec![]),
                &["ci / ci".into()]
            ),
            Some(DeliveryVerdict::RequiredReview("REVIEW_REQUIRED".into()))
        );
        assert_eq!(
            classify(
                &status(
                    true,
                    "UNSTABLE",
                    None,
                    vec![ForgeCheck {
                        name: "ci / ci".into(),
                        status: "COMPLETED".into(),
                        conclusion: Some("FAILURE".into()),
                    }]
                ),
                &["ci / ci".into()]
            ),
            Some(DeliveryVerdict::FailedRequiredChecks(
                vec!["ci / ci".into()]
            ))
        );
    }
}
