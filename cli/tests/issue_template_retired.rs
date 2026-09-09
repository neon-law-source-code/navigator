//! `.github/ISSUE_TEMPLATE/` is retired. Linear is the only intake.
//!
//! ENG-175 deleted the last GitHub issue form in this repository — the
//! `design-mockup` form, the front door for a contributor who prototypes a
//! screen outside the repo and cannot write Rust. Planning already lives in
//! Linear for every other lane; this closed the one gap. `docs/design-mockups.md`
//! now points filers at a Linear issue template on the Engineering team
//! instead.
//!
//! A GitHub issue form is easy to reintroduce a piece at a time: a new
//! `.github/ISSUE_TEMPLATE/config.yml`, a single `.yml` form dropped back in
//! to unblock one contributor. So the retirement is asserted rather than
//! remembered, the same pattern `navigator_manifest_retired.rs` and
//! `forge_coordinate_retired.rs` use for their own retirements: walk every
//! file Git tracks and refuse the directory back, rather than trust that
//! nobody restores it.

use std::path::PathBuf;
use std::process::Command;

/// Files exempt by provenance: only this guard, whose own text names the
/// retired path.
const SKIPPED_FILES: &[&str] = &["issue_template_retired.rs"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every file Git tracks, as a repo-relative path.
///
/// Asking Git rather than walking the filesystem is what keeps this guard
/// from failing on a reviewer's own branch name (`.git` is a file, not a
/// directory, inside a linked worktree checkout) or from drowning in another
/// worktree's full copy of this tree — see `forge_coordinate_retired.rs` for
/// the longer version of this reasoning.
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

/// No tracked file lives under `.github/ISSUE_TEMPLATE/`, and no tracked file
/// names the retired form.
///
/// The path check catches the directory coming back with any content at all;
/// the literal check catches a stray reference surviving in prose (a doc
/// still pointing a filer at "the Design mockup template on the repository's
/// New issue page") even if the directory itself stays gone.
#[test]
fn no_github_issue_template_directory_or_reference_survives() {
    let mut hits = Vec::new();
    for path in tracked_files() {
        if path.starts_with(".github/ISSUE_TEMPLATE/") {
            hits.push(format!("{path}: tracked file under the retired directory"));
            continue;
        }
        if SKIPPED_FILES.contains(&basename(&path)) {
            continue;
        }
        let full = repo_root().join(&path);
        let Ok(body) = std::fs::read_to_string(&full) else {
            continue; // binary or non-UTF-8; the retired path is ASCII
        };
        for (index, line) in body.lines().enumerate() {
            if line.contains(".github/ISSUE_TEMPLATE") {
                hits.push(format!("{path}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "`.github/ISSUE_TEMPLATE/` is retired (ENG-175) — Linear is the only intake, including \
         for a design mockup, which files a Linear issue on the Engineering team instead. Found \
         {} occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}
