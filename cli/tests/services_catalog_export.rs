//! The individual-services catalog's export contract.
//!
//! `navigator-ux` renders the same firm services this repository publishes.
//! Vendoring this artifact is what keeps that from becoming a second, drifting
//! copy of a fee schedule — so the same three properties the shared marketing
//! catalog is held to are proved here, against the real exporter:
//!
//! - the same catalog at the same revision exports the same bytes,
//! - the recorded digest covers the payload, so an edited artifact is
//!   detectable, and
//! - a catalog the contract refuses (unsupported version, two fees on one
//!   matter, a dangling `related`) never becomes an artifact at all.

use std::path::{Path, PathBuf};
use std::process::Command;

use views::locales::services::ServicesCatalog;
use views::locales::shared::Provenance;

/// A real-looking immutable pin. The exporter refuses anything else.
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

const SHIPPED_CATALOG: &str = "neon/locales/en/neon/services-catalog.yaml";

/// The services the shipped catalog publishes. Transcribed from the
/// `navigator-ux` gallery specimen; every fee is a draft pending attorney
/// review.
const SHIPPED_SERVICES: usize = 18;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the cli crate sits in the workspace")
        .to_path_buf()
}

/// The built example. `cargo test` builds examples, so this is normally
/// already on disk beside the test binary; the build is the fallback rather
/// than the rule, and is memoized because these tests run in parallel.
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
        "export-services-catalog{}",
        std::env::consts::EXE_SUFFIX
    ));
    if binary.exists() {
        return binary;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = Command::new(cargo)
        .current_dir(workspace_root())
        .args(["build", "-p", "cli", "--example", "export-services-catalog"])
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
    use std::fmt::Write as _;
    let mut out = String::from("sha256:");
    for byte in sha2::Sha256::digest(payload.as_bytes()) {
        write!(out, "{byte:02x}").expect("write to a String");
    }
    out
}

/// The shipped catalog, rewritten by `edit` — the base every refusal test
/// mutates, so each one proves the refusal against a document that is
/// otherwise real.
fn write_catalog(dir: &tempfile::TempDir, edit: impl Fn(String) -> String) -> PathBuf {
    let shipped =
        std::fs::read_to_string(workspace_root().join(SHIPPED_CATALOG)).expect("shipped catalog");
    let path = dir.path().join("services-catalog.yaml");
    std::fs::write(&path, edit(shipped)).expect("write the fixture catalog");
    path
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
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
    let catalog = ServicesCatalog::parse(&shipped).expect("the shipped catalog is valid");
    let source = Provenance {
        path: SHIPPED_CATALOG.to_string(),
        repository: "neon-law-source-code/navigator".to_string(),
        revision: REVISION.to_string(),
    };
    assert_eq!(canonical(payload), catalog.canonical_payload(&source));
}

/// The artifact carries the whole schedule, not a truncated one: every
/// service, with the fields a consumer filters and links by.
#[test]
fn the_artifact_carries_every_service_with_its_identifiers() {
    let raw = export_ok(&["--revision", REVISION]);
    let document: serde_json::Value = serde_json::from_str(&raw).expect("export is json");
    let services = document
        .pointer("/payload/services")
        .and_then(serde_json::Value::as_array)
        .expect("services");
    assert_eq!(services.len(), SHIPPED_SERVICES);
    for service in services {
        for field in ["id", "item", "name", "blurb", "category", "period"] {
            assert!(
                service
                    .get(field)
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "every service carries `{field}`: {service}"
            );
        }
        // One fee, never two and never none — the rule the catalog enforces,
        // observed on the artifact a consumer actually reads.
        let fees = usize::from(service.get("flat_fee").is_some())
            + usize::from(service.get("amount").is_some());
        assert_eq!(fees, 1, "a service publishes exactly one fee: {service}");
    }
}

/// Hand-editing the vendored artifact must be detectable. This is the check
/// that makes "the consumer renders the pinned schedule" a fact rather than a
/// hope — and a fee is exactly what a tamperer would reach for.
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
        .pointer_mut("/payload/flat_fee")
        .map(|fee| *fee = serde_json::Value::String("$0".into()))
        .expect("the flat fee a tamperer would reach for");

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
        stderr(&output).contains("full 40-character commit sha"),
        "a branch name is not a pin"
    );
}

#[test]
fn an_unsupported_catalog_version_never_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_catalog(&dir, |yaml| {
        yaml.replace("catalog_version: 1", "catalog_version: 99")
    });
    let output = export(&["--revision", REVISION, "--catalog", &path.to_string_lossy()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("catalog version 99 is not supported"),
        "{}",
        stderr(&output)
    );
}

/// Two prices on one matter is a fee-advertising problem, not a rendering
/// one, so it is refused before an artifact exists.
#[test]
fn a_matter_with_two_fees_never_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_catalog(&dir, |yaml| {
        yaml.replace(
            "    amount: \"$350\"\n    period: per year",
            "    amount: \"$350\"\n    flat_fee: form\n    period: per year",
        )
    });
    let output = export(&["--revision", REVISION, "--catalog", &path.to_string_lossy()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("publishes both a flat fee and an amount"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_dangling_related_id_never_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_catalog(&dir, |yaml| {
        yaml.replace("      - llc-launch\n", "      - llc-retire\n")
    });
    let output = export(&["--revision", REVISION, "--catalog", &path.to_string_lossy()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("which this catalog does not define"),
        "{}",
        stderr(&output)
    );
}

/// Writing to a file produces the same bytes standard output does, so the
/// documented build path and a manual run cannot disagree.
#[test]
fn the_written_artifact_matches_the_printed_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("nested").join("services-catalog.json");
    let printed = export_ok(&["--revision", REVISION]);
    let output = export(&["--revision", REVISION, "--out", &out.to_string_lossy()]);
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&out).expect("written artifact"),
        printed
    );
}
