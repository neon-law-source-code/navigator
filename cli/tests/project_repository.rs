//! End-to-end tests for `navigator project gate` against a Project repository.
//!
//! One repository per Project code, holding notation templates under
//! `templates/` and application source under `apps/<app>/`. There is one gate
//! for both. A legacy root `portal/` remains valid during the source-layout
//! transition.
//!
//! Nothing generates this shell anymore — `navigator project repository
//! scaffold` and `sync-skills` are retired; a real repository builds it by
//! hand (see `docs/project-repositories.md#a-repositorys-fixed-shell`). These
//! tests build the same shell directly with [`write_repository_shell`] so the
//! gate itself stays covered end to end.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::{prelude::PredicateBooleanExt, str};
use tempfile::TempDir;

fn navigator() -> Command {
    let mut command = Command::cargo_bin("navigator").unwrap();
    command.env_remove("GITHUB_REPOSITORY");
    command
}

/// The pin these fixtures carry.
const FIXTURE_PIN: &str = "26.8.23";

const CODEOWNERS: &str = "# CODEOWNERS\n\n* @shicholas\n";
const GITATTRIBUTES: &str = "* text=auto eol=lf\n";
const DOCUMENTS_GITIGNORE: &str = "*\n!*/\n!*.yaml\n!.gitignore\n";

/// The one `AGENTS.md` every Project repository carries — read from the same
/// source `cli::projects::repository::AGENT_CONTRACT_BASE` embeds, so the
/// fixture and the gate's own canonical copy can never drift apart.
const AGENT_CONTRACT: &str = include_str!("../src/projects/agent_contract.md");

/// Byte-exact copy of `AUTOMERGE_WORKFLOW_CONTENTS` in
/// `cli/src/projects/repository.rs` — duplicated rather than imported, the
/// same way the rest of this file's fixed strings do, since integration
/// tests cannot reach a `pub(crate)` item.
const AUTOMERGE_WORKFLOW_CONTENTS: &str = r#"name: automerge

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]

permissions:
  contents: read

jobs:
  enable-automerge:
    if: github.event.pull_request.draft == false
    runs-on: ubuntu-latest
    steps:
      - name: Look for the merge-queue App credentials
        id: credentials
        env:
          APP_ID: ${{ secrets.AUTOMERGE_APP_ID }}
          APP_PRIVATE_KEY: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}
        shell: bash
        run: |
          set -euo pipefail
          if [ -n "${APP_ID}" ] && [ -n "${APP_PRIVATE_KEY}" ]; then
              echo "present=true" >> "${GITHUB_OUTPUT}"
          else
              echo "present=false" >> "${GITHUB_OUTPUT}"
          fi
      - name: Mint a merge-queue App token
        id: app-token
        if: steps.credentials.outputs.present == 'true'
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1 # v3.2.0
        with:
          app-id: ${{ secrets.AUTOMERGE_APP_ID }}
          private-key: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}
      - name: Arm auto-merge
        env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}
          PR_URL: ${{ github.event.pull_request.html_url }}
        shell: bash
        run: |
          set -euo pipefail
          if [ -z "${GH_TOKEN:-}" ]; then
              echo "::notice::merge-queue App credentials absent — arming nothing, merge by hand"
              exit 0
          fi
          gh pr merge --squash --auto "${PR_URL}"
"#;

