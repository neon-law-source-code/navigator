//! PDF rendering for Neon Law Navigator's legal documents.
//!
//! Backed by the [Typst](https://typst.app) embedded compiler via the
//! `typst-as-lib` wrapper. Callers feed Typst markup to [`render`] and
//! get back the PDF bytes; the [`StorageService`](cloud::StorageService)
//! seam handles persistence.
//!
//! ## Fonts
//!
//! Every PDF this crate renders is set in the firm stack [`BRAND_FONT_STACK`]:
//! **GORP Serif** first, **Noto Serif** behind it. GORP Serif is the face the
//! website is set in, so a rendered letter and the firm's front door read as
//! one brand. It is proprietary `TrashType` font software licensed separately
//! from this repository, so its bytes are never committed — [`render`]
//! resolves it at run time from the host's installed fonts and from
//! [`FONT_DIR_ENV`]. Where the licensed faces are absent — CI, a fork, an
//! operator who chose different typography — Typst falls through to Noto
//! Serif and the document still renders.
//!
//! Noto Serif is therefore the floor, not a downgrade: a sturdy,
//! screen-and-print legible serif whose broad Unicode coverage (Latin + all
//! European accents, Cyrillic, Greek, Vietnamese) keeps client names and
//! addresses rendering correctly worldwide. Two Google Fonts variable masters
//! (upright + italic, `wght` axis) are embedded into the binary via
//! `include_bytes!` from `pdf/assets/fonts/NotoSerif/`; Typst instantiates
//! Regular and Bold off the weight axis. A caller can still override the whole
//! stack with its own `#set text` rule.
//!
//! Noto Serif ships under the SIL Open Font License 1.1; the full text
//! is at `pdf/assets/fonts/NotoSerif/OFL.txt`. GORP Serif is governed by the
//! `TrashType` terms recorded in
//! `server/public/fonts/gorp-serif/LICENSE.txt`.
//!
//! A third face, **Tinos**, is embedded alongside Noto Serif for a narrower
//! reason: [`pleading`] renders court paper, which has its own typeface rule
//! (CRC 2.105: "essentially equivalent to Courier, Times New Roman, or
//! Arial") independent of the firm's brand stack above. Tinos is metrically
//! identical to Times New Roman — which cannot itself be embedded, being
//! Monotype-licensed — and ships from Google Fonts under the SIL Open Font
//! License 1.1, at `pdf/assets/fonts/Tinos/OFL.txt`. It is registered with
//! the same `.fonts(..)` call as the brand stack so it is available whenever
//! Typst source asks for it, but [`FONT_PREAMBLE`] never sets it as a
//! default — only [`pleading::preamble`] does, for the documents it
//! produces.
//!
//! ## Redaction styles
//!
//! Separate from the typeface above, these are the ways a *redacted*
//! (blacked-out) passage is drawn. Three modes match the
//! [`RedactionStyle`] enum:
//!
//! - [`RedactionStyle::Block`] — a solid black rectangle the width of
//!   the redacted text.
//! - [`RedactionStyle::Bar`] — a thin black bar centred vertically
//!   through the redacted text (the classic "with prejudice" mark).
//! - [`RedactionStyle::Strike`] — a strikethrough on the legible
//!   original text (review-mode style; the recipient can still read
//!   the original but it's marked for redaction).

use thiserror::Error;

pub mod acroform;
pub mod certificate;
pub mod format;
pub mod markdown;
pub mod passage;
pub mod pleading;
pub mod safety;

pub use acroform::{
    blank_acroform, blank_acroform_with, field_names, fill_acroform, flatten, page_text,
    read_field_value, read_field_values, read_widget_appearance_state, reauthor, strip_static_xfa,
    widget_annotation_count, FieldSpec, RadioMergeMember, RadioMergeSpec, ReauthorSpec,
};
pub use certificate::{render_certificate, CertificateParams};
pub use format::{render_document, Letterhead, OutputFormat};
pub use markdown::to_typst;
pub use passage::{
    locate, occurrence_count, page_count, page_render, NormalisedRect, PassageError,
    PassageLocation, PassageRect,
};
pub use safety::{validate_pdf, validate_pdf_with_limit, PdfSafetyError, DEFAULT_MAX_PDF_BYTES};

/// The firm typeface, embedded so PDF rendering never depends on a
/// font installed on the host. These are the Google Fonts Noto Serif
/// variable masters (`wght` + `wdth` axes); Typst reads Regular and
/// Bold off the weight axis, so two files cover regular/bold/italic.
const NOTO_SERIF: &[u8] = include_bytes!("../assets/fonts/NotoSerif/NotoSerif-VF.ttf");
const NOTO_SERIF_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/NotoSerif/NotoSerif-Italic-VF.ttf");

