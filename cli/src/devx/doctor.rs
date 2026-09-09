//! `devx doctor` — diagnose ongoing scheduled-job health in plain language.
//!
//! Built for the failure that hid for days: a trigger `Job` wedged in
//! `ImagePullBackOff` while its `CronJob`'s `concurrencyPolicy: Forbid` quietly
//! skipped every subsequent run, so the nightly email simply stopped with no
//! error anywhere obvious. `doctor` reads the cluster (read-only `kubectl get
//! ... -o json`), classifies what it finds, and prints each problem with the
//! one command that fixes it.
//!
//! The classification ([`diagnose`]) is pure and unit-tested against synthetic
//! observations; the I/O layer ([`run`]) only shells `kubectl` and parses its
//! JSON into those observations. Identifiers and counts only — `doctor` reads
//! object metadata and status, never application data.

use std::process::Command;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

/// A trigger `Job` should POST to the ingress and exit in seconds. Anything
/// still `Active` past this is wedged — and under `Forbid` it blocks the
/// schedule. Ten minutes is far beyond a healthy run yet short of a full
/// schedule interval.
const STUCK_AFTER_SECS: i64 = 600;

/// Container `waiting` reasons that mean a Job will never make progress on its
/// own — it needs an operator (a pushed image, a fixed config), not patience.
const TERMINAL_WAITING: &[&str] = &[
    "ImagePullBackOff",
    "ErrImagePull",
    "CrashLoopBackOff",
    "CreateContainerConfigError",
    "CreateContainerError",
    "InvalidImageName",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Critical,
    Warning,
    Ok,
}

impl Severity {
    fn marker(self) -> &'static str {
        match self {
            Severity::Critical => "CRIT",
            Severity::Warning => "WARN",
            Severity::Ok => "OK  ",
        }
    }
}

/// One diagnosis line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub subject: String,
    pub detail: String,
    /// The exact command (or next step) that resolves it, when there is one.
    pub remedy: Option<String>,
}

/// A single `Job`'s observed state, distilled from `kubectl get jobs -o json`
/// plus its pod's container `waiting` reason (looked up separately).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobObs {
    pub name: String,
    pub active: u64,
    pub failed: u64,
    /// Seconds since `.status.startTime`; `None` if the Job hasn't started.
    pub age_secs: Option<i64>,
    /// The container `waiting` reason of the Job's pod, if any.
    pub waiting_reason: Option<String>,
}

/// A workload's readiness, from `kubectl get deploy -o json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadObs {
    pub name: String,
    pub ready: u64,
    pub desired: u64,
}

/// Classify cluster observations into findings. Pure — the whole point is that
/// this is unit-testable without a cluster. `namespace` is only used to render
/// copy-pasteable remedy commands.
#[must_use]
pub fn diagnose(namespace: &str, jobs: &[JobObs], workloads: &[WorkloadObs]) -> Vec<Finding> {
    let mut out = Vec::new();

    for job in jobs {
        let terminal = job
            .waiting_reason
            .as_deref()
            .is_some_and(|r| TERMINAL_WAITING.contains(&r));
        let stuck = job.active >= 1 && job.age_secs.is_some_and(|a| a > STUCK_AFTER_SECS);

        if job.active >= 1 && (terminal || stuck) {
            let age = job.age_secs.map_or_else(|| "unknown".into(), fmt_duration);
            let reason = job
                .waiting_reason
                .as_deref()
                .map(|r| format!(" (pod {r})"))
                .unwrap_or_default();
            out.push(Finding {
                severity: Severity::Critical,
                subject: format!("job/{}", job.name),
                detail: format!(
                    "Active for {age}{reason}. A CronJob with concurrencyPolicy: Forbid treats \
                     this as a reason to SKIP every subsequent schedule — the silent failure that \
                     stops a nightly/periodic job for days.",
                ),
                remedy: Some(format!(
                    "kubectl -n {namespace} delete job {} — then fix the root cause (push the \
                     missing image / correct the config) so the next schedule succeeds. The \
                     activeDeadlineSeconds backstop now caps this at 120s going forward.",
                    job.name
                )),
            });
        } else if terminal {
            // Not yet old enough to count as wedged, but it never will recover.
            out.push(Finding {
                severity: Severity::Warning,
                subject: format!("job/{}", job.name),
                detail: format!(
                    "Pod is {} — it will not recover on its own.",
                    job.waiting_reason.as_deref().unwrap_or("failing")
                ),
                remedy: Some(format!(
                    "kubectl -n {namespace} describe job {} to see the pull/config error.",
                    job.name
                )),
            });
        }
    }

    for wl in workloads {
        if wl.ready < wl.desired {
            out.push(Finding {
                severity: Severity::Critical,
                subject: format!("deploy/{}", wl.name),
                detail: format!("{}/{} replicas ready.", wl.ready, wl.desired),
                remedy: Some(format!(
                    "kubectl -n {namespace} rollout status deploy/{} and check pod events/logs.",
                    wl.name
                )),
            });
        }
    }

    if out.iter().all(|f| f.severity == Severity::Ok) {
        out.push(Finding {
            severity: Severity::Ok,
            subject: "scheduled jobs".into(),
            detail: format!(
                "No wedged trigger jobs; {} workload(s) at full readiness.",
                workloads.len()
            ),
            remedy: None,
        });
    }
    out
}

