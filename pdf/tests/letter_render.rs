//! Letter geometry, proven through the renderer rather than through the
//! emitted string (#889's pattern, extended to `OutputFormat::Letter`).
//!
//! `pdf/tests/pleading_render.rs` explains why these assertions are
//! structural rather than word-based: Typst's subset-font text-showing
//! operators carry glyph ids, not Unicode, in a naive extraction, so
//! `pdf::locate`/`pdf::occurrence_count` (the `/ToUnicode` `CMap` route) are
//! what actually finds a quote in a rendered PDF.
//!
//! The absent case is the one that matters most for a letter: a block
//! that renders nothing must not still leave a blank paragraph or stray
//! spacing behind, and that shows up as a vertical offset, never as text
//! a word-based assertion could catch.

use std::fmt::Write as _;

use pdf::{Closing, LetterBlocks, Letterhead, OutputFormat};

fn render(blocks: LetterBlocks, body: &str) -> Vec<u8> {
    pdf::render_document(body, OutputFormat::Letter(blocks), &Letterhead::default())
        .expect("letter renders")
}

fn body_start_y(pdf_bytes: &[u8], needle: &str) -> f64 {
    pdf::locate(pdf_bytes, needle, 1)
        .unwrap_or_else(|e| panic!("locate {needle:?}: {e}"))
        .rects[0]
        .rect
        .y
}

/// Every block present, in the declared order: date, delivery method,
/// recipient, `Re:`, salutation — each is a distinct glyph run at a
/// strictly increasing vertical position, never collapsed into or
/// skipping past a neighbor.
#[test]
fn every_optional_block_present_renders_in_order_above_the_body() {
    let blocks = LetterBlocks {
        date: Some("September 6, 2026".to_string()),
        delivery_method: Some("VIA CERTIFIED MAIL".to_string()),
        recipient: vec!["Jane Client".to_string(), "123 Main St".to_string()],
        re_line: Some("Termination of Tenancy".to_string()),
        salutation: Some("Dear Ms. Client:".to_string()),
        closing: Some(Closing {
            valediction: "Sincerely,".to_string(),
            signer_name: "Jane Attorney".to_string(),
            signer_title: Some("Attorney for Client".to_string()),
        }),
        enclosures: vec!["Notice of Termination".to_string()],
        cc: vec!["John Cc".to_string()],
    };
    let pdf = render(blocks, "This letter concerns the above-referenced tenancy.");

    let order = [
        "September 6, 2026",
        "VIA CERTIFIED MAIL",
        "Jane Client",
        "Re: Termination of Tenancy",
        "Dear Ms. Client:",
        "This letter concerns",
        "Sincerely,",
        "Jane Attorney",
        "Enclosures:",
        "cc:",
    ];
    let mut previous = f64::MIN;
    for needle in order {
        let y = body_start_y(&pdf, needle);
        assert!(
            y >= previous,
            "{needle:?} rendered above the previous block in the order (y={y}, previous={previous})"
        );
        previous = y;
    }
}

/// The negative half, and the one ENG-106 calls out as the case that
/// matters: a letter with every optional block absent must start its
/// body at the same vertical position a letter with them present pushes
/// it well past — a stray blank paragraph from a "vanished" block would
/// show up here as an offset, not as text.
#[test]
fn absent_blocks_leave_the_body_where_the_bare_letterhead_puts_it() {
    let bare = render(LetterBlocks::default(), "Body of the letter.");
    let dressed = render(
        LetterBlocks {
            date: Some("September 6, 2026".to_string()),
            delivery_method: Some("VIA CERTIFIED MAIL".to_string()),
            recipient: vec!["Jane Client".to_string()],
            re_line: Some("A Matter".to_string()),
            salutation: Some("Dear Ms. Client:".to_string()),
            ..LetterBlocks::default()
        },
        "Body of the letter.",
    );

    let bare_y = body_start_y(&bare, "Body of the letter.");
    let dressed_y = body_start_y(&dressed, "Body of the letter.");
    assert!(
        dressed_y > bare_y,
        "every block present must push the body below the bare letterhead position \
         (bare y={bare_y}, dressed y={dressed_y})"
    );

    // And the finer-grained claim: absent one at a time, none of them
    // moves the body from the all-absent position — each vanishes
    // completely on its own, not merely "less visibly."
    for (field, only) in [
        (
            "date",
            LetterBlocks {
                date: Some("September 6, 2026".to_string()),
                ..LetterBlocks::default()
            },
        ),
        (
            "salutation",
            LetterBlocks {
                salutation: Some("Dear Ms. Client:".to_string()),
                ..LetterBlocks::default()
            },
        ),
    ] {
        let with_one = render(only, "Body of the letter.");
        let with_one_y = body_start_y(&with_one, "Body of the letter.");
        assert!(
            with_one_y > bare_y,
            "setting only `{field}` must still move the body below the all-absent position"
        );
    }
}

/// A page separated from the rest of the letter should identify itself —
/// nothing did before this issue. Silent on page one, where the
/// letterhead already carries the identity.
#[test]
fn continuation_pages_repeat_the_recipient_and_date() {
    let blocks = LetterBlocks {
        date: Some("September 6, 2026".to_string()),
        recipient: vec!["Jane Client".to_string()],
        ..LetterBlocks::default()
    };
    let mut body = String::new();
    for n in 1..=60 {
        write!(
            body,
            "Paragraph {n}. Filler text long enough to help force the letter across \
             more than one page eventually.\n\n"
        )
        .expect("writing to a String never fails");
    }
    let pdf = render(blocks, &body);
    let pages = pdf::page_count(&pdf).expect("page count");
    assert!(pages > 1, "fixture must actually span pages: {pages}");
    let continuation_pages = pages - 1;
    assert_eq!(
        pdf::occurrence_count(&pdf, "Jane Client").expect("counts"),
        1 + continuation_pages,
        "the recipient must appear once on page one and once per continuation page"
    );
}
