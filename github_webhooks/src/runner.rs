//! Isolated entrypoint contract for one GitHub engineering invocation.
//!
//! The Kubernetes Job supplies this serializable task separately from its
//! short-lived Git credential. Keeping the credential out of this contract
//! prevents it from entering Restate state, result payloads, or telemetry.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::harness::{AgentHarness, AgentOutcome, AgentTask, TriagePlan, Usage};

/// Delimits the one source-bearing result emitted by an isolated triage
/// runner. The controller must parse only the bytes between these markers and
/// must not relay the surrounding process output into Restate or telemetry.
pub const TRIAGE_RESULT_BEGIN: &str = "<<<NAVIGATOR_TRIAGE_RESULT>>>";
/// Closing delimiter for [`TRIAGE_RESULT_BEGIN`].
pub const TRIAGE_RESULT_END: &str = "<<<END_NAVIGATOR_TRIAGE_RESULT>>>";
/// A grounded plan is deliberately bounded before it reaches a pod log. This
/// leaves room for a useful test-driven plan while preventing an untrusted
/// model result from turning the Job-result handoff into an unbounded channel.
pub const MAX_TRIAGE_RESULT_BYTES: usize = 32 * 1024;

/// A CI-equivalent command the runner must verify after the agent exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Format,
    Content,
    Clippy,
    Coverage,
}

impl Gate {
    /// The program and arguments run inside the freshly checked-out task tree.
    #[must_use]
    pub fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Format => ("cargo", &["fmt", "--all", "--", "--check"]),
            Self::Content => ("navigator", &["validate", "."]),
            Self::Clippy => ("cargo", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]),
            Self::Coverage => (
                "cargo",
                &[
                    "llvm-cov",
                    "--workspace",
                    "--fail-under-lines",
                    "90.6",
                    "--ignore-filename-regex",
                    "(cli/src/devx/(browser_e2e|chrome|e2e|garage|orchestrate|staging)|features/src/webdriver)\\.rs$",
                ],
            ),
        }
    }
}

/// Ordered verification contract copied from the required CI job.
pub const GATES: &[Gate] = &[Gate::Format, Gate::Content, Gate::Clippy, Gate::Coverage];

/// The non-secret portion of one runner Job invocation.
///
/// This type deliberately does not implement `Debug`: the embedded prompt and
/// task environment can contain source content.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunnerTask {
    pub agent: AgentTask,
    /// Immutable Git object ID to fetch before any agent or gate command.
    pub commit: String,
}

/// Safe classification for runner task handoff failures.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RunnerTaskError {
    #[error("runner task is missing")]
    Missing,
    #[error("runner task is invalid")]
    Invalid,
}

/// Safe classification for process failures inside the isolated runner Job.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RunnerError {
    #[error("runner task is invalid")]
    Task,
    #[error("runner worktree is not empty")]
    WorktreeNotEmpty,
    #[error("runner source checkout failed")]
    Checkout,
    #[error("runner agent invocation failed")]
    Harness,
    #[error("runner verification gate failed")]
    Gate,
    #[error("runner attempted an unsigned source commit")]
    UnsignedCommit,
    #[error("triage runner changed its checked-out source")]
    UnexpectedChange,
    #[error("triage runner returned an invalid result envelope")]
    InvalidTriageResult,
}

/// The source-bearing result an isolated triage runner writes exactly once.
///
/// This type deliberately implements neither `Debug` nor `Display`: its plan
/// may contain issue, source, or test details. It moves directly from the Job
/// result parser to the narrow GitHub issue-comment call.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TriageRun {
    plan_markdown: String,
    usage: Usage,
}

impl TriageRun {
    fn new(plan: &TriagePlan, usage: Usage) -> Self {
        Self {
            plan_markdown: plan.markdown().to_owned(),
            usage,
        }
    }

    /// The grounded plan for the controller's issue-comment side effect.
    #[must_use]
    pub fn markdown(&self) -> &str {
        &self.plan_markdown
    }

    /// Bounded usage safe for the guardrails reservation and telemetry count.
    #[must_use]
    pub fn usage(&self) -> Usage {
        self.usage
    }
}