/// Human duration like `2d19h` / `14m`.
fn fmt_duration(secs: i64) -> String {
    let s = secs.max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3_600, (s % 3_600) / 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{m}m")
    }
}

/// Run the doctor against a namespace: shell `kubectl`, parse, classify, print.
pub fn run(namespace: &str) -> Result<()> {
    let now = Utc::now();
    let waiting = pod_waiting_reasons(namespace)?;
    let jobs = observe_jobs(namespace, now, &waiting)?;
    let workloads = observe_workloads(namespace)?;

    let findings = diagnose(namespace, &jobs, &workloads);
    let crit = findings
        .iter()
        .filter(|f| f.severity == Severity::Critical)
        .count();

    println!("navigator ops doctor — namespace {namespace}");
    println!(
        "  {} job(s), {} workload(s) inspected\n",
        jobs.len(),
        workloads.len()
    );
    for f in &findings {
        println!("[{}] {}: {}", f.severity.marker(), f.subject, f.detail);
        if let Some(remedy) = &f.remedy {
            println!("       ↳ {remedy}");
        }
    }
    println!(
        "\nRestate ingress/registration is a separate axis: a 401 (stale RESTATE_AUTH_TOKEN) or \
         404 (service not re-registered) shows in the Restate Cloud console → Invocations, not \
         here. See docs/observability.md and docs/durable-workflows.md."
    );
    if crit > 0 {
        anyhow::bail!("{crit} critical finding(s)");
    }
    Ok(())
}

