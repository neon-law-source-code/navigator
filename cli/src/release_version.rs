//! `navigator ops release version` — write the release version the operator
//! names into `[workspace.package].version`, which is the act that cuts a
//! release.
//!
//! THIS COMMAND IS THE RELEASE TRIGGER, one step removed. `deploy.yml` runs
//! `ops release check` on every push to `main`; when the workspace version is
//! newer than every version already tagged, that push builds, proves, tags, and
//! publishes. So bumping this value and landing it is the whole of cutting a
//! release — there is no tag to push afterwards, and no clock decides anything.
//!
//! IT DERIVES NOTHING. `--tag` is required and its value is written verbatim
//! once it parses. Naming a release is an operator decision, and a command that
//! guessed made the name a side effect of when it happened to run.
//!
//! The shape it accepts is semver and nothing narrower — see [`crate::release`]
//! for why the calendar is a convention rather than a rule. `YY.M.D` remains
//! that convention, and the `cut-release` skill is where it is written down; a
//! version that departs from it publishes just as well, provided it is newer
//! than the last one.
//!
//! Every crate inherits this value through `version.workspace = true` and
//! `cli/build.rs` bakes it into `navigator --version`, so a plain build of the
//! released source reports the release it was cut from — which used to stay
//! `0.1.0` forever while tags marched on. Because `deploy.yml` now derives the
//! tag FROM this value rather than comparing a pushed tag against it, that drift
//! is no longer a check that can fail: the tag and the manifest are one
//! decision.
//!
//! It writes `Cargo.lock` alongside the manifest, because the release builds
//! with `--locked` and that flag refuses a lock whose versions the manifest has
//! moved past.
//!
//! It never pushes to `main` itself: `main` is squash-merge-only and no ref may
//! be moved by automation (`docs/gitops.md` → "`main` is sacred"). The bump goes
//! through the ordinary PR flow like any other change.

use std::path::Path;
use std::process::ExitCode;

/// Refuse a version a release could not carry, before a byte is written.
///
/// The grammar lives in [`crate::release`] and is semver's own — this is a thin
/// delegation on purpose. It used to be ninety lines of component checking
/// transcribed from `deploy.yml`'s `grep -E`, which is two copies of one rule
/// and therefore two chances to disagree with the parser that ultimately
/// decides. `Version::parse` refuses everything that grammar refused (a fourth
/// component, a padded month, a padded prerelease number) and refuses it against
/// the specification.
///
/// IT DOES NOT READ THE CLOCK, and no longer needs to: the release is cut by
/// merging this bump, so the version is written days before it publishes.
/// Whether it is newer than everything already released is [`crate::release_check`]'s
/// question, asked against the tags rather than against a calendar.
fn validate_release_version(version: &str) -> Result<(), String> {
    crate::release::parse(version)
        .map(|_| ())
        .map_err(|error| format!("{error:#}"))
}

