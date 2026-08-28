//! End-to-end tests for `navigator template narrate <file> --out <html>`.

use std::fs;
use std::process::Command;

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

const MOTION: &str = "\
---
title: Sample Motion
---

## 1. Introduction

The court should deny the request.

## 2. Argument

> **A. Standard of review.** The question is de novo.
>
> **B. The claim fails.** The record does not support it.
";

fn write(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).expect("write fixture");
    path
}

#[test]
fn narrate_writes_a_stage_with_arabic_and_lettered_units() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "motion.md", MOTION);
    let out = work.path().join("stage.html");
    let result = Command::new(cargo_bin("navigator"))
        .args(["template", "narrate"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run navigator template narrate");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let html = fs::read_to_string(&out).expect("read stage");
    assert!(html.contains("data-harvard-outline"), "{html}");
    assert!(html.contains("data-harvard-path=\"1\""), "{html}");
    assert!(html.contains("data-harvard-path=\"2.A\""), "{html}");
    assert!(html.contains("data-harvard-path=\"2.B\""), "{html}");
    assert!(html.contains("harvard-unit--depth-1"), "{html}");
    assert!(
        html.contains("harvard-unit is-current") || html.contains("data-harvard-index"),
        "{html}"
    );
    assert!(html.contains("Arrow keys"), "{html}");
    assert!(
        html.contains("URLSearchParams"),
        "start-index query: {html}"
    );
}

#[test]
fn narrate_refuses_a_missing_file() {
    let work = TempDir::new().unwrap();
    let out = work.path().join("stage.html");
    let result = Command::new(cargo_bin("navigator"))
        .args(["template", "narrate", "no-such-file.md", "--out"])
        .arg(&out)
        .output()
        .expect("run narrate on missing file");
    assert!(!result.status.success());
    assert!(!out.exists());
}

#[test]
fn narrate_refuses_an_empty_body() {
    let work = TempDir::new().unwrap();
    let src = write(&work, "empty.md", "---\ntitle: Empty\n---\n\n");
    let out = work.path().join("stage.html");
    let result = Command::new(cargo_bin("navigator"))
        .args(["template", "narrate"])
        .arg(&src)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("run narrate on empty body");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("no outline units"), "{stderr}");
}
