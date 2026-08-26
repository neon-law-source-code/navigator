//! `navigator ops release-default-tag` end-to-end: it prints the bare
//! candidate tag on stdout when today is releasable, and nothing on stdout —
//! only a reason on stderr — when a release already exists that makes
//! today's date no improvement. Either way it exits 0: "nothing to cut
//! today" is the ordinary answer, not a failure.
//!
//! These run against the real clock (there is no `--now` to inject one,
//! deliberately — see `cli/src/release_default_tag.rs`), so the "releasable"
//! case computes the expected tag the same way the binary does, and the
//! "nothing to do" case anchors on a fixed tag far enough in the future to
//! stay past today for the life of this repository.

use assert_cmd::Command;
use chrono::{Datelike, Utc};
use std::process::Output;

fn git(repo: &std::path::Path, args: &[&str]) -> Output {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs")
}

fn init_repo(dir: &std::path::Path) {
    for args in [
        vec!["init", "--quiet", "--initial-branch=main"],
        vec![
            "config",
            "user.email",
            "release-default-tag-cli@example.com",
        ],
        vec!["config", "user.name", "release default tag cli"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "root"],
    ] {
        assert!(git(dir, &args).status.success(), "git {args:?} failed");
    }
}

fn today_tag() -> String {
    let now = Utc::now();
    format!("{}.{}.{}", now.year() % 100, now.month(), now.day())
}

#[test]
fn suggests_todays_date_when_nothing_is_released_yet() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());

    let output = Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "release-default-tag",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
        ])
        .output()
        .expect("run navigator");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        today_tag(),
        "stdout must be exactly today's date and nothing else"
    );
}

/// A release already published for a date far past today's: nothing is
/// suggested, and the run still exits 0.
#[test]
fn suggests_nothing_and_still_exits_zero_when_a_later_version_is_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    assert!(git(dir.path(), &["tag", "99.12.31"]).status.success());

    let output = Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "release-default-tag",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
        ])
        .output()
        .expect("run navigator");

    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "stdout must be empty when there is nothing to cut"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to do"),
        "stderr must explain why, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--repo` pointed at a directory with no git history at all fails loudly
/// rather than reporting an empty tag list as "today is releasable."
#[test]
fn a_directory_with_no_git_history_fails_rather_than_guessing() {
    let dir = tempfile::tempdir().expect("tempdir");

    Command::cargo_bin("navigator")
        .unwrap()
        .args([
            "ops",
            "release-default-tag",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
        ])
        .assert()
        .failure();
}
