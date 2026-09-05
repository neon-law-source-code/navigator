//! Pin `.github/actions/validate`'s Project-repository gate against the Rust
//! definitions it mirrors.
//!
//! The gate runs in a Project repository's CI, on a runner that has the
//! `navigator` binary but no database and no deployment configuration. Its
//! mount checks are therefore shell transcriptions of `cloud::workspace` — the
//! slug shape, the reserved Project code, and the mount the portal is served
//! at. Bash cannot call Rust, so the duplication is real and the only question
//! is whether it can drift silently. These tests are the answer: they read the
//! action off disk and fail when a Rust definition moves without its
//! transcription following.
//!
//! They deliberately assert *presence in the action's source*, not behavior. A
//! test that executed the action would need a runner, a build, and a
//! checked-out Project repository; what actually goes wrong here is a constant
//! changing in `cloud` and nobody remembering the YAML.

use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::process::Command;

use cloud::workspace::{PORTAL_MOUNT_SEGMENT, RESERVED_PROJECT_CODES, SLUG_MAX_LEN};

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

#[cfg(unix)]
fn mount_script() -> String {
    let action: serde_yaml::Value =
        serde_yaml::from_str(&action_source()).expect("action.yml parses as YAML");
    action
        .get("runs")
        .and_then(|runs| runs.get("steps"))
        .and_then(serde_yaml::Value::as_sequence)
        .expect("the composite declares steps")
        .iter()
        .find(|step| {
            step.get("name")
                .and_then(serde_yaml::Value::as_str)
                .is_some_and(|name| name.starts_with("the built"))
        })
        .and_then(|step| step.get("run"))
        .and_then(serde_yaml::Value::as_str)
        .expect("the mount step runs a script")
        .to_string()
}

#[cfg(unix)]
fn write_built_app(root: &std::path::Path, relative: &str, mount: &str) {
    let app = root.join(relative);
    fs::create_dir_all(app.join("dist/assets")).expect("create built app");
    fs::create_dir_all(app.join("src")).expect("create app source");
    fs::write(app.join("package.json"), "{}\n").expect("write package manifest");
    fs::write(
        app.join("dist/index.html"),
        format!(r#"<script src="{mount}assets/app.js"></script>"#),
    )
    .expect("write built index");
}

#[cfg(unix)]
fn run_mount_script(root: &std::path::Path) -> std::process::Output {
    let script = root.join("mount.sh");
    fs::write(&script, mount_script()).expect("write mount script");
    Command::new("bash")
        .arg(script)
        .current_dir(root)
        .env("DIR", ".")
        .env("PROJECT_REPOSITORY", "true")
        .env("REPOSITORY", "sample-project")
        .env("PORTAL_DIST", "dist")
        .output()
        .expect("run mount script")
}

/// One gate, one action. The Project-repository half and the generic content
/// lint are branches of the same composite, so every Project repository in
/// every organization consumes one `uses:` line.
#[test]
fn one_action_carries_both_halves_of_a_project_repositorys_gate() {
    let source = action_source();
    assert!(
        source.contains("navigator site projects repository validate"),
        "the action must run the Project repository validator",
    );
    assert!(
        source.contains("project_repository:"),
        "the action must expose the Project-repository input",
    );
    assert!(
        !workspace_root()
            .join(".github/actions/application-gate")
            .exists(),
        "the separate application gate is retired; one repository has one gate",
    );
}

/// The slug cap reaches the action as a literal.
///
/// `SLUG_MAX_LEN` appears inside the slug regex as the 78-character middle: a
/// leading and a trailing alphanumeric make up the other two.
#[test]
fn the_action_carries_the_slug_length_cap() {
    let source = action_source();
    let middle = SLUG_MAX_LEN - 2;
    assert!(
        source.contains(&format!("[a-z0-9-]{{0,{middle}}}")),
        "the action's slug regex does not encode SLUG_MAX_LEN ({SLUG_MAX_LEN})",
    );
}

/// Every reserved Project code is refused by the action, and the action refuses
/// nothing else.
///
/// Both directions matter. A code added to `RESERVED_PROJECT_CODES` and not to
/// the action would pass CI and then collide with a Navigator route; a code left
/// in the action after Rust released it would refuse a legitimate Project with
/// no rule behind the refusal.
#[test]
fn the_action_refuses_exactly_the_reserved_project_codes() {
    let source = action_source();
    let refused: Vec<&str> = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("if [ \"${code}\" = \"")
                .and_then(|rest| rest.split('"').next())
        })
        .collect();

    let mut expected: Vec<&str> = RESERVED_PROJECT_CODES.to_vec();
    expected.sort_unstable();
    let mut actual = refused;
    actual.sort_unstable();

    assert_eq!(
        actual, expected,
        "the action's reserved-code refusals and cloud::workspace::RESERVED_PROJECT_CODES disagree",
    );
}

