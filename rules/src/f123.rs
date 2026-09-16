//! `N123` — an instrument the firm drafts must carry a Harvard outline.
//!
//! An agreement, a pleading, and the two engagement letters that open and
//! close a matter are outline documents: a reader cites a provision by its
//! path (`I.A`, `2.B`), a narrator steps them unit by unit on the notation's
//! own outline stage, and `pdf::outline` numbers the rendered document from
//! the same shape. A body whose sections are unnumbered prose has no path to
//! cite and nothing to step, and the defect is invisible until someone
//! needs the citation.
//!
//! The scheme is the document's, not the author's preference:
//!
//! - **Roman** (`## I.`, `## II.`) for `agreement`, `onboarding`, and
//!   `offboarding` — contracts and engagement letters.
//! - **Arabic** (`## 1.`, `## 2.`) for `pleading` — motion practice, which
//!   numbers its sections the way a court reads them.
//!
//! **A preamble is allowed.** Court paper opens with its formal title line
//! after the caption — `## ANSWER TO COUNTERCLAIM FOR BREACH OF CONTRACT`,
//! `## SUMMONS — CIVIL` — and that line is a caption element, not the first
//! section of the argument. Unnumbered `## ` headings before the first
//! numbered one are therefore left alone, the same preamble
//! `views::harvard_outline` gives depth 0. Once numbering starts it must
//! not stop: an unnumbered heading *after* the first numbered section is
//! a section that lost its marker, and is flagged. A body where numbering
//! never starts has no outline at all, which is the first thing flagged.
//!
//! Every other `kind` is exempt. A `letter` is a demand or notice the firm
//! sends on a client's behalf, often a single page of prose with no sections
//! to number; a `filing` fills a government `AcroForm` and its body is an
//! intake summary rather than the document. Neither has an outline to hold,
//! so neither is asked for one.
//!
//! **What this rule does not restate.** The seven-depth marker table
//! (`I. A. 1. a. (1) (a) (i)`) lives in `word::MARKER_GROUPS` and is read
//! from there by `pdf::outline` and `views::harvard_outline`. This rule
//! never reproduces it: it reads depth-1 headings only, where the whole
//! vocabulary is "a Roman numeral or a decimal", and leaves every deeper
//! level to the parser and the renderer that already own it. That is why
//! the lint crate does not depend on `word` — doing so would put `zip`,
//! `flate2`, and `sha2` inside `navigator-lsp` to borrow a two-variant
//! enum.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

/// How a kind numbers its depth-1 sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Roman,
    Arabic,
}

impl Scheme {
    /// The marker a first section carries, for the message that asks for it.
    const fn first_marker(self) -> &'static str {
        match self {
            Self::Roman => "I.",
            Self::Arabic => "1.",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Roman => "Roman numerals",
            Self::Arabic => "Arabic numerals",
        }
    }
}

/// The kinds that must carry an outline, and the scheme each one uses.
///
/// `inbound_contract` is deliberately absent though it is contract-shaped:
/// it names a contract the *client* uploaded for review, an asset-lane
/// classification that is never a template's own declared kind, so the firm
/// did not draft it and cannot be held to its shape.
const OUTLINED_KINDS: &[(&str, Scheme)] = &[
    ("agreement", Scheme::Roman),
    ("onboarding", Scheme::Roman),
    ("offboarding", Scheme::Roman),
    ("pleading", Scheme::Arabic),
];

pub struct F123HarvardOutlineRequired;

impl F123HarvardOutlineRequired {
    pub const CODE: &'static str = "N123";
}

/// The value of a Roman numeral written in the ASCII capitals a section
/// heading uses. `None` for anything that is not one.
fn roman_value(token: &str) -> Option<u32> {
    if token.is_empty() {
        return None;
    }
    let mut total = 0_u32;
    let mut prev = 0_u32;
    for ch in token.chars().rev() {
        let value = match ch {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => return None,
        };
        if value < prev {
            total -= value;
        } else {
            total += value;
            prev = value;
        }
    }
    Some(total)
}

/// The depth-1 marker a `## ` heading carries, as `(scheme, value, title)`.
///
/// A heading reads `## I. Client and scope` or `## 1. Client and scope`:
/// the marker is the token before the first `.`, and `title` is the text
/// after it. The title comes back so a message can suggest the corrected
/// heading without re-appending the marker it is replacing.
fn depth_one_marker(heading_text: &str) -> Option<(Scheme, u32, &str)> {
    let (marker, rest) = heading_text.split_once('.')?;
    // A bare `## Scope` has no marker; a `## 1.2 Scope` is not one either,
    // because the text after the dot must be the title, not more number.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim();
    let marker = marker.trim();
    if let Ok(value) = marker.parse::<u32>() {
        return Some((Scheme::Arabic, value, title));
    }
    roman_value(marker).map(|value| (Scheme::Roman, value, title))
}

