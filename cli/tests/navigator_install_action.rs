//! ENG-671: `.github/actions/navigator-install` is the one place every job
//! that installs the navigator CLI now goes through, replacing the download
//! block `project-gate.yml` used to carry five times. These tests pin its
//! contract the same way `project_gate.rs` pins `.github/actions/gate`'s:
//! reading the composite action's source rather than driving a GitHub Actions
//! runner.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

fn action_source() -> String {
    let path = workspace_root().join(".github/actions/navigator-install/action.yml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn install_script_source() -> String {
    let path = workspace_root().join(".github/actions/navigator-install/install.sh");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_action_parses_as_yaml_and_declares_version_and_token_inputs() {
    let source = action_source();
    let action: serde_yaml::Value = serde_yaml::from_str(&source).expect("action.yml parses");
    assert_eq!(
        action["inputs"]["version"]["required"].as_bool(),
        Some(false),
        "version must be optional — it defaults to navigator.yaml's own version",
    );
    assert_eq!(
        action["inputs"]["token"]["default"].as_str(),
        Some("${{ github.token }}"),
    );
}

#[test]
fn the_action_refuses_a_non_exact_tag_with_the_existing_message() {
    let source = action_source();
    assert!(
        source.contains("version must be an exact release tag, not '${version}'."),
        "the action must keep the exact refusal message the inline blocks it replaces used",
    );
    for rolling in ["latest", "main", "HEAD"] {
        assert!(
            source.contains(rolling),
            "the action must still refuse '{rolling}'",
        );
    }
}

#[test]
fn the_action_resolves_the_version_from_navigator_yaml_when_no_input_is_given() {
    let source = action_source();
    assert!(
        source.contains("navigator.yaml"),
        "the action must fall back to navigator.yaml's version: field",
    );
}

#[test]
fn the_action_verifies_a_published_checksum_before_extracting() {
    let source = action_source();
    assert!(
        source.contains(".sha256"),
        "the action must download and check a sha256 sidecar",
    );
    assert!(
        source.contains("sha256sum") && source.contains("shasum"),
        "the action must verify on both the Linux (sha256sum) and macOS (shasum) runners it installs on",
    );
}

#[test]
fn the_action_caches_the_installed_binary_by_tag() {
    let source = action_source();
    assert!(
        source.contains("actions/cache@"),
        "the action must cache the download so five jobs in one run do not repeat it",
    );
    assert!(
        source.contains("steps.resolve.outputs.version"),
        "the cache key must be keyed on the resolved, exact tag",
    );
}

#[test]
fn the_action_appends_the_installed_cli_to_path() {
    let source = action_source();
    assert!(
        source.contains(r#"echo "${RUNNER_TEMP}/navigator-bin" >> "${GITHUB_PATH}""#),
        "the action must put the installed binary on PATH the same way the inline blocks did",
    );
}

#[test]
fn install_sh_refuses_a_non_exact_tag_with_the_same_message() {
    let source = install_script_source();
    assert!(
        source.contains("version must be an exact release tag, not '${version}'."),
        "install.sh must refuse a rolling pointer with the same wording as the composite action",
    );
}

#[test]
fn install_sh_installs_without_sudo_by_default() {
    let source = install_script_source();
    assert!(
        !source
            .lines()
            .any(|line| line.trim_start().starts_with("sudo ")),
        "install.sh must never invoke sudo — it installs to a user-writable directory",
    );
    assert!(
        source.contains(".local/bin"),
        "install.sh's default install directory must not need elevated permissions",
    );
}

#[test]
fn install_sh_depends_on_nothing_beyond_a_bare_linux_container() {
    let source = install_script_source();
    // No `gh` CLI: a bare container has no GitHub token to authenticate it
    // with, so the script must reach the public release over a plain
    // download instead.
    assert!(
        !source.contains("gh release"),
        "install.sh must not shell out to the gh CLI",
    );
    assert!(
        source.contains("curl") && source.contains("tar"),
        "install.sh must use curl and tar, the two tools its own contract names",
    );
}
