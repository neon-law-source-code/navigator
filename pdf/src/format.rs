//! Output formats — the chrome wrapped around a rendered notation.
//!
//! A notation template's Markdown body says *what* the document says;
//! the [`OutputFormat`] says *how it is dressed*: a plain document, a
//! firm **letter** on Neon Law letterhead with the logo at the top, or
//! an **agreement** — the same letterhead, typeset curtly for a
//! contract.
//!
//! This is the extension seam — a new form (pleading paper, a fax
//! cover, an invoice) is a new [`OutputFormat`] variant plus the Typst
//! [`OutputFormat::preamble`] that frames it. The body conversion
//! ([`crate::markdown::to_typst`]) and the embedded logo are shared, so
//! a new variant only describes its own page chrome.
//! [`OutputFormat::Agreement`] is the worked example: it reuses the
//! shared `letterhead_block` verbatim and differs from
//! [`OutputFormat::Letter`] in nothing but page geometry and spacing.
//!
//! The set of formats a template may *declare* in its `output:`
//! frontmatter field is validated by the `rules` crate's `N109` rule;
//! keep [`OutputFormat::FRONTMATTER_VALUES`] in step with it.

use crate::{render, PdfError, LOGO_PATH};

/// The firm identity printed on a letterhead.
///
/// Every letterhead-bearing format draws it from the same block, so
/// [`OutputFormat::Letter`] and [`OutputFormat::Agreement`] carry
/// identical marks and differ only below it.
///
/// The `pdf` crate is brand-agnostic: the caller supplies these lines.
/// [`Default`] is the firm's canonical identity, and the CLI renders that
/// default rather than assembling one per render — see the note at the
/// `run_render` call site for why that trade was made deliberately.
///
/// Beneath the wordmark and the rule sits **one** line holding every way
/// to reach the firm — voice line, mailbox, website — so a reader finds
/// them all in a single place instead of scanning a block.
///
/// The letterhead **publishes no street address**. The firm's postal
/// address is a private-mailbox suite that nothing is delivered to and no
/// client visits, so printing it on a letter points the reader at a door
/// that does not answer. The registered address stays in the website
/// footer, where a registered address belongs.
///
/// An empty field drops cleanly rather than printing a gap, so a
/// deployment that publishes no phone still renders a correct letterhead.
#[derive(Debug, Clone)]
pub struct Letterhead {
    /// Firm display name, e.g. `Neon Law` — the trading brand alone. A
    /// wordmark is read at a glance across the top of the page, so it
    /// carries the name a client knows the firm by; the entity of record
    /// belongs in the document's own signature block, where a reader
    /// looking for who is on the hook will go for it. Printed in
    /// letterspaced capitals, so supply it in its natural case.
    pub name: String,
    /// The firm's voice line in dialable form, e.g. `+1 510 800 2080`.
    pub phone: String,
    /// The inbox a reader should write to, e.g. `contact@neonlaw.com`.
    pub email: String,
    /// The firm's website as a reader would type it, e.g.
    /// `www.neonlaw.com` — also where a client reads their own files.
    pub web: String,
}

impl Default for Letterhead {
    fn default() -> Self {
        Self {
            name: "Neon Law".to_string(),
            phone: "+1 510 800 2080".to_string(),
            email: "contact@neonlaw.com".to_string(),
            web: "www.neonlaw.com".to_string(),
        }
    }
}

/// How a rendered notation is framed on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// No letterhead: page geometry and the firm typeface only. The
    /// default when a template declares no `output:` field.
    #[default]
    Plain,
    /// A firm letter on the firm's letterhead — the mark, the wordmark,
    /// a rule across the page, and the contact line head the first page;
    /// the body flows beneath and every page is numbered.
    ///
    /// Typeset **airily**: wide margins, open leading, generous space
    /// around headings. An engagement letter is read once, carefully, by
    /// someone deciding whether to sign it.
    Letter,
    /// An executed contract on the firm's letterhead. Same chrome as
    /// [`OutputFormat::Letter`], deliberately **curt** typesetting: a
    /// contract between represented parties is a reference document,
    /// navigated by section number, not a letter to be read through. It
    /// wants density and a short page count, so the margins, leading,
    /// paragraph spacing, and heading space all tighten, and headings
    /// sit at body size so a section number reads as a label rather
    /// than a title.
    Agreement,
}