impl Rule for F123HarvardOutlineRequired {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "Agreement, pleading, and engagement-letter bodies must carry a Harvard outline"
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let Some(kind) = frontmatter::field(fm, "kind") else {
            return Vec::new();
        };
        let Some(&(_, scheme)) = OUTLINED_KINDS.iter().find(|(name, _)| *name == kind) else {
            return Vec::new();
        };

        let flag = |line: usize, message: String| Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line,
            range: line_byte_range(&file.contents, line),
            message,
        };

        // Depth-1 sections only: `## `, never `# ` (the document title) or
        // `### ` (a deeper level this rule leaves to the parser).
        let headings: Vec<(usize, &str)> = frontmatter::body_lines(&file.contents)
            .into_iter()
            .filter_map(|(line, text)| {
                let rest = text.strip_prefix("## ")?;
                Some((line, rest.trim()))
            })
            .collect();

        // The caption's own title line is a preamble, not section one, so
        // the outline starts at the first heading that carries a marker.
        let first_marked = headings
            .iter()
            .position(|(_, text)| depth_one_marker(text).is_some());
        let Some(first_marked) = first_marked else {
            return vec![flag(
                1,
                format!(
                    "`kind: {kind}` must carry a Harvard outline; the body declares no numbered \
                     `## ` section (expected `## {} …`)",
                    scheme.first_marker()
                ),
            )];
        };

        let mut violations = Vec::new();
        let mut expected = 1_u32;
        for (line, text) in &headings[first_marked..] {
            let (line, text) = (*line, *text);
            let Some((found, value, title)) = depth_one_marker(text) else {
                violations.push(flag(
                    line,
                    format!(
                        "`## {text}` carries no outline marker; `kind: {kind}` numbers depth-1 \
                         sections with {} (expected `## {} {text}`)",
                        scheme.name(),
                        roman_or_arabic(scheme, expected),
                    ),
                ));
                expected += 1;
                continue;
            };
            if found != scheme {
                violations.push(flag(
                    line,
                    format!(
                        "`## {text}` numbers with {}; `kind: {kind}` numbers depth-1 sections \
                         with {} (expected `## {} {title}`)",
                        found.name(),
                        scheme.name(),
                        roman_or_arabic(scheme, expected),
                    ),
                ));
            } else if value != expected {
                violations.push(flag(
                    line,
                    format!(
                        "`## {text}` is section {value}; depth-1 sections run in sequence, so \
                         this one is {expected} (expected `## {} {title}`)",
                        roman_or_arabic(scheme, expected),
                    ),
                ));
            }
            expected += 1;
        }
        violations
    }
}

