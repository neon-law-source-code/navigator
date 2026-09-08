//! ENG-391 pass 1 keeps "Matter" out of rendered copy on the lawyer/admin
//! surfaces it swept, without touching module names, CSS classes, or store
//! identifiers (those stay `matter_*` / `.matter-*` until a follow-on rename
//! issue, per the ENG-391 triage). This guards the fifteen files that pass
//! touched: any reappearance of the word "matter" (word-boundary, so
//! `frontmatter` and `matter_lifecycle`-style identifiers are unaffected) in a
//! rendered string literal is the regression this catches.
//!
//! Scoped to exactly the fifteen in-scope files rather than a tree walk: the
//! rest of the workspace still says "matter" throughout (`open_matter`,
//! `NAVIGATOR_SIMULATED_MATTERS`, the client portal, marketing copy, the
//! testimonial), and that is unchanged by this pass. Widening this guard's
//! file list is a decision for whichever issue sweeps that surface next, not
//! a side effect of extending this test.
//!
//! Only string-literal content is checked, not the whole line: a bare
//! identifier (`MatterRow`, `matter_lifecycle`, a local `matters` variable) or
//! a CSS class/id survives outside quotes and never enters the check, so this
//! guard cannot be satisfied by renaming an identifier. Whole-line comments
//! (`//`, `///`, `//!`) are blanked before scanning, since the doc prose in
//! these files still says "matter" deliberately and is out of this pass's
//! scope. What remains after that — quoted content that still says the
//! retired word in an ALLOWED entry below — is either a CSS/id token embedded
//! in a string (`"matter-flag"`) or fixture/diagnostic text a test chose for
//! itself (`"acme-llc matter"`), never real chrome.

use std::fs;
use std::path::PathBuf;

/// The fifteen files this pass swept, relative to the workspace root. Every
/// rendered "Matter" the ENG-391 triage put in scope for lawyer/admin copy
/// lives in one of these; nothing outside this list is scanned.
const IN_SCOPE_FILES: &[&str] = &[
    "webapp/src/matter_directory.rs",
    "webapp/src/lawyer_project_detail.rs",
    "webapp/src/project_participation.rs",
    "webapp/src/clause_editor.rs",
    "webapp/src/reask.rs",
    "webapp/src/project_edit.rs",
    "webapp/src/review.rs",
    "webapp/src/conversation.rs",
    "webapp/src/clerk.rs",
    "webapp/src/gov_forms.rs",
    "webapp/src/matter_surface.rs",
    "webapp/src/project_list.rs",
    "webapp/src/team_home.rs",
    "webapp/src/admin_landing.rs",
    "webapp/src/project_calendar.rs",
];

/// String-literal substrings (already lowercase) that may legitimately still
/// say "matter": a CSS class or DOM id this pass deliberately left unrenamed
/// (a module/identifier rename is a separate, later issue), or fixture/
/// diagnostic text a unit test chose for itself rather than real chrome.
const ALLOWED: &[&str] = &[
    // `matter_directory.rs` / `lawyer_project_detail.rs` / `matter_surface.rs`
    // / `project_list.rs` — DOM ids and CSS classes. Renaming these is the
    // follow-on identifier/CSS pass the ENG-391 triage named, not this one.
    "matter-directory",
    "matter-flag",
    "matter-dri-cell",
    "matter-dri-toggle",
    "matter-not-found",
    // `matter_directory.rs` test fixtures: `row()`'s own made-up project name,
    // and the assertion reading it back. Test data, not chrome.
    "{code} matter",
    "acme-llc matter",
    // `matter_directory.rs` — an assertion failure message, never rendered.
    "the matter still lists",
    // `project_edit.rs` — assertion failure messages, never rendered.
    "the save must post to the matter's code",
    "a matter with a shared slack channel opens with the toggle on",
    "a matter with no shared notion page opens with the toggle off",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Blank every whole-line comment (`//`, `///`, `//!`) so its prose — which
/// still says "matter" throughout these files, deliberately — never reaches
/// the literal scan below. A blanked line keeps its place so line numbers in
/// a failure report still point at the right source line.
fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                String::new()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every double-quoted string literal's content in `source`, scanned across
/// the whole file rather than line by line so a backslash-continued literal
/// (this crate's help-text constants wrap at ~100 columns with a trailing
/// `\`) is captured whole instead of being cut at each newline.
fn string_literals(source: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut literal = String::new();
        while let Some(&next) = chars.peek() {
            if next == '\\' {
                chars.next();
                if let Some(escaped) = chars.next() {
                    literal.push(escaped);
                }
                continue;
            }
            chars.next();
            if next == '"' {
                break;
            }
            literal.push(next);
        }
        literals.push(literal);
    }
    literals
}

/// Strip every `ALLOWED` substring out of `literal` (already lowercased), the
/// same clear-before-checking shape this workspace's other retired-vocabulary
/// guard uses: whatever "matter" survives afterward is a genuine finding.
fn strip_allowed(literal: &str) -> String {
    let mut out = literal.to_lowercase();
    for allowed in ALLOWED {
        out = out.replace(allowed, " ");
    }
    out
}

/// Whether `text` contains the word "matter" or "matters" at a word
/// boundary — so `matter_lifecycle`, `MatterRow`, and `matter-flag` (already
/// cleared by `strip_allowed` before this runs) do not trip it, but "No
/// matters yet." or "this matter's" would.
fn contains_word_matter(text: &str) -> bool {
    fn is_word_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    let bytes = text.as_bytes();
    let needle = b"matter";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let before_ok = i == 0 || !is_word_byte(bytes[i - 1]);
            let mut end = i + needle.len();
            if end < bytes.len() && bytes[end] == b's' {
                end += 1;
            }
            let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[test]
fn no_rendered_string_literal_in_the_swept_files_says_matter() {
    let mut hits = Vec::new();
    for relative in IN_SCOPE_FILES {
        let path = repo_root().join(relative);
        let source =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let scanned = strip_comment_lines(&source);
        for literal in string_literals(&scanned) {
            let cleared = strip_allowed(&literal);
            if contains_word_matter(&cleared) {
                hits.push(format!("{relative}: {literal:?}"));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "lawyer/admin surfaces say Project, not Matter, on this pass (see ENG-391); \
         a module name, CSS class, or store identifier may still say `matter` until \
         the follow-on rename issue, but a rendered string may not. Found {} \
         occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The exclusions have to stay reachable, or the guard silently widens: an
/// `ALLOWED` entry that matches nothing in the swept files would exempt
/// nothing and should be dropped.
#[test]
fn every_allowed_phrase_still_occurs_in_the_swept_files() {
    let mut unused = Vec::new();
    for allowed in ALLOWED {
        let mut found = false;
        for relative in IN_SCOPE_FILES {
            let path = repo_root().join(relative);
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            if source.to_lowercase().contains(allowed) {
                found = true;
                break;
            }
        }
        if !found {
            unused.push(*allowed);
        }
    }
    assert!(
        unused.is_empty(),
        "these exclusions match nothing in the swept files and should be dropped: {unused:?}"
    );
}
