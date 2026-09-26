use std::fs;
use std::path::PathBuf;

fn ci_workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("ci.yml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn project_gate_workflow() -> serde_yaml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("project-gate.yml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source).expect("project-gate.yml parses as YAML")
}

fn project_publish_workflow() -> serde_yaml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("project-publish.yml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source).expect("project-publish.yml parses as YAML")
}

/// LAW-75: the portal manifest has a fixed path, so workflows use it directly
/// and skip setup when that package file is absent.
#[test]
fn project_workflows_use_the_fixed_portal_manifest() {
    for workflow in [project_gate_workflow(), project_publish_workflow()] {
        let source = serde_yaml::to_string(&workflow).expect("serialize workflow for assertions");
        assert!(source.contains("hashFiles('portal/package.json') != ''"));
        assert!(source.contains("package_json_file: portal/package.json"));
        assert!(!source.contains("navigator project applications --manifest"));
        assert!(!source.contains("id: application"));
    }
}

/// Every job installs the CLI through the shared composite action
/// (self-referenced with a pinned tag, since a step's `uses` key takes no
/// expression) rather than an inline download block, and threads
/// `read-manifest`'s resolved tag into it via `with: version:`, which `uses`
/// cannot carry but a step's `with:` block can.
#[test]
fn the_project_gate_verify_job_installs_the_cli_through_the_composite_action() {
    let workflow = project_gate_workflow();
    let jobs = workflow["jobs"].as_mapping().expect("project gate jobs");
    let verify_steps = jobs[&serde_yaml::Value::String("verify".to_string())]["steps"]
        .as_sequence()
        .expect("verify steps");

    assert!(
        verify_steps.iter().any(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("navigator project gate --ci"))
        }),
        "verify must run the installed CLI directly"
    );
    let install_step = verify_steps.iter().find(|step| {
        step["uses"]
            .as_str()
            .is_some_and(|uses| uses.contains("/.github/actions/navigator-install@"))
    });
    assert!(
        install_step.is_some(),
        "verify must install the CLI through the shared composite action"
    );
    assert_eq!(
        install_step.unwrap()["with"]["version"].as_str(),
        Some("${{ needs.read-manifest.outputs.version }}"),
        "the composite action must receive the tag read-manifest already resolved and validated"
    );
}

/// A step's `uses` key is one of the two workflow keys that accept no context
/// expression, so an interpolated ref is not resolved — it is looked up
/// verbatim and the run fails. Nothing else catches this: the gate is a
/// `workflow_call` workflow Navigator's own CI never invokes.
#[test]
fn no_workflow_or_action_interpolates_a_uses_reference() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github");
    let mut checked = 0;
    let mut pending = vec![root];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("yml" | "yaml")
            ) {
                continue;
            }
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            checked += 1;
            for (number, line) in source.lines().enumerate() {
                let trimmed = line.trim_start();
                let Some(reference) = trimmed
                    .strip_prefix("- uses:")
                    .or_else(|| trimmed.strip_prefix("uses:"))
                else {
                    continue;
                };
                assert!(
                    !reference.contains("${{"),
                    "{}:{} interpolates a `uses` reference, which GitHub does not resolve: {trimmed}",
                    path.display(),
                    number + 1
                );
            }
        }
    }
    assert!(
        checked > 0,
        "expected to inspect at least one workflow file"
    );
}

/// The Windows CLI+LSP check moved to `deploy.yml` (see
/// `deploy_workflow::windows_cli_and_lsp_check_runs_alongside_integration`),
/// so it no longer bills a Windows runner on every PR touching `cli/` or
/// `lsp/`. Nothing in `ci.yml` should still classify or run it.
#[test]
fn windows_cli_and_lsp_check_no_longer_lives_in_ci_yml() {
    let source = ci_workflow();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&source).expect("ci.yml parses as YAML");

    assert!(
        workflow["jobs"]["windows-cli-lsp"].is_null(),
        "the Windows CLI+LSP job must live in deploy.yml, not ci.yml"
    );
    assert!(
        workflow["jobs"]["changes"]["outputs"]["windows"].is_null(),
        "ci.yml's changes job must no longer classify a Windows scope"
    );
    assert!(
        !source.contains("windows=true") && !source.contains("windows=false"),
        "ci.yml must not compute a Windows scope output anywhere"
    );
}

/// ENG-671 moved the version-format guard out of five repeated inline CLI
/// downloads and into one composite action, so `read-manifest`'s own guard is
/// the only one left in this file — the guard that used to need to match five
/// other copies now has nothing to drift from.
#[test]
fn read_manifest_is_the_only_remaining_version_guard() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("project-gate.yml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

    let guards: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let (_, rest) = line.split_once("=~ ")?;
            let (pattern, _) = rest.split_once(" ]]")?;
            Some((index + 1, pattern))
        })
        .collect();

    assert_eq!(
        guards.len(),
        1,
        "expected only read-manifest's guard now that the five CLI downloads install \
         through the composite action instead"
    );

    // A release candidate is the shape a Project migrating to the two thin
    // callers has to pin, because the input contract changed in one.
    let (line, pattern) = guards[0];
    let matched = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(r#"[[ "$1" =~ {pattern} ]]"#))
        .arg("bash")
        .arg("26.9.15-rc.1")
        .status()
        .expect("run bash");
    assert!(
        matched.success(),
        "line {line} refuses a release candidate the manifest read admits"
    );
}

/// The five jobs that install the CLI all reference the same
/// `navigator-install` tag. A job left pinned to a different tag than its
/// siblings would install a different composite action's behavior mid-run,
/// and because the five references are hand-maintained (a step's `uses` key
/// takes no expression), nothing else would catch that drift.
#[test]
fn every_navigator_install_reference_pins_the_same_tag() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("project-gate.yml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));

    let refs: Vec<&str> = source
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(
                "uses: neon-law-source-code/navigator/.github/actions/navigator-install@",
            )?;
            Some(rest.trim())
        })
        .collect();

    assert_eq!(
        refs.len(),
        3,
        "expected all three CLI-installing jobs (verify, documents, seeds) \
         to reference the composite action"
    );
    assert!(
        refs.iter().all(|tag| *tag == refs[0]),
        "every navigator-install reference must pin the same tag: {refs:?}"
    );
}
