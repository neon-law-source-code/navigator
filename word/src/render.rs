//! Deterministic Office Open XML rendering for repository-authored notations.

use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use zip::write::SimpleFileOptions;

/// Firm identity carried by a letter-frame Word document.
pub struct RenderLetterhead<'a> {
    pub name: &'a str,
    pub phone: &'a str,
    pub email: &'a str,
    pub web: &'a str,
    pub logo_png: &'a [u8],
}

/// Stable metadata recorded in a generated Word package.
#[derive(Default)]
pub struct RenderOptions<'a> {
    pub source_revision: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("write Word package: {0}")]
    Io(#[from] std::io::Error),
    #[error("write Word package: {0}")]
    Zip(#[from] zip::result::ZipError),
}

#[derive(Default)]
struct Paragraph {
    style: Option<&'static str>,
    text: String,
}

/// Render notation Markdown into a deterministic `.docx` package.
pub fn render_notation(
    markdown: &str,
    letterhead: Option<&RenderLetterhead<'_>>,
    options: &RenderOptions<'_>,
) -> Result<Vec<u8>, RenderError> {
    let paragraphs = paragraphs(markdown);
    let document = document_xml(&paragraphs, letterhead.is_some());
    let core = core_xml(options.source_revision);
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let file_options = SimpleFileOptions::default();

    write_entry(
        &mut archive,
        "[Content_Types].xml",
        content_types(letterhead.is_some()).as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "_rels/.rels",
        ROOT_RELS.as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "docProps/core.xml",
        core.as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "docProps/app.xml",
        APP_XML.as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/document.xml",
        document.as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/styles.xml",
        STYLES_XML.as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/_rels/document.xml.rels",
        document_rels(letterhead.is_some()).as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/header1.xml",
        running_header_xml().as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/footer1.xml",
        footer_xml().as_bytes(),
        file_options,
    )?;
    write_entry(
        &mut archive,
        "word/footer2.xml",
        footer_xml().as_bytes(),
        file_options,
    )?;
    if let Some(letterhead) = letterhead {
        write_entry(
            &mut archive,
            "word/header2.xml",
            first_header_xml(letterhead).as_bytes(),
            file_options,
        )?;
        write_entry(
            &mut archive,
            "word/_rels/header2.xml.rels",
            HEADER_RELS.as_bytes(),
            file_options,
        )?;
        write_entry(
            &mut archive,
            "word/media/logo-neon-law.png",
            letterhead.logo_png,
            file_options,
        )?;
    }
    Ok(archive.finish()?.into_inner())
}

fn write_entry(
    archive: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<(), RenderError> {
    archive.start_file(name, options)?;
    archive.write_all(bytes)?;
    Ok(())
}

fn paragraphs(markdown: &str) -> Vec<Paragraph> {
    let mut result = Vec::new();
    let mut current: Option<Paragraph> = None;
    let mut list_depth = 0usize;
    let mut quote_depth = 0usize;
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some(Paragraph {
                    style: Some(heading_style(level)),
                    text: String::new(),
                });
            }
            Event::Start(Tag::Paragraph) => current = Some(Paragraph::default()),
            Event::Start(Tag::Item) => {
                let mut paragraph = Paragraph::default();
                paragraph
                    .text
                    .push_str(if list_depth > 1 { "    • " } else { "• " });
                current = Some(paragraph);
            }
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::Start(Tag::TableCell) => {
                if current.is_none() {
                    current = Some(Paragraph::default());
                } else if let Some(paragraph) = current.as_mut() {
                    paragraph.text.push('\t');
                }
            }
            Event::Text(text) | Event::Code(text) => {
                current
                    .get_or_insert_with(Paragraph::default)
                    .text
                    .push_str(&text);
            }
            // A soft break is a plain source-line wrap (S102 packs prose to
            // 120 columns, so most paragraphs carry several); CommonMark
            // renders it as a space, not a line break. Only a real hard
            // break — a trailing `\` or two trailing spaces, and not at the
            // end of the block — becomes a `<w:br/>` in `document_xml`.
            Event::SoftBreak => {
                current
                    .get_or_insert_with(Paragraph::default)
                    .text
                    .push(' ');
            }
            Event::HardBreak => {
                current
                    .get_or_insert_with(Paragraph::default)
                    .text
                    .push('\n');
            }
            Event::Rule => result.push(Paragraph {
                style: None,
                text: "────────────────────────".into(),
            }),
            Event::End(
                TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::Item | TagEnd::TableCell,
            ) => {
                if let Some(mut paragraph) = current.take() {
                    if quote_depth > 0 {
                        paragraph.text.insert_str(0, "    ");
                    }
                    strip_trailing_escaped_break(&mut paragraph.text);
                    if !paragraph.text.is_empty() {
                        result.push(paragraph);
                    }
                }
            }
            Event::End(TagEnd::List(_)) => list_depth = list_depth.saturating_sub(1),
            Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
            _ => {}
        }
    }
    if let Some(paragraph) = current.filter(|paragraph| !paragraph.text.is_empty()) {
        result.push(paragraph);
    }
    result
}

