//! The `staff` role vocabulary is retired; this stops it coming back.
//!
//! At this firm the tier that performs legal work is a lawyer, so the word is
//! `lawyer` everywhere it names that tier: `persons.role`, the `Role` enum, the
//! OPA `lawyer_tier` set, the `/lawyer/*` workbench, `person_project_role`'s
//! `is_lawyer_dri`, the `lawyer_review` workflow state, the `question.audience`
//! and `answer.source` vocabularies, the email `awaiting_lawyer` /
//! `to_lawyer` / `from_lawyer` values, `navigator dev grant-lawyer`, and the
//! Rauthy fixture. `staff` said less and had to be explained every time.
//!
//! A rename spanning 487 files is easy to half-finish and easy to reintroduce:
//! a handler pasted from an old one, a feature file asserting the old URL, a
//! doc paragraph carried forward, a fixture seeding `role = 'staff'`. A stray
//! `'staff'` is not a cosmetic miss either — the `ASSERT $value IN [...]` on
//! `persons.role` rejects it, so the row fails to write rather than writing a
//! wrong tier. So the invariant is asserted rather than remembered.
//!
//! What survives is `staff` in its ordinary English sense — personnel, and the
//! verb "to staff a matter" — which never names the role. Each surviving phrase
//! is allowed explicitly rather than by exempting whole files, so a file
//! carrying allowed prose still fails if a role reference appears in it later.
//!
//! The list shrinks as the prose does. Retiring the nonprofit's public surface
//! took its marketing copy with it, and four entries here described phrases only
//! that copy used — so they were dropped rather than kept as an exemption
//! matching nothing, which is what this file's own
//! `every_allowed_phrase_still_occurs` test insists on.

use std::fs;
use std::path::{Path, PathBuf};

/// Substrings that may legitimately contain `staff`, lowercased. A line is
/// cleared of every one of these before it is checked, so a line carrying both
/// an allowed phrase and a stray role reference still fails.
const ALLOWED: &[&str] = &[
    // `webapp/src/template_gallery.rs` — the gallery's reader is a nonprofit's
    // employee, who holds no role in this application at all.
    "nonprofit staffer",
    // `server/tests/project_participation_management.rs` — "ordinary staffing
    // changes", the personnel sense again, contrasted with a DRI reassignment.
    "staffing",
    // `.claude/settings.json` — the retired word has to stay in the prompt
    // hook's *trigger* alternation, because someone still saying "staff" is
    // exactly who needs the vocabulary reminder. Allowed only in the
    // pipe-delimited regex form, so prose saying `staff` still fails.
    "|staff|",
];

/// Directories the walk never descends into: build output, VCS metadata, and
/// the local dev-environment scratch dir.
///
/// `worktrees` (no dot) is the directory the harness creates linked checkouts
/// under — `.claude/worktrees/<branch>/` is a whole second copy of the tree.
/// Without it the walk reads another branch's files and reports them against
/// this one, so a checkout that predates this rename would fail the guard on
/// somebody else's branch. CI has no such directory, which is the worst shape
/// for a guard: it would cry wolf exactly where someone is verifying a change
/// and never where it would catch one.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".worktrees",
    "worktrees",
    ".devx",
];

/// Files exempt by provenance rather than by name: only this test, whose own
/// allowlist above is written in the thing it forbids.
const SKIPPED_FILES: &[&str] = &["staff_vocabulary_retired.rs"];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The line with every allowed phrase removed. Whatever `staff` remains is a
/// genuine leftover.
fn strip_allowed(line: &str) -> String {
    let mut out = line.to_lowercase();
    for allowed in ALLOWED {
        out = out.replace(allowed, " ");
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `.git` is a directory in a normal checkout and a *file* holding a
        // `gitdir:` pointer inside a worktree, so it has to be skipped by name
        // before the directory test — otherwise the walk reads a path carrying
        // the branch name and the guard fails on the reviewer's branch.
        if SKIPPED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
            continue;
        }
        if SKIPPED_FILES.contains(&name.as_ref()) {
            continue;
        }
        // A path can carry the retired word without any line doing so — a
        // `staff_dashboard.rs` module or a `staff_forms_csrf.rs` test.
        let relative = path.strip_prefix(repo_root()).unwrap_or(&path);
        let displayed = relative.to_string_lossy().to_string();
        if strip_allowed(&displayed).contains("staff") {
            out.push(format!("{displayed}: path names the retired role"));
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            if strip_allowed(line).contains("staff") {
                out.push(format!("{displayed}:{}: {}", i + 1, line.trim()));
            }
        }
    }
}

#[test]
fn the_retired_staff_role_vocabulary_appears_nowhere_it_is_not_allowed() {
    let mut hits = Vec::new();
    walk(&repo_root(), &mut hits);
    assert!(
        hits.is_empty(),
        "the tier that performs legal work is `lawyer`; `staff` survives only \
         in its ordinary English sense (personnel, and the verb \"to staff a \
         matter\"). Found {} occurrence(s) naming the retired role:\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The exclusions have to stay reachable, or the guard silently widens: a
/// typo'd entry in `ALLOWED` would match nothing and the test would keep
/// passing while permitting less than it claims.
#[test]
fn every_allowed_phrase_still_occurs_in_the_workspace() {
    let mut unused = Vec::new();
    for allowed in ALLOWED {
        let mut found = false;
        let mut hits = Vec::new();
        search(&repo_root(), allowed, &mut found, &mut hits);
        if !found {
            unused.push(*allowed);
        }
    }
    assert!(
        unused.is_empty(),
        "these exclusions match nothing in the workspace and should be dropped \
         (or are misspelled, which would let the role vocabulary back in): \
         {unused:?}"
    );
}

fn search(dir: &Path, needle: &str, found: &mut bool, hits: &mut Vec<String>) {
    if *found {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIPPED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            search(&path, needle, found, hits);
            continue;
        }
        if SKIPPED_FILES.contains(&name.as_ref()) {
            continue;
        }
        if let Ok(body) = fs::read_to_string(&path) {
            if body.to_lowercase().contains(needle) {
                *found = true;
                hits.push(path.display().to_string());
                return;
            }
        }
    }
}
