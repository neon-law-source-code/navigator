//! The workspace merge gate runs at Rust and nextest defaults: no extra cap
//! on libtest or on nextest process concurrency.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root is cli/'s parent")
        .to_path_buf()
}

fn ci_workflow() -> String {
    let path = repo_root().join(".github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

#[test]
fn workspace_gate_does_not_cap_test_threads() {
    let workflow = ci_workflow();
    assert!(
        !workflow.contains("RUST_TEST_THREADS"),
        "ci.yml must not set RUST_TEST_THREADS"
    );
    assert!(
        !workflow.contains("--test-threads"),
        "ci.yml must not pass --test-threads"
    );
}
