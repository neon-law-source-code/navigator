//! Integration tests for the top-level `navigator validate` subcommand.
//!
//! `project gate` identifies a repository root and refuses to run anywhere
//! else. `validate` is the directory-scoped rule set: it takes a path, makes
//! no assumption about the surrounding repository, and is what a tree that is
//! neither a Navigator checkout nor a Project repository still has.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::str;
use tempfile::TempDir;

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn navigator() -> Command {
    let mut command = Command::cargo_bin("navigator").unwrap();
    command.env_remove("GITHUB_REPOSITORY");
    command
}

fn validate_output(dir: &Path, extra_args: &[&str]) -> (String, i32) {
    let mut command = navigator();
    command.arg("validate").arg(dir);
    for arg in extra_args {
        command.arg(arg);
    }
    let output = command.output().unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.code().unwrap(),
    )
}

/// A deployment tree with Markdown and YAML, and none of the markers the
/// gate uses to recognise a repository root or a Project.
#[test]
fn validate_lints_a_directory_that_is_not_a_recognised_repository() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Notes.md", "Plain body line.\n");
    write(
        dir.path(),
        "kustomization.yaml",
        "resources:\n  - web.yaml\n",
    );

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("has no README and no .git"));

    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ))
        .stdout(str::contains("Parsed 1 YAML file(s), found 0 error(s)"));
}

#[test]
fn validate_defaults_to_the_current_directory() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Notes.md", "Plain body line.\n");
    navigator()
        .current_dir(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(str::contains(
            "Scanned 1 file(s), found 0 error(s), 0 warning(s)",
        ));
}

#[test]
fn validate_exits_nonzero_on_violations_and_prints_each_one() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Bad.md",
        &format!("Intro.\n\n{}\n", "x".repeat(121)),
    );
    navigator()
        .args(["validate"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stdout(str::contains("S101"))
        .stdout(str::contains("Scanned 1 file(s), found"));
}

#[test]
fn validate_returns_exit_code_2_when_the_directory_does_not_exist() {
    navigator()
        .args(["validate", "/definitely/does/not/exist/12345"])
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("navigator:"));
}

#[test]
fn validate_help_keeps_the_directory_flags() {
    navigator()
        .args(["validate", "--help"])
        .assert()
        .success()
        .stdout(str::contains("[DIR]"))
        .stdout(str::contains("--fix"))
        .stdout(str::contains("--errors-only"))
        .stdout(str::contains("--ci"));
}

/// `--errors-only` narrows the listing and nothing else: the summary still
/// counts every advisory, and the exit code is unchanged.
#[test]
fn validate_errors_only_hides_advisories_but_not_their_count() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "docs/lib.rs", "pub fn placeholder() {}\n");
    write(
        dir.path(),
        "docs/a_long.md",
        &format!("Intro.\n\n{}\n", "x".repeat(130)),
    );
    write(
        dir.path(),
        "docs/m_one.md",
        "Body.\n\nSee [lib](lib.rs) for detail.\n",
    );

    let (stdout, code) = validate_output(dir.path(), &["--errors-only"]);
    assert_eq!(code, 1, "the gate is unchanged by --errors-only:\n{stdout}");
    assert!(
        !stdout.contains("M061"),
        "--errors-only must hide the advisories:\n{stdout}",
    );
    assert!(
        stdout.contains("found 1 error(s), 1 warning(s)"),
        "the summary still counts the hidden advisories:\n{stdout}",
    );
    assert!(stdout.contains("S101"), "the error still prints:\n{stdout}",);
}

#[test]
fn validate_errors_only_is_rejected_with_fix() {
    let dir = TempDir::new().unwrap();
    navigator()
        .args(["validate", "--errors-only", "--fix"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(str::contains(
            "the argument '--errors-only' cannot be used with '--fix'",
        ));
}

#[test]
fn validate_fix_writes_back_autofixable_edits() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "Mixed.md",
        "Body line with trailing spaces   \n\nTabbed\there\n",
    );
    navigator()
        .args(["validate", "--fix"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("fixed"));
    let after = fs::read_to_string(dir.path().join("Mixed.md")).unwrap();
    assert_eq!(
        after, "Body line with trailing spaces\n\nTabbed  here\n",
        "expected M009 + M010 autofixes; got: {after:?}",
    );
}
