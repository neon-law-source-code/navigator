use std::io::Cursor;
use std::path::Path;

use zip::ZipArchive;

use crate::WordError;

const OLE_COMPOUND_FILE_HEADER: &[u8; 8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";

pub(crate) fn validate_filename(filename: &str) -> Result<(), WordError> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("docx") => Ok(()),
        Some("doc") => Err(WordError::LegacyPackage),
        Some("docm" | "dotm") => Err(WordError::MacroEnabledPackage),
        _ => Err(WordError::UnsupportedFormat),
    }
}

pub(crate) fn validate_zip(bytes: &[u8]) -> Result<(), WordError> {
    if bytes.starts_with(OLE_COMPOUND_FILE_HEADER) {
        return Err(WordError::EncryptedPackage);
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| WordError::CorruptPackage)?;
    let mut has_content_types = false;
    let mut has_main_document = false;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| WordError::CorruptPackage)?;
        let name = entry.name().replace('\\', "/");
        if name == "[Content_Types].xml" {
            has_content_types = true;
        }
        if name == "word/document.xml" {
            has_main_document = true;
        }
        if name.starts_with('/')
            || name
                .split('/')
                .any(|segment| segment == ".." || segment == ".")
        {
            return Err(WordError::EscapingPackage);
        }
        if name.contains('\0') {
            return Err(WordError::CorruptPackage);
        }
        if name.eq_ignore_ascii_case("word/vbaproject.bin")
            || name.eq_ignore_ascii_case("word/vbadata.xml")
            || name.eq_ignore_ascii_case("encryptedpackage")
            || name.eq_ignore_ascii_case("encryptioninfo")
        {
            return Err(
                if name.eq_ignore_ascii_case("encryptedpackage")
                    || name.eq_ignore_ascii_case("encryptioninfo")
                {
                    WordError::EncryptedPackage
                } else {
                    WordError::MacroEnabledPackage
                },
            );
        }
    }
    if !has_content_types || !has_main_document {
        return Err(WordError::CorruptPackage);
    }
    Ok(())
}

#[must_use]
pub fn is_docx_filename(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("docx"))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::{validate_filename, validate_zip};
    use crate::WordError;

    #[test]
    fn unsafe_filename_families_fail_closed() {
        assert!(matches!(
            validate_filename("legacy.doc"),
            Err(WordError::LegacyPackage)
        ));
        assert!(matches!(
            validate_filename("macro.docm"),
            Err(WordError::MacroEnabledPackage)
        ));
        assert!(matches!(
            validate_filename("template.dotm"),
            Err(WordError::MacroEnabledPackage)
        ));
        assert!(matches!(
            validate_filename("archive.zip"),
            Err(WordError::UnsupportedFormat)
        ));
    }

    #[test]
    fn package_entry_cannot_escape_its_container() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(&mut bytes);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive.write_all(b"<Types/>").unwrap();
        archive.start_file("word/document.xml", options).unwrap();
        archive.write_all(b"<document/>").unwrap();
        archive.start_file("../outside.xml", options).unwrap();
        archive.write_all(b"outside").unwrap();
        archive.finish().unwrap();

        assert!(matches!(
            validate_zip(&bytes.into_inner()),
            Err(WordError::EscapingPackage)
        ));
    }
}
