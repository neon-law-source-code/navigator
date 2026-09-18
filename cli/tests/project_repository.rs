//! End-to-end tests for the one Project repository scaffold and validator.
//!
//! One repository per Project code, holding notation templates under
//! `templates/` and application source under `apps/<app>/`. There is one
//! scaffold and one validator for both. A legacy root `portal/` remains valid
//! during the source-layout transition. The scaffold writes the versioned
//! nested `navigator.yaml` manifest and the thin `ci.yml`/`cd.yml` callers.

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

/// The pin these fixtures scaffold with.
///
/// A literal, not this binary's own reported version: a `cargo test` build
/// carries neither a runtime nor a build-time `NAVIGATOR_RELEASE_TAG`, so
/// `scaffold`'s default is empty here and these tests are not the ones
/// exercising that default — `the_scaffold_default_pin_is_a_release_tag_or_empty`
/// and `the_scaffold_refuses_a_pin_that_is_not_a_release_tag` in
/// `cli/src/projects/repository.rs` are.
const FIXTURE_PIN: &str = "26.8.23";

fn scaffold(dir: &Path, project_code: &str) -> assert_cmd::assert::Assert {
    let result = navigator()
        .args(["project", "repository", "scaffold", project_code, "--dir"])
        .arg(dir)
        .args([
            "--action-version",
            FIXTURE_PIN,
            "--host",
            "staging.neonlaw.com",
        ])
        .assert();
    init_git_repository(dir);
    result
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
fn the_reusable_gate_keeps_live_work_out_of_the_required_check() {
    let source = project_gate_source();
    assert!(source.contains("navigator project gate --ci"));
    assert!(!source.contains("enable-automerge:"));
    assert!(source.contains("needs: [read-manifest, verify, documents]"));
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

/// The live row is checked only where a session can be minted. That rule now
/// lives in the CLI, which reads the ref and the event itself, so the workflow
/// carries no branch for it and the gate reads the same everywhere.
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

/// The `seeds` job reconciles `seeds/` on a push to `main`, is offline on a
/// pull request (the gate already covers the shape), never overwrites, no-ops
/// cleanly with no `seeds/` directory, and stays outside the required `ci`
/// job's dependencies — its live half needs a reachable deployment, and the
/// always-required check must never depend on that.
#[test]
fn the_reusable_gate_reconciles_seeds_on_push_to_main_only() {
    let source = project_gate_source();
    assert!(source.contains("  seeds:"));
    assert!(source.contains("no seeds — nothing to reconcile"));
    assert!(source.contains(
        r#"if [ -n "${HOST}" ] && [ "${EVENT_NAME}" = "push" ] && [ "${REF}" = "refs/heads/main" ]; then"#
    ));
    assert!(source.contains(r#"navigator site import --ci --host "${HOST}" --dir seeds"#));
    assert!(source.contains("needs: read-manifest"));
    assert!(source.contains("needs: [read-manifest, verify, documents]"));
    assert!(!source.contains("needs: [read-manifest, verify, documents, seeds]"));
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
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

#[test]
fn the_scaffold_produces_a_repository_that_validates_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    gate(dir.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s), 0 error(s)"));

    assert!(dir.path().join("README.md").is_file());
    assert!(dir.path().join("AGENTS.md").is_file());
    assert!(
        !dir.path().join("CLAUDE.md").exists(),
        "the scaffold writes one contract file, and it is AGENTS.md"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".github/CODEOWNERS")).unwrap(),
        "# CODEOWNERS\n\n* @shicholas\n"
    );
    let instructions = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(instructions.contains("`apps/<app>/`"));
    assert!(instructions.contains("source grouping is not a URL segment"));
    assert!(instructions.contains("root `portal/` is also"));
    assert!(instructions.contains("carries no required Project-code prefix"));
    assert!(instructions.contains("`code:` matches the stem"));
    assert!(instructions.contains("A Project code names a matter and its repository."));
    assert!(instructions.contains("It identifies a client, so it is client data."));
    assert!(instructions.contains("The one legitimate use here is this repository naming itself"));
    assert!(
        instructions.contains("commit message, code comment, branch name, or pull-request body")
    );
    assert!(instructions.contains("A precedent"));
    assert!(instructions.contains("citation is still a breach"));
    assert!(dir.path().join("templates/onboarding.md").is_file());
    assert!(
        fs::read_to_string(dir.path().join("templates/onboarding.md"))
            .unwrap()
            .contains("kind: onboarding\n"),
        "the stub must declare the kind it is so a filled-in placeholder inherits it"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("documents/.gitignore")).unwrap(),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n"
    );
    assert!(!dir.path().join("templates/project_template.md").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitattributes")).unwrap(),
        "* text=auto eol=lf\n"
    );
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/ci.yml")).unwrap();
    assert!(workflow.contains("project-gate.yml@"));
    assert!(workflow.contains("on:\n  pull_request:"));
    assert!(!workflow.contains("push:"));
    assert!(!workflow.contains("project_repository: true"));
    let workflow_yaml: serde_yaml::Value =
        serde_yaml::from_str(&workflow).expect("scaffolded ci.yml parses as YAML");
    assert!(workflow_yaml["permissions"].is_null());
    assert!(workflow.contains("project: \"example-project\""));
    assert!(workflow.contains("host: \"staging.neonlaw.com\""));
    let cd = fs::read_to_string(dir.path().join(".github/workflows/cd.yml")).unwrap();
    assert!(
        !cd.contains("TBD"),
        "the publish workflow is still a placeholder:\n{cd}"
    );
    assert!(cd.contains("id-token: write"));
    assert!(
        cd.contains("neon-law-source-code/navigator/.github/workflows/project-publish.yml@26.8.23")
    );
    assert!(cd.contains("workflow_dispatch:"));
    assert!(cd.contains("project: \"example-project\""));
    assert!(cd.contains("host: \"staging.neonlaw.com\""));

    // Neither retired manifest is written. `mount.json` and `navigator.toml`
    // declared a repository's own coordinates and every reader of them is gone;
    // the scaffold must not bring either back.
    assert!(!dir.path().join("navigator.toml").exists());
    assert!(!dir.path().join("mount.json").exists());

    // Idempotent: a second run leaves every existing file alone and still validates.
    fs::write(
        dir.path().join(".gitattributes"),
        "# repository preference\n",
    )
    .unwrap();
    scaffold(dir.path(), "example-project")
        .success()
        .stdout(str::contains("left alone"));
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitattributes")).unwrap(),
        "# repository preference\n"
    );
    gate(dir.path()).success();
}