/// Nothing composes a repository name, and nothing splits one.
///
/// The whole class of ambiguity the old `<code>-<app>` composition carried is
/// gone because the repository name *is* the code. A shell `${name%%-*}` here
/// would be reintroducing a parse of a name that has nothing to parse.
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
    assert!(
        source.contains("code=\"${REPOSITORY}\""),
        "the Project code is the repository name, taken directly",
    );
}

/// The mount the action verifies is the one Navigator serves, literal segment
/// and trailing slash included.
///
/// Vite joins asset URLs directly onto the base, so a missing slash emits
/// `/app/projects/<code>/portalassets/…`; Navigator redirects the bare mount to
/// the slashed form, so the slashed spelling is where a browser ends up
/// regardless.
#[test]
fn the_action_derives_each_application_mount_without_an_apps_url_segment() {
    let source = action_source();
    let expected = "base=\"/app/projects/${code}/${app}/\"";
    assert!(
        source.contains(expected),
        "the action does not compose the mount Navigator serves: expected {expected}",
    );
    assert!(
        !source.contains("/app/projects/${code}/apps/${app}/"),
        "`apps/` groups source in the repository and must not become a product route",
    );
    // The legacy portal remains the generic rule with `app=portal`.
    assert_eq!(
        cloud::workspace::WorkspaceConfig::portal_mount("kizuna"),
        format!("/app/projects/kizuna/{PORTAL_MOUNT_SEGMENT}/"),
    );
}

/// The real mount shell must inspect every direct app. A source-presence test
/// cannot prove the loop continues past the first package manifest, so this
/// executes the composite step with one correct app and one mismounted app.
#[cfg(unix)]
#[test]
fn the_mount_gate_fails_when_any_discovered_app_is_mismounted() {
    let tmp = tempfile::tempdir().expect("fixture checkout");
    write_built_app(
        tmp.path(),
        "apps/portal",
        "/app/projects/sample-project/portal/",
    );
    write_built_app(
        tmp.path(),
        "apps/exchange",
        "/app/projects/sample-project/wrong/",
    );

    let output = run_mount_script(tmp.path());
    assert!(
        !output.status.success(),
        "a mismounted second app was silently skipped: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let message = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("apps/exchange"), "{message}");
    assert!(message.contains("is not mounted at"), "{message}");
}

#[cfg(unix)]
#[test]
fn the_mount_gate_rejects_two_portal_workspaces_for_one_route() {
    let tmp = tempfile::tempdir().expect("fixture checkout");
    write_built_app(tmp.path(), "portal", "/app/projects/sample-project/portal/");
    write_built_app(
        tmp.path(),
        "apps/portal",
        "/app/projects/sample-project/portal/",
    );

    let output = run_mount_script(tmp.path());
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("claim the same application route"),
        "{message}"
    );
}

/// Neither retired manifest is read.
///
/// `mount.json` and `navigator.toml` declared a repository's own coordinates and
/// every reader of them is gone. A `jq` read of one here would resurrect a
/// coordinate the repository name already carries.
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

/// The gate carries no organization allowlist.
///
/// The organization is configuration, so a list of them in an action consumed
/// by every Project repository could only go stale — and an organization the
/// action wrongly accepted would be a repository served by the wrong
/// deployment.
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