/// The synced skill catalog every Project repository carries, named for the
/// canonical copies under Navigator's own `.agents/skills/` (see
/// `SYNCED_SKILLS` in `cli/src/projects/repository.rs`).
const SYNCED_SKILL_NAMES: &[&str] = &[
    "council",
    "legal-council",
    "client-council",
    "human-readable",
    "stay-in-repo",
    "portal-chrome",
    "server",
    "legal-writing",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

fn workflow(action_version: &str) -> String {
    format!(
        r"name: ci

on:
  pull_request:

permissions:
  contents: read
  id-token: write

jobs:
  ci:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@{action_version}
"
    )
}

fn cd_workflow(action_version: &str) -> String {
    format!(
        r"name: cd

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  id-token: write

jobs:
  gate:
    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@{action_version}
  publish:
    needs: gate
    uses: neon-law-source-code/navigator/.github/workflows/project-publish.yml@{action_version}
",
    )
}

fn placeholder_template() -> String {
    [
        "---\n",
        "kind: onboarding\n",
        "title: Onboarding letter\n",
        "respondent_type: entity\n",
        "code: onboarding\n",
        "jurisdiction: NV\n",
        "confidential: true\n",
        "questionnaire:\n",
        "  BEGIN:\n",
        "    _: END\n",
        "  END: {}\n",
        "workflow:\n",
        "  BEGIN:\n",
        "    intake_submitted: lawyer_review\n",
        "  lawyer_review:\n",
        "    approved: END\n",
        "    rejected: END\n",
        "  END: {}\n",
        "---\n",
        "\n",
        "Replace this placeholder with the notation this Project actually uses.\n",
    ]
    .concat()
}

/// Write the fixed Project-repository shell directly, matching
/// `docs/project-repositories.md#a-repositorys-fixed-shell`. Does not touch
/// git — see [`scaffold`] for the git-initialized form every other fixture in
/// this file wants.
fn write_repository_shell(dir: &Path, project_code: &str) {
    fs::create_dir_all(dir.join(".github/workflows")).unwrap();
    fs::write(dir.join(".gitattributes"), GITATTRIBUTES).unwrap();
    fs::write(dir.join(".github/CODEOWNERS"), CODEOWNERS).unwrap();
    fs::write(
        dir.join(".github/workflows/automerge.yml"),
        AUTOMERGE_WORKFLOW_CONTENTS,
    )
    .unwrap();
    fs::write(
        dir.join("README.md"),
        format!("# {project_code}\n\nSource-only material for this Project.\n"),
    )
    .unwrap();
    fs::write(dir.join("AGENTS.md"), AGENT_CONTRACT).unwrap();
    fs::write(dir.join(".github/workflows/ci.yml"), workflow(FIXTURE_PIN)).unwrap();
    fs::write(
        dir.join(".github/workflows/cd.yml"),
        cd_workflow(FIXTURE_PIN),
    )
    .unwrap();
    fs::write(
        dir.join("navigator.yaml"),
        format!(
            "version: {FIXTURE_PIN}\nproject:\n  host: staging.neonlaw.com\n  name: {project_code}\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.join("templates")).unwrap();
    fs::write(dir.join("templates/onboarding.md"), placeholder_template()).unwrap();
    fs::create_dir_all(dir.join("documents")).unwrap();
    fs::write(dir.join("documents/.gitignore"), DOCUMENTS_GITIGNORE).unwrap();
    for name in SYNCED_SKILL_NAMES {
        let source = workspace_root()
            .join(".agents/skills")
            .join(name)
            .join("SKILL.md");
        let contents = fs::read_to_string(&source)
            .unwrap_or_else(|e| panic!("read {}: {e}", source.display()));
        let dest = dir.join(".agents/skills").join(name).join("SKILL.md");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(dest, contents).unwrap();
    }
}

fn scaffold(dir: &Path, project_code: &str) {
    write_repository_shell(dir, project_code);
    init_git_repository(dir);
}

fn init_git_repository(dir: &Path) {
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git init failed in {}", dir.display());
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn project_gate_source() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/project-gate.yml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_reusable_gate_requires_live_checks_and_seed_validation() {
    let source = project_gate_source();
    assert!(source.contains("navigator project gate --ci"));
    assert!(!source.contains("enable-automerge:"));
    assert!(source.contains("needs: [read-manifest, verify, documents, seeds]"));
}

/// The gate is one job, because it is one command: `verify` builds every
/// application and then runs the whole check over the tree, origin pass
/// included. Nothing else may run it, or the repository is gated twice and
/// the second run is the one nobody reads.
#[test]
fn the_reusable_gate_runs_the_gate_exactly_once() {
    let source = project_gate_source();
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&source).expect("project gate parses as YAML");
    let jobs = workflow["jobs"].as_mapping().expect("jobs");
    let running_the_gate: Vec<&str> = jobs
        .iter()
        .filter_map(|(name, job)| {
            let runs = job["steps"].as_sequence()?.iter().any(|step| {
                step["run"]
                    .as_str()
                    .is_some_and(|run| run.contains("navigator project gate --ci"))
            });
            runs.then(|| name.as_str()).flatten()
        })
        .collect();
    assert_eq!(
        running_the_gate,
        vec!["verify"],
        "the gate belongs to `verify` alone:\n{source}"
    );
    let names: Vec<&str> = jobs.keys().filter_map(|key| key.as_str()).collect();
    assert_eq!(
        names,
        vec!["read-manifest", "verify", "documents", "seeds", "ci"]
    );
}

/// The live row is checked on main and pull-request merge refs. That rule lives
/// in the CLI, which reads the ref and event itself, so the verify job carries
/// no ref branch of its own.
#[test]
fn the_project_gate_needs_no_branch_to_stay_offline_on_prs() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&project_gate_source()).expect("project gate parses as YAML");
    let verify = &workflow["jobs"]["verify"];
    assert_eq!(
        verify["permissions"]["id-token"].as_str(),
        Some("write"),
        "the gate mints the live-row session from this job"
    );
    let steps = verify["steps"].as_sequence().expect("verify steps");
    let gate = steps
        .iter()
        .find(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("navigator project gate"))
        })
        .expect("verify gate step");
    let run = gate["run"].as_str().expect("gate script");
    assert_eq!(
        run.trim(),
        "navigator project gate --ci",
        "the gate takes no host and no ref branch:\n{run}"
    );
}

/// The documents job is one CLI invocation. It reads the host from
/// `navigator.yaml` and does not keep a second verb for the same check.
#[test]
fn the_documents_job_runs_project_gate_check() {
    let source = project_gate_source();
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&source).expect("project gate parses as YAML");
    let steps = workflow["jobs"]["documents"]["steps"]
        .as_sequence()
        .expect("documents steps");
    let run = steps
        .iter()
        .find_map(|step| step["run"].as_str())
        .expect("documents run");
    assert_eq!(run.trim(), "navigator project gate --check --ci");
    assert!(!source.contains("site document verify"));
    let action = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.github/actions/document-verify/action.yml"),
    )
    .unwrap();
    assert!(action.contains("navigator project gate --check --ci"));
    assert!(!action.contains("site document verify"));
}

