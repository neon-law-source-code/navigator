use std::io::Cursor;
use std::path::Path;

use zip::ZipArchive;

use crate::WordError;

const OLE_COMPOUND_FILE_HEADER: &[u8; 8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";

// These limits are mirrored by PackageSafety in word/adapter/Program.cs; keep
// the two copies together. A real DOCX has tens to low hundreds of parts, so
// 4,096 leaves generous headroom while bounding central-directory work. The
// 64 MiB per-entry and 256 MiB total uncompressed limits prevent one oversized
// part and cap expansion at 2.56x the existing 100 MiB compressed ceiling.
const MAX_PACKAGE_BYTES: usize = 100 * 1024 * 1024;
const MAX_ZIP_ENTRY_COUNT: usize = 4_096;
const MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;

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
    if bytes.is_empty() || bytes.len() > MAX_PACKAGE_BYTES {
        return Err(WordError::CorruptPackage);
    }
    if bytes.starts_with(OLE_COMPOUND_FILE_HEADER) {
        return Err(WordError::EncryptedPackage);
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| WordError::CorruptPackage)?;
    let entry_count = archive.len();
    if entry_count > MAX_ZIP_ENTRY_COUNT {
        return Err(WordError::ZipEntryCountExceeded {
            actual: entry_count,
            maximum: MAX_ZIP_ENTRY_COUNT,
        });
    }
    let mut has_content_types = false;
    let mut has_main_document = false;
    let mut total_uncompressed_bytes: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| WordError::CorruptPackage)?;
        let uncompressed_bytes = entry.size();
        if uncompressed_bytes > MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES {
            return Err(WordError::ZipEntryUncompressedSizeExceeded {
                actual: uncompressed_bytes,
                maximum: MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES,
            });
        }
        total_uncompressed_bytes = total_uncompressed_bytes.saturating_add(uncompressed_bytes);
        if total_uncompressed_bytes > MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES {
            return Err(WordError::ZipTotalUncompressedSizeExceeded {
                actual: total_uncompressed_bytes,
                maximum: MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES,
            });
        }
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
    use std::io::{Cursor, Write};

    use super::{
        validate_filename, validate_zip, MAX_PACKAGE_BYTES, MAX_ZIP_ENTRY_COUNT,
        MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES, MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES,
    };
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

    #[test]
    fn normal_synthetic_package_passes_preflight() {
        assert!(validate_zip(&synthetic_package(&[])).is_ok());
    }

    #[test]
    fn package_rejects_entry_that_exceeds_uncompressed_limit() {
        let bytes = synthetic_package(&[("parts/large.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES + 1)]);

        assert!(bytes.len() < 1024 * 1024);
        assert!(matches!(
            validate_zip(&bytes),
            Err(WordError::ZipEntryUncompressedSizeExceeded { actual, maximum })
                if actual == MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES + 1
                    && maximum == MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES
        ));
    }

    #[test]
    fn package_rejects_total_uncompressed_limit_from_central_directory() {
        let entries = [
            ("parts/one.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES),
            ("parts/two.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES),
            ("parts/three.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES),
            ("parts/four.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES),
            ("parts/five.xml", MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES),
        ];
        let bytes = synthetic_package(&entries);

        assert!(bytes.len() < 1024 * 1024);
        assert!(matches!(
            validate_zip(&bytes),
            Err(WordError::ZipTotalUncompressedSizeExceeded { actual, maximum })
                if actual > MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES
                    && maximum == MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES
        ));
    }

    #[test]
    fn package_rejects_too_many_entries_before_reading_entry_streams() {
        let entries = (0..MAX_ZIP_ENTRY_COUNT)
            .map(|index| (format!("parts/{index}.xml"), 0))
            .collect::<Vec<_>>();
        let entries = entries
            .iter()
            .map(|(name, size)| (name.as_str(), *size))
            .collect::<Vec<_>>();
        let bytes = synthetic_package(&entries);

        assert!(matches!(
            validate_zip(&bytes),
            Err(WordError::ZipEntryCountExceeded { actual, maximum })
                if actual == MAX_ZIP_ENTRY_COUNT + 2 && maximum == MAX_ZIP_ENTRY_COUNT
        ));
    }

    #[test]
    fn package_size_limit_is_kept_next_to_the_zip_limits() {
        assert_eq!(MAX_PACKAGE_BYTES, 100 * 1024 * 1024);
        assert_eq!(MAX_ZIP_ENTRY_COUNT, 4_096);
        assert_eq!(MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES, 64 * 1024 * 1024);
        assert_eq!(MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES, 256 * 1024 * 1024);
    }

    fn synthetic_package(entries: &[(&str, u64)]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(&mut bytes);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive.write_all(b"<Types/>").unwrap();
        archive.start_file("word/document.xml", options).unwrap();
        archive.write_all(b"<document/>").unwrap();
        for (name, size) in entries {
            archive.start_file(*name, options).unwrap();
            write_zeroes(&mut archive, *size);
        }
        archive.finish().unwrap();
        bytes.into_inner()
    }

    fn write_zeroes(writer: &mut impl Write, mut remaining: u64) {
        let zeroes = [0_u8; 8192];
        while remaining > 0 {
            let length =
                usize::try_from(remaining.min(zeroes.len() as u64)).unwrap_or(zeroes.len());
            writer.write_all(&zeroes[..length]).unwrap();
            remaining -= length as u64;
        }
    }
}