/// Tinos, embedded for [`pleading`] rather than for the firm brand stack —
/// see the module-level `## Fonts` section. Four static faces (Google Fonts
/// ships Tinos unhinted rather than as a `wght`-axis variable font) cover
/// regular, italic, bold, and bold italic.
const TINOS_REGULAR: &[u8] = include_bytes!("../assets/fonts/Tinos/Tinos-Regular.ttf");
const TINOS_ITALIC: &[u8] = include_bytes!("../assets/fonts/Tinos/Tinos-Italic.ttf");
const TINOS_BOLD: &[u8] = include_bytes!("../assets/fonts/Tinos/Tinos-Bold.ttf");
const TINOS_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/Tinos/Tinos-BoldItalic.ttf");

/// The firm logo, embedded so the letterhead in [`OutputFormat::Letter`]
/// never depends on a file on disk. Registered with the Typst engine
/// under the virtual path [`LOGO_PATH`], which a chrome preamble
/// references via `#image(..)`.
const FIRM_LOGO: &[u8] = include_bytes!("../assets/brand/logo-neon-law.png");

/// The virtual path the embedded [`FIRM_LOGO`] is resolvable at inside
/// Typst markup — kept in one place so [`format`] and [`render`] agree.
pub(crate) const LOGO_PATH: &str = "logo-neon-law.png";

/// The firm's typeface stack, most-preferred first. Typst walks it per
/// glyph, so a document renders in GORP Serif wherever the licensed faces
/// resolve and in the embedded Noto Serif everywhere else.
pub const BRAND_FONT_STACK: &[&str] = &["GORP Serif", "Noto Serif"];

/// Environment variable naming a directory of additional font files
/// (OTF/TTF/TTC) to search — the seam an operator uses to supply the
/// licensed GORP Serif desktop faces to a container, which has no
/// installed-font path of its own.
///
/// Point it at an unpacked `gorp-serif-otf.zip` (the archive
/// `navigator assets fonts upload-desktop` publishes, served to Lawyers at
/// `/app/team/fonts/gorp-serif.zip`). WOFF2 is a web format Typst cannot read;
/// this directory needs the desktop OTFs.
pub const FONT_DIR_ENV: &str = "NAVIGATOR_PDF_FONT_DIR";

/// Typst set-rule making [`BRAND_FONT_STACK`] the document default.
/// Prepended to every source by [`render`]; a caller's own `#set text`
/// rule that follows in the source overrides it.
static FONT_PREAMBLE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let families = BRAND_FONT_STACK
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("#set text(font: ({families}))\n")
});

/// Errors that [`render`] can surface to the caller.
#[derive(Debug, Error)]
pub enum PdfError {
    /// The Typst source failed to compile. The wrapped string is the
    /// first diagnostic; the full set is logged at `warn`.
    #[error("typst compile: {0}")]
    Compile(String),
    /// PDF export failed after a successful compile — usually a font
    /// fallback issue or an unsupported feature.
    #[error("typst export: {0}")]
    Export(String),
    /// `lopdf` failed to parse or write a PDF in the `AcroForm` fill path.
    #[error("pdf parse/write: {0}")]
    Lopdf(String),
    /// The PDF handed to [`acroform::fill_acroform`] has no `AcroForm` to
    /// fill.
    #[error("pdf has no AcroForm to fill")]
    NoAcroForm,
    /// The form is dynamic XFA (Adobe's XML form layer). No Rust crate
    /// fills dynamic XFA; filling it would silently emit a blank, so we
    /// fail loudly instead.
    #[error("dynamic XFA forms are not supported (would silently emit a blank)")]
    XfaUnsupported,
    /// A field name passed to [`acroform::fill_acroform`] matched no
    /// field in the form — surfaced rather than silently dropped.
    #[error("no form field named `{0}`")]
    UnmatchedField(String),
    /// A value for a checkbox / radio (`Btn`) field matched none of the
    /// field's appearance states — surfaced with the allowed states so a
    /// mis-mapped field map is corrected, never a silently unchecked box.
    #[error("field `{field}`: `{value}` matches no appearance state (allowed: {allowed:?})")]
    InvalidChoice {
        field: String,
        value: String,
        allowed: Vec<String>,
    },
    /// A character in a value being [`acroform::flatten`]ed has no byte
    /// in the overlay font's `WinAnsiEncoding` — surfaced rather than
    /// silently garbling a glyph in a packet on its way to a government
    /// office.
    #[error("`{ch}` (in `{value}`) has no WinAnsi byte for the flattened overlay")]
    UnencodableChar { ch: char, value: String },
    /// A form field the [`acroform::reauthor`] spec does not cover —
    /// every field of a blank that files must be renamed, merged,
    /// pre-printed, or explicitly namespaced; an unaccounted field means
    /// the plan was guessed, never a silent pass-through.
    #[error("field `{0}` is not accounted for by the re-author spec")]
    UnaccountedField(String),
    /// A structural conflict while re-authoring (duplicate source `/T`,
    /// a merge across mismatched field types, …) — refused loudly, never
    /// guessed on a document that files.
    #[error("re-author: {0}")]
    Reauthor(String),
}