/// The `seeds` job reconciles `seeds/` on a push to `main`, performs a
/// server-enforced dry-run on a pull request, never overwrites, and no-ops
/// cleanly with no `seeds/` directory. Its result is part of the required
/// `ci` check, so a failed live validation cannot surface only after merge.
#[test]
fn the_reusable_gate_reconciles_seeds_on_push_to_main_only() {
    let source = project_gate_source();
    assert!(source.contains("  seeds:"));
    assert!(source.contains("no seeds — nothing to reconcile"));
    assert!(source.contains(
        r#"elif { [ "${EVENT_NAME}" = "push" ] || [ "${EVENT_NAME}" = "workflow_dispatch" ]; } && [ "${REF}" = "refs/heads/main" ]; then"#
    ));
    assert!(source.contains("navigator site import --dry-run --ci --host \"${HOST}\""));
    assert!(source.contains(r#"navigator site import --ci --host "${HOST}""#));
    assert!(source.contains("needs: read-manifest"));
    assert!(source.contains("needs: [read-manifest, verify, documents, seeds]"));
}

#[test]
fn the_seed_import_action_uses_the_repository_root() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.github/actions/seed-import/action.yml"),
    )
    .unwrap();

    assert!(
        source.contains(r#"navigator site import --ci --host "${HOST}""#),
        "the seed-import action must use the CLI's root-only import form",
    );
    assert!(
        !source.contains("inputs.dir"),
        "the seed-import action must not pass the removed directory input",
    );
}

fn gate(dir: &Path) -> assert_cmd::assert::Assert {
    navigator()
        .current_dir(dir)
        .args(["project", "gate"])
        .assert()
}

fn gate_as(dir: &Path, repository: &str) -> assert_cmd::assert::Assert {
    navigator()
        .current_dir(dir)
        .args(["project", "gate"])
        .env("GITHUB_REPOSITORY", format!("org/{repository}"))
        .assert()
}

#[test]
fn the_flat_manifest_shape_is_read_with_a_deprecation_warning() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(
        dir.path().join("navigator.yaml"),
        "host: staging.neonlaw.com\nproject: example-project\n",
    )
    .unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("Y013"))
        .stdout(str::contains("1 warning(s)"));
}

#[test]
fn a_malformed_manifest_is_reported_without_template_cascade() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(
        dir.path().join("navigator.yaml"),
        "version: 26.9.14\nproject: [not, a, project]\n",
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("navigator.yaml"))
        .stderr(str::contains("Y005"))
        .stdout(str::contains("0 template(s)"))
        .stdout(str::contains("template `code`").not());
}

#[test]
fn the_ci_ref_must_match_the_manifest_version() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ci = dir.path().join(".github/workflows/ci.yml");
    let contents = fs::read_to_string(&ci).unwrap();
    fs::write(
        &ci,
        contents.replace("project-gate.yml@26.8.23", "project-gate.yml@26.8.22"),
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("must equal manifest version"))
        .stderr(str::contains("26.8.22"))
        .stderr(str::contains("26.8.23"));
}

/// A minimal Vite workspace, which is the whole portal contract: a
/// `package.json`, an `index.html`, and a lockfile of any flavor. There is
/// deliberately no dependency allowlist.
fn write_vite_workspace(dir: &Path, relative: &str) {
    let app = dir.join(relative);
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(app.join("package.json"), "{}\n").unwrap();
    fs::write(app.join("index.html"), "<!doctype html>\n").unwrap();
    fs::write(app.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
}

fn write_portal(dir: &Path) {
    write_vite_workspace(dir, "portal");
}

/// The fixed shell — see [`write_repository_shell`] — is exactly what
/// `docs/project-repositories.md#a-repositorys-fixed-shell` documents, and it
/// passes its own gate.
#[test]
fn a_fresh_repository_shell_validates() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");

    gate(dir.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s), 0 error(s)"));

    assert!(dir.path().join("README.md").is_file());
    assert!(dir.path().join("AGENTS.md").is_file());
    assert!(
        !dir.path().join("CLAUDE.md").exists(),
        "the shell carries one contract file, and it is AGENTS.md"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".github/CODEOWNERS")).unwrap(),
        "# CODEOWNERS\n\n* @shicholas\n"
    );
    let instructions = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(instructions.contains("## Folders"));
    assert!(instructions.contains("navigator site import"));
    assert!(instructions.contains("## Tools"));
    assert!(instructions.contains("**Navigator CLI**"));
    assert!(instructions.contains("**Gmail**"));
    assert!(instructions.contains("**CourtListener**"));
    assert!(instructions.contains("The last four leave the firm."));
    assert!(instructions.contains("## Feedback"));
    assert!(instructions.contains("navigator project gate --ci"));
    assert!(dir.path().join("templates/onboarding.md").is_file());
    assert!(
        fs::read_to_string(dir.path().join("templates/onboarding.md"))
            .unwrap()
            .contains("kind: onboarding\n"),
        "the stub must declare the kind it is so a filled-in placeholder inherits it"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("documents/.gitignore")).unwrap(),
        "*\n!*/\n!*.yaml\n!.gitignore\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitattributes")).unwrap(),
        "* text=auto eol=lf\n"
    );
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/ci.yml")).unwrap();
    assert!(workflow.contains("project-gate.yml@"));
    assert!(workflow.contains("on:\n  pull_request:"));
    assert!(!workflow.contains("push:"));
    let workflow_yaml: serde_yaml::Value =
        serde_yaml::from_str(&workflow).expect("scaffolded ci.yml parses as YAML");
    assert_eq!(
        workflow_yaml["permissions"],
        serde_yaml::from_str::<serde_yaml::Value>("{contents: read, id-token: write}").unwrap()
    );
    assert!(
        !workflow.contains("with:"),
        "the caller must carry no `with:` block; the reusable workflow reads project/host from navigator.yaml:\n{workflow}"
    );
    let cd = fs::read_to_string(dir.path().join(".github/workflows/cd.yml")).unwrap();
    assert!(cd.contains("id-token: write"));
    assert!(
        cd.contains("neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.8.23")
    );
    assert!(cd.contains("workflow_dispatch:"));
    assert!(
        !cd.contains("with:"),
        "the gate/publish callers must carry no `with:` block; the reusable workflow reads project/host from navigator.yaml:\n{cd}"
    );

    // Neither retired manifest is written. `mount.json` and `navigator.toml`
    // declared a repository's own coordinates and every reader of them is gone.
    assert!(!dir.path().join("navigator.toml").exists());
    assert!(!dir.path().join("mount.json").exists());
}