/// A trailing `\` written as `CommonMark` hard-break syntax loses that meaning
/// when it lands on a block's last line — pulldown-cmark then passes it
/// through as an ordinary character instead of emitting `Event::HardBreak`.
/// Templates wrap every line (S102 packs prose to 120 columns), so the habit
/// of ending a source line with `\` survives onto a paragraph's final line
/// too; drop it rather than print a stray backslash nobody meant to keep.
fn strip_trailing_escaped_break(text: &mut String) {
    if text.ends_with('\\') && !text.ends_with("\\\\") {
        text.pop();
    }
}

fn heading_style(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "Title",
        HeadingLevel::H2 => "Heading1",
        HeadingLevel::H3 => "Heading2",
        HeadingLevel::H4 => "Heading3",
        HeadingLevel::H5 => "Heading4",
        HeadingLevel::H6 => "Heading5",
    }
}

fn document_xml(paragraphs: &[Paragraph], has_letterhead: bool) -> String {
    let mut body = String::new();
    for paragraph in paragraphs {
        body.push_str("<w:p>");
        if let Some(style) = paragraph.style {
            let _ = write!(body, "<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>");
        }
        for (index, line) in paragraph.text.split('\n').enumerate() {
            if index > 0 {
                body.push_str("<w:r><w:br/></w:r>");
            }
            let _ = write!(
                body,
                "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                xml(line)
            );
        }
        body.push_str("</w:p>");
    }
    let first_header = if has_letterhead {
        "<w:headerReference w:type=\"first\" r:id=\"rId3\"/>"
    } else {
        ""
    };
    let title_page = if has_letterhead { "<w:titlePg/>" } else { "" };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}<w:sectPr><w:headerReference w:type="default" r:id="rId2"/>{first_header}<w:footerReference w:type="default" r:id="rId4"/><w:footerReference w:type="first" r:id="rId5"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720"/>{title_page}</w:sectPr></w:body></w:document>"#
    )
}

fn running_header_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Page </w:t></w:r>{}</w:p></w:hdr>"#,
        field("PAGE")
    )
}

fn footer_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>Page </w:t></w:r>{}<w:r><w:t> of </w:t></w:r>{}</w:p></w:ftr>"#,
        field("PAGE"),
        field("NUMPAGES")
    )
}

fn field(instruction: &str) -> String {
    format!("<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>")
}

fn first_header_xml(letterhead: &RenderLetterhead<'_>) -> String {
    let contact = [letterhead.phone, letterhead.email, letterhead.web]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:drawing><wp:inline><wp:extent cx="329184" cy="329184"/><wp:docPr id="1" name="Neon Law mark"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="logo-neon-law.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="329184" cy="329184"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p><w:p><w:pPr><w:jc w:val="center"/><w:pBdr><w:bottom w:val="single" w:sz="8" w:color="06B6D4"/></w:pBdr></w:pPr><w:r><w:rPr><w:b/><w:color w:val="06B6D4"/><w:spacing w:val="80"/></w:rPr><w:t>{}</w:t></w:r></w:p><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p></w:hdr>"#,
        xml(&letterhead.name.to_uppercase()),
        xml(&contact)
    )
}

