//! The worked white-label example is one `ops rebrand` actually accepts.
//!
//! A deployer's first act under their own identity is to copy an example brand
//! manifest and edit it. That example used to sit at the repository root, where
//! nothing exercised it: every field it named was a claim about the schema that
//! no test checked, so a rename in [`views::brand_bundle`] could retire a key
//! while the example kept advertising it and the first person to find out was
//! the operator whose `rebrand build` failed on a file we shipped.
//!
//! So the example lives here instead, as the fixture this guard drives end to
//! end through the compiled binary — `build` from the manifest, then `verify`
//! on what `build` wrote. Documentation points at this path for the copy, which
//! makes the example and its proof the same file.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

/// The example manifest, as a deployer copies it.
const EXAMPLE: &str = include_str!("fixtures/navigator.example.yaml");

/// Static files the example's `assets` block names, source-relative.
///
/// Stubs, not real logos: `build` copies the bytes without reading them, and a
/// checked-in PNG would make this guard a place binary assets accumulate.
const ASSETS: [(&str, &str); 3] = [
    (
        "brand/logo-firm.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    ),
    ("brand/logo-firm.png", "png"),
    ("brand/letterhead.css", ".letterhead { color: teal; }"),
];

/// A source tree shaped the way the example expects: the manifest at
/// `navigator.yaml` with its brand files beside it.
fn source_tree() -> TempDir {
    let dir = TempDir::new().expect("create source tree");
    fs::write(dir.path().join("navigator.yaml"), EXAMPLE).expect("write manifest");
    for (rel, body) in ASSETS {
        let path = dir.path().join(rel);
        fs::create_dir_all(path.parent().expect("asset parent")).expect("create asset directory");
        fs::write(&path, body).expect("write asset");
    }
    dir
}

fn navigator() -> Command {
    Command::cargo_bin("navigator").expect("build the `navigator` binary")
}

/// `build` turns the example into a mountable bundle, and `verify` accepts it.
///
/// Both halves matter. `build` proves every key the example names still parses
/// and validates against the manifest schema; `verify` proves the bundle it
/// wrote loads the way a web or workflow-worker pod loads the mount, which is
/// the failure an operator would otherwise meet in the cluster rather than here.
#[test]
fn the_example_brand_manifest_builds_and_verifies() {
    let source = source_tree();
    let out = TempDir::new().expect("create output root");
    let bundle = out.path().join("brand-bundle");

    navigator()
        .args(["ops", "rebrand", "build"])
        .arg("--file")
        .arg(source.path().join("navigator.yaml"))
        .arg("--out")
        .arg(&bundle)
        .assert()
        .success();

    for (rel, _) in ASSETS {
        assert!(
            bundle.join(rel).is_file(),
            "`rebrand build` must copy `{rel}` into the bundle; a mount missing a \
             brand file renders the deployer's identity with a hole in it"
        );
    }
    assert!(
        bundle.join("navigator.yaml").is_file(),
        "the bundle must carry its own manifest — that is what makes it \
         self-contained and mountable read-only"
    );

    navigator()
        .args(["ops", "rebrand", "verify"])
        .arg("--dir")
        .arg(&bundle)
        .assert()
        .success();
}

/// The example is the file the documentation tells a deployer to copy.
///
/// Moving the example under `tests/` is only safe while the paths that point at
/// it agree. A doc that names a path which no longer exists sends the reader
/// looking for a file that was deleted, which is the exact failure that moving
/// it here was supposed to end.
#[test]
fn the_documentation_points_at_the_example_where_it_now_lives() {
    const FIXTURE: &str = "cli/tests/fixtures/navigator.example.yaml";

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for rel in [
        "docs/oss-install.md",
        "server/content/workshops/navigator/DEPLOY.md",
    ] {
        let body = fs::read_to_string(root.join(rel)).expect("read documentation");
        assert!(
            body.contains(FIXTURE),
            "{rel} must name `{FIXTURE}` as the example brand manifest to copy"
        );
    }
}