/// Replace the `version` value inside the `[workspace.package]` table only,
/// leaving every dependency's own `version =` untouched. Returns the rewritten
/// manifest, or an error naming why the one line could not be found — a manifest
/// whose shape moved should fail loudly, not silently write nothing.
///
/// Scoped to the one table on purpose: `[workspace.dependencies]` holds dozens
/// of `version =` lines, and a blind find-and-replace would rewrite the first
/// dependency pin instead of the workspace version.
fn set_workspace_version(manifest: &str, version: &str) -> Result<String, String> {
    let mut out = String::with_capacity(manifest.len() + version.len());
    let mut in_package = false;
    let mut replaced = false;

    for line in manifest.lines() {
        let trimmed = line.trim_start();
        // A table header re-scopes every following key until the next header.
        if trimmed.starts_with('[') {
            in_package = trimmed.starts_with("[workspace.package]");
        }

        // Match the `version` KEY, not `rust-version` and not a comment: split
        // on the first `=` and compare the trimmed left side exactly.
        let is_version_key = in_package
            && !replaced
            && line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "version");

        if is_version_key {
            out.push_str("version = \"");
            out.push_str(version);
            out.push_str("\"\n");
            replaced = true;
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    if !replaced {
        return Err(
            "Cargo.toml has no `version` key under `[workspace.package]` — the manifest shape moved"
                .to_string(),
        );
    }
    Ok(out)
}

/// Entry point for `ops release version`.
///
/// Writes the version the operator named, refreshes `Cargo.lock` to match, and
/// unless `no_commit` commits both on the current branch so the operator can
/// push them as a PR. It refuses to commit on `main`: that branch takes no
/// direct commits, so the bump must reach it the same way every change does.
///
/// `version` is required and never defaulted. The shape is checked first, so a
/// name the release would refuse fails here instead of after a tag exists.
pub fn run(manifest_path: &Path, version: &str, no_commit: bool) -> ExitCode {
    let version = version.trim().to_string();
    if version.is_empty() {
        eprintln!("navigator: release version: --tag must not be empty");
        return ExitCode::from(2);
    }
    if let Err(error) = validate_release_version(&version) {
        eprintln!("navigator: release version: {error}");
        return ExitCode::from(2);
    }

    let manifest = match std::fs::read_to_string(manifest_path) {
        Ok(text) => text,
        Err(error) => {
            // Do not interpolate the CLI path: `Command` also carries `Secrets`,
            // and CodeQL treats any printed Command field as cleartext logging.
            eprintln!("navigator: release version: could not read the workspace manifest: {error}");
            return ExitCode::from(2);
        }
    };

    let rewritten = match set_workspace_version(&manifest, &version) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("navigator: release version: {error}");
            return ExitCode::from(2);
        }
    };

    if rewritten == manifest {
        println!(
            "navigator: {} already at version {version}",
            manifest_path.display()
        );
    } else if let Err(error) = std::fs::write(manifest_path, &rewritten) {
        eprintln!("navigator: release version: could not write the workspace manifest: {error}");
        return ExitCode::from(2);
    } else {
        println!(
            "navigator: set [workspace.package] version = {version} in {}",
            manifest_path.display()
        );
    }

    // `Cargo.lock` pins every workspace crate's version too, and `deploy.yml`
    // builds the release with `--locked` — in the release decision itself and in
    // all three CLI archive jobs. `--locked` refuses a lock the manifest has
    // moved past, so writing one file without the other is latently fatal rather
    // than untidy. `ci.yml` catches it on the pull request, which is where it is
    // free; unnoticed, it lands after the release tag exists, and the
    // `release-tags` ruleset admits no bypass actor, so that version is spent. The
    // archive jobs are also what `.github/actions/validate` downloads, so the
    // breakage surfaces as a 404 in every Project repository's CI while nothing
    // here goes red. Refresh the lock in the same breath as the manifest.
    //
    // Unconditionally, not only when the manifest changed: a manifest already at
    // the target version beside a lock that never caught up is exactly the state
    // this repairs.
    let lockfile = manifest_path.with_file_name("Cargo.lock");
    let lock_present = lockfile.exists();
    if lock_present {
        if let Err(error) = refresh_lockfile(manifest_path) {
            eprintln!(
                "navigator: release-version: could not refresh {}: {error}",
                lockfile.display()
            );
            return ExitCode::from(2);
        }
        println!("navigator: refreshed {} to {version}", lockfile.display());
    }

    if no_commit {
        println!("navigator: --no-commit: staged nothing; commit and tag it yourself");
        return ExitCode::SUCCESS;
    }

    commit_bump(&version, lock_present)
}

/// Refresh `Cargo.lock` so every workspace crate's locked version equals the one
/// just written to `[workspace.package]`.
///
/// `cargo update --workspace` is the narrow spelling: it re-resolves the
/// workspace members only, so a release cut can never move a third-party pin as
/// a side effect of writing a date.
fn refresh_lockfile(manifest_path: &Path) -> Result<(), String> {
    // Offline is the honest first attempt: only the members' own version strings
    // moved, and that needs no registry data. A lock stale for some other reason
    // — a dependency added since it was written — does need the index, so fall
    // back to an online resolve rather than failing on the flag.
    match cargo_update(manifest_path, true) {
        Ok(()) => Ok(()),
        Err(offline) => cargo_update(manifest_path, false)
            .map_err(|online| format!("{online} (offline attempt: {offline})")),
    }
}

/// One `cargo update --workspace` invocation, surfacing cargo's own stderr as the
/// error so a failure explains itself.
fn cargo_update(manifest_path: &Path, offline: bool) -> Result<(), String> {
    // `CARGO` is set whenever cargo launched this process, which is how a release
    // runs it; using it pins the nested call to the same toolchain.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command.args(["update", "--workspace", "--quiet"]);
    if offline {
        command.arg("--offline");
    }
    command.arg("--manifest-path").arg(manifest_path);

    match command.output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        Err(error) => Err(format!("could not run `cargo update`: {error}")),
    }
}

