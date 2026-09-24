//! `navigator ops cut-release` end-to-end: it names today's UTC `YY.M.D`
//! and either writes that version or fails when a published release already
//! covers today. `--dry-run` is the rehearsal — stdout is the tag, nothing
//! is written.
//!
//! These run against the real clock (there is no `--now` to inject one), so
//! the cuttable case computes the expected tag the same way the binary does,
//! and the covered case anchors on a fixed tag far enough in the future to
//! stay past today for the life of this repository.

use assert_cmd::Command;
use chrono::{Datelike, Utc};
use std::fs;
use std::process::Output;

/// A minimal workspace manifest with a dependency `version =` that MUST
/// survive, so a regression that widens the rewrite is caught here.
const MANIFEST: &str = "\
[workspace.package]
version = \"0.1.0\"
edition = \"2021\"
license = \"BUSL-1.1\"

[workspace.dependencies]
serde = { version = \"1\" }
";

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
        vec!["config", "user.email", "cut-release-cli@example.com"],
        vec!["config", "user.name", "cut release cli"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "root"],
    ] {
        assert!(git(dir, &args).status.success(), "git {args:?} failed");
    }
}

fn today_tag() -> String {
    let now = Utc::now();
    format!("{}.{}.{}", now.year() % 100, now.month(), now.day())
}

fn navigator() -> Command {
    Command::cargo_bin("navigator").unwrap()
}

#[test]
fn dry_run_prints_todays_date_when_nothing_is_released_yet() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());

    let output = navigator()
        .args([
            "ops",
            "cut-release",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
            "--dry-run",
        ])
        .output()
        .expect("run navigator");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        today_tag(),
        "stdout must be exactly today's date and nothing else"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--dry-run"),
        "stderr must say this was a rehearsal, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A release already published for a date far past today's: the cut fails
/// loudly. `release-default-tag` exits 0 in this situation; this command
/// does not — an operator who asked to cut must not read "already covered"
/// as success.
#[test]
fn fails_loudly_when_a_later_version_is_already_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    assert!(git(dir.path(), &["tag", "99.12.31"]).status.success());

    let output = navigator()
        .args([
            "ops",
            "cut-release",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
            "--dry-run",
        ])
        .output()
        .expect("run navigator");

    assert!(
        !output.status.success(),
        "already-covered must be a failure, got status {:?}",
        output.status
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "stdout must be empty when there is nothing to cut"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("99.12.31") && stderr.contains("already published"),
        "stderr must name the published version, got: {stderr}"
    );
}

#[test]
fn a_directory_with_no_git_history_fails_rather_than_guessing() {
    let dir = tempfile::tempdir().expect("tempdir");

    navigator()
        .args([
            "ops",
            "cut-release",
            "--repo",
            dir.path().to_str().unwrap(),
            "--no-fetch",
            "--dry-run",
        ])
        .assert()
        .failure();
}

/// `--no-commit` writes today's version into the manifest and leaves
/// dependency pins alone. The tags come from `--repo`; the file comes
/// from `--manifest-path`.
#[test]
fn writes_todays_version_without_committing() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    navigator()
        .args([
            "ops",
            "cut-release",
            "--repo",
            dir.path().to_str().unwrap(),
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--no-fetch",
            "--no-commit",
        ])
        .assert()
        .success();

    let written = fs::read_to_string(&manifest).expect("read manifest");
    let today = today_tag();
    assert!(
        written.contains(&format!("version = \"{today}\"")),
        "the workspace version must be today's UTC date, got: {written}"
    );
    assert!(
        !written.contains("0.1.0"),
        "the old workspace version must be gone"
    );
    assert!(
        written.contains("serde = { version = \"1\" }"),
        "a dependency pin must never be rewritten"
    );
}

/// `--dry-run` must not touch the manifest even when today is cuttable.
#[test]
fn dry_run_does_not_write_the_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    navigator()
        .args([
            "ops",
            "cut-release",
            "--repo",
            dir.path().to_str().unwrap(),
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--no-fetch",
            "--dry-run",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(&manifest).expect("read manifest"),
        MANIFEST
    );
}
