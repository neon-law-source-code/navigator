//! Guard the identity that arms auto-merge.
//!
//! `ci.yml`'s `enable-automerge` job turns on GitHub auto-merge for a green
//! pull request. WHICH CREDENTIAL IT USES IS NOT COSMETIC: GitHub's
//! workflow-recursion guard creates no workflow runs for a push attributed to
//! `GITHUB_TOKEN`, and in this repository a landed version bump in
//! `[workspace.package].version` IS the publish. A merge armed under
//! `GITHUB_TOKEN` therefore lands on `main` and starts no `deploy` run at all —
//! no release is built, no tag is created, and nothing anywhere goes red.
//!
//! That is not hypothetical. Five merges to `main` landed under `GITHUB_TOKEN`,
//! fired no `deploy` run between them, and stranded a release that had to be
//! recovered by hand. The arming step read
//!
//! ```text
//! GH_TOKEN: ${{ steps.app-token.outputs.token || github.token }}
//! ```
//!
//! with the mint step above it gated on `AUTOMERGE_APP_ID` being present, so an
//! ABSENT secret skipped the mint and silently fell through to the losing
//! identity. Fail-open, and invisible.
//!
//! The absence of that fallback is the whole assertion. A missing App skips
//! arming and leaves the pull request visibly waiting for a human; a present but
//! broken App fails the mint and arms nothing. Both are fail-closed, and neither
//! can lose a release quietly.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root is cli/'s parent")
        .to_path_buf()
}

/// Deliberately `ci.yml`. `cli/tests/deploy_workflow.rs` reads `deploy.yml`,
/// which does not carry the arming step — a guard pointed at the wrong file
/// would pass vacuously forever.
fn ci_workflow() -> String {
    let path = repo_root().join(".github/workflows/ci.yml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn ci_yaml() -> serde_yaml::Value {
    serde_yaml::from_str(&ci_workflow()).expect("ci.yml parses as YAML")
}

/// The `enable-automerge` job's CONFIGURATION, re-serialised from the parsed
/// YAML so comments are not part of the text searched. The prose in this file
/// necessarily names the credential it forbids; the job itself must not carry
/// it anywhere — an `env:`, a `with:`, or an inline expression in a `run:`.
fn arming_job_config() -> String {
    let workflow = ci_yaml();
    serde_yaml::to_string(&workflow["jobs"]["enable-automerge"])
        .expect("the enable-automerge job re-serialises")
}

/// The arming job must not be able to reach `GITHUB_TOKEN`.
///
/// Asserted over the whole job rather than one line: `github.token` and
/// `secrets.GITHUB_TOKEN` are the same credential under two spellings, and a
/// fallback reintroduced with either would lose the next release the same way.
#[test]
fn auto_merge_never_falls_back_to_the_workflow_token() {
    let job = arming_job_config();

    for spelling in ["github.token", "secrets.GITHUB_TOKEN"] {
        assert!(
            !job.contains(spelling),
            "`enable-automerge` must not hand `{spelling}` to any step: GitHub creates no \
             workflow runs for a push attributed to it, and a landed version bump is this \
             repository's publish, so a merge armed under it publishes nothing and reports \
             nothing. Arm with the `navigator-merge-queue` App or arm nothing"
        );
    }

    // The exact shape that caused the incident, named so a reader of a failure
    // knows precisely what not to reintroduce.
    assert!(
        !job.contains("steps.app-token.outputs.token || "),
        "the `|| github.token` fallback is what made a missing App secret arm auto-merge under \
         the losing identity instead of arming nothing"
    );
}

/// Both halves of the arming path are gated on the App being configured.
///
/// The mint step alone is not enough: gating only the mint is exactly the
/// fail-open shape above, where the next step runs anyway with whatever token it
/// can find. The step that calls the GraphQL mutation has to be gated too, so an
/// unconfigured installation runs nothing rather than running with an empty
/// credential.
#[test]
fn both_arming_steps_are_gated_on_the_app_being_configured() {
    let workflow = ci_yaml();
    let steps = workflow["jobs"]["enable-automerge"]["steps"]
        .as_sequence()
        .expect("enable-automerge declares steps");

    for name in ["mint app token", "request auto-merge"] {
        let step = steps
            .iter()
            .find(|step| step["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("enable-automerge must keep its `{name}` step"));
        assert_eq!(
            step["if"].as_str(),
            Some("env.AUTOMERGE_APP_ID != ''"),
            "`{name}` must be gated on the App being configured, so an installation without \
             the App arms nothing rather than arming under the wrong identity"
        );
    }

    // The gate reads `env`, not `secrets`: the `secrets` context is unavailable
    // in a step `if`, so hoisting the id to job-level env is what makes the
    // gate evaluable at all. A gate that silently never fires is worse than none.
    assert_eq!(
        workflow["jobs"]["enable-automerge"]["env"]["AUTOMERGE_APP_ID"].as_str(),
        Some("${{ secrets.AUTOMERGE_APP_ID }}"),
        "the App id must be hoisted to job-level env; the `secrets` context cannot be read \
         from a step `if`, so a gate written against it would never fire"
    );
}

/// The token the mutation runs under is the App's, and only the App's.
#[test]
fn the_arming_step_runs_under_the_minted_app_token() {
    let workflow = ci_yaml();
    let steps = workflow["jobs"]["enable-automerge"]["steps"]
        .as_sequence()
        .expect("enable-automerge declares steps");
    let request = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("request auto-merge"))
        .expect("enable-automerge must keep its `request auto-merge` step");

    assert_eq!(
        request["env"]["GH_TOKEN"].as_str(),
        Some("${{ steps.app-token.outputs.token }}"),
        "auto-merge must be armed with the minted App installation token, with no alternative"
    );
}

/// `deploy.yml` is not where this lives, and saying so keeps the next guard
/// from being written against a file that cannot fail it.
#[test]
fn the_arming_step_lives_in_ci_not_deploy() {
    let deploy = repo_root().join(".github/workflows/deploy.yml");
    let deploy = std::fs::read_to_string(&deploy).expect("read deploy.yml");

    assert!(
        !deploy.contains("enablePullRequestAutoMerge"),
        "auto-merge is armed by `ci.yml`; a guard reading `deploy.yml` would pass vacuously"
    );
    assert!(
        ci_workflow().contains("enablePullRequestAutoMerge"),
        "ci.yml must keep the mutation this guard is written against"
    );
}

/// Path hygiene, so the two helpers above cannot silently read nothing.
#[test]
fn the_guarded_workflow_exists() {
    assert!(
        Path::new(&repo_root().join(".github/workflows/ci.yml")).is_file(),
        "ci.yml must exist at the path this guard reads"
    );
    assert!(
        ci_workflow().contains("enable-automerge:"),
        "ci.yml must carry the `enable-automerge` job"
    );
}

#[test]
fn project_gate_arms_auto_merge_without_the_workflow_token() {
    let path = repo_root().join(".github/workflows/project-gate.yml");
    let source = std::fs::read_to_string(&path).expect("read project-gate.yml");
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&source).expect("project-gate.yml parses");
    let job = serde_yaml::to_string(&workflow["jobs"]["enable-automerge"])
        .expect("enable-automerge re-serialises");
    for spelling in ["github.token", "secrets.GITHUB_TOKEN"] {
        assert!(
            !job.contains(spelling),
            "project-gate.yml must not hand `{spelling}` to enable-automerge"
        );
    }
    assert!(
        source.contains("navigator site projects gate --ci"),
        "project-gate.yml must run the live-status door"
    );
}
