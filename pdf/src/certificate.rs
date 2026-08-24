//! Workshop completion certificate.
//!
//! Renders a single-page landscape PDF certifying that a named person
//! completed a Neon Law Navigator workshop. The workshop "light table" tracks
//! progress client-side (browser `localStorage`, no telemetry); once a
//! student has seen every slide they may request this certificate, which
//! the durable `certificate_send` workflow generates here and emails as
//! an attachment.
//!
//! All three fields are caller-supplied free text (the recipient name
//! comes straight from a public form), so each is injected as a Typst
//! **string literal** — never as markup — via [`escape_typst_string`].
//! That makes a name like `#text(red)[x]` or a stray quote render as the
//! literal characters instead of executing as Typst code.

use crate::{render, PdfError};

/// Inputs for [`render_certificate`]. `issued_date` is pre-formatted by
/// the caller (e.g. `"June 24, 2026"`) so this function stays pure and
/// deterministic — it never reads the clock.
#[derive(Debug, Clone)]
pub struct CertificateParams {
    /// The recipient's name, as they entered it.
    pub recipient_name: String,
    /// The workshop title, e.g. "Using the Neon Law Navigator to Rapidly Solve
    /// Legal Outcomes".
    pub workshop_title: String,
    /// Human-formatted issue date, e.g. "June 24, 2026".
    pub issued_date: String,
}

/// Render a completion certificate to PDF bytes.
///
/// # Errors
///
/// Returns [`PdfError::Compile`] / [`PdfError::Export`] if the Typst
/// engine fails — practically only on an internal regression, since the
/// markup is fixed and all caller input is escaped into string literals.
pub fn render_certificate(params: &CertificateParams) -> Result<Vec<u8>, PdfError> {
    let recipient = escape_typst_string(&params.recipient_name);
    let workshop = escape_typst_string(&params.workshop_title);
    let issued = escape_typst_string(&params.issued_date);

    // Landscape US Letter. A thin double border frames the page; the
    // body is vertically centered between two `1fr` struts. Everything
    // user-supplied is bound to a `#let` string and rendered with `#name`
    // so it can never be interpreted as Typst markup.
    let source = format!(
        r##"#set page(width: 11in, height: 8.5in, margin: 0.75in)
#set text(font: "Noto Serif", fill: rgb("#1a1a2e"))
#let recipient = "{recipient}"
#let workshop = "{workshop}"
#let issued = "{issued}"
#rect(width: 100%, height: 100%, stroke: 1.5pt + rgb("#0aa3c2"), inset: 0pt, radius: 4pt)[
  #rect(width: 100%, height: 100%, stroke: 0.5pt + rgb("#0aa3c2"), inset: 24pt, radius: 2pt)[
    #align(center + horizon)[
      #text(size: 15pt, tracking: 4pt, fill: rgb("#0aa3c2"))[NEON LAW]
      #v(1.4em)
      #text(size: 38pt, weight: "bold")[Certificate of Completion]
      #v(1.8em)
      #text(size: 13pt)[This certifies that]
      #v(0.7em)
      #text(size: 28pt, weight: "bold")[#recipient]
      #v(1.1em)
      #text(size: 13pt)[has completed the workshop]
      #v(0.7em)
      #text(size: 22pt, style: "italic")[#workshop]
      #v(2.2em)
      #text(size: 12pt, fill: rgb("#555"))[Issued #issued]
    ]
  ]
]
"##
    );
    render(&source)
}

/// Escape a string for safe interpolation inside a Typst double-quoted
/// string literal. Only backslash and the double quote can terminate or
/// escape within such a literal, so escaping those two is sufficient;
/// control characters (newlines/tabs from a pasted name) are collapsed to
/// a single space so the literal stays on one line.
fn escape_typst_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{escape_typst_string, render_certificate, CertificateParams};

    fn params() -> CertificateParams {
        CertificateParams {
            recipient_name: "Jane Q. Student".into(),
            workshop_title: "Using the Neon Law Navigator to Rapidly Solve Legal Outcomes".into(),
            issued_date: "June 24, 2026".into(),
        }
    }

    #[test]
    fn renders_a_pdf() {
        let pdf = render_certificate(&params()).expect("certificate renders");
        let head = pdf.get(..8.min(pdf.len()));
        assert!(
            pdf.starts_with(b"%PDF-"),
            "rendered bytes are not a PDF: {head:?}"
        );
        assert!(
            pdf.len() > 1000,
            "certificate unexpectedly tiny: {} bytes",
            pdf.len()
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        assert_eq!(escape_typst_string(r#"a"b\c"#), r#"a\"b\\c"#);
        // Control characters collapse to spaces so the literal is one line.
        assert_eq!(escape_typst_string("a\nb\tc"), "a b c");
    }

    #[test]
    fn hostile_name_is_rendered_as_text_not_executed() {
        // A name full of Typst metacharacters and a quote-break attempt
        // must still compile (it's bound to an escaped string literal),
        // never inject markup.
        let p = CertificateParams {
            recipient_name: r#"#text(red)[x] "]; #panic("pwn") //"#.into(),
            ..params()
        };
        let pdf = render_certificate(&p).expect("hostile input still renders safely");
        assert!(pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn renders_non_latin_recipient() {
        let p = CertificateParams {
            recipient_name: "Nguyễn Khánh · Привет".into(),
            ..params()
        };
        let pdf = render_certificate(&p).expect("non-latin name renders");
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
