#![allow(clippy::doc_markdown)]
//! The OpenAPI document's export contract.
//!
//! `navigator-ux` vendors this artifact instead of hand-copying
//! `portal/src/openapi.rs`'s output. Six properties make that safe, and all
//! six are proved here against the real exporter rather than a constant:
//!
//! - the same document at the same revision exports the same bytes, and the
//!   ambient environment cannot reach them,
//! - the recorded digest covers the payload, so an edited artifact is
//!   detectable,
//! - the exported paths are exactly the operations the document declares — the
//!   export cannot publish a narrower or wider surface than the one
//!   `server/tests/openapi_drift.rs` holds to the router,
//! - a revision that is not reachable from `origin/main` never becomes an
//!   artifact, so a pull-request head cannot be pinned,
//! - a `--revision` that is not the checked-out `HEAD` never becomes an
//!   artifact either, even when that revision is itself reachable from
//!   `origin/main` — the exporter serializes whatever source is on disk, and a
//!   stale or different checkout must not be attributed to another commit,
//!   and
//! - uncommitted local changes refuse the export outright, for the same
//!   reason.
//!
//! Every run works against a temporary fixture repository rather than this
//! checkout, so the suite does not depend on the local `origin/main` and a
//! shallow CI clone cannot change what these tests assert.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// The built example.
///
/// `cargo test` builds examples, so this is normally already on disk beside
/// the test binary; the build is the fallback rather than the rule. It is
/// memoized because the tests below run in parallel and one build is enough.
fn exporter() -> PathBuf {
    static EXPORTER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    EXPORTER.get_or_init(build_exporter).clone()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the cli crate sits in the workspace")
        .to_path_buf()
}

fn build_exporter() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let binary = dir
        .join("examples")
        .join(format!("export-openapi{}", std::env::consts::EXE_SUFFIX));
    if binary.exists() {
        return binary;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = Command::new(cargo)
        .current_dir(workspace_root())
        .args(["build", "-p", "cli", "--example", "export-openapi"])
        .status()
        .expect("build the exporter example");
    assert!(built.success(), "the exporter example must build");
    assert!(
        binary.exists(),
        "expected the exporter at {}",
        binary.display()
    );
    binary
}

/// Run git with an identity and no signing, so the fixture builds the same way
/// on a machine whose global config signs every commit.
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("run git")
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 git output")
        .trim()
        .to_string()
}

/// A repository shaped like the one a squash merge leaves behind, checked out
/// at the tip the pin is supposed to describe:
///
/// ```text
/// root -- shipped        <- origin/main, and HEAD after Fixture::new()
///           \
///            pull_request_head   (dangling: reachable by sha, on no branch)
/// ```
struct Fixture {
    dir: TempDir,
    /// An ancestor of `shipped` — reachable from `origin/main`, but not what
    /// is checked out. Stands in for "a different, also-reachable commit".
    root: String,
    /// Reachable from `origin/main`, and what `HEAD` is checked out at after
    /// construction — what a pin is allowed to be.
    shipped: String,
    /// A child of `shipped` that `origin/main` does not reach — what a
    /// pull-request head looks like after its branch is squash-merged away.
    /// The commit object survives (this test never runs `git gc`) but no ref
    /// names it once construction resets `HEAD` back to `shipped`.
    pull_request_head: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        git_ok(path, &["init", "--quiet"]);
        git_ok(path, &["commit", "--allow-empty", "--quiet", "-m", "root"]);
        let root = git_ok(path, &["rev-parse", "HEAD"]);
        git_ok(
            path,
            &["commit", "--allow-empty", "--quiet", "-m", "shipped"],
        );
        let shipped = git_ok(path, &["rev-parse", "HEAD"]);
        git_ok(path, &["update-ref", "refs/remotes/origin/main", &shipped]);
        git_ok(
            path,
            &["commit", "--allow-empty", "--quiet", "-m", "review head"],
        );
        let pull_request_head = git_ok(path, &["rev-parse", "HEAD"]);
        assert_ne!(shipped, pull_request_head);
        // `HEAD` returns to `shipped`, the commit `origin/main` names, so
        // `export_shipped()` describes a checkout genuinely standing on the
        // tip it pins. `pull_request_head` keeps existing as an object with
        // no ref pointing at it — exactly the state a squash-merged PR branch
        // leaves behind.
        git_ok(path, &["reset", "--quiet", "--hard", &shipped]);
        Self {
            dir,
            root,
            shipped,
            pull_request_head,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Detach `HEAD` onto `sha`, for a test that needs a checkout literally
    /// standing on a commit other than `shipped`.
    fn checkout(&self, sha: &str) {
        git_ok(self.path(), &["checkout", "--quiet", "--detach", sha]);
    }

    fn export(&self, args: &[&str]) -> std::process::Output {
        self.export_with_env(args, &[])
    }

    fn export_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut command = Command::new(exporter());
        command.current_dir(self.path()).args(args);
        // `NAV_BASE_URL` is set by every sourced `.devx/env`, so an exporter
        // that read it would bake a worktree's loopback port into a vendored
        // artifact. Clear it unless a test is deliberately setting it.
        command.env_remove("NAV_BASE_URL");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("run the exporter")
    }

