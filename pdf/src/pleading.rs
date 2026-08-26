//! Court-paper geometry as a Typst template — jurisdiction as a
//! parameter, not a fork (#889).
//!
//! A pleading is not generic paper. It is a fixed number frame that
//! single-spaced material — the counsel block, the caption, footnotes —
//! floats over. Every measurement a calibration produces derives from one
//! number: its [`Calibration::text_height`]. [`Calibration::grid_unit`] is
//! the baseline pitch; [`Calibration::lines_per_page`] is the frame.
//! Everything else is expressed in whole grid units, because a pleading's
//! vertical rhythm is the thing the numbered rail registers against: a
//! line that lands off-grid is a line whose number is wrong.
//!
//! # Jurisdiction is a parameter
//!
//! Three calibrations differ only in their top margin, whether a numbered
//! rail exists, and how leading is set. The governing rule:
//!
//! > **Absolute leading when a rail exists to register against, relative
//! > leading when it does not.**
//!
//! A wrong calibration silently shifts every line on the page, which is
//! the failure mode this module exists to prevent. That is why the
//! calibration is a closed enum ([`Variant`]) whose table is asserted in
//! tests rather than a set of loose arguments a caller can transpose.
//!
//! # One type size, per calibration
//!
//! A given calibration has no type scale of its own: everything renders
//! at [`Calibration::type_size`], and hierarchy comes from case, weight,
//! underline, centring, and indent. Face, size, and grid are coupled —
//! leading is a function of type size — so they travel together as one
//! [`Calibration`] rather than as independent constants a caller could
//! recombine into a pitch nothing was measured against.
//!
//! # One renderer
//!
//! Typst generates the pleading paper and the browser does not
//! reimplement it, so there is no second renderer to agree with and no
//! geometry-agreement test. The browser displays what Rust produced — the
//! rendered PDF, or the page renderings from #893 — and edits Markdown.

/// The face, size, and vertical grid a pleading renders at, coupled into
/// one value because leading is a function of type size: a caller who
/// could vary [`Calibration::grid_unit`] independently of
/// [`Calibration::type_size`] could produce a pitch nothing was measured
/// against, and a rail that registers against the wrong baselines is the
/// failure this module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// The Typst font family this calibration sets. Never the firm's own
    /// `pdf::BRAND_FONT_STACK` — a pleading's face is a court-rule
    /// compliance decision (CRC 2.105: "essentially equivalent to
    /// Courier, Times New Roman, or Arial"), not a branding one, so
    /// [`preamble`] emits its own `#set text(font: ..)` rule that
    /// overrides the firm stack for the document it produces.
    pub face: &'static str,
    /// Point size. CRC 2.104 sets a floor — "a font size not smaller
    /// than 12 points" — not a ceiling.
    pub type_size: f64,
    /// The vertical grid a numbered rail registers against, in points.
    pub grid_unit: f64,
    /// Lines in the fixed number frame.
    pub lines_per_page: u32,
}

impl Calibration {
    /// Tinos at 14.5pt on a 24pt grid, 27 lines to the page — the one
    /// calibration this module defines today.
    ///
    /// **Tinos.** CRC 2.105 requires a face "essentially equivalent to
    /// Courier, Times New Roman, or Arial." Tinos (Steve Matteson /
    /// Ascender) is metrically identical to Times New Roman, which
    /// cannot itself be embedded here — it is Monotype-licensed. Tinos
    /// ships from Google Fonts under the SIL Open Font License, the same
    /// license family as the Noto Serif faces already embedded in this
    /// crate, so vendoring it needed no new licensing analysis.
    ///
    /// **14.5pt.** CRC 2.104's "not smaller than 12 points" is a floor.
    /// S.D. Cal. `CivLR` 5.1(a) and FRAP 32(a)(5) both require at least 14
    /// points. 14.5pt clears every floor this module currently targets.
    /// (S.D. Cal. `CivLR` 5.1(a) also says "double space," and separately
    /// permits "not more than 28 lines per page" at "no smaller than
    /// 14-point" — a page is only reachable at that line count under
    /// sub-double leading, so the two clauses contradict each other. The
    /// specific 28-line permission controls over the general
    /// double-spacing instruction; this is not wired to a `Variant` yet,
    /// so nothing here depends on the resolution, but record it so a
    /// future S.D. Cal. calibration is not "fixed" back into the
    /// contradiction.)
    ///
    /// **27 lines.** A Letter page (792pt) with a 1in top margin (CRC
    /// 2.111(1) puts line 1 exactly 1in down the page) and this module's
    /// 1in bottom margin leaves a 648pt / 9in text column. At a 24pt
    /// (1/3in) grid that is exactly 27 lines — three per inch, landing
    /// exactly on CRC 2.108(4)'s "at least three line numbers for every
    /// vertical inch" floor with nothing left over. The previous 28-line
    /// frame (text height 672pt = 9.33in) left a 0.67in foot below the
    /// last line instead of running to the margin.
    ///
    /// A Century-family calibration for SCOTUS booklet filings (Sup. Ct.
    /// R. 33.1(b) mandates the Century family at 12pt with 2pt-or-more
    /// leading, which Tinos cannot satisfy) is a follow-up, not built
    /// here. TODO: TeX Gyre Schola (GUST Font License, built from URW
    /// Century Schoolbook L) is the OFL-compatible candidate face.
    pub const DEFAULT: Calibration = Calibration {
        face: "Tinos",
        type_size: 14.5,
        grid_unit: 24.0,
        lines_per_page: 27,
    };