/// `n` written in `scheme`, for the marker a message asks the author to use.
fn roman_or_arabic(scheme: Scheme, n: u32) -> String {
    match scheme {
        Scheme::Arabic => format!("{n}."),
        Scheme::Roman => {
            const PAIRS: [(u32, &str); 13] = [
                (1000, "M"),
                (900, "CM"),
                (500, "D"),
                (400, "CD"),
                (100, "C"),
                (90, "XC"),
                (50, "L"),
                (40, "XL"),
                (10, "X"),
                (9, "IX"),
                (5, "V"),
                (4, "IV"),
                (1, "I"),
            ];
            let mut left = n;
            let mut out = String::new();
            for (value, numeral) in PAIRS {
                while left >= value {
                    out.push_str(numeral);
                    left -= value;
                }
            }
            out.push('.');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::F123HarvardOutlineRequired;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: contents.to_string(),
        }
    }

    fn tmpl(kind: &str, body: &str) -> String {
        format!("---\ntitle: T\nkind: {kind}\n---\n\n{body}\n")
    }

    fn lint(kind: &str, body: &str) -> Vec<crate::Violation> {
        F123HarvardOutlineRequired.lint(&file(&tmpl(kind, body)))
    }

    #[test]
    fn a_roman_agreement_outline_passes() {
        let body = "## I. Scope\n\nText.\n\n## II. Fees\n\nText.\n";
        assert!(
            lint("agreement", body).is_empty(),
            "{:?}",
            lint("agreement", body)
        );
    }

    #[test]
    fn an_arabic_pleading_outline_passes() {
        let body = "## 1. Introduction\n\nText.\n\n## 2. Argument\n\nText.\n";
        assert!(
            lint("pleading", body).is_empty(),
            "{:?}",
            lint("pleading", body)
        );
    }

    #[test]
    fn a_contract_numbered_like_motion_practice_is_flagged() {
        // The live drift this rule exists to catch: an engagement letter
        // numbered `## 1.` when its kind numbers with Roman numerals.
        let violations = lint("onboarding", "## 1. Client and scope\n\nText.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("Roman numerals")),
            "an Arabic contract must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn the_suggested_heading_replaces_the_wrong_marker_rather_than_keeping_it() {
        // The message proposes a corrected heading. Building it from the
        // raw heading text would suggest `## I. 1. Client and scope` — the
        // marker being replaced, pasted back in front of its replacement.
        let violations = lint("onboarding", "## 1. Client and scope\n\nText.\n");
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0]
                .message
                .contains("expected `## I. Client and scope`"),
            "{}",
            violations[0].message
        );
    }

    #[test]
    fn the_out_of_sequence_suggestion_also_drops_the_wrong_marker() {
        let body = "## I. Scope\n\nText.\n\n## III. Fees\n\nText.\n";
        let violations = lint("agreement", body);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].message.contains("expected `## II. Fees`"),
            "{}",
            violations[0].message
        );
    }

    #[test]
    fn a_pleading_numbered_like_a_contract_is_flagged() {
        let violations = lint("pleading", "## I. Introduction\n\nText.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("Arabic numerals")),
            "a Roman pleading must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_body_with_no_sections_is_flagged() {
        let violations = lint("agreement", "Just prose, no sections at all.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("no numbered `## ` section")),
            "an outline-less agreement must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_body_whose_numbering_never_starts_is_flagged() {
        // Every heading unnumbered is not a preamble — it is a document
        // with no outline, which is the shape the rule exists to catch.
        let violations = lint("agreement", "## Scope\n\n## Fees\n\nText.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("no numbered `## ` section")),
            "an unnumbered body must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_caption_title_before_the_outline_is_a_preamble() {
        // Court paper opens with its formal title line after the caption.
        // That line is not section one, and the corpus really looks like
        // this: `## SUMMONS — CIVIL` sits above `## 1.`.
        let body = "## SUMMONS — CIVIL\n\nText.\n\n## 1. You must respond\n\nText.\n\n\
                    ## 2. What happens next\n\nText.\n";
        assert!(
            lint("pleading", body).is_empty(),
            "{:?}",
            lint("pleading", body)
        );
    }

    #[test]
    fn an_unnumbered_heading_after_the_outline_starts_is_flagged() {
        // Once numbering begins it must not stop: this is a section that
        // lost its marker, not a preamble.
        let body = "## I. Scope\n\nText.\n\n## Notes\n\nText.\n";
        let violations = lint("agreement", body);
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("carries no outline marker")),
            "a dropped marker must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_skipped_section_number_is_flagged() {
        let body = "## I. Scope\n\nText.\n\n## III. Fees\n\nText.\n";
        let violations = lint("agreement", body);
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("run in sequence")),
            "a skipped numeral must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn the_flagged_line_is_the_heading_not_the_file() {
        let body = "## I. Scope\n\nText.\n\n## III. Fees\n\nText.\n";
        let violations = lint("agreement", body);
        assert_eq!(violations.len(), 1, "{violations:?}");
        // Frontmatter is 4 lines, blank, then the body starts at line 6.
        assert_eq!(violations[0].line, 10, "{violations:?}");
    }

    #[test]
    fn a_letter_is_exempt() {
        // A demand or notice letter is prose the firm sends, not an
        // outline document; it must not be asked for sections it has none of.
        assert!(lint("letter", "Dear Ms. Cruller,\n\nText.\n").is_empty());
    }

    #[test]
    fn a_filing_is_exempt() {
        assert!(lint("filing", "Intake summary prose.\n").is_empty());
    }

    #[test]
    fn an_inbound_contract_is_exempt() {
        // Contract-shaped, but the client uploaded it and the firm did not
        // draft it — an asset classification, never a template's own kind.
        assert!(lint("inbound_contract", "Whatever the client sent.\n").is_empty());
    }

    #[test]
    fn a_file_with_no_frontmatter_is_exempt() {
        assert!(F123HarvardOutlineRequired
            .lint(&file("Plain prose with a ## heading.\n"))
            .is_empty());
    }

    #[test]
    fn a_deeper_heading_is_left_to_the_parser() {
        // `### A.` and below are the parser's and the renderer's business;
        // this rule reads depth 1 only.
        let body = "## I. Scope\n\n### A. Detail\n\nText.\n";
        assert!(
            lint("agreement", body).is_empty(),
            "{:?}",
            lint("agreement", body)
        );
    }

    #[test]
    fn a_heading_inside_a_fence_is_not_a_section() {
        let body = "## I. Scope\n\n```\n## 2. not a section\n```\n\nText.\n";
        assert!(
            lint("agreement", body).is_empty(),
            "{:?}",
            lint("agreement", body)
        );
    }

    #[test]
    fn roman_numerals_read_subtractive_pairs() {
        assert_eq!(super::roman_value("IV"), Some(4));
        assert_eq!(super::roman_value("IX"), Some(9));
        assert_eq!(super::roman_value("XIV"), Some(14));
        assert_eq!(super::roman_value("MCMXCIV"), Some(1994));
        assert_eq!(super::roman_value("Scope"), None);
    }

    #[test]
    fn roman_rendering_round_trips_through_the_reader() {
        for n in 1_u32..=40 {
            let rendered = super::roman_or_arabic(super::Scheme::Roman, n);
            let numeral = rendered.trim_end_matches('.');
            assert_eq!(super::roman_value(numeral), Some(n), "{rendered}");
        }
    }
}