impl OutputFormat {
    /// The `output:` frontmatter values that map to a non-default
    /// format. `Plain` is the implicit default and is not declared, so
    /// it is absent here. The `rules` `N109` validator accepts exactly
    /// these strings.
    pub const FRONTMATTER_VALUES: &'static [&'static str] = &["letter", "agreement"];

    /// Parse a format name as it appears in `output:` frontmatter or on
    /// the CLI `--format` flag. Accepts `plain`, `letter`, and
    /// `agreement`; returns `None` for anything else so callers can
    /// report it.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "plain" => Some(Self::Plain),
            "letter" => Some(Self::Letter),
            "agreement" => Some(Self::Agreement),
            _ => None,
        }
    }

    /// The Typst chrome preamble for this format — page geometry, sizing,
    /// and any letterhead. Prepended to the body's Typst markup before
    /// [`render`]. The font family is set separately by [`render`].
    /// `letterhead` is used by [`OutputFormat::Letter`] and
    /// [`OutputFormat::Agreement`]; [`OutputFormat::Plain`] ignores it.
    #[must_use]
    pub fn preamble(self, letterhead: &Letterhead) -> String {
        // Shared page sizing; the letterhead leaves extra top margin so
        // the mark clears the body.
        match self {
            Self::Plain => concat!(
                "#set page(paper: \"us-letter\", margin: 1in)\n",
                "#set text(size: 11pt, hyphenate: false)\n",
                "#set par(justify: true, leading: 0.65em)\n\n",
            )
            .to_string(),
            // The letter is deliberately airier than `Plain`: wider side
            // margins, open leading, generous space between paragraphs and
            // above headings. An engagement letter is read once, carefully,
            // by someone deciding whether to sign it, so it is typeset for
            // reading rather than for fitting.
            Self::Letter => format!(
                concat!(
                    "#set page(\n",
                    "  paper: \"us-letter\",\n",
                    "  margin: (x: 1.15in, top: 1.15in, bottom: 1.15in),\n",
                    "  footer: context align(center)[#text(size: 8pt, fill: luma(45%))[",
                    "Page #counter(page).display() of #counter(page).final().first()]],\n",
                    ")\n",
                    "#set text(size: 11pt, hyphenate: false)\n",
                    "#set par(justify: true, leading: 0.78em, spacing: 1.5em)\n",
                    "#show heading: set text(size: 11pt, weight: \"bold\")\n",
                    "#show heading: set block(above: 2.1em, below: 1.1em)\n",
                    "{head}",
                ),
                head = letterhead_block(letterhead, "1.6em"),
            ),
            // The agreement is the mirror image of the letter: same
            // chrome, tightened everywhere the letter is open. Narrower
            // margins, closed-up leading, paragraph spacing barely wider
            // than a line, and headings that sit tight to the clause they
            // label. The point is a contract someone can hold in one hand.
            Self::Agreement => format!(
                concat!(
                    "#set page(\n",
                    "  paper: \"us-letter\",\n",
                    "  margin: (x: 0.85in, top: 0.8in, bottom: 0.75in),\n",
                    "  footer: context align(center)[#text(size: 7.5pt, fill: luma(45%))[",
                    "Page #counter(page).display() of #counter(page).final().first()]],\n",
                    ")\n",
                    "#set text(size: 10pt, hyphenate: false)\n",
                    "#set par(justify: true, leading: 0.54em, spacing: 0.72em)\n",
                    "#show heading: set text(size: 10pt, weight: \"bold\")\n",
                    "#show heading: set block(above: 0.95em, below: 0.4em)\n",
                    // A signature block that splits across a page break is a
                    // defect on an executed instrument: a page of orphaned
                    // rows reads as a different document from the one the
                    // first signer saw. Keep every table whole.
                    "#show table: set block(breakable: false)\n",
                    "{head}",
                ),
                head = letterhead_block(letterhead, "1.1em"),
            ),
        }
    }
}

