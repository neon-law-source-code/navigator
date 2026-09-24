//! Guard the hand-off from a release to the Zed extension.
//!
//! `neon-law-source-code/zed-navigator-lsp` is the dev extension that attaches
//! `navigator-lsp` to Markdown buffers in Zed. Its version follows Navigator's
//! because `deploy.yml` tells it that a release landed, and it tags `v<tag>` —
//! the spelling navigator-ux uses — so the three repositories name one version.
//!
//! The two repositories never reference each other, so if this dispatch stops
//! firing the extension's version stops following Navigator's and nothing here
//! goes red. The contract is only holdable by a test, the same shape
//! `homebrew_tap_dispatch.rs` guards for the tap.

use std::fs;
use std::path::PathBuf;

/// The job that fires the dispatch.
const JOB: &str = "release-zed-extension";

/// The extension the version is handed to.
const EXTENSION_REPO: &str = "neon-law-source-code/zed-navigator-lsp";

fn deploy_workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("deploy.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn workflow() -> serde_yaml::Value {
    serde_yaml::from_str(&deploy_workflow()).expect("deploy.yml parses as YAML")
}

fn deploy_job(name: &str) -> serde_yaml::Value {
    workflow()
        .get("jobs")
        .and_then(|jobs| jobs.get(name))
        .cloned()
        .unwrap_or_else(|| panic!("deploy.yml must define the `{name}` job"))
}

fn job_needs(name: &str) -> Vec<String> {
    serde_yaml::from_value(deploy_job(name)["needs"].clone())
        .unwrap_or_else(|error| panic!("`{name}` must declare a `needs` list: {error}"))
}

fn job_text() -> String {
    serde_yaml::to_string(&deploy_job(JOB)).expect("the job serializes")
}

/// The extension is told only once the Release exists, so a bump never names a
/// tag whose `navigator-lsp` archives are not attached yet.
#[test]
fn the_extension_is_told_only_after_the_archives_are_attached() {
    let needs = job_needs(JOB);
    for required in ["release-windows-cli-publish", "release-version"] {
        assert!(
            needs.iter().any(|entry| entry == required),
            "`{JOB}` must need `{required}`"
        );
    }
}

/// Every publishable tag reaches the extension, and only those. A gate
/// narrower than `publishable` fails by skipping, which nothing reports.
#[test]
fn every_publishable_release_and_nothing_else_is_dispatched() {
    let gate = deploy_job(JOB)["if"]
        .as_str()
        .unwrap_or_else(|| panic!("`{JOB}` must declare an `if:` gate"))
        .to_string();
    assert_eq!(
        gate, "needs.release-version.outputs.publishable == 'true'",
        "`{JOB}` must be gated on `publishable` and nothing narrower"
    );
}

/// The payload is the tag and nothing else, and the extension repository is
/// the only writer of its own tree.
#[test]
fn the_dispatch_carries_the_tag_and_writes_nothing() {
    let job = job_text();
    for required in [
        EXTENSION_REPO,
        "event_type=navigator-release",
        "client_payload[tag]=${TAG}",
        "needs.release-version.outputs.tag",
    ] {
        assert!(job.contains(required), "`{JOB}` must retain `{required}`");
    }
    for forbidden in ["--method PUT", "--method PATCH", "git commit", "git push"] {
        assert!(
            !job.contains(forbidden),
            "`{JOB}` must not write to the extension itself (`{forbidden}`) — its `bump.yml` does"
        );
    }
}

/// No write grant here. The cross-repository reach is one scoped secret, and a
/// missing one fails the release rather than skipping the bump.
#[test]
fn the_job_holds_no_write_grant_and_refuses_a_missing_token() {
    assert_eq!(
        deploy_job(JOB)["permissions"]["contents"].as_str(),
        Some("read"),
        "`{JOB}` writes nothing in this repository"
    );
    let job = job_text();
    assert!(job.contains("secrets.ZED_EXTENSION_TOKEN"));
    assert!(
        job.contains("ZED_EXTENSION_TOKEN is unset") && job.contains("exit 1"),
        "a missing `ZED_EXTENSION_TOKEN` must fail the release"
    );
}

/// An accepted dispatch is not a completed bump; the release waits for the
/// `v<tag>` the extension pushes only after the shim builds.
#[test]
fn the_release_waits_for_the_extension_tag() {
    let job = job_text();
    assert!(
        job.contains("/git/ref/tags/v${TAG}"),
        "`{JOB}` must confirm the extension pushed `v<tag>`"
    );
    assert!(
        job.contains("BUMP_BUDGET_MINUTES"),
        "the wait must be bounded"
    );
}

/// A failed hand-off pages engineering like every other release job.
#[test]
fn a_failed_extension_dispatch_pages_engineering() {
    assert!(
        job_needs("notify-failure").iter().any(|job| job == JOB),
        "`notify-failure` must need `{JOB}`"
    );
}
