//! End-to-end tests for the one Project repository scaffold and validator.
//!
//! One repository per Project code, holding notation templates under
//! `templates/` and application source under `apps/<app>/`. There is one
//! scaffold and one validator for both, and the validator takes the Project
//! code from the repository name. A legacy root `portal/` remains valid during
//! the source-layout transition. A repository may also carry a root manifest
//! declaring that code — the layout admits one — but the scaffold does not
//! write it and these tests do not depend on it.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Command as ProcessCommand;

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
        .args([
            "site",
            "projects",
            "repository",
            "scaffold",
            project_code,
            "--dir",
        ])
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
fn the_reusable_gate_asks_the_deployment_and_arms_auto_merge() {
    let source = project_gate_source();
    assert!(source.contains("navigator site projects gate --ci"));
    assert!(source.contains("enable-automerge:"));
    assert!(source.contains("needs: [lint, verify, notation, documents, manifest]"));
}

#[test]
fn the_project_gate_keeps_manifest_validation_offline_on_prs() {
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&project_gate_source()).expect("project gate parses as YAML");
    let manifest_steps = workflow["jobs"]["manifest"]["steps"]
        .as_sequence()
        .expect("manifest steps");
    let live_status = manifest_steps
        .iter()
        .find(|step| {
            step["run"]
                .as_str()
                .is_some_and(|run| run.contains("navigator site projects gate --ci"))
        })
        .expect("manifest live status step");
    let run = live_status["run"].as_str().expect("live status script");

    assert!(
        live_status["env"]["EVENT_NAME"].as_str() == Some("${{ github.event_name }}")
            && live_status["env"]["REF"].as_str() == Some("${{ github.ref }}"),
        "manifest live status must know which event and ref it is running for"
    );
    assert!(
        run.contains(r#"[ "${EVENT_NAME}" = "push" ] && [ "${REF}" = "refs/heads/main" ]"#),
        "manifest live status must be limited to pushes to main"
    );
    assert!(
        run.contains("navigator site projects gate\n")
            || run.contains("navigator site projects gate\r\n"),
        "manifest must retain an offline gate for pull requests"
    );
}