#[test]
fn gate_without_oidc_leaves_the_live_row_alone() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env("GITHUB_REF", "refs/heads/main")
        .env("GITHUB_EVENT_NAME", "push")
        .assert()
        .success()
        .stdout(str::contains("only a push to main mints a CI session"));
}

/// The live row is checked only where a session can be minted: a push to
/// `main`. Anywhere else the gate finishes its offline work and says why it
/// stopped, rather than spending a request the server would refuse.
#[test]
fn gate_ci_off_main_leaves_the_live_row_alone() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    navigator()
        .current_dir(dir.path())
        .args(["project", "gate", "--ci"])
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", "http://127.0.0.1/oidc")
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "token")
        .env("GITHUB_REF", "refs/pull/7/merge")
        .env("GITHUB_EVENT_NAME", "pull_request")
        .assert()
        .success()
        .stdout(str::contains("only a push to main mints a CI session"));
}

/// All three shapes validate: templates only, a portal only, and both.
#[test]
fn templates_only_a_portal_only_and_both_all_validate() {
    // Templates only — what the scaffold produces.
    let templates_only = TempDir::new().unwrap();
    scaffold(templates_only.path(), "example-project").success();
    gate(templates_only.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s)"));

    // Both halves in one repository, which is the point of the collapse.
    let both = TempDir::new().unwrap();
    scaffold(both.path(), "example-project").success();
    write_portal(both.path());
    gate(both.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));

    // Scaffold now writes one placeholder template; a portal is extra.
    let portal_only = TempDir::new().unwrap();
    scaffold(portal_only.path(), "example-project").success();
    write_portal(portal_only.path());
    gate(portal_only.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));
}

#[test]
fn a_nested_template_is_refused_in_a_project_repository() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme").success();
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
    scaffold(dir.path(), "acme").success();
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
    scaffold(dir.path(), "acme").success();
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
    scaffold(dir.path(), "example-project").success();
    write_vite_workspace(dir.path(), "apps/portal");
    write_vite_workspace(dir.path(), "apps/exchange");
    fs::create_dir_all(dir.path().join("apps/shared")).unwrap();
    fs::write(dir.path().join("apps/shared/routes.ts"), "export {};\n").unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));

    fs::remove_file(dir.path().join("apps/exchange/index.html")).unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("apps/exchange"))
        .stderr(str::contains("is not a Vite workspace"))
        .stderr(str::contains("index.html"));

    fs::write(
        dir.path().join("apps/exchange/.env.production"),
        "SECRET=synthetic\n",
    )
    .unwrap();
    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("apps/exchange/.env.production"))
        .stderr(str::contains("must not be committed"));
}