/// Serialize the sole permitted triage-runner handoff.
///
/// Process output may contain tool diagnostics, so the caller must emit this
/// envelope after every command has completed. The controller rejects any
/// missing, duplicate, malformed, or oversized envelope.
pub fn format_triage_result(result: &TriageRun) -> Result<String, RunnerError> {
    let value = serde_json::to_string(result).map_err(|_| RunnerError::InvalidTriageResult)?;
    if value.len() > MAX_TRIAGE_RESULT_BYTES {
        return Err(RunnerError::InvalidTriageResult);
    }
    Ok(format!(
        "{TRIAGE_RESULT_BEGIN}\n{value}\n{TRIAGE_RESULT_END}\n"
    ))
}

/// Parse the bounded, sentinel-framed triage result from a runner Job log.
pub fn parse_triage_result(log: &str) -> Result<TriageRun, RunnerError> {
    let Some((_, after_begin)) = log.split_once(TRIAGE_RESULT_BEGIN) else {
        return Err(RunnerError::InvalidTriageResult);
    };
    if after_begin.contains(TRIAGE_RESULT_BEGIN) {
        return Err(RunnerError::InvalidTriageResult);
    }
    let Some((value, after_end)) = after_begin.split_once(TRIAGE_RESULT_END) else {
        return Err(RunnerError::InvalidTriageResult);
    };
    let value = value.trim();
    if after_end.contains(TRIAGE_RESULT_END) || value.len() > MAX_TRIAGE_RESULT_BYTES {
        return Err(RunnerError::InvalidTriageResult);
    }
    let result: TriageRun =
        serde_json::from_str(value).map_err(|_| RunnerError::InvalidTriageResult)?;
    if result.plan_markdown.trim().is_empty() {
        return Err(RunnerError::InvalidTriageResult);
    }
    Ok(result)
}

impl RunnerTask {
    /// Parse a Job's non-secret task payload.
    ///
    /// The clone URL intentionally travels in a distinct environment variable
    /// because it embeds an installation token.
    pub fn from_json(value: &str) -> Result<Self, RunnerTaskError> {
        let task: Self = serde_json::from_str(value).map_err(|_| RunnerTaskError::Invalid)?;
        if !is_commit_id(&task.commit) {
            return Err(RunnerTaskError::Invalid);
        }
        Ok(task)
    }
}

fn is_commit_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Fetch exactly one immutable revision into the invocation's empty worktree.
///
/// `clone_url` is credential-bearing and never appears in the returned error.
pub async fn checkout(task: &RunnerTask, clone_url: &str) -> Result<(), RunnerError> {
    let worktree = &task.agent.worktree;
    if worktree.exists()
        && std::fs::read_dir(worktree)
            .map_err(|_| RunnerError::Checkout)?
            .next()
            .is_some()
    {
        return Err(RunnerError::WorktreeNotEmpty);
    }
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent).map_err(|_| RunnerError::Checkout)?;
    }

    let worktree_path = worktree.to_string_lossy().into_owned();
    run_git(&["init", &worktree_path]).await?;
    run_git_in(worktree, &["remote", "add", "origin", clone_url]).await?;
    run_git_in(worktree, &["fetch", "--depth", "1", "origin", &task.commit]).await?;
    run_git_in(worktree, &["checkout", "--detach", "FETCH_HEAD"]).await
}

/// Execute one fully checked-out engineering task.
///
/// A successful harness outcome is not trusted as evidence that the change is
/// shippable: each required gate runs in the checked-out worktree afterwards.
///
/// The runner rejects an agent-created Git commit. A later publication step
/// turns the uncommitted tree into a GitHub-signed commit through
/// `createCommitOnBranch`; it must never shell out to `git commit`.
pub async fn execute<H: AgentHarness>(
    task: &RunnerTask,
    clone_url: &str,
    harness: &H,
) -> Result<AgentOutcome, RunnerError> {
    execute_with(task, clone_url, harness, &SystemGateRunner).await
}

/// Execute one triage-only task in a freshly checked-out repository.
///
/// Unlike implementation and revision work, triage never runs verification
/// gates or produces a patch. The checked-out source is strictly read-only:
/// any working-tree change fails the task before its source-bearing plan can
/// be posted to GitHub.
pub async fn execute_triage<H: AgentHarness>(
    task: &RunnerTask,
    clone_url: &str,
    harness: &H,
) -> Result<TriageRun, RunnerError> {
    checkout(task, clone_url).await?;
    let outcome = harness
        .run(&task.agent)
        .await
        .map_err(|_| RunnerError::Harness)?;
    if outcome.exit_code != 0 {
        return Err(RunnerError::Harness);
    }
    verify_head(&task.agent.worktree, &task.commit).await?;
    verify_clean_worktree(&task.agent.worktree).await?;
    let usage = outcome.usage;
    let plan = TriagePlan::from_outcome(outcome).map_err(|_| RunnerError::Harness)?;
    Ok(TriageRun::new(&plan, usage))
}

