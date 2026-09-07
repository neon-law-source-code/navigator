//! `navigator dev flake-hunt` — loop one package's tests and print the
//! pass/fail wall-time distribution.
//!
//! Classification keys on the printed summary (nextest's `Summary` line, or
//! cucumber/`cargo test`'s `test result:` / scenarios line), never on a
//! pipeline's exit status. Piping a suite through `tee` or `tail` makes `$?`
//! report the last process, so a red run can look green.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// One captured process result. The numeric status is recorded but never used
/// to decide pass versus fail.
#[derive(Debug, Clone)]
pub(crate) struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub(crate) trait CommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<CommandOutput>;
}

pub(crate) struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<CommandOutput> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("run `{program} {}`", args.join(" ")))?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Pass,
    Fail,
}

/// Build the argv for one hunt iteration. Never wraps the suite in `taskset`:
/// local contention is the machine's, and CI affinity is not this command's job.
pub(crate) fn hunt_argv(package: &str, filter: Option<&str>) -> (String, Vec<String>) {
    if package == "features" {
        let mut args = vec!["test".into(), "-p".into(), "features".into()];
        if let Some(name) = filter.filter(|value| !value.is_empty()) {
            args.push("--test".into());
            args.push(name.into());
        }
        ("cargo".into(), args)
    } else {
        let mut args = vec!["nextest".into(), "run".into(), "-p".into(), package.into()];
        if let Some(expr) = filter.filter(|value| !value.is_empty()) {
            args.push("-E".into());
            args.push(expr.into());
        }
        ("cargo".into(), args)
    }
}

pub(crate) fn classify(package: &str, output: &CommandOutput) -> RunOutcome {
    let combined = format!("{}{}", output.stdout, output.stderr);
    if package == "features" {
        classify_features(&combined)
    } else {
        classify_nextest(&combined)
    }
}

fn classify_nextest(combined: &str) -> RunOutcome {
    let Some(line) = combined
        .lines()
        .rev()
        .find(|line| line.contains("Summary") && line.contains("tests run:"))
    else {
        return RunOutcome::Fail;
    };
    if failed_count(line) > 0 {
        RunOutcome::Fail
    } else {
        RunOutcome::Pass
    }
}

fn classify_features(combined: &str) -> RunOutcome {
    if let Some(line) = combined
        .lines()
        .rev()
        .find(|line| line.contains("test result:"))
    {
        return if line.contains("test result: ok") {
            RunOutcome::Pass
        } else {
            RunOutcome::Fail
        };
    }
    if let Some(line) = combined.lines().rev().find(|line| {
        let trimmed = line.trim_start();
        trimmed.contains("scenarios (") || trimmed.contains("scenario (")
    }) {
        return if failed_count(line) > 0 {
            RunOutcome::Fail
        } else {
            RunOutcome::Pass
        };
    }
    RunOutcome::Fail
}

fn failed_count(line: &str) -> u64 {
    for (idx, _) in line.match_indices(" failed") {
        let prefix = &line[..idx];
        if let Some(number) = prefix
            .rsplit(|ch: char| !ch.is_ascii_digit())
            .find(|token| !token.is_empty())
            .and_then(|token| token.parse::<u64>().ok())
        {
            return number;
        }
    }
    0
}

/// Run the package `runs` times and print each outcome plus the distribution.
pub(crate) fn hunt<R: CommandRunner>(
    package: &str,
    filter: Option<&str>,
    runs: u32,
    runner: &mut R,
    mut out: impl Write,
) -> Result<()> {
    if runs == 0 {
        bail!("flake-hunt --runs must be at least 1");
    }
    let (program, args) = hunt_argv(package, filter);
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut pass_times = Vec::new();
    let mut fail_times = Vec::new();

    writeln!(
        out,
        "flake-hunt {package}{} — {runs} runs",
        filter
            .filter(|value| !value.is_empty())
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    )?;
    writeln!(out, "command: {program} {}", args.join(" "))?;

    for n in 1..=runs {
        let started = Instant::now();
        let output = runner.run(&program, &args)?;
        let elapsed = started.elapsed();
        let outcome = classify(package, &output);
        match outcome {
            RunOutcome::Pass => {
                passed += 1;
                pass_times.push(elapsed);
            }
            RunOutcome::Fail => {
                failed += 1;
                fail_times.push(elapsed);
            }
        }
        writeln!(
            out,
            "run {n:>width$}  {:4}  {}",
            match outcome {
                RunOutcome::Pass => "pass",
                RunOutcome::Fail => "fail",
            },
            format_duration(elapsed),
            width = runs.to_string().len()
        )?;
    }

    writeln!(out, "{runs} runs: {passed} passed, {failed} failed")?;
    write_time_summary(&mut out, "pass", &pass_times)?;
    write_time_summary(&mut out, "fail", &fail_times)?;

    if failed > 0 {
        bail!("{failed} of {runs} flake-hunt runs failed");
    }
    Ok(())
}