/// The `seeds` job reconciles `seeds/` on a push to `main`, is offline on a
/// pull request (`navigator validate` already covers the shape), never
/// overwrites, no-ops cleanly with no `seeds/` directory, and stays outside
/// the required `ci` job's dependencies — its live half needs a reachable
/// deployment, and the always-required check must never depend on that.
#[test]
fn the_reusable_gate_reconciles_seeds_on_push_to_main_only() {
    let source = project_gate_source();
    assert!(source.contains("  seeds:"));
    assert!(source.contains("no seeds — nothing to reconcile"));
    assert!(source.contains(
        r#"if [ -n "${HOST}" ] && [ "${EVENT_NAME}" = "push" ] && [ "${REF}" = "refs/heads/main" ]; then"#
    ));
    assert!(source.contains(r#"navigator site import --ci --host "${HOST}" --dir seeds"#));
    assert!(source.contains("needs: [lint, verify, notation, documents, manifest]"));
    assert!(!source.contains("needs: [lint, verify, notation, documents, manifest, seeds]"));
}

fn validate(dir: &Path) -> assert_cmd::assert::Assert {
    navigator().args(["validate"]).arg(dir).assert()
}

fn validate_as(dir: &Path, repository: &str) -> assert_cmd::assert::Assert {
    navigator()
        .args(["validate"])
        .arg(dir)
        .env("GITHUB_REPOSITORY", format!("org/{repository}"))
        .assert()
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

#[cfg(unix)]
fn generated_step_script(step_name: &str) -> String {
    let source = project_gate_source();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&source).unwrap();
    workflow
        .get("jobs")
        .and_then(|jobs| jobs.get("verify"))
        .and_then(|job| job.get("steps"))
        .and_then(serde_yaml::Value::as_sequence)
        .unwrap()
        .iter()
        .find(|step| step.get("name").and_then(serde_yaml::Value::as_str) == Some(step_name))
        .and_then(|step| step.get("run"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap()
        .to_string()
}

#[test]
fn the_scaffold_produces_a_repository_that_validates_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    validate(dir.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s), 0 error(s)"));

    assert!(dir.path().join("README.md").is_file());
    assert!(dir.path().join("AGENTS.md").is_file());
    assert!(dir.path().join("CLAUDE.md").is_file());
    let instructions = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
    assert!(instructions.contains("`apps/<app>/`"));
    assert!(instructions.contains("source grouping is not a URL segment"));
    assert!(instructions.contains("root `portal/` is also"));
    assert!(instructions.contains("hyphens become `_`) then `__name`"));
    assert!(instructions.contains("`code:` matches"));
    assert!(instructions.contains("A Project code names a matter and its repository."));
    assert!(instructions.contains("It identifies a client, so it is client data."));
    assert!(instructions.contains("The one legitimate use here is this repository naming itself"));
    assert!(
        instructions.contains("commit message, code comment, branch name, or pull-request body")
    );
    assert!(instructions.contains("A precedent"));
    assert!(instructions.contains("citation is still a breach"));
    assert!(dir
        .path()
        .join("templates/example_project__engagement.md")
        .is_file());
    assert!(!dir.path().join("templates/project_template.md").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitattributes")).unwrap(),
        "* text=auto eol=lf\n"
    );
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/ci.yml")).unwrap();
    assert!(workflow.contains("project-gate.yml@"));
    assert!(workflow.contains("on:\n  pull_request:\n  push:\n    branches: [main]"));
    assert!(!workflow.contains("project_repository: true"));
    let workflow_yaml: serde_yaml::Value =
        serde_yaml::from_str(&workflow).expect("scaffolded ci.yml parses as YAML");
    for (permission, expected) in [
        ("contents", "write"),
        ("id-token", "write"),
        ("pull-requests", "write"),
    ] {
        assert_eq!(
            workflow_yaml["permissions"][permission].as_str(),
            Some(expected),
            "scaffolded caller must grant {permission}: {expected}"
        );
    }
    let cd = fs::read_to_string(dir.path().join(".github/workflows/publish.yml")).unwrap();
    assert!(
        !cd.contains("TBD"),
        "the publish workflow is still a placeholder:\n{cd}"
    );
    assert!(cd.contains("id-token: write"));
    assert!(
        cd.contains("neon-law-source-code/navigator/.github/actions/application-publish@26.8.23")
    );
    assert!(cd.contains("secrets.NAVIGATOR_APPLICATIONS_BUCKET"));

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
    validate(dir.path()).success();
}

#[test]
fn gate_ci_without_oidc_is_a_closed_door() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    navigator()
        .args(["site", "projects", "gate", "--ci"])
        .arg(dir.path())
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("ACTIONS_ID_TOKEN_REQUEST_URL is unset"));
}

#[test]
fn gate_ci_without_host_is_a_closed_door() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    navigator()
        .args(["site", "projects", "gate", "--ci"])
        .arg(dir.path())
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", "http://127.0.0.1/oidc")
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "token")
        .assert()
        .failure()
        .code(2)
        .stderr(str::contains("--ci requires --host"));
}

/// All three shapes validate: templates only, a portal only, and both.
#[test]
fn templates_only_a_portal_only_and_both_all_validate() {
    // Templates only — what the scaffold produces.
    let templates_only = TempDir::new().unwrap();
    scaffold(templates_only.path(), "example-project").success();
    validate(templates_only.path())
        .success()
        .stdout(str::contains("1 template(s), 0 application(s)"));

    // Both halves in one repository, which is the point of the collapse.
    let both = TempDir::new().unwrap();
    scaffold(both.path(), "example-project").success();
    write_portal(both.path());
    validate(both.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));

    // Scaffold now writes one placeholder template; a portal is extra.
    let portal_only = TempDir::new().unwrap();
    scaffold(portal_only.path(), "example-project").success();
    write_portal(portal_only.path());
    validate(portal_only.path())
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));
}

#[test]
fn a_nested_template_is_refused_in_a_project_repository() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme").success();
    let flat = dir.path().join("templates/acme__engagement.md");
    let nested = dir.path().join("templates/neon_law/acme__engagement.md");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    fs::rename(&flat, &nested).unwrap();
    validate(dir.path())
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

#[test]
fn a_template_filename_must_use_the_project_code_prefix() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme").success();
    fs::rename(
        dir.path().join("templates/acme__engagement.md"),
        dir.path().join("templates/project_template.md"),
    )
    .unwrap();
    validate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("acme__"))
        .stderr(str::contains("project_template"));
}

#[test]
fn a_template_code_must_equal_the_filename_stem() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "acme").success();
    let path = dir.path().join("templates/acme__engagement.md");
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("code: acme__engagement", "code: other__engagement");
    fs::write(&path, body).unwrap();
    validate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("acme__"))
        .stderr(str::contains("other__engagement"));
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

    validate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));

    fs::remove_file(dir.path().join("apps/exchange/index.html")).unwrap();
    validate(dir.path())
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
    validate(dir.path())
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

    validate(dir.path())
        .success()
        .stdout(str::contains("2 application(s)"));
}