/// The letterhead itself — mark, wordmark, rule, contact line — shared by
/// every format that carries one, so the firm's identity is drawn in
/// exactly one place and a new format only chooses how much air sits
/// beneath it via `below`.
fn letterhead_block(letterhead: &Letterhead, below: &str) -> String {
    format!(
        concat!(
            "#block(below: {below})[\n",
            "  #align(center)[#image(\"{logo}\", width: 0.34in)]\n",
            "  #v(0.45em)\n",
            "  #align(center)[#text(size: 9.5pt, tracking: 0.22em)[#upper[{name}]]]\n",
            "  #v(0.6em)\n",
            "  #line(length: 100%, stroke: 0.5pt + luma(35%))\n",
            "  #v(0.4em)\n",
            "{reach_line}",
            "]\n\n",
        ),
        below = below,
        logo = LOGO_PATH,
        name = esc(&letterhead.name),
        reach_line = contact_line(&[&letterhead.phone, &letterhead.email, &letterhead.web]),
    )
}

/// The one centred grey line under the rule, joining the non-empty
/// `parts` with a middot. A coordinate a deployment does not publish
/// drops out without leaving its separator behind, and a line with
/// nothing left in it emits nothing at all rather than a blank grey gap.
fn contact_line(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(esc)
        .collect::<Vec<_>>()
        .join("  ·  ");
    if joined.is_empty() {
        return String::new();
    }
    format!(
        "  #align(center)[#text(size: 7.5pt, tracking: 0.07em, \
         fill: luma(40%))[{joined}]]\n"
    )
}

