//! `navigator ops release version` end-to-end: the command rewrites the one
//! workspace-version line and leaves everything else — dependency pins
//! especially — untouched. `--no-commit` keeps these hermetic: no git repo is
//! required and nothing is committed, so the assertion is purely on the file the
//! command wrote.

use assert_cmd::Command;
use std::fs;

/// A minimal workspace manifest with a dependency `version =` that MUST survive,
/// so a regression that widens the rewrite to the whole file is caught here.
const MANIFEST: &str = "\
[workspace.package]
version = \"0.1.0\"
edition = \"2021\"
license = \"BUSL-1.1\"

[workspace.dependencies]
serde = { version = \"1\" }
";

fn run(args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("navigator").unwrap().args(args).assert()
}

#[test]
fn writes_the_explicit_version_and_preserves_dependency_pins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "26.8.14",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .success();

    let written = fs::read_to_string(&manifest).expect("read manifest");
    assert!(
        written.contains("version = \"26.8.14\""),
        "the workspace version must be bumped"
    );
    assert!(
        !written.contains("0.1.0"),
        "the old workspace version must be gone"
    );
    assert!(
        written.contains("serde = { version = \"1\" }"),
        "a dependency pin must never be rewritten"
    );
}

#[test]
fn an_empty_tag_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "   ",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .failure();

    assert!(
        fs::read_to_string(&manifest)
            .expect("read manifest")
            .contains("0.1.0"),
        "a rejected run must not touch the manifest"
    );
}

/// A hotfix version is written exactly as the operator named it. The command
/// composes nothing: `N` is the operator's discriminator, so a value past 23 is
/// as valid as any other and is written verbatim.
///
/// The base being TOMORROW's date is the operator's call too, and the reason is
/// semver's: a prerelease ranks BELOW its own base, so today's base would sort
/// the fix as older than the release it fixes.
#[test]
fn a_hotfix_version_is_written_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "26.8.18-hotfix.37",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .success();

    let written = fs::read_to_string(&manifest).expect("read manifest");
    assert!(
        written.contains("version = \"26.8.18-hotfix.37\""),
        "the named hotfix version must be written verbatim, got: {written}"
    );
    assert!(
        written.contains("serde = { version = \"1\" }"),
        "a dependency pin must never be rewritten"
    );
}

/// THE VERSION IS REQUIRED. Nothing derives it, so omitting `--tag` is a usage
/// error rather than an invitation to guess today's date — a derived name is only
/// ever a fact about when the command ran.
#[test]
fn omitting_the_tag_is_a_usage_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, MANIFEST).expect("write manifest");

    run(&[
        "ops",
        "release",
        "version",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .failure();

    assert!(
        fs::read_to_string(&manifest)
            .expect("read manifest")
            .contains("0.1.0"),
        "a rejected run must not touch the manifest"
    );
}

/// A version the manifest could not carry is refused before anything is written.
///
/// The shape is semver's — `cli/src/release.rs` — so what is refused here is what
/// Cargo itself cannot parse, plus build metadata. `-rc.1` is deliberately NOT in
/// this list: the prerelease label is the operator's to choose, and the old
/// grammar's insistence on `-hotfix.N` exactly was a rule with nothing behind it.
#[test]
fn a_malformed_version_is_refused_before_the_manifest_is_touched() {
    for bad in [
        "26.08.20",
        "26.8.20.13",
        "26.8.18-hotfix.08",
        "26.8.20+build.1",
        "v26.8.20",
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = dir.path().join("Cargo.toml");
        fs::write(&manifest, MANIFEST).expect("write manifest");

        run(&[
            "ops",
            "release",
            "version",
            "--tag",
            bad,
            "--no-commit",
            "--manifest-path",
            manifest.to_str().unwrap(),
        ])
        .failure();

        assert!(
            fs::read_to_string(&manifest)
                .expect("read manifest")
                .contains("0.1.0"),
            "{bad} must leave the manifest untouched"
        );
    }
}

#[test]
fn a_manifest_without_a_workspace_version_fails_loudly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("Cargo.toml");
    fs::write(&manifest, "[workspace.package]\nedition = \"2021\"\n").expect("write manifest");

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "26.8.14",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .failure();
}

