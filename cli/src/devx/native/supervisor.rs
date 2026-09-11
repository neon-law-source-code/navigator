//! Start, health-gate, record, and stop the native tier's host processes.
//!
//! The cluster lane hands supervision to Kubernetes: a Deployment restarts
//! its pod, and `kind delete cluster` reclaims the whole tier at once.
//! Nothing on a host does that, so this module is the substitute — and it
//! is deliberately the smallest one that is still safe.
//!
//! Three properties carry that safety:
//!
//! - **A recorded PID is re-identified before it is signalled.** PIDs are
//!   reused. A ledger entry that outlives its process names a stranger,
//!   and a naive `down` would kill it. [`owns_pid`] re-reads the running
//!   process's command line and only signals a match.
//! - **A process is not reported started until its port answers.**
//!   [`super::super::wait_for_tcp`] is the same gate the cluster lane
//!   applies to its port-forwards, so both lanes mean the same thing by
//!   "ready".
//! - **A start that never binds surfaces its own log.** A dependency that
//!   dies on a bad flag would otherwise present as a bare connection
//!   timeout thirty seconds later, with the reason sitting unread in a
//!   file.
//!
//! Ownership across worktrees is recorded by the native registry. This module
//! answers the process questions that registry transitions need: can a record
//! be adopted, and can its PID be signalled without touching a reused PID?

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// How long a `SIGTERM`ed dependency gets to exit before `SIGKILL`.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// Log lines quoted when a process fails to bind its port.
const LOG_TAIL_LINES: usize = 20;

/// One host process the native tier supervises.
pub(super) struct Service {
    /// Stable name — the ledger key, the log file stem, and how `status`
    /// and errors spell this dependency.
    pub(super) label: &'static str,
    /// Absolute path to the executable. Resolved by the caller rather
    /// than looked up here, so a `PATH` change between `install` and `up`
    /// cannot silently swap the engine under a running tier.
    pub(super) program: PathBuf,
    pub(super) args: Vec<String>,
    /// Extra environment for the child, on top of the inherited process
    /// environment.
    pub(super) env: Vec<(String, String)>,
    /// Working directory. Several of these dependencies resolve their
    /// data directory relative to it.
    pub(super) cwd: PathBuf,
    /// The port that must accept a TCP connection before this service
    /// counts as started.
    pub(super) port: u16,
}

/// A started process, as recorded in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Started {
    pub(super) label: String,
    pub(super) pid: u32,
    pub(super) port: u16,
    /// The executable's file name, re-checked against the live process
    /// before `down` signals the PID. See [`owns_pid`].
    pub(super) program: String,
    /// The complete command line, retained so a missing registry can adopt
    /// only the expected service rather than an unrelated listener.
    pub(super) command: String,
    /// `ps`'s absolute process-start value. A PID and executable can both be
    /// recycled; this third identity component changes for the replacement.
    pub(super) start_time: String,
}

/// Where this worktree records the processes it started.
pub(super) fn ledger_path(root: &Path) -> PathBuf {
    root.join(".devx").join("native-processes.json")
}

/// The directory holding one service's data and log.
pub(super) fn service_dir(root: &Path, label: &str) -> PathBuf {
    root.join(".devx").join("native").join(label)
}

fn log_path(root: &Path, label: &str) -> PathBuf {
    service_dir(root, label).join("process.log")
}

pub(super) fn read_ledger(root: &Path) -> Vec<Started> {
    fs::read_to_string(ledger_path(root))
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

fn write_ledger(root: &Path, records: &[Started]) -> Result<()> {
    let path = ledger_path(root);
    let parent = path.parent().context("ledger path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let body = serde_json::to_string_pretty(records).context("serialize the process ledger")?;
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))
}

