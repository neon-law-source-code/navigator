//! End-to-end tests for `navigator notations format <file>`. Each test writes
//! a fixture to a tempdir, invokes the real binary, and reads the
//! file back to check the transform.

use std::fs;
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

fn write_fixture(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).expect("write fixture");
    path
}

#[test]
fn format_rewrites_dash_bullets_to_star_bullets() {
    let work = TempDir::new().unwrap();
    let path = write_fixture(&work, "note.md", "# Heading\n\n- one\n- two\n");
    let out = Command::new(cargo_bin("navigator"))
        .args(["notations", "format"])
        .arg(&path)
        .output()
        .expect("run navigator notations format");
    assert!(out.status.success(), "exit status: {:?}", out.status);
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, "# Heading\n\n* one\n* two\n");
}

#[test]
fn format_trims_trailing_whitespace() {
    let work = TempDir::new().unwrap();
    let path = write_fixture(&work, "note.md", "trailing   \nclean\n");
    let out = Command::new(cargo_bin("navigator"))
        .args(["notations", "format"])
        .arg(&path)
        .output()
        .expect("run navigator notations format");
    assert!(out.status.success());
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, "trailing\nclean\n");
}

#[test]
fn format_preserves_yaml_frontmatter_verbatim() {
    let work = TempDir::new().unwrap();
    let src = "---\ntitle: Tester   \ncode: T-1\n---\n- item\n";
    let path = write_fixture(&work, "with-frontmatter.md", src);
    let out = Command::new(cargo_bin("navigator"))
        .args(["notations", "format"])
        .arg(&path)
        .output()
        .expect("run navigator notations format");
    assert!(out.status.success());
    let after = fs::read_to_string(&path).unwrap();
    // Frontmatter (including stray trailing whitespace on `title:`)
    // is left untouched — the formatter scopes to the body.
    assert!(after.starts_with("---\ntitle: Tester   \ncode: T-1\n---\n"));
    assert!(after.ends_with("* item\n"));
}

#[test]
fn format_is_idempotent_on_clean_input() {
    let work = TempDir::new().unwrap();
    let clean = "# Heading\n\nA paragraph.\n\n* one\n* two\n";
    let path = write_fixture(&work, "clean.md", clean);
    let out = Command::new(cargo_bin("navigator"))
        .args(["notations", "format"])
        .arg(&path)
        .output()
        .expect("run navigator notations format");
    assert!(out.status.success());
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, clean);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already clean"),
        "expected `already clean` notice, got: {stdout}",
    );
}

#[test]
fn format_missing_file_exits_non_zero() {
    let out = Command::new(cargo_bin("navigator"))
        .args(["notations", "format"])
        .arg("/tmp/this-file-does-not-exist-please-trust.md")
        .output()
        .expect("run navigator notations format on missing file");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("file not found"),
        "expected `file not found` in stderr, got: {stderr}",
    );
}
