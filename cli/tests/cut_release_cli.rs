//! `navigator ops cut-release` end-to-end: it names today's UTC `YY.M.D`
//! and either writes that version or fails when a published release already
//! covers today. `--dry-run` is the rehearsal — stdout is the tag, nothing
//! is written.
//!
//! Fixed-date ordering lives in the decision unit tests. CLI smoke tests
//! accept either UTC date bracketing the subprocess, including midnight.

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

    let before = today_tag();
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
    let after = today_tag();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        [before.as_str(), after.as_str()].contains(&stdout.trim()),
        "{stdout}"
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

    assert_eq!(output.status.code(), Some(2));
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
/// dependency pins alone. The manifest belongs to the selected repository.
#[test]
fn writes_todays_version_without_committing() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    let before = today_tag();
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
    let after = today_tag();
    assert!(
        [before, after]
            .iter()
            .any(|tag| written.contains(&format!("version = \"{tag}\""))),
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

fn seed_cut_repo(root: &std::path::Path) {
    init_repo(root);
    fs::write(root.join("Cargo.toml"), MANIFEST).expect("manifest");
    assert!(git(root, &["add", "Cargo.toml"]).status.success());
    assert!(git(root, &["commit", "--quiet", "-m", "manifest"])
        .status
        .success());
    assert!(git(root, &["switch", "-c", "release-test"])
        .status
        .success());
}

#[test]
fn repo_selects_the_manifest_and_commit_without_touching_the_callers_checkout() {
    let target = tempfile::tempdir().expect("target");
    let caller = tempfile::tempdir().expect("caller");
    seed_cut_repo(target.path());
    seed_cut_repo(caller.path());
    let caller_head = git(caller.path(), &["rev-parse", "HEAD"]).stdout;
    let target_head = git(target.path(), &["rev-parse", "HEAD"]).stdout;

    let output = navigator()
        .current_dir(caller.path())
        .args(["ops", "cut-release", "--repo"])
        .arg(target.path())
        .arg("--no-fetch")
        .output()
        .expect("cut release");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        git(caller.path(), &["rev-parse", "HEAD"]).stdout,
        caller_head
    );
    assert_eq!(
        fs::read_to_string(caller.path().join("Cargo.toml")).unwrap(),
        MANIFEST
    );
    assert_ne!(
        git(target.path(), &["rev-parse", "HEAD"]).stdout,
        target_head
    );
    assert!(git(target.path(), &["status", "--porcelain"])
        .stdout
        .is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("git tag"));
}

#[test]
fn a_manifest_in_another_checkout_is_refused_before_writing() {
    let target = tempfile::tempdir().expect("target");
    let other = tempfile::tempdir().expect("other");
    seed_cut_repo(target.path());
    seed_cut_repo(other.path());
    navigator()
        .current_dir(target.path())
        .args([
            "ops",
            "cut-release",
            "--no-fetch",
            "--no-commit",
            "--manifest-path",
        ])
        .arg(other.path().join("Cargo.toml"))
        .assert()
        .code(2);
    assert_eq!(
        fs::read_to_string(other.path().join("Cargo.toml")).unwrap(),
        MANIFEST
    );
}

#[test]
fn automatic_commit_refuses_staged_changes_before_writing() {
    let target = tempfile::tempdir().expect("target");
    seed_cut_repo(target.path());
    fs::write(target.path().join("unrelated.txt"), "unrelated work").unwrap();
    assert!(git(target.path(), &["add", "unrelated.txt"])
        .status
        .success());
    let head = git(target.path(), &["rev-parse", "HEAD"]).stdout;
    navigator()
        .current_dir(target.path())
        .args(["ops", "cut-release", "--no-fetch"])
        .assert()
        .code(2);
    assert_eq!(git(target.path(), &["rev-parse", "HEAD"]).stdout, head);
    assert_eq!(
        fs::read_to_string(target.path().join("Cargo.toml")).unwrap(),
        MANIFEST
    );
    assert_eq!(
        String::from_utf8_lossy(&git(target.path(), &["diff", "--cached", "--name-only"]).stdout)
            .trim(),
        "unrelated.txt"
    );
}

#[test]
fn automatic_commit_refuses_main_and_detached_head_without_writes() {
    for checkout in ["main", "--detach"] {
        let target = tempfile::tempdir().expect("target");
        seed_cut_repo(target.path());
        assert!(git(target.path(), &["switch", checkout]).status.success());
        navigator()
            .current_dir(target.path())
            .args(["ops", "cut-release", "--no-fetch"])
            .assert()
            .code(2);
        assert_eq!(
            fs::read_to_string(target.path().join("Cargo.toml")).unwrap(),
            MANIFEST
        );
        assert!(git(target.path(), &["status", "--porcelain"])
            .stdout
            .is_empty());
    }
}

#[test]
fn fetch_failure_and_covered_date_preserve_release_files() {
    for covered in [false, true] {
        let target = tempfile::tempdir().expect("target");
        seed_cut_repo(target.path());
        let mut command = navigator();
        command
            .current_dir(target.path())
            .args(["ops", "cut-release"]);
        if covered {
            assert!(git(target.path(), &["tag", "99.12.31"]).status.success());
            command.arg("--no-fetch");
        }
        command.assert().code(2);
        assert_eq!(
            fs::read_to_string(target.path().join("Cargo.toml")).unwrap(),
            MANIFEST
        );
        assert!(git(target.path(), &["status", "--porcelain"])
            .stdout
            .is_empty());
    }
}
