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

/// ENG-674: `verify` (the only remaining job that sets up pnpm, now that
/// `lint`'s duplicate application-linting has folded into it) reads the
/// manifest path from `navigator project applications --manifest`
/// rather than hard-coding one of the three layouts `application_workspaces`
/// admits.
#[test]
fn the_project_gate_derives_the_pnpm_manifest_rather_than_hard_coding_one() {
    let workflow = project_gate_workflow();
    let jobs = workflow["jobs"].as_mapping().expect("project gate jobs");
    let mut pnpm_setup_steps = 0;

    for (job_name, job) in jobs {
        let Some(steps) = job["steps"].as_sequence() else {
            continue;
        };
        let locates_manifest = steps
            .iter()
            .any(|step| step["id"].as_str() == Some("application"));
        for (index, step) in steps.iter().enumerate() {
            if step["uses"].as_str()
                != Some("pnpm/action-setup@ea17c68df8912ef543352723c149a84f56e3d413")
            {
                continue;
            }
            pnpm_setup_steps += 1;
            assert_eq!(
                step["with"]["package_json_file"].as_str(),
                Some("${{ steps.application.outputs.manifest }}"),
                "{job_name:?} step {index} must read the manifest the CLI located"
            );
            assert!(
                locates_manifest,
                "{job_name:?} must locate the manifest before pnpm setup"
            );
        }
    }
    assert_eq!(
        pnpm_setup_steps, 1,
        "verify must keep its one pnpm setup step pinned, now that lint has folded into it"
    );
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

#[test]
fn windows_cli_and_lsp_check_is_path_scoped_and_optional() {
    let source = ci_workflow();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&source).expect("ci.yml parses as YAML");

    let changes = &workflow["jobs"]["changes"];
    assert_eq!(
        changes["outputs"]["windows"].as_str(),
        Some("${{ steps.scope.outputs.windows }}")
    );
    let scope = changes["steps"]
        .as_sequence()
        .expect("changes must declare steps")
        .iter()
        .find(|step| step["id"].as_str() == Some("scope"))
        .and_then(|step| step["run"].as_str())
        .expect("the changes scope step must have a run script");
    assert!(
        scope.contains("^(cli/|lsp/|\\.github/workflows/ci\\.yml$)"),
        "Windows scope must cover only cli/, lsp/, and ci.yml"
    );
    assert!(
        scope.contains("windows=true") && scope.contains("windows=false"),
        "the changes scope must publish a Windows output on both paths"
    );

    let windows = &workflow["jobs"]["windows-cli-lsp"];
    assert_eq!(windows["runs-on"].as_str(), Some("windows-latest"));
    assert_eq!(
        windows["if"].as_str(),
        Some("needs.changes.outputs.windows == 'true'")
    );
    assert_eq!(
        windows["needs"].as_sequence().map(Vec::len),
        Some(1),
        "the Windows check must depend only on change classification"
    );
    assert_eq!(windows["needs"][0].as_str(), Some("changes"));

    let steps = windows["steps"]
        .as_sequence()
        .expect("windows-cli-lsp must declare steps");
    assert!(
        steps
            .iter()
            .any(|step| step["run"].as_str() == Some("cargo check --locked -p cli -p lsp")),
        "Windows must check both cli and lsp with --locked"
    );
    assert!(
        steps.iter().any(|step| {
            step["uses"].as_str() == Some("dtolnay/rust-toolchain@stable")
                && step["with"]["toolchain"].as_str() == Some("1.98.0")
        }),
        "Windows must use the pinned workspace toolchain"
    );
    assert!(
        steps.iter().any(|step| {
            step["uses"].as_str() == Some("Swatinem/rust-cache@v2")
                && step["with"]["shared-key"].as_str() == Some("windows-cli-lsp")
        }),
        "Windows must cache its Rust dependencies"
    );

    let required_needs = workflow["jobs"]["ci"]["needs"]
        .as_sequence()
        .expect("ci must declare its required jobs");
    assert!(
        !required_needs
            .iter()
            .any(|need| need.as_str() == Some("windows-cli-lsp")),
        "the Windows check must remain optional until it has been observed green"
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