/// Execute a task with a supplied gate runner.
///
/// The generic seam makes the sequence testable without replacing the runner's
/// real Git checkout behavior. Production calls [`execute`], which supplies
/// [`SystemGateRunner`].
pub async fn execute_with<H: AgentHarness, R: GateRunner>(
    task: &RunnerTask,
    clone_url: &str,
    harness: &H,
    gates: &R,
) -> Result<AgentOutcome, RunnerError> {
    checkout(task, clone_url).await?;
    let outcome = harness
        .run(&task.agent)
        .await
        .map_err(|_| RunnerError::Harness)?;
    if outcome.exit_code != 0 {
        return Err(RunnerError::Harness);
    }
    verify_head(&task.agent.worktree, &task.commit).await?;

    for gate in GATES {
        gates.run(*gate, &task.agent.worktree).await?;
    }

    Ok(outcome)
}

/// Executes the runner's verification gates in its checked-out worktree.
#[async_trait::async_trait]
pub trait GateRunner: Send + Sync {
    /// Run one CI-equivalent gate.
    async fn run(&self, gate: Gate, worktree: &Path) -> Result<(), RunnerError>;
}

/// Production gate runner. It invokes the same command contract CI uses and
/// captures no process output, because that output can contain source text.
pub struct SystemGateRunner;

#[async_trait::async_trait]
impl GateRunner for SystemGateRunner {
    async fn run(&self, gate: Gate, worktree: &Path) -> Result<(), RunnerError> {
        let (program, arguments) = gate.command();
        let status = tokio::process::Command::new(program)
            .current_dir(worktree)
            .args(arguments)
            .status()
            .await
            .map_err(|_| RunnerError::Gate)?;
        if status.success() {
            Ok(())
        } else {
            Err(RunnerError::Gate)
        }
    }
}

async fn verify_head(worktree: &Path, expected: &str) -> Result<(), RunnerError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .map_err(|_| RunnerError::Checkout)?;
    if output.status.success() && output.stdout.trim_ascii() == expected.as_bytes() {
        Ok(())
    } else {
        Err(RunnerError::UnsignedCommit)
    }
}

async fn verify_clean_worktree(worktree: &Path) -> Result<(), RunnerError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain"])
        .output()
        .await
        .map_err(|_| RunnerError::Checkout)?;
    if output.status.success() && output.stdout.is_empty() {
        Ok(())
    } else if output.status.success() {
        Err(RunnerError::UnexpectedChange)
    } else {
        Err(RunnerError::Checkout)
    }
}

async fn run_git(arguments: &[&str]) -> Result<(), RunnerError> {
    let status = tokio::process::Command::new("git")
        .args(arguments)
        .status()
        .await
        .map_err(|_| RunnerError::Checkout)?;
    if status.success() {
        Ok(())
    } else {
        Err(RunnerError::Checkout)
    }
}

