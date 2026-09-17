//! `navigator project build` — install, lint, typecheck, test, and
//! build every application a Project repository declares.
//!
//! `project-gate.yml` and `project-publish.yml` used to reimplement this
//! detection in bash, once per reusable workflow, reading `application_workspaces`'s
//! comment for the order rather than its code. This verb calls
//! [`repository::discovered_applications`] directly, so the CLI is the one
//! place that decides what an application is; the workflows just run it.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, ExitCode};

use super::repository;

const VERBS: [&str; 5] = ["install", "lint", "typecheck", "test", "build"];

/// Run `pnpm --dir <app> <verb>` for every discovered application, one
/// application at a time, in discovery order, stopping at the first
/// failure. A repository with no application prints a notice and succeeds —
/// the gate still passes, it simply has nothing to build.
pub fn run(dir: &Path) -> ExitCode {
    run_with_pnpm_on(dir, None)
}

/// Same as [`run`], but with `pnpm_dir` prepended to the child processes'
/// `PATH` when given — how tests point this at a stub `pnpm` without
/// touching the real process environment.
fn run_with_pnpm_on(dir: &Path, pnpm_dir: Option<&OsStr>) -> ExitCode {
    let applications = repository::discovered_applications(dir);
    if applications.is_empty() {
        println!("no application — nothing to build");
        return ExitCode::SUCCESS;
    }
    let path = pnpm_dir.map(|extra| {
        let mut entries = vec![extra.to_os_string()];
        if let Some(existing) = std::env::var_os("PATH") {
            entries
                .extend(std::env::split_paths(&existing).map(std::path::PathBuf::into_os_string));
        }
        std::env::join_paths(entries).expect("PATH entries are valid")
    });
    for application in &applications {
        let app_dir = application.to_string_lossy().into_owned();
        for verb in VERBS {
            let mut args: Vec<&str> = vec!["--dir", &app_dir, verb];
            if verb == "install" {
                args.push("--frozen-lockfile");
            }
            println!("pnpm {}", args.join(" "));
            let mut command = Command::new("pnpm");
            command.args(&args);
            if let Some(path) = &path {
                command.env("PATH", path);
            }
            match command.status() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    eprintln!("navigator: `pnpm {}` failed ({status})", args.join(" "));
                    return ExitCode::FAILURE;
                }
                Err(error) => {
                    eprintln!(
                        "navigator: could not run `pnpm {}`: {error}",
                        args.join(" ")
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Writes a fake `pnpm` onto `$PATH` that appends every invocation it
    /// receives, space-joined, as one line of `log`, and exits with `code`.
    fn stub_pnpm(bin_dir: &Path, log: &Path, code: i32) {
        fs::create_dir_all(bin_dir).unwrap();
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> {log}\nexit {code}\n",
            log = log.display(),
        );
        let path = bin_dir.join("pnpm");
        fs::File::create(&path)
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn no_application_succeeds_without_running_pnpm() {
        let root = tempfile::tempdir().unwrap();
        let status = run(root.path());
        assert_eq!(status, ExitCode::SUCCESS);
    }

    #[test]
    fn legacy_portal_runs_every_verb_in_order() {
        let root = tempfile::tempdir().unwrap();
        let portal = root.path().join("portal");
        fs::create_dir_all(&portal).unwrap();
        fs::write(portal.join("package.json"), "{}").unwrap();
        fs::write(portal.join("vite.config.ts"), "").unwrap();
        fs::write(portal.join("tsconfig.json"), "{}").unwrap();
        fs::write(portal.join("vitest.config.ts"), "").unwrap();

        let scratch = tempfile::tempdir().unwrap();
        let bin_dir = scratch.path().join("bin");
        let log = scratch.path().join("log");
        stub_pnpm(&bin_dir, &log, 0);

        let status = run_with_pnpm_on(root.path(), Some(bin_dir.as_os_str()));
        assert_eq!(status, ExitCode::SUCCESS);

        let recorded = fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();
        let app_dir = portal.to_string_lossy().into_owned();
        assert_eq!(
            lines,
            vec![
                format!("--dir {app_dir} install --frozen-lockfile"),
                format!("--dir {app_dir} lint"),
                format!("--dir {app_dir} typecheck"),
                format!("--dir {app_dir} test"),
                format!("--dir {app_dir} build"),
            ]
        );
    }

    #[test]
    fn nested_application_is_discovered_under_apps() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("apps").join("widgets");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), "{}").unwrap();
        fs::write(app.join("vite.config.ts"), "").unwrap();
        fs::write(app.join("tsconfig.json"), "{}").unwrap();
        fs::write(app.join("vitest.config.ts"), "").unwrap();

        let scratch = tempfile::tempdir().unwrap();
        let bin_dir = scratch.path().join("bin");
        let log = scratch.path().join("log");
        stub_pnpm(&bin_dir, &log, 0);

        let status = run_with_pnpm_on(root.path(), Some(bin_dir.as_os_str()));
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(fs::read_to_string(&log).unwrap().contains("widgets"));
    }

    #[test]
    fn root_vite_workspace_is_discovered() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("package.json"), "{}").unwrap();
        fs::write(root.path().join("vite.config.ts"), "").unwrap();
        fs::write(root.path().join("tsconfig.json"), "{}").unwrap();
        fs::write(root.path().join("vitest.config.ts"), "").unwrap();

        let scratch = tempfile::tempdir().unwrap();
        let bin_dir = scratch.path().join("bin");
        let log = scratch.path().join("log");
        stub_pnpm(&bin_dir, &log, 0);

        let status = run_with_pnpm_on(root.path(), Some(bin_dir.as_os_str()));
        assert_eq!(status, ExitCode::SUCCESS);
        let app_dir = root.path().to_string_lossy().into_owned();
        assert!(fs::read_to_string(&log)
            .unwrap()
            .contains(&format!("--dir {app_dir} install --frozen-lockfile")));
    }

    #[test]
    fn a_failing_verb_stops_the_run_and_fails() {
        let root = tempfile::tempdir().unwrap();
        let portal = root.path().join("portal");
        fs::create_dir_all(&portal).unwrap();
        fs::write(portal.join("package.json"), "{}").unwrap();
        fs::write(portal.join("vite.config.ts"), "").unwrap();
        fs::write(portal.join("tsconfig.json"), "{}").unwrap();
        fs::write(portal.join("vitest.config.ts"), "").unwrap();

        let scratch = tempfile::tempdir().unwrap();
        let bin_dir = scratch.path().join("bin");
        let log = scratch.path().join("log");
        stub_pnpm(&bin_dir, &log, 1);

        let status = run_with_pnpm_on(root.path(), Some(bin_dir.as_os_str()));
        assert_eq!(status, ExitCode::FAILURE);

        // Only the first verb (`install`) ran before the failure stopped the
        // loop; `lint`, `typecheck`, `test`, and `build` never executed.
        let recorded = fs::read_to_string(&log).unwrap();
        assert_eq!(recorded.lines().count(), 1);
    }
}