/// `job name -> container waiting reason` for pods in the namespace.
fn pod_waiting_reasons(namespace: &str) -> Result<Vec<(String, String)>> {
    let json = kubectl_json(namespace, "pods")?;
    let mut out = Vec::new();
    for pod in items(&json) {
        let owner = pod["metadata"]["ownerReferences"]
            .as_array()
            .and_then(|refs| refs.iter().find(|r| r["kind"] == "Job"))
            .and_then(|r| r["name"].as_str());
        let Some(owner) = owner else { continue };
        let statuses = ["containerStatuses", "initContainerStatuses"];
        for key in statuses {
            if let Some(arr) = pod["status"][key].as_array() {
                for cs in arr {
                    if let Some(reason) = cs["state"]["waiting"]["reason"].as_str() {
                        out.push((owner.to_string(), reason.to_string()));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn observe_jobs(
    namespace: &str,
    now: DateTime<Utc>,
    waiting: &[(String, String)],
) -> Result<Vec<JobObs>> {
    let json = kubectl_json(namespace, "jobs")?;
    let mut out = Vec::new();
    for job in items(&json) {
        let name = job["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let age_secs = job["status"]["startTime"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| (now - t.with_timezone(&Utc)).num_seconds());
        let waiting_reason = waiting
            .iter()
            .find(|(j, _)| *j == name)
            .map(|(_, r)| r.clone());
        out.push(JobObs {
            name,
            active: job["status"]["active"].as_u64().unwrap_or(0),
            failed: job["status"]["failed"].as_u64().unwrap_or(0),
            age_secs,
            waiting_reason,
        });
    }
    Ok(out)
}

fn observe_workloads(namespace: &str) -> Result<Vec<WorkloadObs>> {
    let json = kubectl_json(namespace, "deployments")?;
    let mut out = Vec::new();
    for d in items(&json) {
        out.push(WorkloadObs {
            name: d["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            ready: d["status"]["readyReplicas"].as_u64().unwrap_or(0),
            desired: d["spec"]["replicas"].as_u64().unwrap_or(0),
        });
    }
    Ok(out)
}

fn items(json: &Value) -> Vec<Value> {
    json["items"].as_array().cloned().unwrap_or_default()
}

fn kubectl_json(namespace: &str, kind: &str) -> Result<Value> {
    let out = Command::new("kubectl")
        .args(["-n", namespace, "get", kind, "-o", "json"])
        .output()
        .with_context(|| format!("running kubectl get {kind}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "kubectl get {kind} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("parsing kubectl get {kind} json"))
}

#[cfg(test)]
mod tests {
    use super::{diagnose, fmt_duration, JobObs, Severity, WorkloadObs};

    fn job(name: &str, active: u64, age: Option<i64>, waiting: Option<&str>) -> JobObs {
        JobObs {
            name: name.into(),
            active,
            failed: 0,
            age_secs: age,
            waiting_reason: waiting.map(Into::into),
        }
    }

    #[test]
    fn flags_the_imagepullbackoff_wedge_we_actually_hit() {
        // The real incident: a trigger Job Active for 2d19h in ImagePullBackOff.
        let jobs = [job(
            "archives-trigger-29684760",
            1,
            Some(241_200),
            Some("ImagePullBackOff"),
        )];
        let findings = diagnose("navigator", &jobs, &[]);
        let crit = &findings[0];
        assert_eq!(crit.severity, Severity::Critical);
        assert!(crit.detail.contains("ImagePullBackOff"));
        assert!(crit.detail.contains("Forbid"));
        assert!(crit.detail.contains("2d19h"));
        assert!(crit
            .remedy
            .as_ref()
            .unwrap()
            .contains("delete job archives-trigger-29684760"));
    }

    #[test]
    fn a_long_running_active_job_is_critical_even_without_a_waiting_reason() {
        let jobs = [job("stuck-trigger", 1, Some(1_200), None)];
        let findings = diagnose("navigator", &jobs, &[]);
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[test]
    fn an_active_imagepull_failure_is_critical_immediately() {
        // An Active pod in a terminal pull state is already holding the Forbid
        // lock — it never recovers on its own, so it's critical regardless of
        // age, not a wait-and-see warning.
        let jobs = [job("new-trigger", 1, Some(30), Some("ErrImagePull"))];
        let findings = diagnose("navigator", &jobs, &[]);
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[test]
    fn a_failed_non_active_pull_error_is_a_warning() {
        // active == 0: no pod holds the schedule, but the image error still
        // wants an operator's eyes.
        let jobs = [job("done-trigger", 0, Some(30), Some("ImagePullBackOff"))];
        let findings = diagnose("navigator", &jobs, &[]);
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn a_healthy_fast_job_and_ready_workload_yield_only_ok() {
        let jobs = [job("archives-trigger-ok", 0, Some(9), None)];
        let workloads = [WorkloadObs {
            name: "workflows-service".into(),
            ready: 1,
            desired: 1,
        }];
        let findings = diagnose("navigator", &jobs, &workloads);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Ok);
    }

    #[test]
    fn an_unready_workload_is_critical() {
        let workloads = [WorkloadObs {
            name: "workflows-service".into(),
            ready: 0,
            desired: 1,
        }];
        let findings = diagnose("navigator", &[], &workloads);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].detail.contains("0/1"));
    }
    #[test]
    fn duration_formatting_reads_like_kubectl() {
        assert_eq!(fmt_duration(241_200), "2d19h");
        assert_eq!(fmt_duration(900), "15m");
        assert_eq!(fmt_duration(5_400), "1h30m");
    }
}
