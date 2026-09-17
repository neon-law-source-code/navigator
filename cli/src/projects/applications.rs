//! `navigator project applications` — the same application discovery
//! [`super::build`] runs, exposed for a workflow step that needs to know
//! before it runs anything.
//!
//! `pnpm/action-setup`'s `package_json_file` input takes one concrete path,
//! which it reads `packageManager` from — a workflow step used to rebuild
//! that candidate list itself, in an order that had already drifted from
//! [`super::repository::discovered_applications`]'s own. `--manifest` prints
//! exactly the path that function would build first, so there is one
//! resolver instead of two.

use std::path::Path;
use std::process::ExitCode;

use super::repository;

/// With `--manifest`, print the first discovered application's
/// `package.json` path and nothing else — empty output when the repository
/// declares none. Without it, list every discovered application's directory,
/// one per line, in the same discovery order `build` runs them in. Always
/// exits successfully: a repository with no application is a valid shape,
/// not an error this command reports on.
pub fn run(dir: &Path, manifest: bool) -> ExitCode {
    let applications = repository::discovered_applications(dir);
    if manifest {
        if let Some(path) = applications
            .first()
            .map(|application| application.join("package.json"))
        {
            println!("{}", path.display());
        }
    } else {
        for application in &applications {
            println!("{}", application.display());
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn no_application_prints_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(run(root.path(), true), ExitCode::SUCCESS);
        assert_eq!(run(root.path(), false), ExitCode::SUCCESS);
    }

    #[test]
    fn legacy_portal_is_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let portal = root.path().join("portal");
        fs::create_dir_all(&portal).unwrap();
        fs::write(portal.join("package.json"), "{}").unwrap();

        let applications = repository::discovered_applications(root.path());
        assert_eq!(applications, vec![portal.clone()]);
        assert_eq!(run(root.path(), true), ExitCode::SUCCESS);
    }

    #[test]
    fn root_vite_workspace_is_the_manifest_when_no_portal_exists() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("package.json"), "{}").unwrap();
        fs::write(root.path().join("vite.config.ts"), "").unwrap();

        let applications = repository::discovered_applications(root.path());
        assert_eq!(applications, vec![root.path().to_path_buf()]);
        assert_eq!(run(root.path(), true), ExitCode::SUCCESS);
    }

    #[test]
    fn nested_application_lists_under_apps() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("apps").join("widgets");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("package.json"), "{}").unwrap();

        let applications = repository::discovered_applications(root.path());
        assert_eq!(applications, vec![app]);
        assert_eq!(run(root.path(), false), ExitCode::SUCCESS);
    }
}