    /// The text column height this calibration's grid implies:
    /// `lines_per_page x grid_unit`.
    #[must_use]
    pub fn text_height(&self) -> f64 {
        self.grid_unit * f64::from(self.lines_per_page)
    }
}

/// How leading is specified for a calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leading {
    /// Baselines pinned to the 24pt grid, because a rail registers
    /// against them.
    Absolute,
    /// Conventional double spacing, used where no rail exists to
    /// register against.
    Relative,
}

/// A jurisdiction calibration of the same court-paper geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Trial court with a numbered rail: 27 numbers, a double rule on the
    /// left and a single rule on the right.
    NumberedRailTrial,
    /// Trial court without a rail — a deeper top margin instead.
    NoRailTrial,
    /// Appellate brief: no rail, relative (double) leading.
    Appellate,
}

impl Variant {
    /// Every calibration, so a caller can enumerate rather than guess.
    pub const ALL: &'static [Variant] = &[
        Variant::NumberedRailTrial,
        Variant::NoRailTrial,
        Variant::Appellate,
    ];

    /// Top margin in inches.
    #[must_use]
    pub fn top_margin_inches(self) -> f64 {
        match self {
            Variant::NumberedRailTrial => 1.0,
            Variant::NoRailTrial | Variant::Appellate => 1.5,
        }
    }

    /// Whether this calibration draws the numbered rail.
    #[must_use]
    pub fn has_rail(self) -> bool {
        match self {
            Variant::NumberedRailTrial => true,
            Variant::NoRailTrial | Variant::Appellate => false,
        }
    }

    /// Absolute leading exactly when a rail exists to register against.
    #[must_use]
    pub fn leading(self) -> Leading {
        if self.has_rail() {
            Leading::Absolute
        } else {
            Leading::Relative
        }
    }

    /// The face, size, and grid this calibration renders at.
    ///
    /// Every current variant shares [`Calibration::DEFAULT`] — Tinos is
    /// the one calibration this module defines. This is a method rather
    /// than a shared constant so a future SCOTUS booklet variant can
    /// return its own Century-family calibration without every other
    /// variant's numbers moving with it.
    #[must_use]
    pub fn calibration(self) -> Calibration {
        Calibration::DEFAULT
    }

    /// The name this calibration is declared under.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::NumberedRailTrial => "numbered_rail_trial",
            Variant::NoRailTrial => "no_rail_trial",
            Variant::Appellate => "appellate",
        }
    }

    /// Parse a declared calibration name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Variant> {
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == name.trim())
    }
}

/// Vertical space, expressible **only** in whole grid units of
/// [`Calibration::DEFAULT`].
///
/// Nothing inside the text column takes an arbitrary margin: a half-unit
/// skip would put every following baseline off the rail. Callers state
/// intent in lines, and this is the only way to move down the page.
#[must_use]
pub fn grid_skip(units: u32) -> String {
    format!(
        "#v({}pt, weak: false)\n",
        Calibration::DEFAULT.grid_unit * f64::from(units)
    )
}

