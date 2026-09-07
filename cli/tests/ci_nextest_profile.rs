//! The workspace merge gate classifies nextest retries as FLAKY and always
//! uploads the `JUnit` report the `ci` profile writes.

use std::path::{Path, PathBuf};

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
fn workspace_gate_runs_nextest_with_the_ci_profile_and_uploads_junit() {
    let workflow = ci_workflow();
    assert!(
        workflow.contains("cargo nextest run --workspace --no-fail-fast --profile ci"),
        "the workspace gate must run nextest with `--profile ci` so retries are recorded FLAKY"
    );
    assert!(
        workflow.contains("target/nextest/ci/junit.xml"),
        "every workspace-gate run must upload the ci-profile JUnit report"
    );
    assert!(
        workflow.contains("RUST_BACKTRACE: full") || workflow.contains("RUST_BACKTRACE: \"full\""),
        "the workspace gate must export RUST_BACKTRACE=full"
    );
    assert!(
        workflow.contains("/tmp/navigator-workspace-tests.log"),
        "the workspace gate must tee nextest output to a failure artifact"
    );
    assert!(
        Path::new(&repo_root().join(".github/workflows/ci.yml")).is_file(),
        "ci.yml must exist at the path this guard reads"
    );
}
