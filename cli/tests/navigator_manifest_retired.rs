//! The Project manifest is one filename, one key: `navigator.yaml`, `project:`.
//!
//! Three artifacts used to answer to `navigator.yaml` or `navigator.yml`, with
//! three schemas — the brand-bundle manifest, the sample-project bundle
//! manifest, and the Project repository manifest six live repositories already
//! shipped. ENG-290 collapsed the latter two: `navigator.yml` and its `name:`
//! key are retired, folded into `navigator.yaml`'s `project:`. The brand-bundle
//! manifest keeps its own name — the two never share a path, so nothing was
//! gained by renaming it, and doing so would break a documented OSS install
//! flow and a served URL.
//!
//! A rename this size is easy to half-finish: a doc paragraph left over from
//! before the decision, a test fixture still writing the old filename. So the
//! retired spelling is asserted absent rather than remembered, the same
//! pattern `forge_coordinate_retired.rs` uses for a different retirement.

use std::path::PathBuf;
use std::process::Command;

/// This file, exempt by provenance: it has to spell the retired name to check
/// for it.
const SKIPPED_FILES: &[&str] = &["navigator_manifest_retired.rs"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every file Git tracks, as a repo-relative path.
///
/// Asking Git rather than walking the filesystem is what keeps this guard from
/// failing on a reviewer's own branch name (`.git` is a file, not a directory,
/// inside a linked worktree checkout) or from drowning in another worktree's
/// full copy of this tree — see `forge_coordinate_retired.rs` for the longer
/// version of this reasoning.
fn tracked_files() -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["ls-files", "-z"])
        .output()
        .expect("run `git ls-files`");
    assert!(
        output.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    assert!(
        files.len() > 100,
        "expected a tracked file list, got {} entries — this guard would pass vacuously",
        files.len()
    );
    files
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The retired filename appears nowhere Git tracks — not in source, not in a
/// test fixture, not in prose.
///
/// Prose counts on purpose: a doc paragraph describing the old spelling is as
/// stale as a fixture still writing it, and a code-only grep would miss the
/// docs this rename touched.
#[test]
fn no_stale_navigator_yml_literal_survives() {
    let mut hits = Vec::new();
    for path in tracked_files() {
        if SKIPPED_FILES.contains(&basename(&path)) {
            continue;
        }
        let full = repo_root().join(&path);
        let Ok(body) = std::fs::read_to_string(&full) else {
            continue; // binary or non-UTF-8; the retired name is ASCII
        };
        for (index, line) in body.lines().enumerate() {
            if line.contains("navigator.yml") {
                hits.push(format!("{path}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the Project manifest is one filename now, `navigator.yaml` — `navigator.yml` and its \
         `name:` key are retired (ENG-290). Found {} stale occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}
