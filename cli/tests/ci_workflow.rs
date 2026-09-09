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
