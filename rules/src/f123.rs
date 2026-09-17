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

/// Whether the body must carry its own `# ` document title.
///
/// This follows the render frame, not preference. An engagement letter goes
/// out on firm letterhead and opens the way a letter opens — a `Re:` line and
/// a salutation — so its name lives in frontmatter and a `# ` heading would
/// print a title block above "Dear …". An instrument carries no such chrome:
/// `Kind::Will` renders with none at all, and the contract and pleading frames
/// print no name of their own, so if the body does not title the document
/// nothing does. That is the drift LAW-16 reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Title {
    /// The body opens with exactly one `# `, above the first numbered section.
    Required,
    /// The frame supplies the opening; the body has no title heading.
    FromFrame,
}

/// The kinds that must carry an outline, the scheme each one uses, and
/// whether the body titles itself.
///
/// `inbound_contract` is deliberately absent though it is contract-shaped:
/// it names a contract the *client* uploaded for review, an asset-lane
/// classification that is never a template's own declared kind, so the firm
/// did not draft it and cannot be held to its shape.
const OUTLINED_KINDS: &[(&str, Scheme, Title)] = &[
    ("agreement", Scheme::Roman, Title::Required),
    ("onboarding", Scheme::Roman, Title::FromFrame),
    ("offboarding", Scheme::Roman, Title::FromFrame),
    ("pleading", Scheme::Arabic, Title::Required),
    // LAW-16: a will drifted out of its outline with zero errors because the
    // rule never bound it. It is an instrument the firm drafts and a reader
    // cites its articles by path, so it belongs here; it renders with no
    // chrome, so it must name itself.
    ("will", Scheme::Roman, Title::Required),
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

/// The depth-2 marker a `### ` heading carries, as `(value, title)`.
///
/// Depth 2 is lettered in every scheme. `word::MARKER_GROUPS` reads
/// `I. A. 1. a. (1) (a) (i)`, and only its depth-1 root varies — upper roman
/// for contracts and instruments, decimal for motion practice — so a
/// subsection is `A.`, `B.`, … whether it sits under `## I.` or under `## 1.`.
/// That is why this takes no [`Scheme`].
fn depth_two_marker(heading_text: &str) -> Option<(u32, &str)> {
    let (marker, rest) = heading_text.split_once('.')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut letters = marker.trim().chars();
    let letter = letters.next()?;
    if letters.next().is_some() || !letter.is_ascii_uppercase() {
        return None;
    }
    Some((u32::from(letter as u8 - b'A') + 1, rest.trim()))
}

/// `n` as a depth-2 letter marker (`1` → `A.`).
///
/// Twenty-six subsections in one section is already past anything the firm
/// drafts, so beyond `Z.` the suggestion stops advising a letter rather than
/// inventing a second-round spelling the renderer does not use.
fn letter_marker(n: u32) -> String {
    u8::try_from(n)
        .ok()
        .filter(|n| (1..=26).contains(n))
        .map_or_else(
            || format!("{n}."),
            |n| format!("{}.", char::from(b'A' + n - 1)),
        )
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
        let Some(&(_, scheme, title_rule)) =
            OUTLINED_KINDS.iter().find(|(name, _, _)| *name == kind)
        else {
            return Vec::new();
        };

        let flag = |line: usize, message: String| Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line,
            range: line_byte_range(&file.contents, line),
            message,
        };

        // Every heading the outline can speak about, in document order.
        // `### ` is tested before `## ` so a deeper heading is never read as a
        // shallower one. Depth 4 and below are left to the parser and the
        // renderer that already own the seven-marker table.
        let headings: Vec<(usize, usize, &str)> = frontmatter::body_lines(&file.contents)
            .into_iter()
            .filter_map(|(line, text)| {
                [(3_usize, "### "), (2, "## "), (1, "# ")]
                    .into_iter()
                    .find_map(|(depth, prefix)| {
                        text.strip_prefix(prefix)
                            .map(|rest| (line, depth, rest.trim()))
                    })
            })
            .collect();

        let numbered = |index: &usize| {
            let (_, depth, text) = headings[*index];
            depth == 2 && depth_one_marker(text).is_some()
        };
        // The caption's own title line is a preamble, not section one, so
        // the outline starts at the first heading that carries a marker.
        let Some(first_marked) = (0..headings.len()).find(numbered) else {
            return vec![flag(
                1,
                format!(
                    "`kind: {kind}` must carry a Harvard outline; the body declares no numbered \
                     `## ` section (expected `## {} …`)",
                    scheme.first_marker()
                ),
            )];
        };
        let last_marked = (0..headings.len()).rfind(numbered).unwrap_or(first_marked);

        // LAW-16: the execution, attestation, and self-proving affidavit
        // blocks that follow the testimonium carry no number and are peers of
        // the articles in depth. They open at the first unnumbered `## ` after
        // the last numbered section and run to the end of the body. Before
        // that point an unnumbered `## ` is still a section that lost its
        // marker, and a subsection of the final numbered section is still a
        // subsection — which is why this is anchored to the last marker rather
        // than to whatever heading happens to come last.
        let tail = (last_marked + 1..headings.len()).find(|index| {
            let (_, depth, text) = headings[*index];
            depth == 2 && depth_one_marker(text).is_none()
        });
        let in_tail = |index: usize| tail.is_some_and(|start| index >= start);

        let mut violations = Vec::new();

        let titles: Vec<usize> = (0..headings.len())
            .filter(|index| headings[*index].1 == 1)
            .collect();
        match title_rule {
            Title::Required => match titles.as_slice() {
                [] => violations.push(flag(
                    headings[first_marked].0,
                    format!(
                        "`kind: {kind}` renders without a title of its own, so the body must open \
                         with one `# ` document title above the first numbered section"
                    ),
                )),
                [first, extra @ ..] => {
                    if *first > first_marked {
                        violations.push(flag(
                            headings[*first].0,
                            format!(
                                "`# {}` sits below the outline; the `# ` document title opens the \
                                 body, above the first numbered section",
                                headings[*first].2
                            ),
                        ));
                    }
                    for index in extra {
                        violations.push(flag(
                            headings[*index].0,
                            format!(
                                "`# {}` is a second `# ` heading; a document has exactly one title \
                                 and its outline sections are `## `",
                                headings[*index].2
                            ),
                        ));
                    }
                }
            },
            Title::FromFrame => {
                for index in &titles {
                    violations.push(flag(
                        headings[*index].0,
                        format!(
                            "`# {}` titles the body, but `kind: {kind}` renders on letterhead and \
                             opens with its own salutation; the document name belongs in \
                             frontmatter `title:`",
                            headings[*index].2
                        ),
                    ));
                }
            }
        }

        let mut expected = 1_u32;
        let mut expected_sub = 1_u32;
        for index in first_marked..headings.len() {
            let (line, depth, text) = headings[index];
            match depth {
                2 => {
                    expected_sub = 1;
                    let Some((found, value, title)) = depth_one_marker(text) else {
                        if !in_tail(index) {
                            violations.push(flag(
                                line,
                                format!(
                                    "`## {text}` carries no outline marker; `kind: {kind}` numbers \
                                     depth-1 sections with {} (expected `## {} {text}`)",
                                    scheme.name(),
                                    roman_or_arabic(scheme, expected),
                                ),
                            ));
                            expected += 1;
                        }
                        continue;
                    };
                    if found != scheme {
                        violations.push(flag(
                            line,
                            format!(
                                "`## {text}` numbers with {}; `kind: {kind}` numbers depth-1 \
                                 sections with {} (expected `## {} {title}`)",
                                found.name(),
                                scheme.name(),
                                roman_or_arabic(scheme, expected),
                            ),
                        ));
                    } else if value != expected {
                        violations.push(flag(
                            line,
                            format!(
                                "`## {text}` is section {value}; depth-1 sections run in sequence, \
                                 so this one is {expected} (expected `## {} {title}`)",
                                roman_or_arabic(scheme, expected),
                            ),
                        ));
                    }
                    expected += 1;
                }
                3 => {
                    // A heading outside the numbering is a peer of the
                    // articles, so it belongs at `## ` — LAW-16 asks for the
                    // level to be stated rather than left to the drafter.
                    if in_tail(index) {
                        violations.push(flag(
                            line,
                            format!(
                                "`### {text}` follows the outline but sits below it; an execution, \
                                 attestation, or affidavit block is a peer of the numbered \
                                 sections (expected `## {text}`)"
                            ),
                        ));
                        continue;
                    }
                    let Some((value, title)) = depth_two_marker(text) else {
                        violations.push(flag(
                            line,
                            format!(
                                "`### {text}` carries no outline marker; depth-2 subsections are \
                                 lettered (expected `### {} {text}`)",
                                letter_marker(expected_sub),
                            ),
                        ));
                        expected_sub += 1;
                        continue;
                    };
                    if value != expected_sub {
                        violations.push(flag(
                            line,
                            format!(
                                "`### {text}` is subsection {}; depth-2 subsections run in sequence \
                                 under their section and restart at `A.` beneath each one, so this \
                                 one is {} (expected `### {} {title}`)",
                                letter_marker(value),
                                letter_marker(expected_sub),
                                letter_marker(expected_sub),
                            ),
                        ));
                    }
                    expected_sub += 1;
                }
                _ => {}
            }
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

    /// A body for a kind that renders without chrome and so must name
    /// itself. Every `agreement`/`pleading`/`will` fixture opens with this
    /// unless it is exercising the title rule itself.
    fn titled(body: &str) -> String {
        format!("# THE INSTRUMENT\n\n{body}")
    }

    #[test]
    fn a_roman_agreement_outline_passes() {
        let body = titled("## I. Scope\n\nText.\n\n## II. Fees\n\nText.\n");
        assert!(
            lint("agreement", &body).is_empty(),
            "{:?}",
            lint("agreement", &body)
        );
    }

    #[test]
    fn an_arabic_pleading_outline_passes() {
        let body = titled("## 1. Introduction\n\nText.\n\n## 2. Argument\n\nText.\n");
        assert!(
            lint("pleading", &body).is_empty(),
            "{:?}",
            lint("pleading", &body)
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
        let body = titled("## I. Scope\n\nText.\n\n## III. Fees\n\nText.\n");
        let violations = lint("agreement", &body);
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
        let body = titled(
            "## SUMMONS — CIVIL\n\nText.\n\n## 1. You must respond\n\nText.\n\n\
             ## 2. What happens next\n\nText.\n",
        );
        assert!(
            lint("pleading", &body).is_empty(),
            "{:?}",
            lint("pleading", &body)
        );
    }

    #[test]
    fn an_unnumbered_heading_after_the_outline_starts_is_flagged() {
        // Between two numbered sections, numbering must not stop: this is a
        // section that lost its marker, not a preamble and not the execution
        // tail, which can only run to the end of the body.
        let body = titled("## I. Scope\n\nText.\n\n## Notes\n\nText.\n\n## II. Fees\n\nText.\n");
        let violations = lint("agreement", &body);
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
        let body = titled("## I. Scope\n\nText.\n\n## III. Fees\n\nText.\n");
        let violations = lint("agreement", &body);
        assert_eq!(violations.len(), 1, "{violations:?}");
        // Frontmatter is 4 lines, blank, then the body starts at line 6 with
        // the title and its blank line, so `## I. Scope` is line 8.
        assert_eq!(violations[0].line, 12, "{violations:?}");
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
        // `####` and below are the parser's and the renderer's business.
        // Depth 2 stopped being theirs with LAW-16, so a correctly lettered
        // `### A.` passes here and a deeper level is still untouched.
        let body = titled("## I. Scope\n\n### A. Detail\n\n#### (1) Deeper\n\nText.\n");
        assert!(
            lint("agreement", &body).is_empty(),
            "{:?}",
            lint("agreement", &body)
        );
    }

    #[test]
    fn a_heading_inside_a_fence_is_not_a_section() {
        let body = titled("## I. Scope\n\n```\n## 2. not a section\n```\n\nText.\n");
        assert!(
            lint("agreement", &body).is_empty(),
            "{:?}",
            lint("agreement", &body)
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

    // ---- LAW-16: the outline below depth 1, the title, and the tail ----

    #[test]
    fn a_will_is_bound_by_the_outline_at_all() {
        // The kind LAW-16 reported. It was absent from OUTLINED_KINDS, so a
        // forty-heading will drifted with zero errors reported.
        let violations = lint("will", "Some prose with no sections at all.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N123" && v.message.contains("no numbered `## ` section")),
            "an unoutlined will must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_will_numbered_and_lettered_in_the_harvard_scheme_passes() {
        let body = titled(
            "## I. Revocation\n\nText.\n\n### A. Prior wills\n\nText.\n\n\
             ### B. Codicils\n\nText.\n\n## II. Residuary estate\n\nText.\n\n\
             ### A. Distribution\n\nText.\n",
        );
        assert!(lint("will", &body).is_empty(), "{:?}", lint("will", &body));
    }

    #[test]
    fn a_subsection_out_of_sequence_is_flagged() {
        let body = titled("## I. Revocation\n\n### A. One\n\n### C. Three\n\nText.\n");
        let violations = lint("will", &body);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].message.contains("expected `### B. Three`"),
            "{}",
            violations[0].message
        );
    }

    #[test]
    fn a_subsection_carrying_no_letter_is_flagged() {
        let body = titled("## I. Revocation\n\n### Prior wills\n\nText.\n");
        let violations = lint("will", &body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("expected `### A. Prior wills`")),
            "an unlettered subsection must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn subsection_lettering_restarts_beneath_each_section() {
        // `B.` under section II would be a gap if the counter ran on from
        // section I, which is the drift the report described as "lettered
        // subsections split between levels".
        let body = titled(
            "## I. One\n\n### A. First\n\n### B. Second\n\n## II. Two\n\n### A. First\n\nText.\n",
        );
        assert!(lint("will", &body).is_empty(), "{:?}", lint("will", &body));
    }

    #[test]
    fn a_subsection_above_the_first_numbered_section_is_preamble() {
        // The shipped summons: `### To the defendant named above` sits under
        // the caption's own title line, before section 1 begins. A caption
        // element is not part of the outline and carries no letter.
        let body = titled(
            "## SUMMONS — CIVIL\n\n### To the defendant named above\n\nText.\n\n\
             ## 1. You must respond\n\nText.\n",
        );
        assert!(
            lint("pleading", &body).is_empty(),
            "{:?}",
            lint("pleading", &body)
        );
    }

    #[test]
    fn the_execution_blocks_after_the_last_section_carry_no_number() {
        // The testimonium and what follows it: peers of the articles in
        // depth, outside the numbering by design.
        let body = titled(
            "## I. Revocation\n\nText.\n\n## II. Residuary estate\n\nText.\n\n\
             ## Execution\n\nText.\n\n## Attestation\n\nText.\n\n\
             ## Self-proving affidavit\n\nText.\n",
        );
        assert!(lint("will", &body).is_empty(), "{:?}", lint("will", &body));
    }

    #[test]
    fn a_subsection_of_the_last_numbered_section_is_still_a_subsection() {
        // The tail opens at the first unnumbered `## `, not at the last
        // numbered one — otherwise a final section's own subsections would
        // fall outside the outline and stop being checked.
        let body = titled("## I. Revocation\n\n### B. Wrong\n\nText.\n\n## Execution\n\nText.\n");
        let violations = lint("will", &body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("expected `### A. Wrong`")),
            "a subsection before the tail must still be checked; got {violations:?}"
        );
    }

    #[test]
    fn a_heading_in_the_execution_tail_must_sit_at_depth_one() {
        let body = titled("## I. Revocation\n\nText.\n\n## Execution\n\n### Notary\n\nText.\n");
        let violations = lint("will", &body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("expected `## Notary`")),
            "a tail heading below depth 1 must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn an_instrument_without_a_title_is_flagged() {
        let violations = lint("will", "## I. Revocation\n\nText.\n");
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("must open with one `# ` document title")),
            "an untitled instrument must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_second_title_heading_is_flagged() {
        let body = titled("## I. Revocation\n\nText.\n\n# SECOND TITLE\n\nText.\n");
        let violations = lint("will", &body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("is a second `# ` heading")),
            "a second title must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_title_below_the_outline_is_flagged() {
        let body = "## I. Revocation\n\nText.\n\n# LATE TITLE\n\nText.\n";
        let violations = lint("will", body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("sits below the outline")),
            "a late title must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn a_letterhead_kind_keeps_its_name_in_frontmatter() {
        // An engagement letter opens with `Re:` and a salutation on firm
        // letterhead. A `# ` heading there would print a title block above
        // "Dear …", so the rule refuses one rather than requiring it — which
        // is why the two shipped engagement letters carry no title line.
        let body = "# ONBOARDING LETTER\n\n## I. Client and scope\n\nText.\n";
        let violations = lint("onboarding", body);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("frontmatter `title:`")),
            "a titled letterhead body must fail N123; got {violations:?}"
        );
    }

    #[test]
    fn an_untitled_letterhead_kind_passes() {
        let body = "## I. Client and scope\n\nText.\n\n## II. Fees\n\nText.\n";
        assert!(
            lint("onboarding", body).is_empty(),
            "{:?}",
            lint("onboarding", body)
        );
    }
}