/// ENG-675 reproduction 1: a freshly built repository's `.github/` tree is
/// closed to CODEOWNERS and the two thin workflow callers. Every one of the
/// five mutations below used to exit `0` against the CLI at
/// `a63c7a91aafa5159bfb5b4cf507537940d030db5`; each is now a real-CLI
/// regression against the fix.
#[test]
fn the_gate_refuses_an_extra_github_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(dir.path().join(".github/extra.txt"), "scratch\n").unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains(".github/extra.txt"))
        .stderr(str::contains("closed `.github` file set"));
}

/// ENG-675 reproduction 2: an arbitrary replacement `cd.yml` — a repository's
/// own deploy script standing in for the two pinned reusable-workflow calls
/// — was never validated at all.
#[test]
fn the_gate_refuses_an_arbitrary_replacement_cd_workflow() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(
        dir.path().join(".github/workflows/cd.yml"),
        "name: cd\n\
         on:\n  push:\n    branches: [main]\n  workflow_dispatch:\n\
         permissions:\n  contents: read\n  id-token: write\n\
         jobs:\n  deploy:\n    runs-on: ubuntu-latest\n    steps:\n      - run: ./deploy.sh\n",
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("exactly the `gate` and `publish` jobs"));
}

/// ENG-675 reproduction 3: an extra job carrying `contents: write` and
/// `id-token: write` next to the thin project-gate caller is a second,
/// unreviewed door into the same required check.
#[test]
fn the_gate_refuses_an_extra_privileged_ci_job() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ci = dir.path().join(".github/workflows/ci.yml");
    let contents = fs::read_to_string(&ci).unwrap();
    fs::write(
        &ci,
        format!(
            "{contents}  smuggled:\n    permissions:\n      contents: write\n      id-token: write\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo pwned\n"
        ),
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("exactly one job"));
}

/// ENG-828 regression guard: the closed `.github` set used to make an
/// in-repo auto-merge job unrepresentable — `ci.yml` was held to exactly one
/// job (refusing the job smuggled in above), and `.github/workflows/automerge.yml`
/// was refused outright as outside the closed set. The only way to a green
/// gate was to delete the control. A repository shaped like the fleet's own
/// three `neon-law-staging` samples — the auto-merge job in its own file,
/// `ci.yml` carrying only `ci` — must gate green with no job moved back.
///
/// The content below is typed independently of
/// `AUTOMERGE_WORKFLOW_CONTENTS` in `cli/src/projects/repository.rs`, the
/// same way the fixed strings elsewhere in this file duplicate rather than
/// import the crate's own constants: the point is to prove the validator
/// accepts this exact fleet body, not that it agrees with itself.
#[test]
fn a_fleet_representative_automerge_workflow_gates_green() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(
        dir.path().join(".github/workflows/automerge.yml"),
        "name: automerge\n\
         \n\
         on:\n  pull_request:\n    types: [opened, synchronize, reopened, ready_for_review]\n\
         \n\
         permissions:\n  contents: read\n\
         \n\
         jobs:\n  enable-automerge:\n    if: github.event.pull_request.draft == false\n    runs-on: ubuntu-latest\n    steps:\n      \
         - name: Look for the merge-queue App credentials\n        id: credentials\n        env:\n          APP_ID: ${{ secrets.AUTOMERGE_APP_ID }}\n          APP_PRIVATE_KEY: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          if [ -n \"${APP_ID}\" ] && [ -n \"${APP_PRIVATE_KEY}\" ]; then\n              echo \"present=true\" >> \"${GITHUB_OUTPUT}\"\n          else\n              echo \"present=false\" >> \"${GITHUB_OUTPUT}\"\n          fi\n      \
         - name: Mint a merge-queue App token\n        id: app-token\n        if: steps.credentials.outputs.present == 'true'\n        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1 # v3.2.0\n        with:\n          app-id: ${{ secrets.AUTOMERGE_APP_ID }}\n          private-key: ${{ secrets.AUTOMERGE_APP_PRIVATE_KEY }}\n      \
         - name: Arm auto-merge\n        env:\n          GH_TOKEN: ${{ steps.app-token.outputs.token }}\n          PR_URL: ${{ github.event.pull_request.html_url }}\n        shell: bash\n        run: |\n          set -euo pipefail\n          if [ -z \"${GH_TOKEN:-}\" ]; then\n              echo \"::notice::merge-queue App credentials absent — arming nothing, merge by hand\"\n              exit 0\n          fi\n          gh pr merge --squash --auto \"${PR_URL}\"\n",
    )
    .unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}

/// Before ENG-828, this exact file was refused as outside the closed
/// `.github` set. Under `--ci`, where nothing is written, a missing
/// `automerge.yml` is now a required-file finding rather than silently
/// absent. (The local, non-`--ci` gate self-repairs it instead — see
/// `project_gate_rewrites_a_drifted_automerge_workflow` and
/// `a_fleet_representative_automerge_workflow_gates_green`, which relies on
/// the file the shell already carries.)
#[test]
fn the_ci_gate_requires_the_automerge_workflow() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::remove_file(dir.path().join(".github/workflows/automerge.yml")).unwrap();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .assert()
        .failure()
        .stderr(str::contains("missing required"))
        .stderr(str::contains(".github/workflows/automerge.yml"));
}