/// How a redacted passage is rendered in the output PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionStyle {
    /// Solid black box covering the redacted text.
    Block,
    /// Horizontal black bar through the middle of the line.
    Bar,
    /// Strikethrough; original text remains legible.
    Strike,
}

impl RedactionStyle {
    /// Typst markup snippet that wraps the redacted span. Used by
    /// [`render_with_redactions`].
    #[must_use]
    pub fn typst_wrapper(self, content: &str) -> String {
        match self {
            Self::Block => format!("#box(fill: black, inset: 2pt)[#text(fill: white)[{content}]]"),
            Self::Bar => format!("#box(stroke: (top: 1.2pt + black, bottom: 0pt))[{content}]"),
            Self::Strike => format!("#strike[{content}]"),
        }
    }
}

/// Compile Typst source `source` and return the rendered PDF bytes.
///
/// # Errors
///
/// Returns [`PdfError::Compile`] if the Typst source is malformed,
/// or [`PdfError::Export`] if the PDF stage fails after a successful
/// compile.
pub fn render(source: &str) -> Result<Vec<u8>, PdfError> {
    use typst_as_lib::TypstEngine;
    use typst_layout::PagedDocument;

    // Prepend the font set-rule so the firm stack is the default family
    // for whatever `source` renders. The embedded Noto Serif masters are
    // registered with `.fonts(..)` so the floor of the stack never depends
    // on the host; the searcher adds the host's installed fonts and any
    // `FONT_DIR_ENV` directory, which is where the licensed GORP Serif
    // desktop faces come from.
    let with_font = format!("{}{source}", *FONT_PREAMBLE);
    let engine = TypstEngine::builder()
        .main_file(with_font)
        .fonts([
            NOTO_SERIF,
            NOTO_SERIF_ITALIC,
            TINOS_REGULAR,
            TINOS_ITALIC,
            TINOS_BOLD,
            TINOS_BOLD_ITALIC,
        ])
        .with_static_file_resolver([(LOGO_PATH, FIRM_LOGO)])
        .search_fonts_with(font_search_options())
        .build();

    let doc: PagedDocument = engine
        .compile()
        .output
        .map_err(|diags| PdfError::Compile(format_diagnostics(&diags)))?;

    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default())
        .map_err(|diags| PdfError::Export(format_diagnostics(&diags)))
}

/// Render a Typst document where one passage has been wrapped in the
/// chosen [`RedactionStyle`]. The `redacted` slice is splice-inserted
/// into `template` at the literal token `{{redacted}}`; the rest of
/// the template is rendered verbatim.
///
/// # Errors
///
/// Same as [`render`]: compile or export failure.
pub fn render_with_redactions(
    template: &str,
    redacted: &str,
    style: RedactionStyle,
) -> Result<Vec<u8>, PdfError> {
    let wrapped = style.typst_wrapper(redacted);
    let source = template.replace("{{redacted}}", &wrapped);
    render(&source)
}

/// Font-search configuration for [`render`]: the host's installed fonts
/// plus every directory named by [`FONT_DIR_ENV`]. The variable takes a
/// `PATH`-style list so an operator can point at several unpacked font
/// bundles; blank entries are dropped so a trailing separator is harmless.
fn font_search_options() -> typst_as_lib::typst_kit_options::TypstKitFontOptions {
    typst_as_lib::typst_kit_options::TypstKitFontOptions::default()
        .include_dirs(font_dirs(std::env::var_os(FONT_DIR_ENV).as_deref()))
}