/// The executable's file name — what `ps` prints as `argv[0]`'s basename
/// and what [`owns_pid`] matches on.
fn program_name(program: &Path) -> String {
    program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a live process's command line is the program we recorded.
///
/// This is the guard against signalling a recycled PID. `ps -o command=`
/// prints the full argv, so an exact-equality check would be brittle
/// against flag changes; the executable's own name is the stable part,
/// and matching it is enough to distinguish our process from whatever
/// unrelated process inherited the number.
///
/// Empty output means the PID is gone, which is not ownership.
pub(super) fn owns_pid(ps_command: &str, program: &str) -> bool {
    !program.is_empty()
        && ps_command
            .split_whitespace()
            .next()
            .is_some_and(|argv0| Path::new(argv0).file_name().is_some_and(|n| n == program))
}

/// Whether a live process is the exact command the service definition expects.
pub(super) fn matches_command(actual: &str, expected: &str) -> bool {
    actual == expected
}

pub(super) fn matches_identity(
    command: &str,
    expected_command: &str,
    start_time: &str,
    expected_start_time: &str,
    program: &str,
) -> bool {
    owns_pid(command, program)
        && matches_command(command, expected_command)
        && start_time == expected_start_time
}

/// Whether a ledger entry still describes the service we are about to
/// start. A slot change moves a port and a version bump can move a
/// binary, so a stale entry must be replaced rather than reused.
pub(super) fn describes(record: &Started, service: &Service) -> bool {
    record.label == service.label
        && record.port == service.port
        && record.program == program_name(&service.program)
}

/// The last few lines of a log, for an error message.
pub(super) fn tail(log: &str, lines: usize) -> String {
    let all: Vec<&str> = log.lines().filter(|line| !line.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// What `ps` reports for a PID, or `None` when it has exited.
fn ps_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "command="])
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!command.is_empty()).then_some(command)
}

fn ps_start_time(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "lstart="])
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// The stable identity fields for a live process.
pub(super) fn process_identity(pid: u32) -> Option<(String, String)> {
    Some((ps_command(pid)?, ps_start_time(pid)?))
}

/// Whether the recorded process is still alive and still ours.
fn still_ours(record: &Started) -> bool {
    process_identity(record.pid).is_some_and(|(command, start_time)| {
        matches_identity(
            &command,
            &record.command,
            &start_time,
            &record.start_time,
            &record.program,
        )
    })
}

pub(super) fn is_live(record: &Started) -> bool {
    still_ours(record) && port_listening(record.port)
}

fn port_listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}")
            .parse()
            .expect("a loopback socket address is well-formed"),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// Start or adopt shared services, optionally using records from the host
/// registry. The registry is read and written by the caller while holding the
/// host lock; this function only performs process identity checks.
pub(super) fn ensure_all_with_existing(
    root: &Path,
    services: &[Service],
    registered: &[Started],
) -> Result<Vec<Started>> {
    let existing = read_ledger(root);
    let mut records = Vec::with_capacity(services.len());
    for service in services {
        let reusable = registered
            .iter()
            .chain(existing.iter())
            .find(|record| describes(record, service))
            .filter(|record| is_live(record));
        match reusable {
            Some(record) => {
                eprintln!(
                    "==> {} already serving 127.0.0.1:{} (pid {})",
                    service.label, record.port, record.pid
                );
                records.push(record.clone());
            }
            None => records.push(adopt_or_start(root, service)?),
        }
    }
    write_ledger(root, &records)?;
    Ok(records)
}

/// Find a listener on the service port and adopt it only when its command line
/// is the command this service would start. A port probe alone is never
/// ownership proof.
fn adopt_or_start(root: &Path, service: &Service) -> Result<Started> {
    if let Some(record) = discover(root, service)? {
        eprintln!(
            "==> {} adopted 127.0.0.1:{} (pid {})",
            service.label, record.port, record.pid
        );
        return Ok(record);
    }
    start(root, service)
}

