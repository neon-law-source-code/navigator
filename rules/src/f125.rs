//! `N125` — a subsection under a numbered section must be a lettered block
//! quote, not a flush-left bold-led paragraph.
//!
//! `N123` numbers the depth-1 sections of an outlined document and letters
//! its depth-2 subsections, reading both forms the renderer understands: the
//! `### A.` heading and the `> **A. Label.**` block quote. What it cannot see
//! is a subsection that never announced itself as one. A drafter who writes
//!
//! ```text
//! ## VI. Conflicts
//!
//! **No Present Conflict.** The firm has run the check and found none.
//! ```
//!
//! has written an outline in their head and body prose on the page.
//! `pdf::markdown::to_typst` maps a block quote to `#quote(block: true)`, the
//! only construct in the supported subset that indents; a bold run at the
//! start of a paragraph indents nothing. The rendered PDF puts that sentence
//! flush left, exactly like the paragraph above it, so *"see Section VI(E)"*
//! points at a page with no `E.` on it.
//!
//! **Report only.** Deciding that a bold-led paragraph is a subsection rather
//! than an emphasized opening clause is the drafter's judgment, and the
//! letter it should take depends on how many subsections precede it, so this
//! rule names the drift and stops. It writes no fix.
//!
//! **What it does not restate.** The letter sequence — `A.`, `B.`, …,
//! restarting beneath each section — is `N123`'s, which already reads the
//! block-quote form through its own `quoted_subsection_marker`. Once the
//! drafter quotes the paragraph, an out-of-sequence letter is `N123`'s to
//! report. This rule therefore letters nothing and counts nothing; it only
//! says that the construct is wrong. The outlined kinds are `N123`'s too,
//! read through [`f123::is_outlined`] rather than listed again here.