#[test]
fn the_legacy_and_new_portal_locations_cannot_claim_the_same_route() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/portal");

    validate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("apps/portal"))
        .stderr(str::contains("claim the same application route"));
}

/// `CLAUDE.md` must deliver the bytes of `AGENTS.md` on every platform.
///
/// Stated in resolved content rather than link type, the way
/// `cli/tests/agent_instruction_links.rs` states it for Navigator's own tree:
/// on Unix the scaffold writes a relative symlink, on Windows a copy, and a
/// harness reading either sees the same contract. The release archive for
/// Windows compiles this path, so this is also the test that runs it.
#[test]
fn the_scaffold_makes_claude_md_deliver_the_agents_contract() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project")
        .success()
        .stdout(str::contains("CLAUDE.md ("));

    let agents = fs::read(dir.path().join("AGENTS.md")).unwrap();
    let claude = fs::read(dir.path().join("CLAUDE.md")).unwrap();
    assert!(!agents.is_empty());
    assert_eq!(claude, agents, "CLAUDE.md does not resolve to AGENTS.md");
}

/// The contract `CLAUDE.md` delivers is the `AGENTS.md` on disk, not the
/// template: `scaffold` leaves an existing `AGENTS.md` alone, and a symlink
/// resolves to that file, so the copy written where links are unavailable
/// must be taken from it too or the two platforms diverge silently.
#[test]
fn the_scaffold_links_claude_md_to_an_existing_agents_md() {
    let dir = TempDir::new().unwrap();
    let hand_written = "# A contract this repository already had
";
    fs::write(dir.path().join("AGENTS.md"), hand_written).unwrap();

    scaffold(dir.path(), "example-project")
        .success()
        .stdout(str::contains("AGENTS.md (left alone)"));

    let claude = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
    assert_eq!(claude, hand_written);
}

/// Execute the generated build step rather than only looking for a glob in
/// its source: every direct app and the compatibility root portal must reach
/// pnpm exactly once.
#[cfg(unix)]
#[test]
fn the_generated_build_step_runs_every_discovered_application() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_vite_workspace(dir.path(), "apps/intake");
    write_vite_workspace(dir.path(), "apps/exchange");
    write_portal(dir.path());

    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = dir.path().join("pnpm.log");
    let pnpm = bin.join("pnpm");
    fs::write(
        &pnpm,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"{}\"\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&pnpm, fs::Permissions::from_mode(0o755)).unwrap();

    let script = dir.path().join("build.sh");
    fs::write(&script, generated_step_script("Build applications")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = ProcessCommand::new("bash")
        .arg(script)
        .current_dir(dir.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
        [
            "--dir apps/exchange build",
            "--dir apps/intake build",
            "--dir portal build",
        ]
    );
}

#[test]
fn an_application_directory_name_must_be_a_route_safe_slug() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_vite_workspace(dir.path(), "apps/Client_Exchange");

    validate(dir.path())
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

    validate(dir.path())
        .success()
        .stdout(str::contains("carries neither"));
}

/// The repository name is the Project code, so a name that could not be one is
/// the error — there is no manifest to disagree with.
#[test]
fn a_repository_name_that_is_not_a_valid_project_code_is_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    validate_as(dir.path(), "Not_A_Code")
        .failure()
        .code(1)
        .stderr(str::contains("is not a valid Navigator Project code"));

    // `new` is well-formed and still refused: `/app/projects/new` is
    // Navigator's matter-open form.
    validate_as(dir.path(), "new")
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

    validate(dir.path())
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
    fs::write(dir.path().join(".env.production"), "SECRET=x").unwrap();

    validate(dir.path())
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
        "*\n!*/\n!*.yml\n!.gitignore\n",
    )
    .unwrap();

    validate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));

    let binary = dir
        .path()
        .join("documents/exhibits/2026-09-05/screenshot.png");
    fs::write(&binary, b"synthetic image bytes").unwrap();
    validate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains(binary.display().to_string()))
        .stderr(str::contains(
            "legal documents and raw document bytes must not be committed",
        ));
}

