//! Integration tests for the `navigator site {login,logout,whoami}` group.
//!
//! These drive the real binary as a subprocess so they cover both the
//! `Command::Auth` dispatch in `main.rs` and the `run_*` handlers in
//! `login.rs`, using a per-process `NAVIGATOR_CREDENTIALS_FILE` so no two
//! tests race on the shared credential dotfile.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};

use assert_cmd::Command as AssertCommand;

/// Run `navigator site <args>` against a credentials file at `creds_path`
/// (which may or may not exist) and return `(exit_code, stderr)`.
fn run_auth(creds_path: &std::path::Path, args: &[&str]) -> (Option<i32>, String) {
    let output = AssertCommand::cargo_bin("navigator")
        .unwrap()
        .args(args)
        .env("NAVIGATOR_CREDENTIALS_FILE", creds_path)
        .output()
        .expect("run navigator site");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn auth_whoami_without_any_login_reports_no_stored_logins() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");
    // File is absent → load yields an empty store → resolve_base with no
    // --host has nothing to resolve.
    let (code, stderr) = run_auth(&creds, &["site", "whoami"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("navigator site whoami:") && stderr.contains("no stored logins"),
        "stderr: {stderr}"
    );
}

#[test]
fn auth_logout_without_any_login_reports_no_stored_logins() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");
    let (code, stderr) = run_auth(&creds, &["site", "logout"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("navigator site logout:") && stderr.contains("no stored logins"),
        "stderr: {stderr}"
    );
}

#[test]
fn auth_whoami_with_an_unknown_host_reports_not_logged_in() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");
    // A host is named, so `resolve_base` succeeds, but the store has no
    // entry for it → the "not logged in to …" branch.
    let (code, stderr) = run_auth(
        &creds,
        &["site", "whoami", "--host", "https://live.example.com"],
    );
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("not logged in to https://live.example.com"),
        "stderr: {stderr}"
    );
}

#[test]
fn auth_whoami_reports_a_corrupt_credential_file() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");
    std::fs::write(&creds, b"{ this is not json").unwrap();
    let (code, stderr) = run_auth(&creds, &["site", "whoami"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("navigator site whoami:"),
        "stderr: {stderr}"
    );
}

#[test]
fn auth_logout_reports_a_corrupt_credential_file() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");
    std::fs::write(&creds, b"{ this is not json").unwrap();
    let (code, stderr) = run_auth(&creds, &["site", "logout"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("navigator site logout:"),
        "stderr: {stderr}"
    );
}

/// Drive `navigator login` far enough to exercise the browser-loopback
/// error path: read the redirect port off stdout, then POST a state-mismatch
/// callback so `login_inner` rejects the token and `run_login` prints its
/// error and exits `2` — without ever contacting a real identity provider.
#[test]
fn auth_login_rejects_a_tampered_callback() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("navigator.json");

    let mut child = Command::new(env!("CARGO_BIN_EXE_navigator"))
        .args(["site", "login", "--host", "127.0.0.1:9", "--no-browser"])
        .env("NAVIGATOR_CREDENTIALS_FILE", &creds)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn navigator login");

    // The loopback port is printed on the "Waiting for the redirect on
    // http://127.0.0.1:<port>/cb" line, before the accept() blocks.
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let needle = "http://127.0.0.1:";
    let mut port = None;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        if let Some(idx) = line.find(needle) {
            let digits: String = line[idx + needle.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                port = Some(digits);
                break;
            }
        }
        line.clear();
    }
    let port = port.expect("redirect port printed on stdout");

    let mut sock = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    sock.write_all(b"GET /cb?token=leaked&state=tampered HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut resp = Vec::new();
    let _ = sock.read_to_end(&mut resp);

    let status = child.wait().expect("navigator login exits");
    assert_eq!(status.code(), Some(2));

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("navigator login:"), "stderr: {stderr}");
}