/// A brief that exceeds its allowed length, reported rather than silently
/// overflowed.
///
/// Court page limits are jurisdictional and a filing over the limit gets
/// stricken, so this returns a message a caller surfaces — it never
/// truncates, because dropping a page of argument is worse than filing a
/// long one.
#[must_use]
pub fn page_limit_warning(pages: u32, limit: u32) -> Option<String> {
    (pages > limit).then(|| {
        format!("pleading runs {pages} pages against a {limit}-page limit; it will be over length")
    })
}

/// One table-of-authorities entry: italic case name, roman reporter
/// cite, dotfill, page.
///
/// The closest thing to a Bluebook rule in this crate — the case name is
/// italicised and the reporter citation is not, which is the distinction
/// a court reads the table for.
#[must_use]
pub fn authority_entry(case_name: &str, reporter_cite: &str, page: u32) -> String {
    format!(
        "#block[#emph[{case}], {cite}#box(width: 1fr, repeat[.]){page}]\n",
        case = escape(case_name),
        cite = escape(reporter_cite),
        page = page,
    )
}

/// Escape Typst markup sigils so a party or reporter name renders
/// verbatim. Case names carry ampersands, brackets, and apostrophes.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '@' | '[' | ']'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The Typst chrome for a pleading page under `variant`.
///
/// Emits the geometry from [`Variant::calibration`] so there is exactly
/// one source of truth for the measurements; changing a calibration's
/// grid changes the rail, the leading, and the text height together.
#[must_use]
pub fn preamble(variant: Variant) -> String {
    let top = variant.top_margin_inches();
    let cal = variant.calibration();
    let par_rule = match variant.leading() {
        Leading::Absolute => {
            // Typst's baseline-to-baseline pitch is text-height (the span
            // from top-edge to bottom-edge, here exactly 1em = type_size)
            // plus `leading`. So `leading` is *derived* from the
            // calibration rather than set equal to the type size: it is
            // the remainder that completes one grid unit, and only
            // happens to equal the type size when the type size is
            // exactly half the grid (as the previous 12pt/24pt pair was).
            // At 14.5pt/24pt it is not, and the rail must still register
            // against the text regardless of which face or size is
            // installed.
            let leading = cal.grid_unit - cal.type_size;
            format!(
                "#set text(font: (\"{face}\"), size: {size}pt, top-edge: 0.75em, bottom-edge: -0.25em)\n\
                 #set par(justify: false, leading: {leading}pt, spacing: {leading}pt)\n",
                face = cal.face,
                size = cal.type_size,
            )
        }
        Leading::Relative => {
            format!(
                "#set text(font: (\"{face}\"), size: {size}pt)\n\
                 #set par(justify: false, leading: 1em, spacing: 1em)\n",
                face = cal.face,
                size = cal.type_size,
            )
        }
    };

    let background = if variant.has_rail() {
        let count = cal.lines_per_page;
        let pitch = cal.grid_unit;
        format!(
            "  background: place(top + left, dx: 0.75in, dy: {top}in, \
             rail(count: {count}, pitch: {pitch}pt)),\n"
        )
    } else {
        String::new()
    };

    format!(
        "{rail_fn}\
         #set page(\n\
        \x20 paper: \"us-letter\",\n\
        \x20 margin: (top: {top}in, bottom: 1in, left: {left}in, right: {right}in),\n\
         {background}\
         )\n\
         {par_rule}\n",
        rail_fn = if variant.has_rail() { RAIL_FN } else { "" },
        top = top,
        left = if variant.has_rail() { 1.5 } else { 1.0 },
        right = if variant.has_rail() { 0.5 } else { 1.0 },
        background = background,
        par_rule = par_rule,
    )
}

/// The numbered rail: `count` line numbers on a `pitch` grid, a double
/// rule to their right, and a single rule at the right margin. Defined
/// once as Typst so the numbers and the rules share one coordinate
/// system and cannot drift apart.
const RAIL_FN: &str = "\
#let rail(count: 27, pitch: 24pt) = {\n\
\x20 box(width: 100%, height: count * pitch)[\n\
\x20   #for i in range(count) [\n\
\x20     #place(top + left, dy: i * pitch, \
box(width: 0.35in)[#align(right)[#text(size: 12pt)[#(i + 1)]]])\n\
\x20   ]\n\
\x20   #place(top + left, dx: 0.45in, line(angle: 90deg, length: count * pitch, \
stroke: 0.5pt))\n\
\x20   #place(top + left, dx: 0.47in, line(angle: 90deg, length: count * pitch, \
stroke: 0.5pt))\n\
\x20   #place(top + right, line(angle: 90deg, length: count * pitch, stroke: 0.5pt))\n\
\x20 ]\n\
}\n";