/// `automerge.yml` is machine-owned, so a hand edit is drift the local gate
/// self-repairs — the same `write_fixes` behavior `documents/.gitignore`
/// already gets, and notably the one `validate_codeowners` does not.
#[test]
fn project_gate_rewrites_a_drifted_automerge_workflow() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let path = dir.path().join(".github/workflows/automerge.yml");
    let canonical = fs::read_to_string(&path).unwrap();
    fs::write(&path, "name: automerge\n# hand-edited\n").unwrap();

    gate(dir.path()).success().stdout(str::contains("fixed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), canonical);
}

/// Under `--ci`, nothing is written: a drifted `automerge.yml` becomes a
/// finding instead, the same as every other `--ci` fix-vs-report split.
#[test]
fn gate_ci_reports_a_drifted_automerge_workflow_without_writing() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let path = dir.path().join(".github/workflows/automerge.yml");
    fs::write(&path, "name: automerge\n# hand-edited\n").unwrap();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .assert()
        .failure()
        .stderr(str::contains(
            "must match the canonical auto-merge workflow",
        ));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "name: automerge\n# hand-edited\n"
    );
}

/// ENG-675 reproduction 4: `pull_request_target` runs with the base
/// repository's secrets and write token against a fork's checked-out code.
#[test]
fn the_gate_refuses_pull_request_target() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ci = dir.path().join(".github/workflows/ci.yml");
    let contents = fs::read_to_string(&ci).unwrap();
    fs::write(
        &ci,
        contents.replace("pull_request:", "pull_request_target:"),
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("pull_request_target"));
}

/// The reusable workflow reads `project`/`host` from the caller's own
/// `navigator.yaml`, so a caller has nothing left to repeat there — a
/// `with:` block of any shape is rejected outright, not merely checked for
/// agreement with the manifest.
#[test]
fn the_gate_refuses_a_with_block_on_the_ci_caller() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ci = dir.path().join(".github/workflows/ci.yml");
    let contents = fs::read_to_string(&ci).unwrap();
    fs::write(
        &ci,
        contents.replace(
            "uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23\n",
            "uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.8.23\n    with:\n      project: \"example-project\"\n",
        ),
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("must not declare a `with:` block"));
}

/// After 26.9.23, a retired `gate.yml` is refused outright — the door
/// `docs/gate.md` said this release would close.
#[test]
fn the_gate_refuses_a_retired_workflow_filename() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ci = dir.path().join(".github/workflows/ci.yml");
    fs::rename(&ci, dir.path().join(".github/workflows/gate.yml")).unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains("gate.yml"))
        .stderr(str::contains("this release refuses it"));
}

/// A `.yaml` spelling of either workflow reports the extension it actually
/// found and the one Navigator reads, not a bare "missing required" that
/// sends the operator looking for a typo they already made correctly.
#[test]
fn the_gate_reports_a_yaml_extension_workflow_precisely() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::rename(
        dir.path().join(".github/workflows/ci.yml"),
        dir.path().join(".github/workflows/ci.yaml"),
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .stderr(str::contains(".github/workflows/ci.yaml"))
        .stderr(str::contains(".github/workflows/ci.yml"));
}

#[test]
fn gate_without_oidc_leaves_the_live_row_alone() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env("GITHUB_REF", "refs/heads/main")
        .env("GITHUB_EVENT_NAME", "push")
        .assert()
        .success()
        .stdout(str::contains("this event/ref cannot mint a CI session"));
}

/// A malformed PR ref remains offline; only the exact merge-ref shape reaches
/// the OIDC door.
#[test]
fn gate_ci_malformed_pr_ref_leaves_the_live_row_alone() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", "http://127.0.0.1/oidc")
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "token")
        .env("GITHUB_REF", "refs/heads/topic")
        .env("GITHUB_EVENT_NAME", "pull_request")
        .assert()
        .success()
        .stdout(str::contains("this event/ref cannot mint a CI session"));
}

/// All three shapes validate: templates only, a portal only, and both.
#[test]
fn templates_only_a_portal_only_and_both_all_validate() {
    // Templates only — what the shell carries.
    let templates_only = TempDir::new().unwrap();
    scaffold(templates_only.path(), "example-project");
    gate(templates_only.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s)"));

    // Both halves in one repository, which is the point of the collapse.
    let both = TempDir::new().unwrap();
    scaffold(both.path(), "example-project");
    write_portal(both.path());
    gate(both.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));

    // The shell carries one placeholder template; a portal is extra.
    let portal_only = TempDir::new().unwrap();
    scaffold(portal_only.path(), "example-project");
    write_portal(portal_only.path());
    gate(portal_only.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));
}

#[test]
fn a_nested_template_is_refused_in_a_project_repository() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme");
    let flat = dir.path().join("templates/onboarding.md");
    let nested = dir.path().join("templates/neon_law/onboarding.md");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    fs::rename(&flat, &nested).unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stdout(str::contains("N110"))
        .stdout(str::contains(
            "Project templates must be direct `templates/<code>.md` files",
        ))
        .stderr(str::contains(
            "Project templates must be direct `templates/<code>.md` files",
        ));
}