/// Pure half of [`font_search_options`]: split a `PATH`-style variable
/// value into search directories. Kept separate so the parsing is tested
/// without mutating the process environment.
fn font_dirs(value: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    value
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn format_diagnostics<T: std::fmt::Debug>(diags: &T) -> String {
    format!("{diags:?}")
}

#[cfg(test)]
mod tests {
    use super::{render, render_with_redactions, PdfError, RedactionStyle, FIRM_LOGO};

    #[test]
    fn embedded_neon_law_mark_is_a_high_resolution_png() {
        assert!(FIRM_LOGO.starts_with(b"\x89PNG\r\n\x1a\n"));
        let width = u32::from_be_bytes(FIRM_LOGO[16..20].try_into().expect("PNG width"));
        let height = u32::from_be_bytes(FIRM_LOGO[20..24].try_into().expect("PNG height"));
        assert_eq!((width, height), (1024, 1024));
    }

    #[test]
    fn redaction_style_block_emits_a_filled_box() {
        let wrapped = RedactionStyle::Block.typst_wrapper("secret name");
        assert!(wrapped.contains("fill: black"));
        assert!(wrapped.contains("secret name"));
    }

    #[test]
    fn redaction_style_bar_emits_a_top_stroke() {
        let wrapped = RedactionStyle::Bar.typst_wrapper("classified");
        assert!(wrapped.contains("stroke"));
        assert!(wrapped.contains("classified"));
    }

    #[test]
    fn redaction_style_strike_emits_a_strike_block() {
        let wrapped = RedactionStyle::Strike.typst_wrapper("draft only");
        assert!(wrapped.starts_with("#strike["));
        assert!(wrapped.contains("draft only"));
    }

    #[test]
    fn render_returns_pdf_bytes_for_a_minimal_document() {
        let pdf = render("Hello, world.").expect("typst minimal compile + export");
        let head = pdf.get(..8.min(pdf.len()));
        assert!(
            pdf.starts_with(b"%PDF-"),
            "rendered bytes are not a PDF: first 8 = {head:?}"
        );
        assert!(
            pdf.len() > 100,
            "PDF unexpectedly tiny: {} bytes",
            pdf.len()
        );
    }

    #[test]
    fn rendered_pdf_embeds_a_face_from_the_firm_font_stack() {
        // Typst silently falls back to some arbitrary family if none of
        // the stack resolves, so a clean compile isn't enough — assert a
        // firm face actually made it into the embedded font set. Which one
        // is host-dependent by design: GORP Serif where the licensed
        // desktop faces are installed, the embedded Noto Serif otherwise.
        // Both are a pass; anything else means the stack resolved to
        // neither and the .ttf went missing or its name table drifted.
        let pdf = render("Defendant rests.").expect("render");
        let embedded = |needle: &[u8]| pdf.windows(needle.len()).any(|w| w == needle);
        assert!(
            embedded(b"NotoSerif") || embedded(b"GORPSerif"),
            "rendered PDF embeds neither firm face — Typst fell back off the stack",
        );
    }

    #[test]
    fn the_font_preamble_sets_the_whole_firm_stack_in_order() {
        // GORP Serif must come first or a host with the licensed faces
        // installed would still render Noto Serif; Noto Serif must be
        // present or a host without them has no floor to fall back to.
        assert_eq!(
            *super::FONT_PREAMBLE,
            "#set text(font: (\"GORP Serif\", \"Noto Serif\"))\n"
        );
        assert_eq!(super::BRAND_FONT_STACK, ["GORP Serif", "Noto Serif"]);
    }

    #[test]
    fn the_font_dir_variable_splits_into_search_directories() {
        use std::ffi::OsString;
        use std::path::PathBuf;

        assert!(
            super::font_dirs(None).is_empty(),
            "unset means no extra dirs"
        );
        assert_eq!(
            super::font_dirs(Some(&OsString::from("/opt/gorp"))),
            vec![PathBuf::from("/opt/gorp")]
        );
        // `PATH`-style so an operator can stack bundles; a trailing
        // separator must not add an empty directory that searches `.`.
        let joined =
            std::env::join_paths([PathBuf::from("/opt/gorp"), PathBuf::from("/opt/extra")])
                .expect("join");
        assert_eq!(
            super::font_dirs(Some(&joined)),
            vec![PathBuf::from("/opt/gorp"), PathBuf::from("/opt/extra")]
        );
        assert!(super::font_dirs(Some(&OsString::from(""))).is_empty());
    }

    #[test]
    fn bold_weight_renders_off_the_variable_axis() {
        // The embedded masters are variable; bold must instantiate from
        // the weight axis rather than error or silently stay regular.
        let bold = render("#text(weight: \"bold\")[Heavy.]").expect("bold renders");
        assert!(bold.starts_with(b"%PDF-"));
    }

    #[test]
    fn renders_non_latin_scripts_for_global_clients() {
        // Cyrillic + Greek + Vietnamese + accented Latin all come from
        // the one embedded family — a client's name must not vanish.
        let pdf = render("Привет · Γειά · Tiếng Việt · Núñez").expect("multi-script renders");
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() > 100);
    }

    #[test]
    fn render_surfaces_typst_compile_errors() {
        // `#let x = ` is an incomplete statement; the parser bails.
        let err = render("#let x =").unwrap_err();
        assert!(
            matches!(err, PdfError::Compile(_)),
            "expected Compile, got {err:?}",
        );
    }

    #[test]
    fn render_with_redactions_splices_the_wrapper_into_the_template() {
        let template = "Defendant: {{redacted}}.";
        let pdf = render_with_redactions(template, "John Doe", RedactionStyle::Block)
            .expect("render with redactions");
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