#[test]
fn gate_ignores_raw_document_bytes_materialised_by_a_pull() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yml\n!.gitignore\n",
    )
    .unwrap();
    let raw = dir.path().join("documents/memos/agreement.md");
    fs::write(&raw, "synthetic pulled bytes\n").unwrap();

    navigator()
        .args(["site", "projects", "gate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("0 error(s)"))
        .stderr(predicates::str::is_empty())
        .stderr(predicates::str::contains(raw.display().to_string()).not());
}

#[test]
fn gate_reports_a_tracked_raw_document_byte() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join("documents/memos")).unwrap();
    fs::write(
        dir.path().join("documents/.gitignore"),
        "*\n!*/\n!*.yml\n!.gitignore\n",
    )
    .unwrap();
    let raw = dir.path().join("documents/memos/agreement.md");
    fs::write(&raw, "synthetic tracked bytes\n").unwrap();
    run_git(dir.path(), &["add", "-f", "documents/memos/agreement.md"]);

    navigator()
        .args(["site", "projects", "gate"])
        .arg(dir.path())
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
fn gate_honours_a_nested_ignore_file() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::write(dir.path().join(".github/.gitignore"), "*.env\n").unwrap();
    let ignored = dir.path().join(".github/hidden.env");
    fs::write(&ignored, "synthetic secret\n").unwrap();

    navigator()
        .args(["site", "projects", "gate"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(str::contains("0 error(s)"))
        .stderr(predicates::str::is_empty())
        .stderr(predicates::str::contains(ignored.display().to_string()).not());
}

#[test]
fn gate_fails_clearly_when_the_directory_is_not_a_git_repository() {
    let dir = TempDir::new().unwrap();
    navigator()
        .args(["site", "projects", "repository", "scaffold"])
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
        .args(["site", "projects", "gate"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stderr(str::contains(
            "could not enumerate git-tracked and stageable files",
        ))
        .stderr(str::contains("not a Git repository"));
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
        .args(["site", "projects", "repository", "sync-skills"])
        .arg(dir)
        .assert()
}

/// ENG-383: a Project repository can carry `.claude/skills/` without the
/// layout gate refusing it as an unexpected root, and `sync-skills` is what
/// populates it from Navigator's own compiled-in copies.
#[test]
fn sync_skills_writes_the_canonical_catalog_and_validate_accepts_it() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    sync_skills(dir.path())
        .success()
        .stdout(str::contains("synced"));

    for skill in ["council", "legal-council", "client-council", "stay-in-repo"] {
        let path = dir
            .path()
            .join(".claude/skills")
            .join(skill)
            .join("SKILL.md");
        assert!(path.is_file(), "expected {} to exist", path.display());
        assert!(!fs::read_to_string(&path).unwrap().is_empty());
    }

    validate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
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

    let path = dir.path().join(".claude/skills/council/SKILL.md");
    let canonical = fs::read_to_string(&path).unwrap();
    fs::write(&path, "hand-edited drift").unwrap();

    sync_skills(dir.path()).success();
    assert_eq!(fs::read_to_string(&path).unwrap(), canonical);
}

/// A synced skill that has drifted from the canonical copy fails `validate`
/// and names the file, so a hand edit or a stale sync is caught rather than
/// silently diverging across 19 repositories.
#[test]
fn validate_fails_on_a_drifted_synced_skill() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    sync_skills(dir.path()).success();

    fs::write(
        dir.path().join(".claude/skills/council/SKILL.md"),
        "drifted content",
    )
    .unwrap();

    validate(dir.path())
        .failure()
        .code(1)
        .stderr(str::contains("synced skill `council` has drifted"))
        .stderr(str::contains("sync-skills"));
}

/// A repository with no `.claude/` directory is not failed for having no
/// skills. It has not adopted agent tooling, and the catalog is a statement
/// about what an agent working here must be told — which is nothing at all if
/// no agent works here.
#[test]
fn validate_passes_when_no_skills_have_been_synced() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    assert!(
        !dir.path().join(".claude").exists(),
        "scaffold must not create `.claude/`, or this asserts the wrong branch"
    );

    validate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}

/// `.claude/` is the opt-in, and it opts into the whole catalog.
///
/// The moment a repository has one, an agent is working in it under whatever
/// skills it happens to find, and the ones it does not find are precisely the
/// conventions nobody told it about. `portal-chrome` is why this is a finding
/// rather than a suggestion: "the portal wears the library's teal and never
/// repaints it" lived for months as a header comment inside the very file
/// that violated it, in sixteen repositories, claiming a fleet-wide
/// uniformity that had already broken in two directions.
#[test]
fn validate_fails_when_claude_exists_without_the_catalog() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join(".claude")).unwrap();

    validate(dir.path())
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
fn validate_passes_when_claude_exists_and_the_catalog_is_synced() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::create_dir_all(dir.path().join(".claude")).unwrap();
    sync_skills(dir.path()).success();

    validate(dir.path())
        .success()
        .stdout(str::contains("0 error(s)"));
}
