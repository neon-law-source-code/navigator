//! The `contract` render frame — what an executed instrument may and may
//! not carry, proven through the renderer rather than through the emitted
//! string (#889's pattern).
//!
//! See `pdf/tests/pleading_render.rs` for why these assertions are
//! structural (counted glyph runs) rather than word-based: Typst's subset
//! fonts mean a naive text extraction returns glyph ids, not Unicode, so
//! `pdf::occurrence_count` (the `/ToUnicode` `CMap` route) is what
//! actually finds a quote.
//!
//! LAW-14 set this frame's contract. A contract is not firm
//! correspondence, and the numbering belongs to the document: `N123`
//! already requires the body to carry its own Harvard markers so a reader
//! can cite `I.A` and `views::harvard_outline` can step the notation
//! stage. The frame numbered them a second time on top of that, so a
//! section written `## I. Scope` reached the page as `A. I. Scope`.

use pdf::{Letterhead, OutputFormat};

/// A contract body in the shape `N123` requires: depth-1 sections
/// carrying their own Roman markers.
const BODY: &str = "# Master Services Agreement\n\n\
                    ## I. Scope\n\n\
                    The parties agree as follows.\n\n\
                    ### Payment\n\n\
                    Fees are due on receipt.\n\n\
                    ## II. Term\n\n\
                    This agreement begins on the effective date.\n";

fn render(body: &str, format: OutputFormat) -> Vec<u8> {
    pdf::render_document(body, format, &Letterhead::default()).expect("renders")
}

#[test]
fn the_contract_frame_carries_no_firm_letterhead() {
    // An instrument that gets executed must not go out over the drafter's
    // letterhead. The `letter` frame keeps it; the contract frame prints
    // the firm's contact line nowhere, and carries none of the embedded
    // logo bytes that made it twice the size of a plain render.
    let contract = render(BODY, OutputFormat::Contract);
    let letterhead = Letterhead::default();
    for mark in [letterhead.email.as_str(), letterhead.web.as_str()] {
        assert_eq!(
            pdf::occurrence_count(&contract, mark).expect("scan the rendered pdf"),
            0,
            "the contract frame must not print the firm contact line (`{mark}`)"
        );
    }

    let letter = render(BODY, OutputFormat::Letter(pdf::LetterBlocks::default()));
    assert!(
        contract.len() < letter.len(),
        "a letterhead-free contract ({}) must be smaller than the letter frame ({}) \
         — an embedded logo is still riding along",
        contract.len(),
        letter.len()
    );
}

#[test]
fn the_contract_frame_does_not_number_a_heading_the_body_already_labels() {
    // The double-numbering LAW-14 reports: the frame auto-numbered every
    // heading into its own Harvard outline, so the `I.` the body is
    // *required* to carry collided with an `A.` the frame invented — and
    // the document title consumed depth 1, shifting the whole scheme.
    let pdf = render(BODY, OutputFormat::Contract);

    for authored in ["I. Scope", "II. Term"] {
        assert_eq!(
            pdf::occurrence_count(&pdf, authored).expect("scan the rendered pdf"),
            1,
            "the body's own marker `{authored}` must reach the page exactly once"
        );
    }
    for invented in ["A. I. Scope", "B. II. Term", "I. Master Services Agreement"] {
        assert_eq!(
            pdf::occurrence_count(&pdf, invented).expect("scan the rendered pdf"),
            0,
            "the frame must not add a marker of its own (`{invented}`)"
        );
    }
}

#[test]
fn an_unlabelled_heading_is_left_exactly_as_written() {
    // The sub-heading carries no marker at all. Left alone it must print
    // as written rather than acquiring one — the frame has no opinion
    // about numbering, which is the whole change.
    let pdf = render(BODY, OutputFormat::Contract);
    assert_eq!(
        pdf::occurrence_count(&pdf, "Payment").expect("scan the rendered pdf"),
        1
    );
    assert_eq!(
        pdf::occurrence_count(&pdf, "1. Payment").expect("scan the rendered pdf"),
        0,
        "an unlabelled heading must not be numbered for the author"
    );
}

#[test]
fn agreement_is_still_accepted_as_the_frame_name() {
    // The frame was called `agreement`; "contract" is the plainer word and
    // the one clients use. The old spelling stays accepted for a release
    // so a template carrying `output: agreement` keeps rendering.
    assert_eq!(
        OutputFormat::parse("contract"),
        Some(OutputFormat::Contract)
    );
    assert_eq!(
        OutputFormat::parse("agreement"),
        Some(OutputFormat::Contract)
    );
    assert!(
        OutputFormat::FRONTMATTER_VALUES.contains(&"contract"),
        "`contract` is the name the frame is documented and offered under"
    );
}