async fn run_git_in(worktree: &Path, arguments: &[&str]) -> Result<(), RunnerError> {
    let status = tokio::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(arguments)
        .status()
        .await
        .map_err(|_| RunnerError::Checkout)?;
    if status.success() {
        Ok(())
    } else {
        Err(RunnerError::Checkout)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        execute_triage, execute_with, format_triage_result, parse_triage_result, Gate, GateRunner,
        RunnerError, RunnerTask, RunnerTaskError, TriageRun, GATES,
    };
    use crate::github::RepositoryRef;
    use crate::harness::{AgentHarness, AgentOutcome, AgentTask, HarnessError, Usage};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct WritingHarness;

    #[async_trait]
    impl AgentHarness for WritingHarness {
        async fn run(&self, task: &AgentTask) -> Result<AgentOutcome, HarnessError> {
            std::fs::write(task.worktree.join("runner-proof.txt"), "written by harness")
                .map_err(|_| HarnessError::Unavailable)?;
            Ok(AgentOutcome {
                exit_code: 0,
                result: serde_json::json!({ "status": "done" }),
                usage: Usage::default(),
            })
        }
    }

    struct CommittingHarness;

    #[async_trait]
    impl AgentHarness for CommittingHarness {
        async fn run(&self, task: &AgentTask) -> Result<AgentOutcome, HarnessError> {
            std::fs::write(task.worktree.join("runner-proof.txt"), "written by harness")
                .map_err(|_| HarnessError::Unavailable)?;
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&task.worktree)
                .args(["add", "runner-proof.txt"])
                .status()
                .map_err(|_| HarnessError::Unavailable)?;
            if !status.success() {
                return Err(HarnessError::Unavailable);
            }
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&task.worktree)
                .args([
                    "-c",
                    "user.email=runner@example.com",
                    "-c",
                    "user.name=Runner Test",
                    "commit",
                    "-m",
                    "agent change",
                ])
                .status()
                .map_err(|_| HarnessError::Unavailable)?;
            if !status.success() {
                return Err(HarnessError::Unavailable);
            }
            Ok(AgentOutcome {
                exit_code: 0,
                result: serde_json::json!({ "status": "done" }),
                usage: Usage::default(),
            })
        }
    }

    struct PlanningHarness;

    #[async_trait]
    impl AgentHarness for PlanningHarness {
        async fn run(&self, _task: &AgentTask) -> Result<AgentOutcome, HarnessError> {
            Ok(AgentOutcome {
                exit_code: 0,
                result: serde_json::json!({ "plan_markdown": "## Grounded plan" }),
                usage: Usage::default(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingGates {
        seen: Mutex<Vec<Gate>>,
        failure: Option<Gate>,
    }

    impl RecordingGates {
        fn failing(gate: Gate) -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                failure: Some(gate),
            }
        }
    }

    #[async_trait]
    impl GateRunner for RecordingGates {
        async fn run(&self, gate: Gate, _worktree: &Path) -> Result<(), RunnerError> {
            self.seen.lock().unwrap().push(gate);
            if self.failure == Some(gate) {
                Err(RunnerError::Gate)
            } else {
                Ok(())
            }
        }
    }

    fn run_git(directory: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(directory)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn fixture() -> (TempDir, PathBuf, String) {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        run_git(root.path(), &["init", "--bare", origin.to_str().unwrap()]);
        run_git(&source, &["init"]);
        run_git(&source, &["config", "user.email", "runner@example.com"]);
        run_git(&source, &["config", "user.name", "Runner Test"]);
        std::fs::write(source.join("README.md"), "fixture").unwrap();
        run_git(&source, &["add", "."]);
        run_git(&source, &["commit", "-m", "fixture"]);
        let commit = String::from_utf8(
            std::process::Command::new("git")
                .current_dir(&source)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        run_git(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        run_git(&source, &["push", "origin", "HEAD:main"]);
        (root, origin, commit)
    }

    fn task(worktree: PathBuf, commit: String) -> RunnerTask {
        RunnerTask {
            agent: AgentTask {
                repository: RepositoryRef {
                    owner: "neon-law-source-code".into(),
                    name: "navigator".into(),
                },
                worktree,
                prompt: "make the requested change".into(),
                model: "test-model".into(),
                max_turns: 1,
                token_budget: 1,
                environment: BTreeMap::new(),
            },
            commit,
        }
    }

    #[test]
    fn runner_task_requires_an_immutable_commit_id() {
        let payload = r#"{"agent":{"repository":{"owner":"neon-law-source-code","name":"navigator"},"worktree":"/worktree","prompt":"triage","model":"model","max_turns":1,"token_budget":1,"environment":{}},"commit":"main"}"#;
        assert!(matches!(
            RunnerTask::from_json(payload),
            Err(RunnerTaskError::Invalid)
        ));
    }

    #[test]
    fn runner_replays_the_required_ci_verification_order() {
        assert_eq!(
            GATES,
            [Gate::Format, Gate::Content, Gate::Clippy, Gate::Coverage,]
        );
        assert_eq!(Gate::Coverage.command().0, "cargo");
        // The floor is duplicated here and in `.github/workflows/ci.yml`, and
        // the runner is only CI-equivalent while the two agree. Pin it so a
        // raise on one side cannot silently leave the other behind.
        let coverage = Gate::Coverage.command().1;
        let floor = coverage
            .iter()
            .position(|arg| *arg == "--fail-under-lines")
            .map(|index| coverage[index + 1]);
        assert_eq!(floor, Some("90.6"));
    }

    #[tokio::test]
    async fn runner_keeps_agent_changes_uncommitted_after_every_gate_succeeds() {
        let (root, origin, commit) = fixture();
        let task = task(root.path().join("worktree"), commit);
        let gates = RecordingGates::default();

        execute_with(&task, origin.to_str().unwrap(), &WritingHarness, &gates)
            .await
            .unwrap();

        assert_eq!(*gates.seen.lock().unwrap(), GATES);
        assert_eq!(
            std::fs::read_to_string(task.agent.worktree.join("runner-proof.txt")).unwrap(),
            "written by harness"
        );
        let origin_head = std::process::Command::new("git")
            .args(["--git-dir", origin.to_str().unwrap(), "rev-parse", "main"])
            .output()
            .unwrap();
        assert_eq!(origin_head.stdout.trim_ascii(), task.commit.as_bytes());
    }

    #[tokio::test]
    async fn failed_gate_stops_before_a_verified_commit_can_be_published() {
        let (root, origin, commit) = fixture();
        let task = task(root.path().join("worktree"), commit);
        let gates = RecordingGates::failing(Gate::Format);

        let result = execute_with(&task, origin.to_str().unwrap(), &WritingHarness, &gates).await;

        assert!(matches!(result, Err(RunnerError::Gate)));
        assert_eq!(*gates.seen.lock().unwrap(), [Gate::Format]);
        let origin_head = std::process::Command::new("git")
            .args(["--git-dir", origin.to_str().unwrap(), "rev-parse", "main"])
            .output()
            .unwrap();
        assert_eq!(origin_head.stdout.trim_ascii(), task.commit.as_bytes());
    }

    #[tokio::test]
    async fn runner_rejects_a_harness_that_shells_out_to_git_commit() {
        let (root, origin, commit) = fixture();
        let task = task(root.path().join("worktree"), commit);

        let result = execute_with(
            &task,
            origin.to_str().unwrap(),
            &CommittingHarness,
            &RecordingGates::default(),
        )
        .await;

        assert!(matches!(result, Err(RunnerError::UnsignedCommit)));
    }

    #[tokio::test]
    async fn triage_returns_only_a_plan_and_leaves_source_unchanged() {
        let (root, origin, commit) = fixture();
        let task = task(root.path().join("worktree"), commit);

        let plan = execute_triage(&task, origin.to_str().unwrap(), &PlanningHarness)
            .await
            .unwrap();

        assert_eq!(plan.markdown(), "## Grounded plan");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&task.agent.worktree)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.stdout.is_empty());
    }

    #[tokio::test]
    async fn triage_rejects_a_harness_that_changes_source() {
        let (root, origin, commit) = fixture();
        let task = task(root.path().join("worktree"), commit);

        let result = execute_triage(&task, origin.to_str().unwrap(), &WritingHarness).await;

        assert!(matches!(result, Err(RunnerError::UnexpectedChange)));
    }

    #[test]
    fn triage_result_round_trips_only_one_bounded_envelope() {
        let result = TriageRun {
            plan_markdown: "## Grounded plan".into(),
            usage: Usage {
                turns: 2,
                input_tokens: 10,
                output_tokens: 20,
            },
        };

        let rendered = format_triage_result(&result).unwrap();
        let parsed =
            parse_triage_result(&format!("runner diagnostic\n{rendered}trailer\n")).unwrap();

        assert_eq!(parsed.markdown(), "## Grounded plan");
        assert_eq!(parsed.usage(), result.usage());
    }

    #[test]
    fn triage_result_rejects_duplicate_or_unbounded_envelopes() {
        let duplicate = format!(
            "{begin}\n{{\"plan_markdown\":\"plan\",\"usage\":{{\"turns\":0,\"input_tokens\":0,\"output_tokens\":0}}}}\n{end}\n{begin}\n{{\"plan_markdown\":\"plan\",\"usage\":{{\"turns\":0,\"input_tokens\":0,\"output_tokens\":0}}}}\n{end}",
            begin = super::TRIAGE_RESULT_BEGIN,
            end = super::TRIAGE_RESULT_END,
        );
        assert!(matches!(
            parse_triage_result(&duplicate),
            Err(RunnerError::InvalidTriageResult)
        ));

        let oversized = format!(
            "{begin}\n{}\n{end}",
            "x".repeat(super::MAX_TRIAGE_RESULT_BYTES + 1),
            begin = super::TRIAGE_RESULT_BEGIN,
            end = super::TRIAGE_RESULT_END,
        );
        assert!(matches!(
            parse_triage_result(&oversized),
            Err(RunnerError::InvalidTriageResult)
        ));
    }
}
