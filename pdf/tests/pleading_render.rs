//! Court-paper geometry, proven through the renderer rather than through
//! the emitted string (#889).
//!
//! `pleading::preamble` producing the right Typst text is not proof that
//! the page comes out right — the compiler has to accept it, the page has
//! to be the right size, and the rail has to actually draw. These tests
//! render real PDFs and read the result back.
//!
//! # Why these assert structure, not words
//!
//! Typst embeds subset fonts, so the text-showing operators in the
//! rendered page carry **glyph ids, not Unicode** — `pdf::page_text` on a
//! Typst PDF returns `"\0\u{1}\0\u{2}…"`, and no assertion on the string
//! `"TABLE OF AUTHORITIES"` can ever pass. (That helper exists for
//! `AcroForm` PDFs, which are authored elsewhere and carry a usable
//! encoding.) So the page is pinned by geometry and by counted, positioned
//! glyph runs, which is what the rail actually is.

use lopdf::{Document, Object};
use pdf::pleading::{self, Variant};

/// US Letter in PDF points, the paper every calibration is cut to.
const US_LETTER: (f64, f64) = (612.0, 792.0);

fn render(variant: Variant, body: &str) -> Vec<u8> {
    let source = format!("{}{body}", pleading::preamble(variant));
    pdf::render(&source)
        .unwrap_or_else(|err| panic!("{} failed to compile: {err}", variant.as_str()))
}

/// The declared page box of the first page, in points.
fn page_size(bytes: &[u8]) -> (f64, f64) {
    let doc = Document::load_mem(bytes).expect("load pdf");
    let (_, page_id) = doc.get_pages().into_iter().next().expect("a first page");
    let media = doc
        .get_object(page_id)
        .and_then(Object::as_dict)
        .ok()
        .and_then(|d| d.get(b"MediaBox").ok().cloned())
        .or_else(|| {
            // Inherited from the page tree when not set on the page.
            doc.get_dictionary(page_id)
                .ok()
                .and_then(|d| d.get(b"Parent").ok().and_then(|p| p.as_reference().ok()))
                .and_then(|parent| doc.get_dictionary(parent).ok())
                .and_then(|d| d.get(b"MediaBox").ok().cloned())
        })
        .expect("a MediaBox");
    let nums: Vec<f64> = media
        .as_array()
        .expect("MediaBox array")
        .iter()
        .map(|o| {
            o.as_float()
                .map(f64::from)
                .or_else(|_| o.as_i64().map(|i| f64::from(i32::try_from(i).unwrap_or(0))))
                .expect("number")
        })
        .collect();
    (nums[2] - nums[0], nums[3] - nums[1])
}

/// Positioned glyph runs on the page. The rail is 28 separately placed
/// numbers, so this counts the thing the rail *is*.
fn glyph_runs(bytes: &[u8]) -> usize {
    pdf::page_text(bytes)
        .expect("extract")
        .split_whitespace()
        .count()
}

/// Every calibration must actually compile and pass the safety check. A
/// preamble that only looks right is worth nothing.
#[test]
fn every_calibration_renders_a_safe_pdf() {
    for variant in Variant::ALL {
        let bytes = render(
            *variant,
            "Plaintiff moves this Court for summary judgment.\n",
        );
        assert!(
            bytes.starts_with(b"%PDF"),
            "{} did not produce a PDF",
            variant.as_str()
        );
        pdf::validate_pdf(&bytes).expect("rendered pleading must pass the safety check");
    }
}

/// Court paper is US Letter under every calibration — the margins vary,
/// the sheet does not.
#[test]
fn every_calibration_is_cut_to_us_letter() {
    for variant in Variant::ALL {
        let bytes = render(*variant, "Body.\n");
        let (w, h) = page_size(&bytes);
        assert!(
            (w - US_LETTER.0).abs() < 1.0 && (h - US_LETTER.1).abs() < 1.0,
            "{} rendered {w}x{h}, expected {}x{}",
            variant.as_str(),
            US_LETTER.0,
            US_LETTER.1,
        );
    }
}

/// The rail is the railed calibration's whole point. It draws 28
/// separately placed numbers, so the railed page must carry at least 28
/// more positioned glyph runs than the same body without a rail.
#[test]
fn the_railed_calibration_draws_twenty_eight_more_runs_than_an_unrailed_one() {
    let body = "Body text.\n";
    let railed = glyph_runs(&render(Variant::NumberedRailTrial, body));
    let unrailed = glyph_runs(&render(Variant::NoRailTrial, body));

    let lines_per_page = Variant::NumberedRailTrial.calibration().lines_per_page as usize;
    assert!(
        railed >= unrailed + lines_per_page,
        "railed page carried {railed} runs against {unrailed} unrailed; \
         the {lines_per_page}-number rail did not draw"
    );
}