fn document_rels(has_letterhead: bool) -> String {
    let first = if has_letterhead {
        r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/>"#
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>{first}<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer2.xml"/></Relationships>"#
    )
}

fn content_types(has_letterhead: bool) -> String {
    let header = if has_letterhead {
        r#"<Override PartName="/word/header2.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>"#
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>{header}<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/word/footer2.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#
    )
}

fn core_xml(revision: Option<&str>) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>Neon Law Navigator</dc:creator><dc:identifier>{}</dc:identifier></cp:coreProperties>"#,
        xml(revision.unwrap_or("uncommitted"))
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;
const HEADER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/logo-neon-law.png"/></Relationships>"#;
const APP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Neon Law Navigator</Application></Properties>"#;
const STYLES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:rPr><w:rFonts w:ascii="Noto Serif" w:hAnsi="Noto Serif"/><w:sz w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:qFormat/><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:basedOn w:val="Normal"/></w:style></w:styles>"#;

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{paragraphs, render_notation, RenderLetterhead, RenderOptions};

    fn render() -> Vec<u8> {
        render_notation(
            "# Demand\n\n## I. Payment\n\nPay now.",
            Some(&RenderLetterhead {
                name: "Neon Law",
                phone: "+1 510 800 2080",
                email: "contact@neonlaw.com",
                web: "www.neonlaw.com",
                logo_png: b"png",
            }),
            &RenderOptions {
                source_revision: Some("abc123"),
            },
        )
        .unwrap()
    }

    #[test]
    fn repeated_word_renders_are_byte_identical() {
        assert_eq!(render(), render());
    }

    #[test]
    fn letter_package_has_editable_outline_and_page_chrome() {
        let bytes = render();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut document = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut document)
            .unwrap();
        assert!(document.contains("w:pStyle w:val=\"Heading1\""));
        assert!(document.contains("<w:titlePg/>"));
        assert!(archive.by_name("word/media/logo-neon-law.png").is_ok());
        let mut footer = String::new();
        archive
            .by_name("word/footer1.xml")
            .unwrap()
            .read_to_string(&mut footer)
            .unwrap();
        assert!(footer.contains(" PAGE ") && footer.contains(" NUMPAGES "));
    }

    #[test]
    fn soft_wraps_join_as_spaces_not_hard_breaks() {
        let result = paragraphs(
            "If the account was opened in someone\nelse's name, tell us before\nwe pursue anything.",
        );
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].text,
            "If the account was opened in someone else's name, tell us before we pursue anything."
        );
    }

    #[test]
    fn a_real_hard_break_still_becomes_a_break() {
        let result = paragraphs("First line.  \nSecond line.");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "First line.\nSecond line.");
    }

    #[test]
    fn a_trailing_backslash_on_the_last_line_is_dropped() {
        let result = paragraphs("Date: {{client.date}} \\");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Date: {{client.date}} ");
    }

    #[test]
    fn a_mid_paragraph_trailing_backslash_still_breaks() {
        let result = paragraphs("Date: {{client.date}} \\\nSigned,");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Date: {{client.date}} \nSigned,");
    }

    #[test]
    fn rendering_a_wrapped_paragraph_has_no_br_and_one_text_run() {
        let bytes = render_notation(
            "Please confirm the account was opened in someone\nelse's name before\nwe file anything.",
            None,
            &RenderOptions::default(),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut document = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut document)
            .unwrap();
        assert!(!document.contains("w:br"));
        assert_eq!(document.matches("<w:t ").count(), 1);
        assert!(document.contains("someone else&apos;s name before we file anything."));
    }
}
