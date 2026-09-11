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
            .any(|step| step["id"].as_str() == Some("pnpm-manifest"));
        for (index, step) in steps.iter().enumerate() {
            if step["uses"].as_str()
                != Some("pnpm/action-setup@0977fd99725f1db4007ccb2928dbb4e90d06cc86")
            {
                continue;
            }
            pnpm_setup_steps += 1;
            // `portal/` is only one of the three layouts `application_workspaces`
            // admits; `apps/portal/` and a root Vite workspace are equally valid,
            // so the path is resolved on the runner rather than written here.
            assert_eq!(
                step["with"]["package_json_file"].as_str(),
                Some("${{ steps.pnpm-manifest.outputs.manifest }}"),
                "{job_name:?} step {index} must read the manifest the locator found"
            );
            assert!(
                locates_manifest,
                "{job_name:?} must locate the manifest before pnpm setup"
            );
        }
    }
    assert_eq!(
        pnpm_setup_steps, 2,
        "the gate must keep both pnpm setup steps pinned"
    );
}

#[test]
fn the_project_gate_notation_job_runs_the_pinned_cli_directly() {
    let workflow = project_gate_workflow();
    let jobs = workflow["jobs"].as_mapping().expect("project gate jobs");
    let notation_steps = jobs[&serde_yaml::Value::String("notation".to_string())]["steps"]
        .as_sequence()
        .expect("notation steps");

    assert!(
        notation_steps.iter().any(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("navigator validate ."))
        }),
        "notation must run the downloaded CLI directly"
    );
    // The composite action cannot be reached from here: a step's `uses` key
    // takes no expression, so it could not carry `inputs.version`, and this
    // shared workflow has no literal tag to pin that would track the caller's.
    assert!(
        notation_steps.iter().all(|step| {
            !step["uses"]
                .as_str()
                .is_some_and(|uses| uses.contains("/.github/actions/validate@"))
        }),
        "notation must not reference the validate composite action"
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