/// The unrailed calibrations must not draw one — an accidental rail would
/// register against nothing, which is the failure this issue exists to
/// prevent.
#[test]
fn the_unrailed_calibrations_draw_no_rail() {
    let body = "Argument.\n";
    let baseline = glyph_runs(&render(Variant::NoRailTrial, body));
    let appellate = glyph_runs(&render(Variant::Appellate, body));
    let lines_per_page = Variant::NumberedRailTrial.calibration().lines_per_page as usize;

    assert!(
        appellate < baseline + lines_per_page,
        "appellate drew rail-like runs it should not have"
    );
    // A bare body is a handful of runs, nowhere near a full-page rail.
    assert!(
        baseline < lines_per_page,
        "an unrailed page should carry only the body, got {baseline} runs"
    );
}

/// Rendering is deterministic: the same source twice is the same bytes.
/// This is what makes pinning the output meaningful at all — a renderer
/// that varied run to run could never be regression-checked.
#[test]
fn rendering_the_same_pleading_twice_is_byte_identical() {
    for variant in Variant::ALL {
        let first = render(*variant, "Comes now the Plaintiff.\n");
        let second = render(*variant, "Comes now the Plaintiff.\n");
        assert_eq!(
            first,
            second,
            "{} renders non-deterministically",
            variant.as_str()
        );
    }
}

/// Grid skips and table-of-authorities entries have to survive the
/// compiler, including a party name carrying Typst sigils.
#[test]
fn grid_skips_and_authority_entries_compile_into_the_page() {
    let body = format!(
        "{skip}TABLE OF AUTHORITIES\n\n{entry}{entry2}",
        skip = pleading::grid_skip(2),
        entry = pleading::authority_entry("Marbury v. Madison", "5 U.S. 137", 12),
        entry2 = pleading::authority_entry("Ford & Sons [Nev.]", "1 P.2d 1", 3),
    );
    let bytes = render(Variant::Appellate, &body);
    pdf::validate_pdf(&bytes).expect("a table of authorities must render safely");
    assert!(
        glyph_runs(&bytes) > 10,
        "the authorities table rendered nothing"
    );
}

/// Every calibration renders in Tinos, never in the firm's own brand stack
/// — a clean compile is not proof, because Typst silently falls back off a
/// font stack that fails to resolve, and the firm stack (GORP Serif, Noto
/// Serif) is registered with the engine too. Assert the embedded font
/// program is actually Tinos.
#[test]
fn every_calibration_embeds_tinos_not_the_firm_stack() {
    for variant in Variant::ALL {
        let bytes = render(*variant, "Defendant answers the complaint.\n");
        let embedded = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(
            embedded(b"Tinos"),
            "{} did not embed Tinos — the calibration's face fell back to \
             something else",
            variant.as_str()
        );
        assert!(
            !embedded(b"NotoSerif") && !embedded(b"GORPSerif"),
            "{} embedded the firm brand stack instead of its calibration's \
             own face",
            variant.as_str()
        );
    }
}

/// The whole point of deriving `leading` from the grid instead of echoing
/// the type size (#889, and the fix pinned in `pleading`'s own
/// `absolute_leading_completes_the_grid_unit_rather_than_echoing_the_type_size`)
/// is that consecutive baselines land exactly [`Calibration::grid_unit`]
/// apart. Proven here through the renderer: two one-line paragraphs on the
/// railed calibration must be positioned one 24pt grid step apart, not the
/// 14.5pt a naive `leading: {type_size}pt` would have produced.
#[test]
fn consecutive_lines_on_the_railed_calibration_land_one_grid_step_apart() {
    let bytes = render(
        Variant::NumberedRailTrial,
        "Plaintiff states as follows.\n\nDefendant denies each allegation.\n",
    );
    let first = pdf::locate(&bytes, "Plaintiff states as follows", 1).expect("locate line one");
    let second =
        pdf::locate(&bytes, "Defendant denies each allegation", 1).expect("locate line two");
    assert_eq!(first.rects[0].page_index, 0);
    assert_eq!(second.rects[0].page_index, 0);

    let (_, page_height) = US_LETTER;
    let delta_pt = (second.rects[0].rect.y - first.rects[0].rect.y) * page_height;
    let grid_unit = Variant::NumberedRailTrial.calibration().grid_unit;
    assert!(
        (delta_pt - grid_unit).abs() < 0.5,
        "consecutive lines landed {delta_pt}pt apart, expected exactly the \
         {grid_unit}pt grid step"
    );
}
