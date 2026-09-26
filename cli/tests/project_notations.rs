//! Integration tests for the project notation workflow board command.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str;

#[test]
fn invalid_stale_duration_is_not_echoed_to_stderr() {
    let root = tempfile::tempdir().expect("create project checkout");
    std::fs::write(
        root.path().join("navigator.yaml"),
        "project:\n  host: staging.neonlaw.com\n  name: sample-project\n",
    )
    .expect("write project manifest");

    Command::cargo_bin("navigator")
        .expect("navigator binary")
        .args(["project", "notations", "--stale", "private-value"])
        .current_dir(root.path())
        .assert()
        .code(2)
        .stderr(
            str::contains("invalid --stale duration").and(str::contains("private-value").not()),
        );
}