/// Commit the bump on the current branch, refusing `main`. The commit carries the
/// manifest and the refreshed lock together; it forms a PR, and the operator
/// merges it and tags the merged commit.
fn commit_bump(version: &str, lock_present: bool) -> ExitCode {
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output();
    if let Ok(output) = &branch {
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "main" {
            eprintln!(
                "navigator: release-version: refusing to commit on `main` — it takes no direct \
                 commits. The version is written; open a branch, commit Cargo.toml, and PR it, \
                 then tag the merged commit {version}."
            );
            return ExitCode::from(2);
        }
    }

    // Both files or neither: the release builds with `--locked`, so a commit
    // carrying the manifest alone names a version its own lock refuses to build.
    let mut paths = vec!["Cargo.toml"];
    if lock_present {
        paths.push("Cargo.lock");
    }
    let staged = std::process::Command::new("git")
        .arg("add")
        .args(&paths)
        .status();
    let committed = staged.is_ok_and(|status| status.success())
        && std::process::Command::new("git")
            .args(["commit", "-m", &format!("chore(release): {version}")])
            .status()
            .is_ok_and(|status| status.success());

    if committed {
        println!(
            "navigator: committed chore(release): {version}. Push it, open a PR, and after it \
             lands on main tag that commit:\n    git tag {version} && git push origin {version}"
        );
        ExitCode::SUCCESS
    } else {
        // Not fatal — the file is written. The operator can commit by hand.
        eprintln!(
            "navigator: release-version: could not create the commit (no git repo, or nothing \
             to commit). Cargo.toml is written; commit it yourself, then tag the merged commit \
             {version}."
        );
        ExitCode::from(2)
    }
}

#[cfg(test)]
mod tests {
    use super::{set_workspace_version, validate_release_version};

    #[test]
    fn read_and_write_failures_do_not_echo_the_cli_manifest_path() {
        let src = include_str!("release_version.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes the test module");
        assert!(
            production.contains("could not read the workspace manifest"),
            "a missing manifest must still explain the failure"
        );
        assert!(
            production.contains("could not write the workspace manifest"),
            "a write failure must still explain the failure"
        );
        assert!(
            !production.contains("release version: read {}"),
            "echoing the CLI manifest path trips CodeQL cleartext-logging because Command also carries Secrets"
        );
        assert!(
            !production.contains("release version: write {}"),
            "echoing the CLI manifest path trips CodeQL cleartext-logging because Command also carries Secrets"
        );
    }

    /// The convention is accepted, and so is every other shape semver admits.
    ///
    /// The grammar is `crate::release`'s and is tested exhaustively there. What
    /// this asserts is that the command DELEGATES to it — a version this
    /// command once refused for departing from `YY.M.D` now writes cleanly,
    /// because the calendar is a convention and the only hard rule is that a
    /// release version is a version.
    #[test]
    fn accepts_any_semver_version_not_only_the_convention() {
        for version in [
            "26.8.5",
            "26.12.25",
            "26.8.18-hotfix.0",
            "26.8.18-hotfix.9999",
            "2026.8.20",
            "1.4.2",
            "26.8.18-rc.1",
        ] {
            assert!(
                validate_release_version(version).is_ok(),
                "`{version}` must be writable into the manifest"
            );
        }
    }

    /// A version the manifest could not carry is refused here, while nothing has
    /// been published and the name is still free to change.
    #[test]
    fn rejects_what_a_release_version_cannot_be() {
        for version in [
            "26.08.20",          // padded month
            "26.8.20.13",        // a fourth component, which Cargo cannot parse
            "26.8.18-hotfix.08", // semver forbids a leading zero here
            "v26.8.20",
            "26.8",
            "26.8.20+build.1", // build metadata does not order
            "",
        ] {
            assert!(
                validate_release_version(version).is_err(),
                "`{version}` must never reach the manifest"
            );
        }
    }

    /// The one line under `[workspace.package]` is rewritten and nothing else.
    #[test]
    fn set_version_rewrites_the_workspace_package_version() {
        let manifest = "[workspace.package]\nversion = \"0.1.0\"\nedition = \"2021\"\n";
        let out = set_workspace_version(manifest, "26.8.14").expect("version present");
        assert!(out.contains("version = \"26.8.14\""));
        assert!(!out.contains("0.1.0"));
        assert!(
            out.contains("edition = \"2021\""),
            "other keys are untouched"
        );
    }

    /// The critical safety property: a dependency's own `version =` is NEVER
    /// touched, even though it appears before `[workspace.package]` and matches
    /// the same key name. A blind replace would pin the wrong thing.
    #[test]
    fn set_version_leaves_dependency_versions_untouched() {
        let manifest = "\
[workspace.dependencies]
serde = { version = \"1\" }
anyhow = \"1\"

[workspace.package]
version = \"0.1.0\"
rust-version = \"1.95\"
";
        let out = set_workspace_version(manifest, "26.8.14").expect("version present");
        assert!(
            out.contains("serde = { version = \"1\" }"),
            "the dependency pin must be preserved verbatim"
        );
        assert!(
            out.contains("version = \"26.8.14\""),
            "the workspace version is bumped"
        );
        assert!(
            out.contains("rust-version = \"1.95\""),
            "rust-version is a different key and must not be mistaken for `version`"
        );
    }

    /// A manifest whose `[workspace.package]` has no `version` fails loudly
    /// rather than writing an unchanged file and reporting success.
    #[test]
    fn set_version_errors_when_the_key_is_absent() {
        let manifest = "[workspace.package]\nedition = \"2021\"\n";
        assert!(set_workspace_version(manifest, "26.8.14").is_err());
    }
}