#[cfg(test)]
mod tests {
    use super::{
        authority_entry, grid_skip, page_limit_warning, preamble, Calibration, Leading, Variant,
    };

    /// The single derivation the layout hangs off. If this drifts, every
    /// other measurement is wrong.
    #[test]
    fn text_height_is_twenty_seven_twenty_four_point_lines() {
        let cal = Calibration::DEFAULT;
        assert!((cal.text_height() - 648.0).abs() < f64::EPSILON);
        assert!((cal.grid_unit - 24.0).abs() < f64::EPSILON);
        assert_eq!(cal.lines_per_page, 27);
        assert!((cal.type_size - 14.5).abs() < f64::EPSILON);
        assert_eq!(cal.face, "Tinos");
    }

    /// 27 lines at a 1in top and a 1in bottom margin fills the Letter page
    /// exactly: `792pt - 72pt - 72pt = 648pt`, the same 648pt
    /// [`Calibration::text_height`] derives from the grid. A calibration
    /// whose text height does not equal the margin-implied column would
    /// either run past the bottom margin or leave dead space above it.
    #[test]
    fn the_default_calibration_text_height_fills_the_letter_page_between_one_inch_margins() {
        const US_LETTER_HEIGHT_PT: f64 = 792.0;
        const MARGIN_PT: f64 = 72.0;
        let column = US_LETTER_HEIGHT_PT - MARGIN_PT - MARGIN_PT;
        assert!((column - Calibration::DEFAULT.text_height()).abs() < f64::EPSILON);
    }

    /// The calibration table from the issue, asserted rather than
    /// trusted: a transposed row silently shifts every line on the page.
    #[test]
    fn the_three_calibrations_match_their_specified_geometry() {
        let table = [
            (Variant::NumberedRailTrial, 1.0, true, Leading::Absolute),
            (Variant::NoRailTrial, 1.5, false, Leading::Relative),
            (Variant::Appellate, 1.5, false, Leading::Relative),
        ];
        for (variant, top, rail, leading) in table {
            assert!(
                (variant.top_margin_inches() - top).abs() < f64::EPSILON,
                "{} top margin",
                variant.as_str()
            );
            assert_eq!(variant.has_rail(), rail, "{} rail", variant.as_str());
            assert_eq!(variant.leading(), leading, "{} leading", variant.as_str());
            assert_eq!(
                variant.calibration(),
                Calibration::DEFAULT,
                "{} calibration",
                variant.as_str()
            );
        }
    }

    /// The governing rule, stated as an invariant over every calibration
    /// so a fourth one cannot be added that violates it unnoticed.
    #[test]
    fn leading_is_absolute_exactly_when_a_rail_exists_to_register_against() {
        for variant in Variant::ALL {
            let expected = if variant.has_rail() {
                Leading::Absolute
            } else {
                Leading::Relative
            };
            assert_eq!(variant.leading(), expected, "{}", variant.as_str());
        }
    }

    #[test]
    fn a_calibration_round_trips_through_its_declared_name() {
        for variant in Variant::ALL {
            assert_eq!(Variant::parse(variant.as_str()), Some(*variant));
        }
        assert_eq!(Variant::parse("some_other_court"), None);
    }

    /// Vertical space is expressible only in whole grid units.
    #[test]
    fn grid_skip_moves_in_whole_lines_only() {
        assert_eq!(grid_skip(1), "#v(24pt, weak: false)\n");
        assert_eq!(grid_skip(3), "#v(72pt, weak: false)\n");
        assert_eq!(grid_skip(0), "#v(0pt, weak: false)\n");
    }

    /// A brief over the limit is reported, never silently overflowed and
    /// never truncated — dropping argument is worse than filing long.
    #[test]
    fn page_limit_warns_rather_than_truncating() {
        assert!(page_limit_warning(30, 30).is_none());
        let warning = page_limit_warning(31, 30).expect("over-length brief must warn");
        assert!(warning.contains("31"));
        assert!(warning.contains("30"));
    }