    fn export_ok(&self, args: &[&str]) -> String {
        let output = self.export(args);
        assert!(
            output.status.success(),
            "exporter failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8 export")
    }

    /// The export at the one revision this fixture's `origin/main` reaches.
    fn export_shipped(&self) -> String {
        self.export_ok(&["--revision", &self.shipped])
    }
}

/// The canonical form a digest covers: compact JSON with sorted object keys.
/// `serde_json::Map` is a `BTreeMap` here, so serializing a parsed value
/// reproduces exactly what the exporter hashed.
fn canonical(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("canonical json")
}

fn digest(payload: &str) -> String {
    use sha2::Digest as _;
    let mut out = String::from("sha256:");
    for byte in sha2::Sha256::digest(payload.as_bytes()) {
        write!(out, "{byte:02x}").expect("write to a String");
    }
    out
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn the_same_document_at_the_same_revision_exports_the_same_bytes() {
    let fixture = Fixture::new();
    let once = fixture.export_shipped();
    let twice = fixture.export_shipped();
    assert_eq!(once, twice, "the export must be reproducible");
    assert!(once.ends_with('\n'), "the artifact ends with a newline");
}

/// A developer who sourced `.devx/env` must not publish their worktree's
/// loopback port as the API's server. The exported bytes are the same with and
/// without the override.
#[test]
fn the_ambient_base_url_does_not_reach_the_exported_bytes() {
    let fixture = Fixture::new();
    let neutral = fixture.export_shipped();
    let output = fixture.export_with_env(
        &["--revision", &fixture.shipped],
        &[("NAV_BASE_URL", "http://localhost:20345")],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf-8 export"),
        neutral,
        "the environment must not reach a vendored artifact"
    );
    assert!(
        !neutral.contains("localhost:20345"),
        "a loopback port must never be published as the API's server"
    );
}

/// The digest covers the payload the consumer reads, and the payload records
/// the immutable revision it came from.
#[test]
fn the_recorded_digest_covers_the_payload_and_the_payload_names_its_source() {
    let fixture = Fixture::new();
    let raw = fixture.export_shipped();
    let artifact: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let payload = artifact.get("payload").expect("payload");

    assert_eq!(
        artifact
            .get("integrity")
            .and_then(serde_json::Value::as_str),
        Some(digest(&canonical(payload)).as_str())
    );
    assert_eq!(
        artifact
            .get("generator")
            .and_then(serde_json::Value::as_str),
        Some("cargo run -p cli --example export-openapi")
    );
    assert_eq!(
        payload
            .pointer("/source/revision")
            .and_then(serde_json::Value::as_str),
        Some(fixture.shipped.as_str())
    );
    assert_eq!(
        payload
            .pointer("/source/repository")
            .and_then(serde_json::Value::as_str),
        Some("neon-law-source-code/navigator")
    );
    assert_eq!(
        payload
            .pointer("/source/path")
            .and_then(serde_json::Value::as_str),
        Some("portal/src/openapi.rs")
    );
    assert_eq!(
        payload
            .pointer("/document/openapi")
            .and_then(serde_json::Value::as_str),
        Some("3.1.0"),
        "the payload carries a whole OpenAPI document, not a fragment"
    );
}

/// Hand-editing the vendored artifact must be detectable. This is what makes
/// "the consumer reads the pinned document" a fact rather than a hope.
#[test]
fn an_edited_artifact_no_longer_matches_its_digest() {
    let fixture = Fixture::new();
    let raw = fixture.export_shipped();
    let mut artifact: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let recorded = artifact
        .get("integrity")
        .and_then(serde_json::Value::as_str)
        .expect("integrity")
        .to_string();

    *artifact
        .pointer_mut("/payload/document/info/title")
        .expect("every OpenAPI document has a title") =
        serde_json::Value::String("Something nobody authored".into());

    assert_ne!(
        digest(&canonical(artifact.get("payload").expect("payload"))),
        recorded,
        "an edited artifact must fail its own digest"
    );
}

/// The export publishes the operation set the document declares — no more, no
/// less. `server/tests/openapi_drift.rs` is what holds that set to the routes
/// the router registers; this test holds the artifact to the document.
#[test]
fn the_exported_operations_are_the_documented_operations() {
    let fixture = Fixture::new();
    let raw = fixture.export_shipped();
    let artifact: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let paths = artifact
        .pointer("/payload/document/paths")
        .and_then(serde_json::Value::as_object)
        .expect("the exported document has paths");

    let mut exported = BTreeSet::new();
    for (path, methods) in paths {
        for verb in methods
            .as_object()
            .expect("an OpenAPI path item is an object")
            .keys()
        {
            exported.insert((verb.to_uppercase(), path.clone()));
        }
    }
    let documented: BTreeSet<(String, String)> = portal::openapi::documented_operations()
        .into_iter()
        .collect();

    assert_eq!(
        exported,
        documented,
        "only in the artifact = {:?}; only in the document = {:?}",
        exported.difference(&documented).collect::<Vec<_>>(),
        documented.difference(&exported).collect::<Vec<_>>(),
    );
}

/// The defect this exporter exists to prevent: a squash merge replaces a
/// pull-request head with a new commit, so pinning the head produces a
/// revision a fresh clone cannot resolve. The checkout stands on that exact
/// commit (so the `HEAD`-match check passes) — only reachability fails.
#[test]
fn a_revision_that_origin_main_does_not_reach_is_refused() {
    let fixture = Fixture::new();
    fixture.checkout(&fixture.pull_request_head);
    let output = fixture.export(&["--revision", &fixture.pull_request_head]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("is not reachable from origin/main"),
        "{}",
        stderr(&output)
    );
}

/// A well-formed sha the checkout is not standing on is refused before
/// reachability is even checked — the exporter cannot tell a never-held
/// commit from a real one apart from the fact that `HEAD` isn't it, and that
/// alone is reason enough to refuse.
#[test]
fn a_revision_this_repository_has_never_held_is_refused() {
    let fixture = Fixture::new();
    let output = fixture.export(&["--revision", "0123456789abcdef0123456789abcdef01234567"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("does not match --revision"),
        "{}",
        stderr(&output)
    );
}

/// The exact scenario the review comment named: `--revision` names a commit
/// that genuinely is reachable from `origin/main`, but it is not the commit
/// the checkout is standing on. Exporting must not attribute the checked-out
/// source (`shipped`) to a different revision (`root`) just because that
/// revision happens to be an ancestor.
#[test]
fn a_reachable_revision_that_is_not_the_checked_out_head_is_refused() {
    let fixture = Fixture::new();
    let output = fixture.export(&["--revision", &fixture.root]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains("does not match --revision") && message.contains(&fixture.shipped),
        "{message}"
    );
}

/// Uncommitted changes make the checked-out tree describe something other
/// than the pinned commit, even though `HEAD` itself still names it.
#[test]
fn a_dirty_working_tree_is_refused() {
    let fixture = Fixture::new();
    std::fs::write(fixture.path().join("untracked.txt"), b"not part of shipped")
        .expect("write an untracked file");
    let output = fixture.export(&["--revision", &fixture.shipped]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("uncommitted changes"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_floating_revision_is_refused() {
    let fixture = Fixture::new();
    let output = fixture.export(&["--revision", "main"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("full 40-character commit sha"),
        "a branch name is not a pin: {}",
        stderr(&output)
    );
}

/// An unfetched remote must refuse rather than silently skip the check — a
/// guarantee that quietly downgrades is the one that lets a bad pin through.
#[test]
fn an_unresolvable_origin_main_refuses_rather_than_skipping_the_check() {
    let fixture = Fixture::new();
    git_ok(
        fixture.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    let output = fixture.export(&["--revision", &fixture.shipped]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot resolve `origin/main`"),
        "{}",
        stderr(&output)
    );
}

/// Writing to a file produces the same bytes standard output does, so the
/// documented refresh command and a manual run cannot disagree.
#[test]
fn the_written_artifact_matches_the_printed_one() {
    let fixture = Fixture::new();
    let out = fixture.path().join("nested").join("openapi.json");
    let printed = fixture.export_shipped();
    let output = fixture.export(&[
        "--revision",
        &fixture.shipped,
        "--out",
        &out.to_string_lossy(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(&out).expect("written artifact"),
        printed
    );
}

/// A bare filename's parent is `Some("")`, an empty path that names no
/// directory to create — not `None`. `--out openapi.json` must write into the
/// current directory rather than asking `create_dir_all` to create `""`.
///
/// The comparison export runs *first*: once the bare-filename export leaves
/// `openapi.json` sitting untracked in the fixture directory, a second export
/// there would itself be refused as a dirty working tree.
#[test]
fn a_bare_output_filename_writes_into_the_current_directory() {
    let fixture = Fixture::new();
    let printed = fixture.export_shipped();
    let output = fixture.export(&["--revision", &fixture.shipped, "--out", "openapi.json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("openapi.json")).expect("written artifact"),
        printed
    );
}
