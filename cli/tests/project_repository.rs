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
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use predicates::str;
use tempfile::TempDir;

fn navigator() -> Command {
    Command::cargo_bin("navigator").unwrap()
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
    navigator()
        .args([
            "site",
            "projects",
            "repository",
            "scaffold",
            project_code,
            "--dir",
        ])
        .arg(dir)
        .args(["--action-version", FIXTURE_PIN])
        .assert()
}

fn validate(dir: &Path, repository: &str) -> assert_cmd::assert::Assert {
    navigator()
        .args(["site", "projects", "repository", "validate"])
        .arg(dir)
        .args(["--repository", repository])
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
fn generated_step_script(root: &Path, step_name: &str) -> String {
    let source = fs::read_to_string(root.join(".github/workflows/gate.yml")).unwrap();
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

    validate(dir.path(), "example-project")
        .success()
        .stdout(str::contains("1 template(s), 0 application(s), 0 error(s)"));

    assert!(dir.path().join("README.md").is_file());
    assert!(dir.path().join("AGENTS.md").is_file());
    assert!(dir.path().join("CLAUDE.md").is_file());
    let instructions = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
    assert!(instructions.contains("`apps/<app>/`"));
    assert!(instructions.contains("source grouping is not a URL segment"));
    assert!(instructions.contains("root `portal/` is also"));
    assert!(instructions.contains("A Project code names a matter and its repository."));
    assert!(instructions.contains("It identifies a client, so it is client"));
    assert!(
        instructions.contains("data. The one legitimate use here is this repository naming itself")
    );
    assert!(
        instructions.contains("commit message, code comment, branch name, or pull-request body")
    );
    assert!(instructions.contains("A precedent"));
    assert!(instructions.contains("citation is still a breach"));
    assert!(dir.path().join("templates/project_template.md").is_file());
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/gate.yml")).unwrap();
    assert!(workflow.contains("project_repository: true"));
    assert!(workflow.contains("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"));
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

    // Idempotent: a second run leaves every file alone and still validates.
    scaffold(dir.path(), "example-project")
        .success()
        .stdout(str::contains("left alone"));
    validate(dir.path(), "example-project").success();
}

/// All three shapes validate: templates only, a portal only, and both.
#[test]
fn templates_only_a_portal_only_and_both_all_validate() {
    // Templates only — what the scaffold produces.
    let templates_only = TempDir::new().unwrap();
    scaffold(templates_only.path(), "example-project").success();
    validate(templates_only.path(), "example-project")
        .success()
        .stdout(str::contains("1 template(s), 0 application(s)"));

    // Both halves in one repository, which is the point of the collapse.
    let both = TempDir::new().unwrap();
    scaffold(both.path(), "example-project").success();
    write_portal(both.path());
    validate(both.path(), "example-project")
        .success()
        .stdout(str::contains("1 template(s), 1 application(s)"));

    // A portal only: no `templates/` at all.
    let portal_only = TempDir::new().unwrap();
    scaffold(portal_only.path(), "example-project").success();
    fs::remove_dir_all(portal_only.path().join("templates")).unwrap();
    write_portal(portal_only.path());
    validate(portal_only.path(), "example-project")
        .success()
        .stdout(str::contains("0 template(s), 1 application(s)"));
}

/// Direct `apps/<app>/package.json` files are the app declarations. Every
/// discovered directory is validated as its own Vite workspace, while a
/// directory under `apps/` with no package manifest is ordinary source rather
/// than a second declaration the repository has to keep synchronized.
#[test]
fn direct_apps_are_discovered_and_each_is_validated() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    fs::remove_dir_all(dir.path().join("templates")).unwrap();
    write_vite_workspace(dir.path(), "apps/portal");
    write_vite_workspace(dir.path(), "apps/exchange");
    fs::create_dir_all(dir.path().join("apps/shared")).unwrap();
    fs::write(dir.path().join("apps/shared/routes.ts"), "export {};\n").unwrap();

    validate(dir.path(), "example-project")
        .success()
        .stdout(str::contains("2 application(s)"));

    fs::remove_file(dir.path().join("apps/exchange/index.html")).unwrap();
    validate(dir.path(), "example-project")
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
    validate(dir.path(), "example-project")
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
    fs::remove_dir_all(dir.path().join("templates")).unwrap();
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/exchange");

    validate(dir.path(), "example-project")
        .success()
        .stdout(str::contains("2 application(s)"));
}

#[test]
fn the_legacy_and_new_portal_locations_cannot_claim_the_same_route() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();
    write_portal(dir.path());
    write_vite_workspace(dir.path(), "apps/portal");

    validate(dir.path(), "example-project")
        .failure()
        .code(1)
        .stderr(str::contains("apps/portal"))
        .stderr(str::contains("claim the same application route"));
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
    fs::write(
        &script,
        generated_step_script(dir.path(), "Build applications"),
    )
    .unwrap();
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

    validate(dir.path(), "example-project")
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
    fs::remove_dir_all(dir.path().join("templates")).unwrap();

    validate(dir.path(), "example-project")
        .success()
        .stdout(str::contains("carries neither"));
}

/// The repository name is the Project code, so a name that could not be one is
/// the error — there is no manifest to disagree with.
#[test]
fn a_repository_name_that_is_not_a_valid_project_code_is_refused() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    validate(dir.path(), "Not_A_Code")
        .failure()
        .code(1)
        .stderr(str::contains("is not a valid Navigator Project code"));

    // `new` is well-formed and still refused: `/app/projects/new` is
    // Navigator's matter-open form.
    validate(dir.path(), "new")
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

    validate(dir.path(), "example-project")
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

    validate(dir.path(), "example-project")
        .failure()
        .code(1)
        .stderr(str::contains("forbidden `uploads` path"))
        .stderr(str::contains("forbidden `target` path"))
        .stderr(str::contains("must not be committed"));
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

    validate(dir.path(), "example-project")
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

    validate(dir.path(), "example-project")
        .failure()
        .code(1)
        .stderr(str::contains("synced skill `council` has drifted"))
        .stderr(str::contains("sync-skills"));
}

/// A repository that has never synced skills at all is not failed for it:
/// syncing is opt-in per repository, the same policy `templates/`/`portal/`
/// get.
#[test]
fn validate_passes_when_no_skills_have_been_synced() {
    let dir = TempDir::new().unwrap();
    scaffold(dir.path(), "example-project").success();

    validate(dir.path(), "example-project")
        .success()
        .stdout(str::contains("0 error(s)"));
}
