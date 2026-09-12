//! Harvard outline numbering for [`crate::OutputFormat::Agreement`] — seven
//! fixed depths, each a distinct marker style:
//!
//! | Depth | Marker |
//! | --- | --- |
//! | 1 | `I.` |
//! | 2 | `A.` |
//! | 3 | `1.` |
//! | 4 | `a.` |
//! | 5 | `(1)` |
//! | 6 | `(a)` |
//! | 7 | `(i)` |
//!
//! # A heading shows only its own level; a reference shows the full path
//!
//! These are two independent computations, not one falling out of the
//! other. [`preamble`] sets Typst's genuine heading numbering
//! (`#set heading(numbering: HARVARD_OUTLINE_PATTERN)`) so that Typst's
//! *own* cross-reference machinery — `<label>` and `@label` — resolves a
//! reference to a heading's full cumulative path automatically (`Section
//! I.A.1` for a depth-3 heading), exactly the way a native Typst document
//! numbers a heading reference. Left alone, that same machinery would also
//! print the *full* path next to the heading itself (a depth-2 heading
//! would print "I.A." instead of "A."), which is not this outline's
//! contract, so [`preamble`] additionally installs a `#show heading`
//! recipe that overrides only the *displayed* marker: it reads the
//! heading's own per-level count off the same counter a reference reads,
//! and formats it with only that level's marker group. The recipe never
//! touches the heading's `numbering` field, so a reference elsewhere is
//! computed by Typst's unmodified machinery and is never assumed to be
//! derivable from the on-page marker.
//!
//! # Depth eight fails loudly
//!
//! `outline-groups.at(it.level - 1)` on a level-8 heading is an
//! out-of-bounds array index — a hard Typst compile error
//! ([`crate::PdfError::Compile`]), not a silently invented eighth marker.
//! An eighth outline depth is a drafting problem (the clause needs
//! restructuring), not a rendering one, so this module does not paper over
//! it with a repeating or default marker.
//!
//! # Renumbering is Typst's, not ours
//!
//! No counter here is tracked in Rust. Every marker and every reference is
//! Typst's own `counter(heading)`, so inserting a clause anywhere in the
//! document shifts every marker and every reference below it on the next
//! compile — there is no cached number for an edit to leave stale.
//!
//! # Markdown reaches at most six of the seven depths
//!
//! `CommonMark` defines six ATX heading levels (`#` through `######`); a
//! seventh has no Markdown syntax, so [`crate::markdown::to_typst`] cannot
//! emit one from a notation body. This module's numbering and its
//! depth-eight failure apply to whatever Typst heading levels 1 through 8
//! reach the compiler — bundled Markdown bodies today reach at most level
//! 6; a caller that needs the seventh writes (or generates) a literal
//! Typst `=======` heading directly, the same escape a caller already has
//! for any Typst construct Markdown cannot express.

/// The Typst `numbering()` pattern carrying all seven marker groups, most
/// significant first. Passed once to `#set heading(numbering: ..)` so
/// Typst's reference machinery has the whole path to compute from.
pub const HARVARD_OUTLINE_PATTERN: &str = word::HARVARD_OUTLINE_PATTERN;

/// The deepest outline level this module numbers. A ninth `=` (heading
/// level 8) is refused loudly at compile time rather than silently
/// numbered — see the module-level docs.
pub const MAX_DEPTH: u8 = word::MAX_DEPTH;

/// The Typst preamble fragment that installs Harvard outline numbering:
/// the shared pattern (for Typst's own reference machinery) plus the
/// per-level marker override (for the heading's own on-page display).
/// Appended to [`crate::OutputFormat::Agreement`]'s chrome; a caller does
/// not invoke this directly.
#[must_use]
pub fn preamble() -> String {
    let groups = word::MARKER_GROUPS
        .iter()
        .map(|group| format!("\"{group}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        concat!(
            "#let outline-groups = ({groups})\n",
            "#set heading(numbering: \"{pattern}\")\n",
            "#show heading: it => {{\n",
            "  let n = counter(heading).at(it.location())\n",
            "  let own = n.last()\n",
            "  let pat = outline-groups.at(it.level - 1)\n",
            "  [#numbering(pat, own) #h(0.3em) #it.body]\n",
            "}}\n",
        ),
        groups = groups,
        pattern = HARVARD_OUTLINE_PATTERN,
    )
}

#[cfg(test)]
mod tests {
    use super::{preamble, HARVARD_OUTLINE_PATTERN, MAX_DEPTH};
    use crate::{Letterhead, OutputFormat};

    /// Compile `typst_body` (literal Typst, not Markdown) under
    /// [`OutputFormat::Agreement`]'s real chrome — the same preamble a
    /// contract renders through, [`preamble`] included. Literal Typst is
    /// deliberate here: a `<label>`/`@ref` pair and a raw level-7/8
    /// `#heading(level: ..)` call are Typst syntax
    /// [`crate::markdown::to_typst`] does not carry through unescaped (it
    /// drops raw HTML-shaped tokens and escapes a stray `#`), so these
    /// properties are proven at the Typst layer this module actually
    /// generates, exactly as [`crate::pleading`]'s own tests prove its
    /// geometry directly against `preamble` rather than through Markdown.
    /// [`markdown_reaches_the_outline_through_the_real_template_pipeline`]
    /// is the companion proof that an ordinary Markdown contract body (up
    /// to the six levels Markdown can express) numbers correctly
    /// end-to-end through [`crate::render_document`].
    fn render_raw(typst_body: &str) -> Result<Vec<u8>, crate::PdfError> {
        let source = format!(
            "{}{typst_body}",
            OutputFormat::Agreement.preamble(&Letterhead::default())
        );
        crate::render(&source)
    }