/// Existing Project repositories can move one application at a time: a root
/// `portal/` and a new `apps/exchange/` are both discovered and checked during
/// the transition instead of either layout silently shadowing the other.
#[test]
fn a_legacy_root_portal_and_new_apps_can_transition_together() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/exchange");

    gate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));
}

#[test]
fn the_legacy_and_new_portal_locations_cannot_claim_the_same_route() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/portal");

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("apps/portal"))
        .stderr(str::contains("claim the same application route"));
}

/// The scaffold writes one contract file and no harness mirror.
///
/// `AGENTS.md` is read directly by every harness pointed at the tree, so there
/// is nothing to keep in sync and nothing to materialise. That matters most on
/// Windows: the mirror this replaced was a symlink on Unix and a copy
/// elsewhere, and a clone without `core.symlinks` received a nine-byte stub
/// holding its own target path — silently, with a clean `git status`. A file
/// that is only ever a file cannot fail that way. The release archive for
/// Windows compiles this path, so this is also the test that runs it.
#[test]
fn the_scaffold_writes_one_contract_and_no_mirror() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    let agents = fs::read(dir.path().join("AGENTS.md")).unwrap();
    assert!(!agents.is_empty());
    assert!(
        !dir.path().join("CLAUDE.md").exists(),
        "a CLAUDE.md mirror is what this layout retired"
    );
}

/// `scaffold` leaves an existing `AGENTS.md` alone rather than overwriting the
/// contract a repository already wrote for itself.
#[test]
fn the_scaffold_leaves_an_existing_agents_md_alone() {
    let dir = TempDir::new().unwrap();
    let hand_written = "# A contract this repository already had\n";
    fs::write(dir.path().join("AGENTS.md"), hand_written).unwrap();

    scaffold(dir.path(), "example-project")
        .success()
        .stdout(str::contains("AGENTS.md (left alone)"));

    let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(agents, hand_written);
}

/// ENG-674: `verify` no longer generates a per-application bash loop — it
/// calls `navigator project build`, which discovers and builds every
/// application itself. That call's own per-application, per-verb behavior
/// (order, `pnpm` arguments, stopping at the first failure) is covered
/// directly in `cli/src/projects/build.rs`'s unit tests; this just pins that
/// `verify` invokes it rather than a shell loop.
#[test]
fn the_verify_job_installs_lints_typechecks_tests_and_builds_through_the_cli() {
    let source = project_gate_source();
    assert!(
        source.contains("navigator project build --dir ."),
        "verify must call the CLI's build verb"
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();

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
    scaffold(TempDir::new().unwrap().path(), "new")
        .failure()
        .code(2)
        .stderr(str::contains("invalid Project code"));
}

/// A `portal/` that is not a Vite workspace is a failure, not a warning:
/// provisioning would otherwise assume a build that cannot run.
#[test]
fn a_portal_that_is_not_a_vite_workspace_is_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join("documents/exhibits/2026-09-05")).unwrap();
    fs::write(
        dir.path()
            .join("documents/exhibits/2026-09-05/screenshot.png.yml"),
        "kind: exhibit\nvisibility: internal\ncurrent_version:\n  version: 1\n  asset_id: 0199b9e4-14b7-7ad0-87a5-71ef24a46d40\n  created_at: 2026-09-05T12:00:00Z\n  sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n  size_bytes: 42\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n",
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
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n",
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
fn gate_leaves_an_application_owned_templates_directory_alone() {
    // The published Project gate runs `navigator validate .` over the whole
    // checkout, and the layout permits application source under `apps/<app>/`
    // and a root `portal/`. A Vite application's own `src/templates/*.md` is
    // an application asset, not a notation, so asking it for a `kind:` would
    // redden the required check on every repository carrying that shape. The
    // lane is the repository's own `templates/` root and nothing else.
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n",
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
    scaffold(dir.path(), "example-project").success();
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
    scaffold(dir.path(), "example-project").success();
    let ignore = dir.path().join("documents/.gitignore");
    fs::write(&ignore, "!*/\n!*.yml\n!.gitignore\n").unwrap();

    gate(dir.path())
        .success()
        .stdout(str::contains("fixed"))
        .stdout(str::contains("0 error(s)"));

    assert_eq!(
        fs::read_to_string(&ignore).unwrap(),
        "*\n!*/\n!*.yaml\n!*.yml\n!.gitignore\n"
    );
}

#[test]
fn gate_honours_a_nested_ignore_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::write(dir.path().join(".github/.gitignore"), "*.env\n").unwrap();
    let ignored = dir.path().join(".github/hidden.env");
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
fn gate_refuses_a_scaffolded_tree_that_is_not_a_git_repository() {
    let dir = TempDir::new().unwrap();
    navigator()
        .args(["project", "repository", "scaffold"])
        .arg("example-project")
        .args(["--dir"])
        .arg(dir.path())
        .args([
            "--action-version",
            FIXTURE_PIN,
            "--host",
            "staging.neonlaw.com",
        ])
        .assert()
        .success();

    navigator()
        .current_dir(dir.path())
        .args(["project", "gate"])
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("has no .git"));
}