/// ENG-693: a Project template's `code` is already scoped to this repository's
/// own Project by `template.project_id`, so the filename carries no required
/// Project-code prefix — a bare stem validates like any other.
#[test]
fn a_template_filename_needs_no_project_code_prefix() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme");
    let path = dir.path().join("templates/onboarding.md");
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("code: onboarding", "code: project_template");
    fs::rename(&path, dir.path().join("templates/project_template.md")).unwrap();
    fs::write(dir.path().join("templates/project_template.md"), body).unwrap();
    gate(dir.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s), 0 error(s)"));
}

#[test]
fn a_template_code_must_equal_the_filename_stem() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme");
    let path = dir.path().join("templates/onboarding.md");
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("code: onboarding", "code: other");
    fs::write(&path, body).unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("onboarding"))
        .stderr(str::contains("other"));
}

/// Direct `apps/<app>/package.json` files are the app declarations. Every
/// discovered directory is validated as its own Vite workspace, while a
/// directory under `apps/` with no package manifest is ordinary source rather
/// than a second declaration the repository has to keep synchronized.
#[test]
fn direct_apps_are_discovered_and_each_is_validated() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    write_vite_workspace(dir.path(), "apps/portal");
    write_vite_workspace(dir.path(), "apps/exchange");
    let exchange = dir.path().join("apps").join("exchange");
    fs::create_dir_all(dir.path().join("apps/shared")).unwrap();
    fs::write(dir.path().join("apps/shared/routes.ts"), "export {};\n").unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));

    fs::remove_file(dir.path().join("apps/exchange/index.html")).unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains(exchange.display().to_string()))
        .stderr(str::contains("is not a Vite workspace"))
        .stderr(str::contains("index.html"));

    fs::write(exchange.join("index.html"), "<!doctype html>\n").unwrap();
    fs::write(exchange.join(".env.production"), "SECRET=synthetic\n").unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(
            str::is_match(
                r"(?m)[^\r\n]*[\\/]apps[\\/]exchange[\\/]\.env\.production: error: client answers and environment secrets must not be committed",
            )
            .unwrap(),
        )
        .stderr(str::contains("is not a Vite workspace").not())
        .stderr(str::contains("must not be committed"));
}

/// Existing Project repositories can move one application at a time: a root
/// `portal/` and a new `apps/exchange/` are both discovered and checked during
/// the transition instead of either layout silently shadowing the other.
#[test]
fn a_legacy_root_portal_and_new_apps_can_transition_together() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/exchange");

    gate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));
}

#[test]
fn the_legacy_and_new_portal_locations_cannot_claim_the_same_route() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/portal");

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("apps/portal"))
        .stderr(str::contains("claim the same application route"));
}

/// LAW-74: `verify` calls the portal-specific command. Its lifecycle
/// (order, `pnpm` arguments, stopping at the first failure) is covered in
/// `cli/src/projects/portal.rs`; this pins that `verify` invokes that command.
#[test]
fn the_verify_job_builds_the_portal_through_the_cli() {
    let source = project_gate_source();
    assert!(
        source.contains("navigator project portal --dir ."),
        "verify must call the CLI's portal verb"
    );
    for retired in [
        "Install application dependencies",
        "Lint applications",
        "Typecheck applications",
        "Test applications",
        "Build applications",
    ] {
        assert!(
            !source.contains(retired),
            "the per-verb application shell step {retired:?} must be retired"
        );
    }
}

#[test]
fn an_application_directory_name_must_be_a_route_safe_slug() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    write_vite_workspace(dir.path(), "apps/Client_Exchange");

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("Client_Exchange"))
        .stderr(str::contains("is not a valid application name"));
}

/// A Project carrying neither half is reported distinctly and is not a failure.
///
/// A Project may legitimately open before either half exists, so this is a note
/// rather than an error — the same split the doctor keeps between a warning and
/// a failure.
#[test]
fn a_repository_carrying_neither_half_is_reported_and_not_failed() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let templates = dir.path().join("templates");
    if templates.exists() {
        fs::remove_dir_all(templates).unwrap();
    }

    gate(dir.path())
        .success()
        .stdout(str::contains("carries neither"));
}

/// The repository name is the Project code, so a name that could not be one is
/// the error — there is no manifest to disagree with.
#[test]
fn a_repository_name_that_is_not_a_valid_project_code_is_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");

    gate_as(dir.path(), "Not_A_Code")
        .failure()
        .code(1)
        .stderr(str::contains("is not a valid Navigator Project code"));

    // `new` is well-formed and still refused: `/app/projects/new` is
    // Navigator's matter-open form.
    gate_as(dir.path(), "new")
        .failure()
        .code(1)
        .stderr(str::contains("is not a valid Navigator Project code"));
}

/// A `portal/` that is not a Vite workspace is a failure, not a warning:
/// provisioning would otherwise assume a build that cannot run.
#[test]
fn a_portal_that_is_not_a_vite_workspace_is_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("portal/src")).unwrap();
    fs::write(dir.path().join("portal/package.json"), "{}\n").unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("is not a Vite workspace"))
        .stderr(str::contains("index.html"))
        .stderr(str::contains("a lockfile"));
}

/// The check the validator exists for, kept through the restructuring.
#[test]
fn client_uploads_and_generated_output_are_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("uploads")).unwrap();
    fs::write(dir.path().join("uploads/client-document.pdf"), "synthetic").unwrap();
    fs::create_dir_all(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/build.log"), "synthetic").unwrap();
    fs::write(dir.path().join(".env.production"), "SECRET=x").unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("forbidden `uploads` path"))
        .stderr(str::contains("forbidden `target` path"))
        .stderr(str::contains("must not be committed"));
}

