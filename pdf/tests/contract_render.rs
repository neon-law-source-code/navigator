//! Contract outline geometry, proven through the renderer rather than
//! through the emitted string (#889's pattern, extended to
//! `OutputFormat::Agreement` and `pdf::outline`).
//!
//! See `pdf/tests/pleading_render.rs` for why these assertions are
//! structural (counted, positioned glyph runs) rather than word-based:
//! Typst's subset fonts mean a naive text extraction returns glyph ids,
//! not Unicode, so `pdf::locate`/`pdf::occurrence_count` (the
//! `/ToUnicode` `CMap` route) is what actually finds a quote.
//!
//! `pdf::outline`'s own unit tests already prove the numbering mechanism
//! in isolation (bare Typst headings, no letterhead). These tests prove
//! the same properties through the real, published entry point a caller
//! actually uses — `OutputFormat::Agreement` — and add the one case
//! ENG-106 asks for specifically: a cross-reference that must follow an
//! inserted clause, not just a heading's own marker.

use pdf::{Letterhead, OutputFormat};

fn render(body: &str) -> Vec<u8> {
    pdf::render_document(body, OutputFormat::Agreement, &Letterhead::default())
        .expect("agreement renders")
}

/// A `<label>`/`@ref` pair is Typst syntax `pdf::markdown::to_typst` does
/// not carry through unescaped (a bare `<name>` parses as an HTML tag and
/// is dropped, per that module's own docs), so a cross-reference cannot
/// be authored inside a Markdown notation body today. This helper renders
/// literal Typst through the *same* `OutputFormat::Agreement` chrome
/// `render_document` uses, matching `pdf/tests/pleading_render.rs`'s own
/// pattern of compiling hand-written Typst directly against a format's
/// `preamble` for exactly this reason.
fn render_raw_typst(typst_body: &str) -> Vec<u8> {
    let source = format!(
        "{}{typst_body}",
        OutputFormat::Agreement.preamble(&Letterhead::default())
    );
    pdf::render(&source).expect("agreement with raw typst headings renders")
}

/// A realistic six-level contract body (Markdown's own ceiling — see
/// `pdf::outline`'s module docs) numbers every depth correctly through the
/// real `OutputFormat::Agreement` entry point, not just a bare Typst
/// heading.
#[test]
fn all_markdown_reachable_outline_depths_number_correctly_through_agreement() {
    let body = "# Purchase\n\n\
                Buyer shall purchase the Interest.\n\n\
                ## Price\n\n\
                The price is stated in Schedule A.\n\n\
                ### Adjustments\n\n\
                Subject to customary adjustments.\n\n\
                #### Escrow\n\n\
                Held by a mutually agreeable escrow agent.\n\n\
                ##### Release Conditions\n\n\
                Released upon closing.\n\n\
                ###### Timing\n\n\
                Within five business days.\n";
    let pdf = render(body);
    for (needle, depth) in [
        ("I. Purchase", 1),
        ("A. Price", 2),
        ("1. Adjustments", 3),
        ("a. Escrow", 4),
        ("(1) Release Conditions", 5),
        ("(a) Timing", 6),
    ] {
        assert_eq!(
            pdf::occurrence_count(&pdf, needle).expect("counts"),
            1,
            "depth {depth} marker `{needle}` missing or wrong"
        );
    }
    // None of the shallower headings leak a cumulative path onto the
    // page — a heading shows only its own level's marker.
    assert_eq!(
        pdf::occurrence_count(&pdf, "I.A.").expect("counts"),
        0,
        "a heading's own marker must never show the cumulative path"
    );
}

/// An off-by-one outline level is exactly what this suite exists to
/// catch: swapping the depth-3 marker (`1.`) for the depth-2 marker
/// (`A.`) on the same heading must fail, because the test asserts the
/// *specific* marker glyphs at that heading, not merely that some marker
/// is present. Demonstrated directly rather than asserted as a
/// tautology: render the correct fixture and the wrong one, and show
/// only the correct one satisfies the check.
#[test]
fn an_off_by_one_outline_level_is_caught_by_the_marker_assertion() {
    let correct = "# One\n\n## Two\n\n### Three\n";
    let pdf_correct = render(correct);
    assert_eq!(
        pdf::occurrence_count(&pdf_correct, "1. Three").expect("counts"),
        1,
        "the correct fixture must show the depth-3 marker"
    );
    assert_eq!(
        pdf::occurrence_count(&pdf_correct, "A. Three").expect("counts"),
        0,
        "the correct fixture must not show a depth-2 marker on a depth-3 heading"
    );

    // The off-by-one mistake: "Three" demoted to depth 2 while "Two" stays
    // at depth 2 too — the shape a bad diff produces. It renders (Typst
    // is happy either way), but under the depth-3 assertion above it is
    // wrong, which is exactly the point: the assertion is sensitive to
    // the shift, not merely to *a* marker being present.
    let off_by_one = "# One\n\n## Two\n\n## Three\n";
    let pdf_wrong = render(off_by_one);
    assert_eq!(
        pdf::occurrence_count(&pdf_wrong, "1. Three").expect("counts"),
        0,
        "the off-by-one fixture must not satisfy the depth-3 assertion"
    );
}

/// The case ENG-106 names specifically: a cross-reference must follow an
/// inserted clause, not just the referenced heading's own marker. Render
/// the same three clauses with and without a clause inserted ahead of the
/// referenced one, and confirm the reference text — not merely the
/// heading's own marker — advances.
#[test]
fn a_cross_reference_follows_an_inserted_clause() {
    let before = "= Purchase <sec-purchase>\n\
                  Buyer shall purchase the Interest.\n\n\
                  == Price <sec-price>\n\
                  See @sec-price for the price term.\n";
    let pdf_before = render_raw_typst(before);
    assert_eq!(
        pdf::occurrence_count(&pdf_before, "A. Price").expect("counts"),
        1
    );
    assert_eq!(
        pdf::occurrence_count(&pdf_before, "Section I.A").expect("counts"),
        1,
        "the reference must resolve Price's full ancestor path before any insertion"
    );

    // Insert a new top-level clause ahead of Purchase. Price's own depth-2
    // position is unchanged, but its top-level ancestor shifts from I to
    // II, so the *reference* to it must now read "Section II.A" — the
    // number an unwary manual cross-reference would leave stale at
    // "Section I.A".
    let after = "= Preamble\n\
                 Recitals go here.\n\n\
                 = Purchase <sec-purchase>\n\
                 Buyer shall purchase the Interest.\n\n\
                 == Price <sec-price>\n\
                 See @sec-price for the price term.\n";
    let pdf_after = render_raw_typst(after);
    assert_eq!(
        pdf::occurrence_count(&pdf_after, "A. Price").expect("counts"),
        1,
        "Price's own on-page marker is unaffected by a sibling top-level insertion"
    );
    assert_eq!(
        pdf::occurrence_count(&pdf_after, "Section II.A").expect("counts"),
        1,
        "the reference must follow the inserted clause to Section II.A"
    );
    assert_eq!(
        pdf::occurrence_count(&pdf_after, "Section I.A").expect("counts"),
        0,
        "the reference must not still read the pre-insertion Section I.A"
    );
}
