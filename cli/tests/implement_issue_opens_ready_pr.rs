//! `/implement-issue` ships a ready pull request, not a draft and not a
//! worktree that still needs a second command to open one.
//!
//! Auto-merge is armed only on a non-draft open (`docs/gitops.md`). A session
//! that implements an issue and stops at a draft, or at "hand this tree to
//! create-pr", leaves the change published-but-unmerged until a human notices.
//! The skill text is the instruction agents actually follow, so the contract
//! is asserted here rather than remembered.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn skill(name: &str) -> String {
    fs::read_to_string(
        repo_root()
            .join(".agents/skills")
            .join(name)
            .join("SKILL.md"),
    )
    .unwrap_or_else(|e| panic!("read {name} SKILL.md: {e}"))
}

/// The implementation skill must tell the agent to open the PR, and to open
/// it ready for review.
#[test]
fn implement_issue_opens_a_ready_pull_request() {
    let body = skill("implement-issue");
    assert!(
        body.contains("ready for review, not as a draft"),
        "implement-issue must require a non-draft PR, got:\n{body}"
    );
    assert!(
        body.to_ascii_lowercase().contains("open a pull request")
            || body.contains("open the pull request"),
        "implement-issue must open the PR in the same session, got:\n{body}"
    );
    assert!(
        !body.contains("do not push, change Linear, or open a pull request unless the user asks"),
        "implement-issue must not stop before the PR"
    );
}

/// `create-pr` is the open step. If it defaulted to draft, implement-issue
/// would still ship a held PR.
#[test]
fn create_pr_opens_ready_not_draft() {
    let body = skill("create-pr");
    assert!(
        body.contains("ready for review, not as a draft"),
        "create-pr must open ready for review, got:\n{body}"
    );
    assert!(
        !body.contains("gh pr create --draft"),
        "create-pr must not pass --draft to gh pr create"
    );
}