#[test]
fn document_pointers_are_source_but_document_bytes_are_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("documents/exhibits/2026-09-05")).unwrap();
    fs::write(
        dir.path()
            .join("documents/exhibits/2026-09-05/screenshot.png.yaml"),
        "kind: exhibit\nvisibility: internal\ncurrent_version:\n  version: 1\n  asset_id: 0199b9e4-14b7-7ad0-87a5-71ef24a46d40\n  created_at: 2026-09-05T12:00:00Z\n  sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n  size_bytes: 42\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    )
    .unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));

    // A raw byte staged behind the `documents/.gitignore` is the supported
    // workflow's own output — `site pull` puts it there — so it is not a
    // committed legal document and the gate leaves it alone (LAW-12).
    let binary = dir
        .path()
        .join("documents/exhibits/2026-09-05/screenshot.png");
    fs::write(&binary, b"synthetic image bytes").unwrap();
    // Ignored, so nothing proposes it and the gate stays green.
    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));

    // Forcing it past the guard and into the index is the thing the rule
    // names, and that is still refused.
    run_git(
        dir.path(),
        &["add", "-f", "documents/exhibits/2026-09-05/screenshot.png"],
    );
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains(binary.display().to_string()))
        .stderr(str::contains(
            "legal documents and raw document bytes must not be committed",
        ));
}

/// `site sync` and `site pull` exist to put bytes under `documents/`, and the
/// `documents/.gitignore` they write keeps those bytes untracked. The gate
/// enumerates what Git would carry, so it reads them as absent rather than as
/// committed legal material — otherwise the only way to get a green run would
/// be to delete the bytes those commands exist to fetch (LAW-12).
#[test]
fn gate_ignores_raw_document_bytes_materialised_by_a_pull() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    )
    .unwrap();
    let raw = dir.path().join("documents/memos/agreement.md");
    fs::write(&raw, "synthetic pulled bytes\n").unwrap();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .success()
        .stdout(str::contains("0 error(s)"))
        .stderr(predicates::str::is_empty())
        .stderr(predicates::str::contains(raw.display().to_string()).not());
}

#[test]
fn gate_leaves_ignored_document_bodies_byte_for_byte_unchanged() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    let raw = dir.path().join("documents/memos/agreement.md");
    let contents = concat!(
        "The court applied §\n",
        "2033.300 in *Case Name* (2008), describing *Elston* and the\n",
        "applicable standard. See [the authority][authority].\n\n",
        "[authority]: <https://www.example.com/cases/california/supreme-court/",
        "extraordinarily-long-reference-path/2025/1234567890/",
        "opinion?download=1&format=html#section-2033-300>\n",
    );
    fs::write(&raw, contents.as_bytes()).unwrap();
    fs::write(
        dir.path().join("documents/memos/agreement.pdf.yaml"),
        "kind: agreement\nvisibility: internal\ncurrent_version:\n  version: 1\n  asset_id: 0199b9e4-14b7-7ad0-87a5-71ef24a46d40\n  created_at: 2026-09-05T12:00:00Z\n  sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n  size_bytes: 42\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("authored.md"),
        "Authored repository content with an unwrapped URL https://example.com/path.\n",
    )
    .unwrap();

    let output = navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .output()
        .unwrap();

    assert_eq!(fs::read(&raw).unwrap(), contents.as_bytes());
    assert!(
        !output.status.success(),
        "the authored Markdown violation must still fail the gate:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("authored.md") && stdout.contains("M034"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Validated 1 document pointer(s), found 0 error(s)"),
        "{stdout}"
    );
    assert!(
        !stdout.contains(&raw.display().to_string()),
        "ignored document body entered the Markdown walk:\n{stdout}"
    );
}

#[test]
fn gate_leaves_an_application_owned_templates_directory_alone() {
    // The published Project gate runs `navigator validate .` over the whole
    // checkout, and the layout permits application source under `apps/<app>/`
    // and a root `portal/`. A Vite application's own `src/templates/*.md` is
    // an application asset, not a notation, so asking it for a `kind:` would
    // redden the required check on every repository carrying that shape. The
    // lane is the repository's own `templates/` root and nothing else.
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    // A real Vite application, since a bare directory trips the layout's
    // own workspace check before this rule is ever reached.
    let app = dir.path().join("apps/web");
    fs::create_dir_all(app.join("src/templates")).unwrap();
    fs::write(app.join("package.json"), "{}\n").unwrap();
    fs::write(app.join("index.html"), "<!doctype html>\n").unwrap();
    fs::write(app.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    fs::write(app.join("src/templates/page.md"), "# Page layout\n").unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));

    // The repository's own template root is still held to it.
    fs::write(
        dir.path().join("templates/will.md"),
        "---\ntitle: Last Will\ncode: sample__will\nconfidential: true\n---\n\n# Last Will\n",
    )
    .unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stdout(str::contains("under `templates/` but declares no `kind:`"));
}

#[test]
fn gate_reports_a_tracked_raw_document_byte() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!.gitignore\n",
    )
    .unwrap();
    let raw = dir.path().join("documents/memos/agreement.md");
    fs::write(&raw, "synthetic tracked bytes\n").unwrap();
    run_git(dir.path(), &["add", "-f", "documents/memos/agreement.md"]);

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains(raw.display().to_string()))
        .stderr(str::contains(
            "legal documents and raw document bytes must not be committed",
        ))
        .stdout(str::contains("1 error(s)"));
}