/// The gate is never satisfied by being skipped.
///
/// A path-filtered job that skips reports success for work it never did, and a
/// required check that a skip satisfies is not a gate. So each half no-ops
/// internally and the one job always runs — asserted here as the absence of a
/// filter and the presence of the two no-op exits.
#[test]
fn each_half_no_ops_rather_than_being_filtered_out() {
    let source = action_source();
    assert!(
        !source.contains("paths:"),
        "the action must not carry a path filter",
    );
    assert!(
        source.contains("no applications in this repository — nothing to mount"),
        "the mount half must no-op over a repository with no portal",
    );
}

/// Keys in a deployment's `config.toml` whose values identify *that*
/// deployment.
///
/// `NAVIGATOR_GITHUB_ORG` is included: with the organization allowlist gone, it
/// may not appear in an action consumed identically by every Project
/// repository. Its paired `NAVIGATOR_GIT_HOST` is deliberately absent, because
/// it does not identify a deployment — every deployment the Firm operates is on
/// the same host, which is why that half of the coordinate carries a default
/// and this half does not.
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

/// The identifiers of the deployments that actually exist.
///
/// Listed, and only because there is nowhere left to read them from. This used
/// to walk `deployments/` and derive every value, which kept it correct as rows
/// were added or renamed — but the tree moved to a private repository, and a
/// gate cannot derive from a repository it cannot see.
///
/// These are public facts rather than a leak of the configuration: the hostname
/// is on the site, and both project ids are already spelled in
/// `store/src/deployment.rs` and `docs/environments.md`. What is lost is
/// automatic coverage of a NEW row, which is why the synthetic tree is scanned
/// as well — the derivation stays exercised, so this list is the only part a
/// new deployment has to be added to.
const REAL_DEPLOYMENT_IDENTIFIERS: &[(&str, &str)] = &[
    ("NAVIGATOR_PUBLIC_HOST", "www.neonlaw.com"),
    ("NAVIGATOR_WORKFLOWS_HOST", "workflows.neonlaw.com"),
    ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
    ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
];

/// Every deployment-identifying value this repository can see: the real
/// identifiers above, plus everything derived from the synthetic tree.
///
/// Both, because neither is sufficient alone. The list catches a leak of a real
/// host or project id, which is the failure that matters. The derivation keeps
/// the mechanism honest — it is what still covers a key added to
/// `DEPLOYMENT_IDENTIFYING_KEYS`, and it fails loudly if the fixture stops
/// parsing.
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

/// The action names no deployment-specific value.
///
/// It is consumed byte-identically by every Project repository in every
/// organization, which only works because the repository name and the mount do
/// not vary between deployments. A host, a bucket, a cluster, an organization,
/// or a project id appearing here would make it one deployment's gate wearing a
/// generic name.
#[test]
fn the_action_is_deployment_agnostic() {
    // The action's own address is not a deployment coordinate. Navigator is
    // published from `neon-law-source-code/navigator`, and a Project repository
    // pins the action by that slug — which became the same string as
    // `NAVIGATOR_GITHUB_ORG` when the source moved into the organization that
    // holds the Project repositories. Reading a `uses:` line as configuration
    // would fail this test on a coincidence of names rather than on a leak, so
    // the action's self-reference is removed before the check.
    let source = action_source().replace("neon-law-source-code/navigator", "<this-action>");
    for (key, value) in deployment_identifying_values() {
        assert!(
            !source.contains(&value),
            "the action carries the value of {key} from a deployment configuration; the mount and \
             repository name are identical in every deployment, and the host and organization — \
             the only things that differ — never appear in a Vite base",
        );
    }

    // Nor may it *read* one. Resolving a deployment would make the gate depend
    // on configuration a Project repository's runner does not have.
    for key in DEPLOYMENT_IDENTIFYING_KEYS {
        assert!(
            !source.contains(key),
            "the action reads {key}; it runs in a repository with no deployment configuration",
        );
    }
}
