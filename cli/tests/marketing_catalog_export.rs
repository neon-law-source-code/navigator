//! The shared marketing catalog's export contract.
//!
//! `navigator-ux` vendors this artifact and renders from it. Three properties
//! are what make that safe, and all three are proved here against the real
//! exporter rather than against a constant:
//!
//! - the same catalog at the same revision exports the same bytes,
//! - the recorded digest covers the payload, so an edited artifact is
//!   detectable, and
//! - a catalog the contract refuses (unsupported version, missing required
//!   copy) never becomes an artifact at all.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use views::locales::shared::{Provenance, SharedCatalog};

/// A real-looking immutable pin. The exporter refuses anything else.
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

const SHIPPED_CATALOG: &str = "neon/locales/en/shared.yaml";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the cli crate sits in the workspace")
        .to_path_buf()
}

/// The built example.
///
/// `cargo test` builds examples, so this is normally already on disk beside
/// the test binary; the build is the fallback rather than the rule. It is
/// memoized because the tests below run in parallel and one build is enough.
fn exporter() -> PathBuf {
    static EXPORTER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    EXPORTER.get_or_init(build_exporter).clone()
}

fn build_exporter() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let binary = dir.join("examples").join(format!(
        "export-marketing-catalog{}",
        std::env::consts::EXE_SUFFIX
    ));
    if binary.exists() {
        return binary;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = Command::new(cargo)
        .current_dir(workspace_root())
        .args([
            "build",
            "-p",
            "cli",
            "--example",
            "export-marketing-catalog",
        ])
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

fn export(args: &[&str]) -> std::process::Output {
    Command::new(exporter())
        .current_dir(workspace_root())
        .args(args)
        .output()
        .expect("run the exporter")
}

fn export_ok(args: &[&str]) -> String {
    let output = export(args);
    assert!(
        output.status.success(),
        "exporter failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 export")
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

fn write_catalog(dir: &tempfile::TempDir, yaml: &str) -> PathBuf {
    let path = dir.path().join("shared.yaml");
    std::fs::write(&path, yaml).expect("write the fixture catalog");
    path
}

/// A catalog with every required key, so a test can remove exactly one.
fn complete_catalog() -> String {
    let mut yaml = String::from("catalog_version: 1\nentries:\n");
    for key in views::locales::shared::REQUIRED_KEYS {
        writeln!(yaml, "  {key}: Words for {key}.").expect("write to a String");
    }
    yaml
}

#[test]
fn the_same_catalog_at_the_same_revision_exports_the_same_bytes() {
    let once = export_ok(&["--revision", REVISION]);
    let twice = export_ok(&["--revision", REVISION]);
    assert_eq!(once, twice, "the export must be reproducible");
    assert!(once.ends_with('\n'), "the artifact ends with a newline");
}

/// The digest covers the payload the consumer renders from, and the payload
/// records the immutable revision it came from.
#[test]
fn the_recorded_digest_covers_the_payload_and_the_payload_names_its_source() {
    let raw = export_ok(&["--revision", REVISION]);
    let document: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let payload = document.get("payload").expect("payload");

    assert_eq!(
        document
            .get("integrity")
            .and_then(serde_json::Value::as_str),
        Some(digest(&canonical(payload)).as_str())
    );
    assert_eq!(
        payload
            .pointer("/source/revision")
            .and_then(serde_json::Value::as_str),
        Some(REVISION)
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
        Some(SHIPPED_CATALOG)
    );

    // The exported bytes are the ones `views` canonicalizes, so the producer
    // and the artifact cannot drift apart.
    let shipped = std::fs::read_to_string(workspace_root().join(SHIPPED_CATALOG))
        .expect("the shipped catalog");
    let catalog = SharedCatalog::parse(&shipped).expect("the shipped catalog is valid");
    let source = Provenance {
        path: SHIPPED_CATALOG.to_string(),
        repository: "neon-law-source-code/navigator".to_string(),
        revision: REVISION.to_string(),
    };
    assert_eq!(canonical(payload), catalog.canonical_payload(&source));
}

/// Hand-editing the vendored artifact must be detectable. This is the check
/// that makes "the consumer renders the pinned catalog" a fact rather than a
/// hope.
#[test]
fn an_edited_artifact_no_longer_matches_its_digest() {
    let raw = export_ok(&["--revision", REVISION]);
    let mut document: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let recorded = document
        .get("integrity")
        .and_then(serde_json::Value::as_str)
        .expect("integrity")
        .to_string();

    document
        .pointer_mut("/payload/entries/litigation.title")
        .map(|entry| *entry = serde_json::Value::String("Something nobody authored.".into()))
        .expect("the entry a tamperer would reach for");

    let payload = document.get("payload").expect("payload");
    assert_ne!(
        digest(&canonical(payload)),
        recorded,
        "an edited artifact must fail its own digest"
    );
}

#[test]
fn a_floating_revision_is_refused() {
    let output = export(&["--revision", "main"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("full 40-character commit sha"),
        "a branch name is not a pin"
    );
}

#[test]
fn an_unsupported_catalog_version_never_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_catalog(
        &dir,
        &complete_catalog().replace("catalog_version: 1", "catalog_version: 99"),
    );
    let output = export(&["--revision", REVISION, "--catalog", &path.to_string_lossy()]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("catalog version 99 is not supported"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_catalog_missing_required_copy_never_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_catalog(
        &dir,
        &complete_catalog().replace("  litigation.title:", "  litigation.other:"),
    );
    let output = export(&["--revision", REVISION, "--catalog", &path.to_string_lossy()]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("required key `litigation.title` is missing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Writing to a file produces the same bytes standard output does, so the
/// documented build path and a manual run cannot disagree.
#[test]
fn the_written_artifact_matches_the_printed_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("nested").join("marketing-catalog.json");
    let printed = export_ok(&["--revision", REVISION]);
    let output = export(&["--revision", REVISION, "--out", &out.to_string_lossy()]);
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&out).expect("written artifact"),
        printed
    );
}