#[test]
fn a_documents_gitignore_that_drops_the_deny_line_fails_under_ci() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ignore = dir.path().join("documents/.gitignore");
    let drifted = "# Files never land, only pointers.\n!*/\n!*.yml\n!.gitignore\n";
    fs::write(&ignore, drifted).unwrap();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains("Y014"))
        .stderr(str::contains("documents/.gitignore"));

    assert_eq!(fs::read_to_string(&ignore).unwrap(), drifted);
}

#[test]
fn project_gate_rewrites_a_drifted_documents_gitignore() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let ignore = dir.path().join("documents/.gitignore");
    fs::write(&ignore, "!*/\n!*.yml\n!.gitignore\n").unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("fixed"))
        .stdout(str::contains("0 error(s)"));

    assert_eq!(
        fs::read_to_string(&ignore).unwrap(),
        "*\n!*/\n!*.yaml\n!.gitignore\n"
    );
}

/// `.github/` is now closed to exactly CODEOWNERS and the two thin workflow
/// callers, so a nested ignore file there would itself be an unenumerated
/// path; `tests/` carries no such closed set and still proves the same
/// thing: `git ls-files --exclude-standard` honours a directory's own
/// `.gitignore`, not only the root one. `tests/` is an allowed root a
/// repository may create for its own source-level checks (LAW-49: the gate
/// neither requires nor generates one), so this test creates it itself.
#[test]
fn gate_honours_a_nested_ignore_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::create_dir_all(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("tests/.gitignore"), "*.env\n").unwrap();
    let ignored = dir.path().join("tests/hidden.env");
    fs::write(&ignored, "synthetic secret\n").unwrap();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .success()
        .stdout(str::contains("0 error(s)"))
        .stderr(predicates::str::is_empty())
        .stderr(predicates::str::contains(ignored.display().to_string()).not());
}

#[test]
fn gate_refuses_a_repository_shell_that_is_not_a_git_repository() {
    let dir = TempDir::new().unwrap();
    write_repository_shell(dir.path(), "example-project");

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("has no .git"));
}

/// The retired `notation repository` command is gone rather than aliased.
#[test]
fn the_notation_repository_command_is_gone() {
    navigator()
        .args(["notation", "repository", "validate", "."])
        .assert()
        .failure();
}

/// The harness-specific mirrors are refused by name, in one place, for every
/// Project repository at once.
///
/// `CLAUDE.md` was already refused — as an anonymous unenumerated root, which
/// said nothing about what survives it. A `.claude/` or a `.codex/` was not
/// refused at all: the layout walk matched a path's first component against
/// the allowed roots only when that component was the whole path, so a
/// committed mirror directory was never examined. Each now names its survivor
/// — `AGENTS.md` or `.agents/skills/` — and the remedy.
#[test]
fn gate_refuses_the_retired_agent_mirrors_by_name() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::write(dir.path().join("CLAUDE.md"), "AGENTS.md").unwrap();
    for mirror in [".claude", ".codex"] {
        let skill = dir.path().join(mirror).join("skills/council");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# mirrored council\n").unwrap();
    }

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains(
            "`CLAUDE.md` is a retired agent-instruction mirror",
        ))
        .stderr(str::contains("`AGENTS.md` is the whole contract"))
        .stderr(str::contains(
            "`.claude/` is a retired agent-instruction mirror",
        ))
        .stderr(str::contains(
            "`.codex/` is a retired agent-instruction mirror",
        ))
        .stderr(str::contains("`.agents/skills/` is the whole catalog"));
}

/// The canonical pair is what a repository is meant to carry, so a checkout
/// holding both passes: this refuses the mirrors, not agent tooling.
#[test]
fn gate_accepts_the_canonical_contract_and_catalog() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");

    assert!(dir.path().join("AGENTS.md").is_file());
    assert!(dir.path().join(".agents/skills").is_dir());
    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}

#[test]
fn gate_requires_the_canonical_codeowners_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    let path = dir.path().join(".github/CODEOWNERS");

    fs::remove_file(&path).unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("missing required `.github/CODEOWNERS`"));

    fs::write(&path, "* @nick\n").unwrap();
    gate(dir.path()).failure().code(1).stderr(str::contains(
        "must contain the canonical single-owner rule",
    ));
}

/// A synced skill that has drifted from the canonical copy fails `validate`
/// and names the file, so a hand edit is caught rather than silently
/// diverging across 19 repositories.
#[test]
fn gate_fails_on_a_drifted_synced_skill() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");

    fs::write(
        dir.path().join(".agents/skills/council/SKILL.md"),
        "drifted content",
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("synced skill `council` has drifted"));
}

/// A repository carrying the full canonical catalog gates green with no
/// further step.
#[test]
fn gate_passes_with_the_full_skill_catalog() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    assert!(
        dir.path().join(".agents/skills/server/SKILL.md").is_file(),
        "the shell must include every synced skill"
    );

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}

/// `.agents/` is the opt-in, and it opts into the whole catalog.
///
/// The moment a repository has one, an agent is working in it under whatever
/// skills it happens to find, and the ones it does not find are precisely the
/// conventions nobody told it about. `portal-chrome` is why this is a finding
/// rather than a suggestion: "the portal wears the library's teal and never
/// repaints it" lived for months as a header comment inside the very file
/// that violated it, in sixteen repositories, claiming a fleet-wide
/// uniformity that had already broken in two directions.
#[test]
fn gate_fails_when_agents_exists_without_the_catalog() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project");
    fs::remove_dir_all(dir.path().join(".agents/skills/portal-chrome")).unwrap();
    fs::remove_dir_all(dir.path().join(".agents/skills/server")).unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("missing synced skill `portal-chrome`"))
        .stderr(str::contains("missing synced skill `server`"));
}