    #[test]
    fn the_pattern_and_depth_are_seven_groups() {
        assert_eq!(HARVARD_OUTLINE_PATTERN, "I.A.1.a.(1)(a)(i)");
        assert_eq!(MAX_DEPTH, 7);
    }

    #[test]
    fn preamble_installs_the_pattern_and_a_heading_show_rule() {
        let p = preamble();
        assert!(p.contains(HARVARD_OUTLINE_PATTERN));
        assert!(p.contains("#show heading: it => {"));
        assert!(p.contains("outline-groups.at(it.level - 1)"));
    }

    #[test]
    fn every_one_of_the_seven_depths_shows_only_its_own_marker() {
        let body = "= One\n== Two\n=== Three\n==== Four\n===== Five\n\
                    ====== Six\n#heading(level: 7)[Seven]\n";
        let pdf = render_raw(body).expect("all seven depths render");
        for (needle, marker) in [
            ("I. One", "depth 1"),
            ("A. Two", "depth 2"),
            ("1. Three", "depth 3"),
            ("a. Four", "depth 4"),
            ("(1) Five", "depth 5"),
            ("(a) Six", "depth 6"),
            ("(i) Seven", "depth 7"),
        ] {
            assert_eq!(
                crate::passage::occurrence_count(&pdf, needle).expect("counts"),
                1,
                "{marker} must show exactly its own marker `{needle}`"
            );
        }
        // None of the shallower headings leak a cumulative prefix onto the
        // page — "A." must render alone, never "I.A.".
        assert_eq!(
            crate::passage::occurrence_count(&pdf, "I.A.").expect("counts"),
            0,
            "a heading's own marker must never show the cumulative path"
        );
    }

    #[test]
    fn a_reference_resolves_the_full_path_independent_of_the_own_marker() {
        // The claim the module docs make explicit: the on-page marker and
        // the reference text are two different computations. A reference
        // to the depth-3 heading must show its full ancestry ("I.A.1"),
        // which is strictly more than that heading's own on-page marker
        // ("1." alone) — proof the two are not derived from each other.
        let body = "= Purchase <sec-purchase>\nText.\n\
                    == Price <sec-price>\nText.\n\
                    === Adjustments <sec-adjustments>\n\
                    Text.\n\nSee @sec-adjustments for the mechanism.\n";
        let pdf = render_raw(body).expect("labeled headings and a reference render");
        assert_eq!(
            crate::passage::occurrence_count(&pdf, "1. Adjustments").expect("counts"),
            1,
            "the heading's own marker is just its level's group"
        );
        assert_eq!(
            crate::passage::occurrence_count(&pdf, "Section I.A.1").expect("counts"),
            1,
            "the reference must resolve the full ancestor path"
        );
    }

    #[test]
    fn inserting_a_clause_renumbers_everything_below_it() {
        // No number here is cached anywhere in Rust — every marker is
        // Typst's own counter, so inserting a heading before others must
        // shift every marker below it on the very next compile.
        let before = "= First\nText.\n= Second\nText.\n";
        let after = "= Inserted\nText.\n= First\nText.\n= Second\nText.\n";

        let pdf_before = render_raw(before).expect("renders");
        assert_eq!(
            crate::passage::occurrence_count(&pdf_before, "I. First").expect("counts"),
            1
        );
        assert_eq!(
            crate::passage::occurrence_count(&pdf_before, "II. Second").expect("counts"),
            1
        );

        let pdf_after = render_raw(after).expect("renders");
        assert_eq!(
            crate::passage::occurrence_count(&pdf_after, "I. Inserted").expect("counts"),
            1,
            "the inserted clause takes the first mark"
        );
        assert_eq!(
            crate::passage::occurrence_count(&pdf_after, "II. First").expect("counts"),
            1,
            "First must renumber to II. once a clause is inserted before it"
        );
        assert_eq!(
            crate::passage::occurrence_count(&pdf_after, "III. Second").expect("counts"),
            1,
            "Second must renumber to III."
        );
        // "II. First" and "III. Second" (asserted above) are themselves the
        // proof the stale marks did not survive: had First kept its
        // pre-insertion "I." mark, the heading would read "I. First", not
        // "II. First" (a bare "I. First" substring is not a safe negative
        // check here — it is a substring of "II. First" too).
    }

    #[test]
    fn an_eighth_depth_fails_loudly_instead_of_inventing_a_marker() {
        let body = "= One\n== Two\n=== Three\n==== Four\n===== Five\n\
                    ====== Six\n#heading(level: 7)[Seven]\n#heading(level: 8)[Eight]\n";
        let err = render_raw(body).expect_err("an eighth outline depth must not compile");
        assert!(
            matches!(err, crate::PdfError::Compile(_)),
            "expected a compile error, got {err:?}"
        );
    }

    #[test]
    fn markdown_reaches_the_outline_through_the_real_template_pipeline() {
        // The end-to-end proof: an ordinary notation body, authored in
        // Markdown like any other, numbers correctly through
        // render_document — Markdown's own six-level ceiling (#889's
        // rationale in the module docs) covers a realistic contract.
        let body = "# Purchase\n\nBuyer shall purchase the Interest.\n\n\
                    ## Price\n\nThe price is stated in Schedule A.\n\n\
                    ### Adjustments\n\nSubject to customary adjustments.\n";
        let pdf = crate::render_document(body, OutputFormat::Agreement, &Letterhead::default())
            .expect("a realistic markdown contract body renders");
        for needle in ["I. Purchase", "A. Price", "1. Adjustments"] {
            assert_eq!(
                crate::passage::occurrence_count(&pdf, needle).expect("counts"),
                1,
                "{needle}"
            );
        }
    }
}