/// Escape the Typst markup sigils so a letterhead string renders
/// verbatim in content context. Mirrors `markdown::escape_text`'s set;
/// a white-label fork's firm name/address may carry arbitrary
/// characters, so this is a correctness guard, not cosmetic.
fn esc(s: &str) -> String {
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

/// Render a notation's Markdown `body` to PDF bytes, framed by `format`.
///
/// Converts the Markdown to Typst ([`crate::markdown::to_typst`]),
/// prepends the format's chrome ([`OutputFormat::preamble`]), and
/// compiles ([`render`]). `letterhead` supplies the firm identity for
/// [`OutputFormat::Letter`] and [`OutputFormat::Agreement`] (ignored by
/// [`OutputFormat::Plain`]). Placeholder tokens
/// are the caller's responsibility — substitute them in `body` first.
///
/// # Errors
///
/// Returns [`PdfError::Compile`] / [`PdfError::Export`] when the
/// converted document fails to compile or export.
pub fn render_document(
    body: &str,
    format: OutputFormat,
    letterhead: &Letterhead,
) -> Result<Vec<u8>, PdfError> {
    let source = format!(
        "{}{}",
        format.preamble(letterhead),
        crate::markdown::to_typst(body)
    );
    render(&source)
}

#[cfg(test)]
mod tests {
    use super::{Letterhead, OutputFormat};
    use std::fmt::Write as _;

    #[test]
    fn parse_accepts_known_names_and_rejects_others() {
        assert_eq!(OutputFormat::parse("plain"), Some(OutputFormat::Plain));
        assert_eq!(OutputFormat::parse("letter"), Some(OutputFormat::Letter));
        assert_eq!(OutputFormat::parse(" letter "), Some(OutputFormat::Letter));
        assert_eq!(
            OutputFormat::parse("agreement"),
            Some(OutputFormat::Agreement)
        );
        assert_eq!(
            OutputFormat::parse(" agreement "),
            Some(OutputFormat::Agreement)
        );
        assert_eq!(OutputFormat::parse("demand_letter"), None);
        assert_eq!(OutputFormat::parse("contract"), None);
        assert_eq!(OutputFormat::parse(""), None);
    }

    #[test]
    fn default_is_plain() {
        assert_eq!(OutputFormat::default(), OutputFormat::Plain);
    }

    #[test]
    fn no_format_breaks_a_word_across_a_line() {
        // Justified text tempts Typst into hyphenating, which splits a word
        // mid-word at the line end ("coordina-tion"). In a document that
        // gets read aloud in a deposition and quoted in a brief, a word is
        // never broken; the line just ends early.
        for format in [
            OutputFormat::Plain,
            OutputFormat::Letter,
            OutputFormat::Agreement,
        ] {
            let preamble = format.preamble(&Letterhead::default());
            assert!(
                preamble.contains("hyphenate: false"),
                "{format:?} allows mid-word breaks: {preamble}"
            );
        }
    }

    #[test]
    fn a_long_word_reaches_the_page_whole() {
        // The behavioural half of the rule above: prose full of the long
        // Latinate words a retainer is made of must come off the page
        // unbroken, with no hyphen inserted at any line end.
        let body = "The representation includes indemnification, \
                    counterclaims, and the characterization of every \
                    reimbursable disbursement, notwithstanding any \
                    representation, misrepresentation, or \
                    recharacterization of the underlying consideration \
                    between the parties to this representation.";
        for format in [
            OutputFormat::Plain,
            OutputFormat::Letter,
            OutputFormat::Agreement,
        ] {
            let pdf =
                super::render_document(body, format, &Letterhead::default()).expect("renders");
            // A hyphenated word is split across two text runs with a `-`
            // between, so it stops matching in the text layer entirely.
            for (word, expected) in [
                ("representation", 4), // also inside `misrepresentation`
                ("indemnification", 1),
                ("characterization", 2), // also inside `recharacterization`
                ("reimbursable", 1),
                ("notwithstanding", 1),
            ] {
                let found = crate::passage::occurrence_count(&pdf, word)
                    .unwrap_or_else(|e| panic!("{format:?} counting `{word}`: {e}"));
                assert_eq!(
                    found, expected,
                    "{format:?} broke `{word}` across a line (found {found}, want {expected})"
                );
            }
        }
    }

    #[test]
    fn frontmatter_values_parse_back_to_a_format() {
        // The validator's accepted strings must each map to a real
        // format, or a template could declare an output that can't be
        // rendered.
        for v in OutputFormat::FRONTMATTER_VALUES {
            assert!(OutputFormat::parse(v).is_some(), "unparseable: {v}");
        }
    }

    #[test]
    fn every_frontmatter_value_names_a_distinct_declarable_format() {
        // The loop above only proves each string parses. Pin the whole
        // contract: the set is exactly the declarable formats, `plain`
        // stays the *implicit* default and so is absent, no two values
        // collapse onto the same variant, and each one actually
        // compiles — a name a template may write that renders nothing
        // is worse than a name it may not write at all.
        assert_eq!(
            OutputFormat::FRONTMATTER_VALUES,
            &["letter", "agreement"],
            "the declarable set changed; N109's `VALID` must move with it"
        );
        let mut seen = Vec::new();
        for v in OutputFormat::FRONTMATTER_VALUES {
            let format = OutputFormat::parse(v).unwrap_or_else(|| panic!("unparseable: {v}"));
            assert_ne!(
                format,
                OutputFormat::Plain,
                "`{v}` is the implicit default and must not be declarable"
            );
            assert!(!seen.contains(&format), "`{v}` duplicates {format:?}");
            seen.push(format);
            super::render_document("# Clause\n\nBody.", format, &Letterhead::default())
                .unwrap_or_else(|e| panic!("declarable `{v}` does not render: {e}"));
        }
    }

    #[test]
    fn plain_render_produces_a_pdf() {
        let pdf = super::render_document(
            "# Notice\n\nBody text.",
            OutputFormat::Plain,
            &Letterhead::default(),
        )
        .expect("plain renders");
        assert_eq!(&pdf[..4], b"%PDF", "not a PDF");
    }

    #[test]
    fn letter_render_embeds_the_logo_and_produces_a_pdf() {
        // The letterhead path must actually compile the `#image(..)` —
        // i.e. the embedded logo resolves through the file resolver.
        let lh = Letterhead::default();
        let pdf = super::render_document(
            "Dear Counsel,\n\nThis letter concerns **NEON LAW**.",
            OutputFormat::Letter,
            &lh,
        )
        .expect("letter renders with embedded logo");
        assert_eq!(&pdf[..4], b"%PDF", "not a PDF");
        // A letter carries the embedded PNG, so the output is materially
        // larger than the same body rendered plain.
        let plain = super::render_document(
            "Dear Counsel,\n\nThis letter concerns **NEON LAW**.",
            OutputFormat::Plain,
            &lh,
        )
        .expect("plain renders");
        assert!(
            pdf.len() > plain.len(),
            "letter ({}) should be larger than plain ({}) — logo missing?",
            pdf.len(),
            plain.len()
        );
    }

    #[test]
    fn letter_preamble_prints_the_whole_identity_and_plain_does_not() {
        let lh = Letterhead::default();
        let letter = OutputFormat::Letter.preamble(&lh);
        assert!(
            letter.contains("www.neonlaw.com"),
            "letterhead must carry the website: {letter}"
        );
        assert!(letter.contains("contact\\@neonlaw.com"));
        // The wordmark is the trading brand alone. Pin the whole `#upper`
        // argument rather than a substring, so re-attaching an entity
        // suffix ("Neon Law by Shook Law PLLC") fails here.
        assert!(
            letter.contains("#upper[Neon Law]"),
            "the wordmark is not exactly `Neon Law`: {letter}"
        );
        assert!(
            letter.contains("+1 510 800 2080"),
            "letterhead must carry the voice line: {letter}"
        );
        assert!(letter.contains("logo-neon-law.png"));
        assert!(
            letter.contains("#line(length: 100%"),
            "no rule across: {letter}"
        );
        // The plain format carries no letterhead at all.
        let plain = OutputFormat::Plain.preamble(&lh);
        assert!(!plain.contains("neonlaw.com"), "{plain}");
        assert!(!plain.contains("Neon Law"), "{plain}");
    }

    #[test]
    fn the_letterhead_publishes_no_street_address() {
        // The private-mailbox suite is not a door anyone answers, so a
        // letter must not point the reader at it. Its home is the website
        // footer. This guards the whole struct: there is no address field
        // to leak, and none of the remaining fields may smuggle one in.
        let letter = OutputFormat::Letter.preamble(&Letterhead::default());
        for street in ["Mae Anne", "Ste ", "Suite ", "89523", "Reno"] {
            assert!(
                !letter.contains(street),
                "letterhead leaked a street address ({street}): {letter}"
            );
        }
    }

    #[test]
    fn every_way_to_reach_the_firm_sits_on_one_line() {
        // A reader should find the phone, the mailbox, and the website in
        // one place rather than scanning a block, so there is exactly one
        // grey line and it carries all three.
        let letter = OutputFormat::Letter.preamble(&Letterhead::default());
        assert_eq!(
            letter.matches("fill: luma(40%)").count(),
            1,
            "the contact block split across lines: {letter}"
        );
        let line = letter
            .lines()
            .find(|l| l.contains("fill: luma(40%)"))
            .expect("contact line");
        for part in [
            "+1 510 800 2080",
            "contact\\@neonlaw.com",
            "www.neonlaw.com",
        ] {
            assert!(line.contains(part), "`{part}` missing from `{line}`");
        }
    }

    #[test]
    fn an_unpublished_field_drops_out_without_leaving_a_separator() {
        let no_phone = Letterhead {
            phone: "   ".into(),
            ..Letterhead::default()
        };
        let letter = OutputFormat::Letter.preamble(&no_phone);
        assert!(
            !letter.contains("[  ·") && !letter.contains("·  ]"),
            "a dropped field left its separator behind: {letter}"
        );
        assert!(letter.contains("www.neonlaw.com"), "{letter}");
        super::render_document("Body.", OutputFormat::Letter, &no_phone)
            .expect("a letterhead with no voice line must still render");

        // Nothing left to print at all: emit no line rather than a blank
        // grey gap under the rule.
        let bare = Letterhead {
            phone: String::new(),
            email: String::new(),
            web: String::new(),
            ..Letterhead::default()
        };
        // `luma(40%)` is the contact line's fill and the *only* place it
        // appears — matching the rule's `luma(35%)` here would pass no
        // matter what `contact_line` emitted.
        assert_eq!(
            OutputFormat::Letter
                .preamble(&bare)
                .matches("fill: luma(40%)")
                .count(),
            0,
            "an empty contact line still emitted itself"
        );
        super::render_document("Body.", OutputFormat::Letter, &bare)
            .expect("a letterhead with no coordinates must still render");
    }

    #[test]
    fn letter_pages_are_numbered_out_of_the_total() {
        // An engagement letter runs several pages and gets signed; a
        // reader must be able to tell a page is missing.
        let letter = OutputFormat::Letter.preamble(&Letterhead::default());
        assert!(letter.contains("counter(page).display()"), "{letter}");
        assert!(letter.contains("counter(page).final().first()"), "{letter}");
        assert!(!OutputFormat::Plain
            .preamble(&Letterhead::default())
            .contains("counter(page)"));
    }

    #[test]
    fn agreement_render_produces_a_pdf_on_the_letterhead() {
        // The whole point of the variant is a contract that goes out
        // under the firm's name, so the `#image(..)` must resolve and the
        // document must actually compile — not merely produce a plausible
        // preamble string.
        let lh = Letterhead::default();
        let body = "# 1. Purchase\n\nBuyer shall purchase the Interest.\n\n\
                    ## 1.1 Price\n\nThe price is stated in Schedule A.";
        let pdf = super::render_document(body, OutputFormat::Agreement, &lh)
            .expect("agreement renders with embedded logo");
        assert_eq!(&pdf[..4], b"%PDF", "not a PDF");
        // The embedded PNG makes the letterhead-bearing output materially
        // larger than the same body rendered plain.
        let plain = super::render_document(body, OutputFormat::Plain, &lh).expect("plain renders");
        assert!(
            pdf.len() > plain.len(),
            "agreement ({}) should be larger than plain ({}) — logo missing?",
            pdf.len(),
            plain.len()
        );
    }

    #[test]
    fn agreement_carries_exactly_the_same_letterhead_as_a_letter() {
        // Both letterhead-bearing formats draw the mark from
        // `letterhead_block`, so a change that reached one and not the
        // other would mean the firm had two identities on the wire. Pin
        // every element of the mark, then pin that the two blocks are
        // byte-identical apart from the trailing air beneath them.
        let lh = Letterhead::default();
        let agreement = OutputFormat::Agreement.preamble(&lh);
        let letter = OutputFormat::Letter.preamble(&lh);
        for element in [
            "logo-neon-law.png",         // the embedded mark
            "width: 0.34in",             // at the agreed size
            "#upper[Neon Law]",          // the wordmark
            "tracking: 0.22em",          // letterspaced
            "#line(length: 100%",        // the rule across the page
            "stroke: 0.5pt + luma(35%)", // at the agreed weight
            "+1 510 800 2080",           // the contact line, entire
            "contact\\@neonlaw.com",
            "www.neonlaw.com",
            "fill: luma(40%)",
        ] {
            assert!(
                agreement.contains(element),
                "agreement letterhead is missing `{element}`: {agreement}"
            );
            assert!(
                letter.contains(element),
                "letter letterhead is missing `{element}`: {letter}"
            );
        }
        // And the block *entire* — everything inside `#block(below: …)[…]`,
        // which is the last thing either preamble emits — is identical.
        // Only the `below:` distance, the air each format wants beneath
        // the mark, is allowed to differ, so it is excluded by starting
        // the comparison at the block's opening bracket.
        let mark = |p: &str| {
            let start = p.find("#block(below:").expect("letterhead block");
            let open = start + p[start..].find(")[").expect("the block's content");
            p[open..].to_string()
        };
        assert_eq!(
            mark(&agreement),
            mark(&letter),
            "the two letterheads have drifted apart"
        );
        // An agreement is signed and paginated like a letter.
        assert!(agreement.contains("counter(page).display()"), "{agreement}");
        assert!(
            agreement.contains("counter(page).final().first()"),
            "{agreement}"
        );
    }

    #[test]
    fn agreement_is_denser_than_a_letter_in_every_dimension() {
        // A contract between represented parties is navigated by section
        // number, not read through, so it is typeset curtly. These are the
        // exact values chosen against rendered output; each assertion
        // fails if someone loosens that dimension back toward the letter.
        let lh = Letterhead::default();
        let agreement = OutputFormat::Agreement.preamble(&lh);
        let letter = OutputFormat::Letter.preamble(&lh);

        // Margins: narrower on all three edges than the letter's 1.15in.
        assert!(
            agreement.contains("margin: (x: 0.85in, top: 0.8in, bottom: 0.75in)"),
            "agreement margins loosened: {agreement}"
        );
        assert!(
            letter.contains("margin: (x: 1.15in, top: 1.15in, bottom: 1.15in)"),
            "letter margins moved; the density comparison is no longer meaningful"
        );
        // Body size, leading, and paragraph spacing all tighter.
        assert!(
            agreement.contains("#set text(size: 10pt, hyphenate: false)"),
            "agreement body size grew: {agreement}"
        );
        assert!(
            agreement.contains("leading: 0.54em, spacing: 0.72em"),
            "agreement leading or paragraph spacing opened up: {agreement}"
        );
        assert!(
            letter.contains("leading: 0.78em, spacing: 1.5em"),
            "letter leading moved; the density comparison is no longer meaningful"
        );
        // Headings sit at body size so a section number reads as a label,
        // and hug the clause they introduce.
        assert!(
            agreement.contains("#show heading: set text(size: 10pt, weight: \"bold\")"),
            "agreement headings are no longer at body size: {agreement}"
        );
        assert!(
            agreement.contains("#show heading: set block(above: 0.95em, below: 0.4em)"),
            "agreement heading space opened up: {agreement}"
        );
        assert!(
            letter.contains("#show heading: set block(above: 2.1em, below: 1.1em)"),
            "letter heading space moved; the density comparison is no longer meaningful"
        );

        // The behavioural half: the very same body must come off the
        // press in strictly fewer pages than the letter does. Every
        // assertion above is on a string; this one is on the artefact.
        let mut body = String::new();
        for n in 1..=40 {
            write!(
                body,
                "## Section {n}\n\nEach party shall perform its obligations under this \
                 Section {n} promptly and in good faith, and shall bear its own costs of \
                 performance except as this Agreement expressly provides otherwise.\n"
            )
            .expect("writing to a String never fails");
        }
        let agreement_pdf =
            super::render_document(&body, OutputFormat::Agreement, &lh).expect("agreement renders");
        let letter_pdf =
            super::render_document(&body, OutputFormat::Letter, &lh).expect("letter renders");
        let dense = crate::passage::page_count(&agreement_pdf).expect("agreement page count");
        let airy = crate::passage::page_count(&letter_pdf).expect("letter page count");
        assert!(
            dense < airy,
            "the agreement ran {dense} pages against the letter's {airy}; \
             it is supposed to be the shorter document"
        );
    }

    #[test]
    fn an_agreement_never_splits_a_table_across_a_page_break() {
        // A signature block is a table. When one splits across a page
        // break, the executed instrument ends with a page of orphaned
        // signature rows that reads as a different document from the one
        // the first signer saw — and a counterparty can plausibly claim
        // the page they signed was not this page. Keeping every table
        // unbreakable pushes the whole block to the next page instead.
        let lh = Letterhead::default();
        let agreement = OutputFormat::Agreement.preamble(&lh);
        assert!(
            agreement.contains("#show table: set block(breakable: false)"),
            "an agreement's signature block may now split across pages: {agreement}"
        );

        // Behaviourally: fill the page to within a few lines of the
        // bottom, then hang a four-row signature table off the end. Under
        // a breakable table the header and the first signer would sit on
        // page one and the rest would fall to page two; unbreakable, the
        // whole block moves down together. Sweep a range of fill lengths
        // so the assertion does not depend on one lucky page position.
        let signature_block = "\n| Party | Signature |\n\
             | --- | --- |\n\
             | Sellersignaturerow | ______________ |\n\
             | Buyersignaturerow | ______________ |\n";
        for lines in 34..=42 {
            let filler = "The parties acknowledge the foregoing recital and agree to be \
                          bound by it in all respects.\n\n"
                .repeat(lines);
            let pdf = super::render_document(
                &format!("{filler}{signature_block}"),
                OutputFormat::Agreement,
                &lh,
            )
            .expect("agreement with a signature block renders");
            let seller = crate::passage::locate(&pdf, "Sellersignaturerow", 1)
                .expect("the first signature row");
            let buyer = crate::passage::locate(&pdf, "Buyersignaturerow", 1)
                .expect("the second signature row");
            assert_eq!(
                seller.pages(),
                buyer.pages(),
                "at {lines} lines of filler the signature block split across a page break"
            );
        }
    }

    #[test]
    fn letterhead_strings_are_escaped_against_typst_injection() {
        // A white-label identity carrying a `#` must not start a Typst call.
        let lh = Letterhead {
            name: "#1 Law Group".into(),
            ..Letterhead::default()
        };
        // Must still compile (escaped), not error.
        super::render_document("Body.", OutputFormat::Letter, &lh)
            .expect("letterhead with a sigil must render");
    }
}