pub(crate) fn run(package: &str, filter: Option<&str>, runs: u32) -> Result<()> {
    hunt(
        package,
        filter,
        runs,
        &mut ProcessCommandRunner,
        io::stdout(),
    )
}

fn write_time_summary(out: &mut impl Write, label: &str, times: &[Duration]) -> io::Result<()> {
    if times.is_empty() {
        writeln!(out, "{label} wall times: (none)")?;
        return Ok(());
    }
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let median = sorted[sorted.len() / 2];
    writeln!(
        out,
        "{label} wall times: min={} median={} max={}",
        format_duration(min),
        format_duration(median),
        format_duration(max)
    )
}

fn format_duration(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    struct FakeRunner {
        scripted: Vec<Result<CommandOutput>>,
        calls: Vec<(String, Vec<String>)>,
    }

    impl FakeRunner {
        fn new(scripted: Vec<Result<CommandOutput>>) -> Self {
            Self {
                scripted,
                calls: Vec::new(),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, program: &str, args: &[String]) -> Result<CommandOutput> {
            self.calls.push((program.to_owned(), args.to_vec()));
            if self.scripted.is_empty() {
                return Err(anyhow!("unexpected extra run"));
            }
            self.scripted.remove(0)
        }
    }

    fn nextest_summary(failed: u64) -> CommandOutput {
        CommandOutput {
            stdout: format!(
                "        STARTING\n     Summary [   1.234s] 10 tests run: {} passed, {failed} failed, 0 skipped\n",
                10 - failed
            ),
            stderr: String::new(),
        }
    }

    #[test]
    fn hunt_argv_for_a_library_package_is_nextest_without_taskset() {
        let (program, args) = hunt_argv("store", Some("test(concurrent_find)"));
        assert_eq!(program, "cargo");
        assert_eq!(
            args,
            [
                "nextest",
                "run",
                "-p",
                "store",
                "-E",
                "test(concurrent_find)"
            ]
        );
        assert!(!args.iter().any(|arg| arg.contains("taskset")));
        assert_ne!(program, "taskset");
    }

    #[test]
    fn hunt_argv_for_features_uses_cargo_test() {
        let (program, args) = hunt_argv("features", Some("brand_routing"));
        assert_eq!(program, "cargo");
        assert_eq!(args, ["test", "-p", "features", "--test", "brand_routing"]);
    }

    #[test]
    fn nextest_summary_beats_a_misleading_exit_status() {
        let pass_despite_status = CommandOutput {
            stdout: "     Summary [   0.100s] 5 tests run: 5 passed, 0 skipped\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("store", &pass_despite_status), RunOutcome::Pass);

        let fail_despite_green_pipe = nextest_summary(2);
        assert_eq!(
            classify("store", &fail_despite_green_pipe),
            RunOutcome::Fail
        );
    }

    #[test]
    fn missing_nextest_summary_is_a_failure() {
        let output = CommandOutput {
            stdout: "pipe swallowed the suite\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("portal", &output), RunOutcome::Fail);
    }

    #[test]
    fn features_classifies_from_test_result_and_scenarios() {
        let ok = CommandOutput {
            stdout: "test result: ok. 15 passed; 0 failed; 0 ignored\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("features", &ok), RunOutcome::Pass);

        let failed = CommandOutput {
            stdout: "test result: FAILED. 14 passed; 1 failed; 0 ignored\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("features", &failed), RunOutcome::Fail);

        let scenarios_pass = CommandOutput {
            stdout: "12 scenarios (12 passed)\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("features", &scenarios_pass), RunOutcome::Pass);

        let scenarios_fail = CommandOutput {
            stdout: "12 scenarios (2 failed, 10 passed)\n".into(),
            stderr: String::new(),
        };
        assert_eq!(classify("features", &scenarios_fail), RunOutcome::Fail);
    }

    #[test]
    fn hunt_records_the_distribution_from_the_fake_runner() {
        let mut runner = FakeRunner::new(vec![
            Ok(nextest_summary(0)),
            Ok(nextest_summary(1)),
            Ok(nextest_summary(0)),
        ]);
        let mut buf = Vec::new();
        let err = hunt("store", None, 3, &mut runner, &mut buf).unwrap_err();
        assert!(err.to_string().contains("1 of 3 flake-hunt runs failed"));
        assert_eq!(runner.calls.len(), 3);
        assert_eq!(runner.calls[0].0, "cargo");
        assert_eq!(runner.calls[0].1, ["nextest", "run", "-p", "store"]);
        let printed = String::from_utf8(buf).unwrap();
        assert!(printed.contains("3 runs: 2 passed, 1 failed"));
        assert!(printed.contains("run 1  pass"));
        assert!(printed.contains("run 2  fail"));
        assert!(!printed.contains("taskset"));
    }

    #[test]
    fn hunt_refuses_zero_runs() {
        let mut runner = FakeRunner::new(vec![]);
        let mut buf = Vec::new();
        let err = hunt("store", None, 0, &mut runner, &mut buf).unwrap_err();
        assert!(err.to_string().contains("--runs must be at least 1"));
        assert!(runner.calls.is_empty());
    }
}
