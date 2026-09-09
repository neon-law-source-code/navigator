//! The digest never names a bucket it could not resolve.
//!
//! `build_report` used to substitute the literal `<unset>` when the bucket
//! env was missing, so a misconfigured deployment posted per-table links
//! reading `.../storage/browser/<unset>/application/persons` — a broken link
//! in a channel rather than an error anyone could act on. That fallback is
//! gone and the digest fails closed instead.
//!
//! Three unit tests pinned the old behaviour and were deleted with it, which
//! is exactly why this guard is worth having: nothing else would notice the
//! sentinel being reintroduced somewhere else in the crate. This is a
//! source-level assertion rather than a behavioural one because the point is
//! that *no* code path can emit it, and a test per path cannot prove that.

use std::fs;
use std::path::{Path, PathBuf};

/// The sentinel, assembled at runtime so this file does not itself contain
/// the phrase it forbids.
fn sentinel() -> String {
    format!("{}unset{}", '<', '>')
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_source_in_archives_emits_the_unset_bucket_sentinel() {
    let root = crate_root();
    let needle = sentinel();
    let mut sources = Vec::new();
    rust_sources(&root.join("src"), &mut sources);
    rust_sources(&root.join("tests"), &mut sources);
    assert!(!sources.is_empty(), "the crate must have Rust sources");

    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| {
            fs::read_to_string(path).is_ok_and(|body| {
                // This file names the sentinel only by assembling it.
                path.file_name().and_then(|n| n.to_str()) != Some("no_unset_bucket_sentinel.rs")
                    && body.contains(&needle)
            })
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "the archive digest must fail closed rather than name an unresolved \
         bucket; these files reintroduce the sentinel:\n  {}",
        offenders.join("\n  ")
    );
}
