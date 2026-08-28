//! ENG-73 guard: no workflow or composite action in this repository may name
//! Google Drive as a CI publish destination.
//!
//! Object storage is the working-file authority; Drive stays a per-Project
//! ingest source that Navigator itself reads from (ENG-225) and is never
//! written by CI (ENG-73). A workflow or action naming a Drive folder ID —
//! `drive_folder_id` is the field `store::projects` and
//! `store/src/schema/navigator.surql` use for it — would be exactly the
//! attacker-controlled repository literal ENG-73 warns against: anyone who
//! can land a commit could repoint a publish at it. This test does not prove
//! CI *can't* write Drive (the Workload Identity binding is what actually
//! decides that); it proves nothing checked into this repository already
//! asks it to, so a future change that wires one in fails here first.
//!
//! Comment lines are excluded, matching the convention
//! `cli/tests/application_publish.rs` already uses (see
//! `the_publish_never_rsyncs`): prose is free to name Drive while explaining
//! why it is out of scope, only live configuration is forbidden from it.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

/// Every workflow and composite action file this repository's own CI runs.
fn ci_definition_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();

    let workflows = root.join(".github/workflows");
    for entry in
        fs::read_dir(&workflows).unwrap_or_else(|e| panic!("read {}: {e}", workflows.display()))
    {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("yml") {
            files.push(path);
        }
    }

    let actions = root.join(".github/actions");
    for entry in
        fs::read_dir(&actions).unwrap_or_else(|e| panic!("read {}: {e}", actions.display()))
    {
        let action_dir = entry.expect("dir entry").path();
        if !action_dir.is_dir() {
            continue;
        }
        let manifest = action_dir.join("action.yml");
        if manifest.is_file() {
            files.push(manifest);
        }
    }

    for expected in [
        ".github/actions/validate/action.yml",
        ".github/actions/application-publish/action.yml",
    ] {
        assert!(
            files.iter().any(|f| f.ends_with(expected)),
            "expected {expected} among the scanned CI definitions; the file \
             layout moved and this guard's file list must move with it"
        );
    }

    files
}

/// Markers naming Google Drive as a live coordinate rather than discussing
/// the decision in prose.
const FORBIDDEN_MARKERS: [&str; 4] = [
    "drive_folder_id",
    "driveFolderId",
    "drive.googleapis.com",
    "www.googleapis.com/auth/drive",
];

#[test]
fn no_ci_definition_names_a_drive_coordinate() {
    for path in ci_definition_files() {
        let source =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (number, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with('#') {
                continue;
            }
            for marker in FORBIDDEN_MARKERS {
                assert!(
                    !line.contains(marker),
                    "{}:{} names a Drive coordinate `{marker}`; CI must publish \
                     only to object storage or the applications bucket, never \
                     Drive: {code}",
                    path.display(),
                    number + 1,
                );
            }
        }
    }
}