/// The retired `notations repository` command is gone rather than aliased.
#[test]
fn the_notations_repository_command_is_gone() {
    navigator()
        .args(["notations", "repository", "validate", "."])
        .assert()
        .failure();
}

fn sync_skills(dir: &Path) -> assert_cmd::assert::Assert {
    navigator()
        .args(["project", "repository", "sync-skills"])
        .arg(dir)
        .assert()
}

/// ENG-383: a Project repository can carry `.agents/skills/` without the
/// layout gate refusing it as an unexpected root, and `sync-skills` is what
/// populates it from Navigator's own compiled-in copies.
#[test]
fn sync_skills_writes_the_canonical_catalog_and_validate_accepts_it() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    sync_skills(dir.path())
        .success()
        .stdout(str::contains("synced"));

    for skill in [
        "council",
        "legal-council",
        "client-council",
        "human-readable",
        "stay-in-repo",
    ] {
        let path = dir
            .path()
            .join(".agents/skills")
            .join(skill)
            .join("SKILL.md");
        assert!(path.is_file(), "expected {} to exist", path.display());
        assert!(!fs::read_to_string(&path).unwrap().is_empty());
    }

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}

#[test]
fn gate_requires_the_canonical_codeowners_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
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

/// `sync-skills` overwrites rather than leaving an existing file alone (unlike
/// `scaffold`) — the whole point is that the repository's copy stays
/// identical to the canonical one, so re-running it is also how an operator
/// clears the drift `validate` reports.
#[test]
fn sync_skills_overwrites_a_hand_edited_copy() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    sync_skills(dir.path()).success();

    let path = dir.path().join(".agents/skills/council/SKILL.md");
    let canonical = fs::read_to_string(&path).unwrap();
    fs::write(&path, "hand-edited drift").unwrap();

    sync_skills(dir.path()).success();
    assert_eq!(fs::read_to_string(&path).unwrap(), canonical);
}

/// A synced skill that has drifted from the canonical copy fails `validate`
/// and names the file, so a hand edit or a stale sync is caught rather than
/// silently diverging across 19 repositories.
#[test]
fn gate_fails_on_a_drifted_synced_skill() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    sync_skills(dir.path()).success();

    fs::write(
        dir.path().join(".agents/skills/council/SKILL.md"),
        "drifted content",
    )
    .unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("synced skill `council` has drifted"))
        .stderr(str::contains("sync-skills"));
}

/// A repository with no `.agents/` directory is not failed for having no
/// skills. It has not adopted agent tooling, and the catalog is a statement
/// about what an agent working here must be told — which is nothing at all if
/// no agent works here.
#[test]
fn gate_passes_when_no_skills_have_been_synced() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    assert!(
        !dir.path().join(".agents").exists(),
        "scaffold must not create `.agents/`, or this asserts the wrong branch"
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
fn gate_fails_when_claude_exists_without_the_catalog() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join(".agents")).unwrap();

    gate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("missing synced skill `portal-chrome`"))
        .stderr(str::contains("missing synced skill `server`"))
        .stderr(str::contains("sync-skills"));
}

/// And syncing is the fix, not an exemption list: the same repository passes
/// once `sync-skills` has run. A check whose only remedy is deleting the
/// directory that triggered it would just teach people to delete it.
#[test]
fn gate_passes_when_claude_exists_and_the_catalog_is_synced() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join(".agents")).unwrap();
    sync_skills(dir.path()).success();

    gate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}