/// A minimal two-crate workspace whose members inherit `version.workspace =
/// true` — the shape that makes `Cargo.lock` go stale the moment only the
/// manifest is written. No external dependencies, so it resolves offline.
fn seed_workspace(root: &std::path::Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("alpha/src")).expect("alpha src");
    fs::create_dir_all(root.join("beta/src")).expect("beta src");
    fs::write(root.join("alpha/src/lib.rs"), "").expect("alpha lib");
    fs::write(root.join("beta/src/lib.rs"), "").expect("beta lib");
    fs::write(
        root.join("alpha/Cargo.toml"),
        "[package]\nname = \"alpha\"\nversion.workspace = true\nedition.workspace = true\n\n\
         [dependencies]\nbeta = { path = \"../beta\" }\n",
    )
    .expect("alpha manifest");
    fs::write(
        root.join("beta/Cargo.toml"),
        "[package]\nname = \"beta\"\nversion.workspace = true\nedition.workspace = true\n",
    )
    .expect("beta manifest");

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"2\"\n\n\
         [workspace.package]\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("workspace manifest");

    // The stale lock this command has to refresh: written while the workspace
    // still says 0.1.0, exactly as the previous release left it.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let generated = std::process::Command::new(cargo)
        .args([
            "generate-lockfile",
            "--offline",
            "--quiet",
            "--manifest-path",
        ])
        .arg(&manifest)
        .status()
        .expect("run cargo generate-lockfile");
    assert!(generated.success(), "the fixture lock must be generated");

    manifest
}

/// Every `[[package]]` version in a lock, keyed by package name.
fn locked_versions(lockfile: &std::path::Path) -> Vec<(String, String)> {
    let text = fs::read_to_string(lockfile).expect("read lock");
    let mut found = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("name = \"") {
            name = value.strip_suffix('"').map(str::to_string);
        } else if let Some(value) = line.strip_prefix("version = \"") {
            if let (Some(name), Some(version)) = (name.take(), value.strip_suffix('"')) {
                found.push((name, version.to_string()));
            }
        }
    }
    found
}

/// THE PROPERTY A RELEASE DEPENDS ON, and the one that used to be missing.
/// `deploy.yml` builds the release with `--locked` in four places — the
/// provenance step and all three CLI archive jobs — and `--locked` refuses a
/// lock the manifest has moved past. A bump that wrote only `Cargo.toml` failed
/// AFTER the tag was pushed, and the `release-tags` ruleset admits no bypass
/// actor, so the name could not be moved and the day's release was spent. The
/// manifest and the lock must therefore agree the moment this command returns.
#[test]
fn the_lockfile_agrees_with_the_manifest_it_wrote() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = seed_workspace(dir.path());
    let lockfile = dir.path().join("Cargo.lock");

    assert!(
        locked_versions(&lockfile)
            .iter()
            .all(|(_, version)| version == "0.1.0"),
        "the fixture starts with a lock at the previous version"
    );

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "26.8.14",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .success();

    let locked = locked_versions(&lockfile);
    assert_eq!(
        locked.len(),
        2,
        "both workspace crates must still be locked: {locked:?}"
    );
    for (name, version) in &locked {
        assert_eq!(
            version, "26.8.14",
            "{name} is locked at {version}, but the manifest says 26.8.14 — \
             `cargo build --locked` would refuse this lock"
        );
    }
}

/// The refresh is not conditional on the manifest having changed. A rerun of an
/// already-bumped manifest is exactly how the lock was left stale in the first
/// place, so a second run must repair it rather than report success and do
/// nothing.
#[test]
fn a_rerun_repairs_a_lock_left_behind_by_an_earlier_bump() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = seed_workspace(dir.path());
    let lockfile = dir.path().join("Cargo.lock");

    // The state the bug produced: manifest bumped by hand, lock untouched.
    let text = fs::read_to_string(&manifest).expect("read manifest");
    fs::write(
        &manifest,
        text.replace("version = \"0.1.0\"", "version = \"26.8.14\""),
    )
    .expect("write manifest");
    assert!(
        locked_versions(&lockfile)
            .iter()
            .all(|(_, version)| version == "0.1.0"),
        "the lock is stale before the rerun"
    );

    run(&[
        "ops",
        "release",
        "version",
        "--tag",
        "26.8.14",
        "--no-commit",
        "--manifest-path",
        manifest.to_str().unwrap(),
    ])
    .success();

    for (name, version) in locked_versions(&lockfile) {
        assert_eq!(
            version, "26.8.14",
            "{name} must be refreshed even though the manifest already said 26.8.14"
        );
    }
}

/// ANY SEMVER PRERELEASE IS WRITABLE, because the pipeline's only ordering rule
/// is "newer than the last release" and the comparator handles every label the
/// same way. The convention stays `-hotfix.N`; nothing enforces it.
#[test]
fn any_semver_prerelease_is_written_verbatim() {
    for good in [
        "26.8.23-hotfix.1",
        "26.8.23-rc.1",
        "26.8.23-hotfix",
        "2026.8.23",
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = dir.path().join("Cargo.toml");
        fs::write(&manifest, MANIFEST).expect("write manifest");

        run(&[
            "ops",
            "release",
            "version",
            "--tag",
            good,
            "--no-commit",
            "--manifest-path",
            manifest.to_str().unwrap(),
        ])
        .success();

        let written = fs::read_to_string(&manifest).expect("read manifest");
        assert!(
            written.contains(&format!("version = \"{good}\"")),
            "{good} must be written verbatim, got: {written}"
        );
    }
}
