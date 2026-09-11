//! Every autofix must behave identically on a document and on its CRLF
//! twin, and must never leave a lone LF behind in a CRLF file.
//!
//! Git for Windows defaults to `core.autocrlf=true` and this tree
//! carries no `.gitattributes`, so a Windows checkout materialises every
//! tracked `.md` with CRLF, and `engine` reads them with
//! `fs::read_to_string`, which performs no newline translation. CI has
//! no non-Linux runner and every unit test in this crate builds its
//! input from inline LF literals, so nothing else in the suite exercises
//! this.
//!
//! `navigator validate --fix` is not a dry run — `cli`'s `fix_directory`
//! writes the result back with `fs::write`, and the flag documents these
//! autofixes as "safe-by-construction". It is also pointed at *other*
//! repositories' working trees. A fix that corrupts a line ending is
//! therefore a defect in someone else's checkout, not just this one.
//!
//! This is a table over every fix-capable rule rather than a case per
//! rule, so a rule added later is one row here and cannot quietly ship
//! without the guarantee.

use rules::{Rule, SourceFile, TextEdit};
use std::path::PathBuf;

/// Each fix-capable rule paired with a document that violates it.
fn cases() -> Vec<(&'static str, Box<dyn Rule>, &'static str)> {
    vec![
        (
            "M009",
            Box::new(rules::m009::M009NoTrailingSpaces),
            "a   \nb\n",
        ),
        ("M010", Box::new(rules::m010::M010NoHardTabs), "a\tb\n"),
        (
            "M012",
            Box::new(rules::m012::M012NoMultipleBlanks),
            "a\n\n\n\nb\n",
        ),
        (
            "M018",
            Box::new(rules::m018::M018NoMissingSpaceATX),
            "#Heading\n",
        ),
        (
            "M019",
            Box::new(rules::m019::M019NoMultipleSpaceATX),
            "#   Heading\n",
        ),
        (
            "M020",
            Box::new(rules::m020::M020NoMissingSpaceClosedATX),
            "## Heading##\n",
        ),
        (
            "M021",
            Box::new(rules::m021::M021NoMultipleSpaceClosedATX),
            "## Heading  ##\n",
        ),
        (
            "M027",
            Box::new(rules::m027::M027NoMultipleSpaceBlockquote),
            ">  quoted\n",
        ),
        (
            "M030",
            Box::new(rules::m030::M030ListMarkerSpace),
            "*  item\n",
        ),
        (
            "M037",
            Box::new(rules::m037::M037NoSpaceInEmphasis),
            "This * is * not fine.\n",
        ),
        (
            "M038",
            Box::new(rules::m038::M038NoSpaceInCode),
            "a ` code ` b\n",
        ),
        (
            "M039",
            Box::new(rules::m039::M039NoSpaceInLinks),
            "[ text ](u)\n",
        ),
        (
            "M047",
            Box::new(rules::m047::M047SingleTrailingNewline),
            "a\n\n",
        ),
        // Three 50-character lines pack to two, so the reflow has to
        // author a line ending of its own — the one fix in the table
        // that can leave a lone LF behind in a CRLF document.
        (
            "S102",
            Box::new(rules::s102::S102LinePacking::default()),
            "The quick brown fox jumps over the lazy dog again.\nThe quick brown fox jumps over the lazy dog again.\nThe quick brown fox jumps over the lazy dog again.\n",
        ),
    ]
}

fn source(contents: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from("t.md"),
        contents: contents.to_string(),
    }
}

/// Apply every fix the rule offers, highest offset first so earlier
/// offsets stay valid — the same order `cli`'s `fix_directory` and the
/// LSP's `source.fixAll` use.
fn apply_all(rule: &dyn Rule, contents: &str) -> String {
    let file = source(contents);
    let mut edits: Vec<TextEdit> = rule
        .lint(&file)
        .iter()
        .filter_map(|v| rule.fix(&file, v))
        .collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut out = file.contents.clone();
    for edit in &edits {
        out.replace_range(edit.range.clone(), &edit.new_text);
    }
    out
}

/// A rule that reports on LF must report the same on CRLF. `M047`
/// regressed exactly here: `ends_with("\n\n")` is false for `\r\n\r\n`,
/// so it silently stopped flagging surplus trailing newlines. A fixed-
/// output assertion alone would not have caught it, because a rule that
/// reports nothing also changes nothing.
#[test]
fn every_rule_reports_the_same_count_on_a_crlf_twin() {
    for (code, rule, lf) in cases() {
        let crlf = lf.replace('\n', "\r\n");
        assert_eq!(
            rule.lint(&source(lf)).len(),
            rule.lint(&source(&crlf)).len(),
            "{code} disagrees between LF and CRLF on {lf:?}"
        );
    }
}

/// A rule that fixes on LF must produce the same text on CRLF. `M009`
/// regressed exactly here: it anchors its edit to `line_byte_range(..).end`,
/// which used to include the `\r`, so its fix deleted the carriage
/// return instead of the trailing space it was called to remove. Its
/// violation count was identical on both twins, so only comparing the
/// output catches it.
#[test]
fn every_rule_fixes_a_crlf_twin_to_the_same_text() {
    for (code, rule, lf) in cases() {
        let crlf = lf.replace('\n', "\r\n");
        let fixed_lf = apply_all(rule.as_ref(), lf);
        let fixed_crlf = apply_all(rule.as_ref(), &crlf);
        assert_eq!(
            fixed_lf.replace('\n', "\r\n"),
            fixed_crlf,
            "{code} fixes the CRLF twin differently"
        );
    }
}

/// No fix may downgrade a CRLF document to mixed line endings. This is
/// the user-visible damage: a whole-file diff on a line nobody edited.
#[test]
fn no_fix_leaves_a_lone_lf_in_a_crlf_document() {
    for (code, rule, lf) in cases() {
        let crlf = lf.replace('\n', "\r\n");
        let fixed = apply_all(rule.as_ref(), &crlf);
        assert!(
            !fixed.replace("\r\n", "").contains('\n'),
            "{code} left a lone LF in a CRLF document: {fixed:?}"
        );
    }
}

/// Re-linting a fixed CRLF document must be clean, so `--fix` converges
/// in one pass on Windows exactly as it does on Linux. `M009` failed
/// this too: deleting the `\r` left the trailing space in place, so the
/// violation survived its own fix.
#[test]
fn fixing_a_crlf_document_actually_clears_the_violation() {
    for (code, rule, lf) in cases() {
        let crlf = lf.replace('\n', "\r\n");
        let fixed = apply_all(rule.as_ref(), &crlf);
        assert!(
            rule.lint(&source(&fixed)).is_empty(),
            "{code} still reports after fixing its own CRLF document: {fixed:?}"
        );
    }
}