use crate::{f123, frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F125OutlineSubsectionIsQuoted;

impl F125OutlineSubsectionIsQuoted {
    pub const CODE: &'static str = "N125";
}

/// The bold lead-in a flush-left subsection opens with, as
/// `(lead, label)` — `("**Costs.**", "Costs.")`.
///
/// The signature is narrow on purpose, because the alternative to a false
/// negative here is flagging a drafter's emphasis:
///
/// - **Flush left.** The line starts in column 0. A `> ` quote is already the
///   right construct, a `- ` list item is a list — which is how the shipped
///   answer writes its affirmative defenses — and an indented line is a
///   continuation of one of those.
/// - **Closed on the same line, with prose after it.** `**Label.** Text.` is
///   a label introducing a unit. A line that is bold end to end is a heading
///   the drafter spelled with emphasis, a different defect, and a bold run
///   left open is not a lead-in at all.
/// - **Punctuated like a label.** The bold run ends in `.` or `:`. That is
///   what separates `**Costs.** The client pays …` from
///   `**Notwithstanding the foregoing**, the parties agree …`, where the bold
///   is an emphasized clause inside the sentence it opens.
fn bold_lead(line: &str) -> Option<(&str, &str)> {
    let delimiter = ["**", "__"].into_iter().find(|d| line.starts_with(d))?;
    let rest = &line[delimiter.len()..];
    let close = rest.find(delimiter)?;
    let label = rest[..close].trim();
    if !label.ends_with('.') && !label.ends_with(':') {
        return None;
    }
    if rest[close + delimiter.len()..].trim().is_empty() {
        return None;
    }
    let lead = &line[..delimiter.len() * 2 + close];
    Some((lead, label))
}

impl Rule for F125OutlineSubsectionIsQuoted {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "A subsection under a numbered section must be a lettered block quote, not a \
         flush-left bold-led paragraph"
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let Some(kind) = frontmatter::field(fm, "kind") else {
            return Vec::new();
        };
        if !f123::is_outlined(&kind) {
            return Vec::new();
        }

        // Only beneath a numbered section. A `## ` that carries no marker
        // closes the run: before the outline starts that is the caption's
        // preamble — a pleading's formal title line — and after the last
        // numbered section it is the execution or attestation tail. Neither
        // is an outline unit, so a bold lead-in there is prose the drafter
        // meant. `frontmatter::body_lines` has already dropped the
        // frontmatter block and every fenced example, so a `**Label.**`
        // inside a ```text fence is never read as drift.
        let mut numbered = false;
        let mut violations = Vec::new();
        for (line, text) in frontmatter::body_lines(&file.contents) {
            if let Some(heading) = text.strip_prefix("## ") {
                numbered = f123::depth_one_marker(heading.trim()).is_some();
                continue;
            }
            if !numbered {
                continue;
            }
            let Some((lead, label)) = bold_lead(text) else {
                continue;
            };
            violations.push(Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line,
                range: line_byte_range(&file.contents, line),
                message: format!(
                    "`{lead}` leads a flush-left paragraph under a numbered section; it reads \
                     as a subsection in Markdown but renders as body prose, so a cross-reference \
                     has nothing on the page to point at. A depth-2 subsection is a block quote \
                     lettered `A.`, `B.`, … in sequence beneath its section (expected \
                     `> **A. {label}** …`)"
                ),
            });
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::F125OutlineSubsectionIsQuoted;
    use crate::{Rule, SourceFile, Violation};
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: contents.to_string(),
        }
    }

    fn lint(kind: &str, body: &str) -> Vec<Violation> {
        F125OutlineSubsectionIsQuoted.lint(&file(&format!(
            "---\ntitle: T\nkind: {kind}\n---\n\n{body}\n"
        )))
    }

    #[test]
    fn a_flush_left_bold_led_paragraph_under_a_numbered_section_is_flagged() {
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Conflicts\n\n**No Present Conflict.** The firm has run \
             the check.\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "N125");
        assert_eq!(found[0].line, 10);
        assert!(
            found[0]
                .message
                .contains("expected `> **A. No Present Conflict.** …`"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn the_same_text_as_a_lettered_block_quote_passes() {
        // The hand fix the message asks for. `N123` owns the letter it
        // carries from here on; this rule has nothing left to say.
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Conflicts\n\n> **A. No Present Conflict.** The firm has \
             run the check.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_parenthesized_label_is_the_same_drift() {
        let found = lint(
            "pleading",
            "# THE MOTION\n\n## 1. Argument\n\n**(a) No Present Conflict.** The firm has run \
             the check.\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].message.contains("`**(a) No Present Conflict.**`"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn an_underscore_delimited_lead_in_is_the_same_drift() {
        // The shipped answer writes strong with `__`, so the drift arrives
        // spelled that way as readily as with `**`.
        let found = lint(
            "will",
            "# THE WILL\n\n## I. Bequests\n\n__Residue.__ Everything not otherwise given.\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("`__Residue.__`"), "{found:?}");
    }

    #[test]
    fn ordinary_prose_under_a_numbered_section_is_untouched() {
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Fees\n\nThe client pays the fees set out in the schedule, \
             and the firm bills monthly.\n\nA second paragraph carries **emphasis** mid-sentence \
             and is still prose.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_emphasized_opening_clause_is_not_a_label() {
        // No `.` or `:` closing the bold run: the emphasis is part of the
        // sentence rather than a name for the unit beneath it.
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Fees\n\n**Notwithstanding the foregoing**, the parties \
             agree that no fee accrues before the engagement opens.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_fully_bold_line_is_not_a_lead_in() {
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Execution\n\n**IN WITNESS WHEREOF.**\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_bold_led_list_item_is_a_list_and_is_untouched() {
        // `templates/notations/neon_law/answer_to_counterclaim_nevada.md`
        // writes its affirmative defenses exactly this way, and a list is
        // not a subsection that lost its indent.
        let found = lint(
            "pleading",
            "# THE ANSWER\n\n## 3. Affirmative defenses\n\n- __First Affirmative Defense.__ The \
             Counterclaim fails to state a claim.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_bold_led_paragraph_in_the_preamble_is_untouched() {
        // A pleading opens with its formal title line, which `N123` reads as
        // a preamble at depth 0. Nothing above the first numbered section is
        // an outline unit.
        let found = lint(
            "pleading",
            "# THE MOTION\n\n## ANSWER TO COUNTERCLAIM\n\n**Counter-defendant.** Answers as \
             follows.\n\n## 1. General denial\n\nDenied.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_bold_led_paragraph_in_the_execution_tail_is_untouched() {
        // The unnumbered `## ` blocks that follow the last numbered section
        // are peers of the articles, not sections of the argument.
        let found = lint(
            "will",
            "# THE WILL\n\n## I. Bequests\n\nGiven.\n\n## SELF-PROVING AFFIDAVIT\n\n**State of \
             Nevada.** The testator appeared before me.\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_fenced_example_is_untouched() {
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Drafting notes\n\n```text\n**Costs.** Do not write a \
             subsection this way.\n```\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_exempt_kind_is_untouched() {
        // `N123` binds `agreement`, `offboarding`, `pleading`, and `will`.
        // Everything else — including `onboarding`, which renders through
        // the letter frame — has no outline to carry a subsection.
        for kind in ["post", "memo", "letter", "onboarding", "filing"] {
            let found = lint(
                kind,
                "## I. Fees\n\n**Costs.** The client pays the fees set out in the schedule.\n",
            );
            assert!(found.is_empty(), "{kind}: {found:?}");
        }
    }

    #[test]
    fn a_file_with_no_kind_is_untouched() {
        // Plain doc Markdown leads paragraphs with bold labels constantly.
        let found = F125OutlineSubsectionIsQuoted.lint(&file(
            "---\ntitle: T\n---\n\n## I. Fees\n\n**Costs.** Plain prose, no declared kind.\n",
        ));
        assert!(found.is_empty(), "{found:?}");
        let found = F125OutlineSubsectionIsQuoted
            .lint(&file("## I. Fees\n\n**Costs.** No frontmatter at all.\n"));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn every_subsection_in_a_section_is_reported_not_just_the_first() {
        let found = lint(
            "agreement",
            "# THE INSTRUMENT\n\n## I. Fees\n\n**Costs.** Text.\n\n**Invoices.** Text.\n\n\
             ## II. Term\n\n**Renewal.** Text.\n",
        );
        assert_eq!(found.len(), 3, "{found:?}");
    }

    #[test]
    fn linting_the_same_file_twice_reports_the_same_violations() {
        // The rule holds no state between runs, so a repeated validation
        // pass over an unchanged tree neither accumulates nor loses reports.
        let body = "# THE INSTRUMENT\n\n## I. Fees\n\n**Costs.** Text.\n\n## II. Term\n\n\
                    **Renewal.** Text.\n";
        let first = lint("agreement", body);
        let second = lint("agreement", body);
        assert_eq!(first.len(), 2, "{first:?}");
        assert_eq!(
            first
                .iter()
                .map(|v| (v.line, &v.message))
                .collect::<Vec<_>>(),
            second
                .iter()
                .map(|v| (v.line, &v.message))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_shipped_outlined_template_passes() {
        // `N123` accepts every template under `templates/notations/`; this
        // rule must not be the reason one of them starts failing.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates");
        let mut checked = 0_usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read templates") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "md") {
                    continue;
                }
                let contents = std::fs::read_to_string(&path).expect("read template");
                let found = F125OutlineSubsectionIsQuoted.lint(&SourceFile {
                    path: path.clone(),
                    contents,
                });
                assert!(found.is_empty(), "{}: {found:?}", path.display());
                checked += 1;
            }
        }
        assert!(checked > 10, "expected the template tree, found {checked}");
    }
}