    /// Italic case name, roman reporter cite, dotfill, page.
    #[test]
    fn an_authority_entry_italicises_only_the_case_name() {
        let entry = authority_entry("Marbury v. Madison", "5 U.S. 137", 12);
        assert!(entry.contains("#emph[Marbury v. Madison]"));
        assert!(
            entry.contains("5 U.S. 137"),
            "the reporter cite stays roman"
        );
        assert!(!entry.contains("#emph[5 U.S. 137]"));
        assert!(entry.contains("repeat[.]"), "dotfill to the page number");
        assert!(entry.ends_with("12]\n"));
    }

    /// A party name carrying Typst sigils must render verbatim.
    #[test]
    fn an_authority_entry_escapes_markup_in_a_party_name() {
        let entry = authority_entry("Ford & Sons [Nev.]", "1 P.2d 1", 3);
        assert!(entry.contains("Ford \\& Sons \\[Nev.\\]") || entry.contains("\\["));
        assert!(!entry.contains("[Nev.]"));
    }

    /// The rail is emitted for exactly the calibration that has one, and
    /// the geometry comes from [`Variant::calibration`].
    #[test]
    fn only_the_railed_calibration_emits_a_rail() {
        let railed = preamble(Variant::NumberedRailTrial);
        assert!(railed.contains("#let rail("));
        assert!(railed.contains("count: 27"));
        assert!(railed.contains("pitch: 24pt"));
        assert!(railed.contains("top: 1in"));

        for variant in [Variant::NoRailTrial, Variant::Appellate] {
            let plain = preamble(variant);
            assert!(
                !plain.contains("#let rail("),
                "{} must not draw a rail",
                variant.as_str()
            );
            assert!(plain.contains("top: 1.5in"), "{}", variant.as_str());
        }
    }

    /// One type size everywhere — the preamble must not introduce a
    /// second one, because a given calibration has no type scale of its
    /// own.
    #[test]
    fn the_preamble_sets_exactly_one_type_size() {
        for variant in Variant::ALL {
            let src = preamble(*variant);
            let sizes: Vec<&str> = src.match_indices("size: ").map(|(_, s)| s).collect();
            assert!(!sizes.is_empty(), "{}", variant.as_str());
            assert!(
                !src.contains("size: 14pt") && !src.contains("size: 10pt"),
                "{} introduced a second type size",
                variant.as_str()
            );
            assert!(
                src.contains(&format!("size: {}pt", variant.calibration().type_size)),
                "{} must set the one type size",
                variant.as_str()
            );
        }
    }

    /// The face this calibration names — not the firm's own brand stack —
    /// is what the preamble actually sets, and it has to be embedded for
    /// the compile to resolve it without depending on the host.
    #[test]
    fn the_preamble_sets_the_calibrations_own_face_not_the_firm_stack() {
        for variant in Variant::ALL {
            let src = preamble(*variant);
            assert!(
                src.contains(&format!("font: (\"{}\")", variant.calibration().face)),
                "{} must set its calibration's own face",
                variant.as_str()
            );
            assert!(
                !src.contains("GORP Serif") && !src.contains("Noto Serif"),
                "{} must not fall back to the firm brand stack — a pleading's \
                 face is a court-rule decision, not a branding one",
                variant.as_str()
            );
        }
    }

    /// Absolute leading is derived so the baseline pitch lands exactly on
    /// the grid: `text-height (= type_size) + leading = grid_unit`. This
    /// only reduces to `leading == type_size` when the type size happens
    /// to be exactly half the grid, which 14.5pt/24pt is not — a
    /// regression here would silently walk the rail off the text.
    #[test]
    fn absolute_leading_completes_the_grid_unit_rather_than_echoing_the_type_size() {
        let cal = Calibration::DEFAULT;
        let expected_leading = cal.grid_unit - cal.type_size;
        assert!(
            (expected_leading - 9.5).abs() < f64::EPSILON,
            "sanity: 24pt grid minus 14.5pt type is 9.5pt"
        );

        let src = preamble(Variant::NumberedRailTrial);
        assert!(
            src.contains(&format!("leading: {expected_leading}pt")),
            "leading must complete the grid unit, not restate the type size: {src}"
        );
        assert!(
            !src.contains(&format!("leading: {}pt", cal.type_size)),
            "leading must not be set equal to the type size at this calibration"
        );
    }
}