/// Discover a process listening on `service.port`. `lsof` is part of the
/// supported macOS host and no listener is a normal "start it" result.
fn discover(_root: &Path, service: &Service) -> Result<Option<Started>> {
    let output = Command::new("lsof")
        .args([
            "-nP",
            "-t",
            "-iTCP",
            &service.port.to_string(),
            "-sTCP:LISTEN",
        ])
        .output()
        .context("find a native service listening on its port")?;
    if !output.status.success() {
        return Ok(None);
    }
    let expected = expected_command(service);
    for pid in String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
    {
        let Some((command, start_time)) = process_identity(pid) else {
            continue;
        };
        if owns_pid(&command, &program_name(&service.program))
            && matches_command(&command, &expected)
        {
            return Ok(Some(Started {
                label: service.label.to_string(),
                pid,
                port: service.port,
                program: program_name(&service.program),
                command,
                start_time,
            }));
        }
    }
    Ok(None)
}

fn expected_command(service: &Service) -> String {
    std::iter::once(service.program.display().to_string())
        .chain(service.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spawn one service and wait for its port.
fn start(root: &Path, service: &Service) -> Result<Started> {
    let dir = service_dir(root, service.label);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    // Truncated, not appended. This log is only ever opened for a
    // process about to be started, so the previous run's contents are
    // finished business — and leaving them in place puts an old failure
    // directly above a new one in the tail below, which reads as one
    // event.
    let log = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path(root, service.label))
        .with_context(|| format!("open {}", log_path(root, service.label).display()))?;
    let log_err = log
        .try_clone()
        .context("duplicate the process log handle")?;

    eprintln!(
        "==> starting {} on 127.0.0.1:{}",
        service.label, service.port
    );
    let mut command = Command::new(&service.program);
    command
        .args(&service.args)
        .envs(service.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&service.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    detach(&mut command);
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", service.program.display()))?;
    let pid = child.id();
    // Detached on purpose: the tier outlives this CLI invocation, and
    // `Child::drop` does not kill a process it never waited on.
    std::mem::forget(child);

    let Some((command_identity, start_time)) = process_identity(pid) else {
        bail!(
            "{} process {} disappeared before identity could be recorded",
            service.label,
            pid
        );
    };
    let record = Started {
        label: service.label.to_string(),
        pid,
        port: service.port,
        program: program_name(&service.program),
        command: command_identity,
        start_time,
    };
    if let Err(err) = super::super::wait_for_tcp("127.0.0.1", service.port) {
        // The process either died or never bound. Reclaim it before
        // reporting, so a failed `up` does not leave an unrecorded
        // process holding a data directory open.
        signal(&record);
        let log = fs::read_to_string(log_path(root, service.label)).unwrap_or_default();
        bail!(
            "{} did not accept connections on 127.0.0.1:{}: {err}\n\
             last lines of {}:\n{}",
            service.label,
            service.port,
            log_path(root, service.label).display(),
            tail(&log, LOG_TAIL_LINES),
        );
    }
    Ok(record)
}

/// Stop one record only when its identity still matches the process that was
/// recorded. This is the only native path allowed to signal a shared service.
pub(super) fn stop(record: &Started) {
    if still_ours(record) {
        eprintln!("==> stopping {} (pid {})", record.label, record.pid);
        signal(record);
    }
}

/// `SIGTERM`, then `SIGKILL` if the process is still there.
///
/// Garage flushes on `SIGTERM`, so the grace period is not politeness —
/// a `SIGKILL`ed process leaves a data directory that needs recovery on
/// the next start.
fn signal(record: &Started) {
    if !still_ours(record) {
        return;
    }
    kill("-TERM", record.pid);
    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if !still_ours(record) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    kill("-KILL", record.pid);
}

fn kill(signal: &str, pid: u32) {
    let _ = Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Put the child in its own process group so a Ctrl-C in the terminal
/// that ran `up` does not tear the tier down with it.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn detach(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::{
        describes, ledger_path, matches_command, matches_identity, owns_pid, program_name,
        service_dir, tail, Service, Started,
    };
    use std::path::{Path, PathBuf};

    fn service() -> Service {
        Service {
            label: "surreal",
            program: PathBuf::from("/opt/homebrew/bin/surreal"),
            args: vec!["start".into(), "memory".into()],
            env: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            port: 20_034,
        }
    }

    /// PIDs are reused. Signalling a recorded number without re-reading
    /// what now holds it is how a teardown kills an unrelated process, so
    /// the match is on the executable rather than on presence.
    #[test]
    fn a_recycled_pid_is_not_recognized_as_ours() {
        assert!(owns_pid(
            "/opt/homebrew/bin/surreal start memory --bind 0.0.0.0:20034",
            "surreal"
        ));
        assert!(!owns_pid("/usr/bin/vim notes.txt", "surreal"));
        assert!(!owns_pid("", "surreal"));
        assert!(!owns_pid("/opt/homebrew/bin/garage server", "surreal"));
    }

    /// An empty program name would match anything with an argv[0], which
    /// is the one input that must never be treated as ownership.
    #[test]
    fn an_empty_program_name_never_claims_a_pid() {
        assert!(!owns_pid("/usr/bin/anything", ""));
    }

    /// `ps` prints the absolute path the process was launched with. The
    /// ledger records only the file name, so the comparison has to be on
    /// the basename — matching the whole string would fail for every
    /// process we actually start.
    #[test]
    fn ownership_matches_the_basename_not_the_full_path() {
        assert!(owns_pid(
            "/opt/homebrew/bin/surreal start memory",
            "surreal"
        ));
        assert_eq!(
            program_name(Path::new("/opt/homebrew/bin/surreal")),
            "surreal"
        );
    }

    /// A slot change moves every port. Reusing a ledger entry whose port
    /// no longer matches would report a service ready at an address
    /// nothing is listening on.
    #[test]
    fn a_ledger_entry_on_a_different_port_does_not_describe_the_service() {
        let service = service();
        let matching = Started {
            label: "surreal".into(),
            pid: 4242,
            port: 20_034,
            program: "surreal".into(),
            command: "/opt/homebrew/bin/surreal start memory".into(),
            start_time: "Mon Jan  1 00:00:00 2024".into(),
        };
        assert!(describes(&matching, &service));

        let moved = Started {
            port: 20_035,
            ..matching.clone()
        };
        assert!(!describes(&moved, &service));

        let renamed = Started {
            program: "garage".into(),
            ..matching.clone()
        };
        assert!(!describes(&renamed, &service));

        let other = Started {
            label: "garage".into(),
            ..matching
        };
        assert!(!describes(&other, &service));
    }

    #[test]
    fn a_recycled_pid_with_the_same_binary_has_a_different_command_identity() {
        assert!(matches_command(
            "/opt/homebrew/bin/surreal start memory",
            "/opt/homebrew/bin/surreal start memory"
        ));
        assert!(!matches_command(
            "/opt/homebrew/bin/surreal start --bind 127.0.0.1:8000 memory",
            "/opt/homebrew/bin/surreal start memory"
        ));
        assert!(!matches_identity(
            "/opt/homebrew/bin/surreal start memory",
            "/opt/homebrew/bin/surreal start memory",
            "Tue Jan  2 00:00:00 2024",
            "Mon Jan  1 00:00:00 2024",
            "surreal"
        ));
    }

    /// The failure message is the whole diagnostic when a dependency
    /// refuses to bind, so it must survive a log shorter than the tail
    /// it asks for, and must drop the blank lines these servers pad with.
    #[test]
    fn the_log_tail_is_the_last_lines_without_blanks() {
        assert_eq!(tail("a\nb\nc\nd\n", 2), "c\nd");
        assert_eq!(tail("only\n", 5), "only");
        assert_eq!(tail("", 5), "");
        assert_eq!(tail("a\n\n\nb\n", 5), "a\nb");
    }

    /// Both paths live under the worktree's gitignored `.devx/`, which is
    /// what keeps a native tier's data out of the tree and reclaimable by
    /// `worktree-env down`.
    #[test]
    fn every_state_path_stays_inside_the_worktrees_devx_directory() {
        let root = Path::new("/checkout");

        assert_eq!(
            ledger_path(root),
            PathBuf::from("/checkout/.devx/native-processes.json")
        );
        assert_eq!(
            service_dir(root, "garage"),
            PathBuf::from("/checkout/.devx/native/garage")
        );
    }
}
