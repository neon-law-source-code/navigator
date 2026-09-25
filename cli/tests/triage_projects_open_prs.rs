//! Portfolio triage must stay off pull requests that are still open.
//!
//! The skill text is the instruction agents follow. These assertions hold the
//! three rules that keep a lane from repeating work an open pull request
//! already owns.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn skill() -> String {
    let raw = fs::read_to_string(repo_root().join(".agents/skills/triage-projects/SKILL.md"))
        .expect("read triage-projects SKILL.md");
    // The project gate wraps prose. Match the instruction, not the line break.
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn refresh_records_open_pull_requests() {
    let body = skill();
    assert!(
        body.contains("record every open pull request"),
        "refresh must record open pull requests, got:\n{body}"
    );
}

#[test]
fn lanes_refuse_open_pull_request_overlap() {
    let body = skill();
    assert!(
        body.contains("Drop any issue an open pull request already links"),
        "lanes must refuse issues an open pull request already links, got:\n{body}"
    );
    assert!(
        body.contains("touch a path an open pull request or active worktree already changes"),
        "lanes must stay off paths an open pull request already changes, got:\n{body}"
    );
}

#[test]
fn prompts_cite_open_pull_requests_they_avoided() {
    let body = skill();
    assert!(
        body.contains("The open pull requests reviewed, and a statement that this prompt's files do not overlap them."),
        "prompts must cite the open pull requests they stayed clear of, got:\n{body}"
    );
    assert!(
        body.contains(
            "If every ready issue collides, report that and do not emit a duplicate prompt."
        ),
        "a full collision must be reported instead of a duplicate prompt, got:\n{body}"
    );
}
