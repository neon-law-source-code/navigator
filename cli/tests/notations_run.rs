//! Black-box contract for the ephemeral `navigator notations run` workbench.

use std::path::Path;

use assert_cmd::Command;
use predicates::str::contains;

fn without_deployment(command: &mut Command) -> &mut Command {
    // Poison the ordinary deployment selectors instead of clearing the
    // process environment. macOS needs its standard environment to launch a
    // freshly linked test binary; these deliberately unusable values prove
    // the runner does not consult a deployment or a live provider.
    command
        .env(
            "NAVIGATOR_SURREAL_ENDPOINT",
            "http://not-a-deployment.invalid",
        )
        .env("NAVIGATOR_SURREAL_NAMESPACE", "must-not-be-read")
        .env("NAVIGATOR_SURREAL_DATABASE", "must-not-be-read")
        .env("RESTATE_BROKER_URL", "http://not-a-provider.invalid")
}

fn run(file: &Path) -> String {
    let mut command = Command::cargo_bin("navigator").expect("navigator binary");
    let output = without_deployment(&mut command)
        .args(["notations", "run"])
        .arg(file)
        .output()
        .expect("run notation workbench");
    assert!(
        output.status.success(),
        "notations run failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("workbench output is utf-8")
}

fn notation_id(output: &str) -> &str {
    output
        .lines()
        .find_map(|line| line.strip_prefix("notation "))
        .expect("runner reports its notation id")
}

#[test]
fn run_walks_a_bundled_template_without_deployment_configuration_and_isolated_runs() {
    let template = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../templates/notations/neon_law/shared/onboarding_letter.md");

    let first = run(&template);
    let second = run(&template);

    for output in [&first, &second] {
        assert!(
            output.contains("client subset complete: 1 answer(s)"),
            "{output}"
        );
        assert!(
            output.contains("firm questionnaire complete: 8 answer(s)"),
            "{output}"
        );
        assert!(
            output.contains("persisted 9 answer(s); workflow state lawyer_review"),
            "{output}"
        );
        // `InMemoryRuntime` keeps its transition history process-local; the
        // runner journals that same history through `store::notation_events`
        // (the table a real `notations run` leaves rows in) and reports the
        // count, so a broken journal write fails this run rather than
        // silently keeping the transcript in memory only.
        assert!(
            output.contains("journaled 11 notation_events row(s)"),
            "{output}"
        );
        assert!(
            output.contains("questionnaire custom_single_choice__governing_law --_--> END"),
            "{output}"
        );
        assert!(
            output.contains("workflow BEGIN --intake_submitted--> intake_persisted__client"),
            "{output}"
        );
        assert!(
            output
                .contains("workflow intake_persisted__client --retainer_rendered--> lawyer_review"),
            "{output}"
        );
    }
    assert_ne!(
        notation_id(&first),
        notation_id(&second),
        "a new embedded store and synthetic notation must be used per run"
    );
}

#[test]
fn run_explains_a_malformed_template() {
    let directory = tempfile::tempdir().expect("temporary malformed template directory");
    let malformed = directory.path().join("broken.md");
    std::fs::write(&malformed, "not a notation template\n").expect("write malformed template");

    let mut command = Command::cargo_bin("navigator").expect("navigator binary");
    without_deployment(&mut command)
        .args(["notations", "run"])
        .arg(malformed)
        .assert()
        .failure()
        .stderr(contains("template has no YAML frontmatter"));
}
