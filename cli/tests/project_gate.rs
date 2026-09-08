//! Pin `.github/actions/validate` as the one CLI download plus `navigator validate`.
//!
//! Project layout, mount, and origin live in that command when the tree
//! declares `navigator.yaml`. The composite must not grow a second verb or a
//! `project_repository` input.

use std::fs;
use std::path::PathBuf;

/// The workspace root (`CARGO_MANIFEST_DIR` points at `cli/`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

fn action_source() -> String {
    let path = workspace_root().join(".github/actions/validate/action.yml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_action_runs_navigator_validate_and_nothing_else() {
    let source = action_source();
    assert!(
        source.contains("navigator validate \"${DIR}\""),
        "the action must run navigator validate",
    );
    assert!(
        !source.contains("project_repository"),
        "the action must not expose a Project-repository input; navigator.yaml is the switch",
    );
    assert!(
        !source.contains("site projects repository validate"),
        "the retired repository-validate verb must not remain in the action",
    );
    assert!(
        !workspace_root()
            .join(".github/actions/application-gate")
            .exists(),
        "the separate application gate is retired; one repository has one gate",
    );
}

#[test]
fn the_action_neither_composes_nor_splits_a_repository_name() {
    let source = action_source();
    for split in [
        "REPOSITORY%%-",
        "REPOSITORY#*-",
        "REPOSITORY##*-",
        "REPOSITORY%-",
        "code%%-",
        "code#*-",
    ] {
        assert!(
            !source.contains(split),
            "the action splits the repository name with `{split}`; the name is the code",
        );
    }
    assert!(
        !source.contains("${code}-${app}"),
        "nothing composes a second identifier into the repository name",
    );
}

/// Neither retired manifest is read.
#[test]
fn the_action_reads_no_manifest() {
    let source = action_source();
    for retired in ["mount.json", "navigator.toml", "projectCode", "jq -er"] {
        assert!(
            !source.contains(retired),
            "the action still reads the retired `{retired}`",
        );
    }
}

#[test]
fn the_action_carries_no_organization_allowlist() {
    let source = action_source();
    assert!(
        !source.contains("repository_owner"),
        "the action reads the owning organization; the organization is configuration",
    );
    assert!(
        !source.contains("is not a Project application organization"),
        "the organization allowlist cannot survive a configurable organization",
    );
}

#[test]
fn each_half_no_ops_rather_than_being_filtered_out() {
    let source = action_source();
    assert!(
        !source.contains("paths:"),
        "the action must not carry a path filter",
    );
}

const DEPLOYMENT_IDENTIFYING_KEYS: &[&str] = &[
    "NAVIGATOR_PUBLIC_HOST",
    "NAVIGATOR_PRIMARY_DOMAIN",
    "CANONICAL_HOST",
    "NAV_BASE_URL",
    "NAVIGATOR_GCP_PROJECT_ID",
    "NAVIGATOR_GKE_CLUSTER_NAME",
    "NAVIGATOR_K8S_NAMESPACE",
    "NAVIGATOR_ASSETS_BUCKET",
    "NAVIGATOR_GITHUB_ORG",
];

const REAL_DEPLOYMENT_IDENTIFIERS: &[(&str, &str)] = &[
    ("NAVIGATOR_PUBLIC_HOST", "www.neonlaw.com"),
    ("NAVIGATOR_WORKFLOWS_HOST", "workflows.neonlaw.com"),
    ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
    ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
];

fn deployment_identifying_values() -> Vec<(String, String)> {
    let deployments = workspace_root()
        .join("cli/tests/fixtures/deployment-tree")
        .join("deployments");
    let mut values: Vec<(String, String)> = REAL_DEPLOYMENT_IDENTIFIERS
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    for entry in fs::read_dir(&deployments).expect("the fixture deployment tree exists") {
        let config = entry
            .expect("a readable deployments/ entry")
            .path()
            .join("config.toml");
        let Ok(body) = fs::read_to_string(&config) else {
            continue;
        };
        for line in body.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if !DEPLOYMENT_IDENTIFYING_KEYS.contains(&key) {
                continue;
            }
            let value = value.trim().trim_matches('"').to_string();
            if !value.is_empty() {
                values.push((key.to_string(), value));
            }
        }
    }
    assert!(
        !values.is_empty(),
        "no deployment-identifying values were read; this test would pass vacuously",
    );
    values
}

#[test]
fn the_action_is_deployment_agnostic() {
    let source = action_source().replace("neon-law-source-code/navigator", "<this-action>");
    for (key, value) in deployment_identifying_values() {
        assert!(
            !source.contains(&value),
            "the action carries the value of {key} from a deployment configuration; the mount and \
             repository name are identical in every deployment, and the host and organization — \
             the only things that differ — never appear in a Vite base",
        );
    }

    for key in DEPLOYMENT_IDENTIFYING_KEYS {
        assert!(
            !source.contains(key),
            "the action reads {key}; it runs in a repository with no deployment configuration",
        );
    }
}
