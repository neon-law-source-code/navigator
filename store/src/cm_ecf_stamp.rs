//! `store::cm_ecf_stamp` — extract the CM/ECF header stamp a federal
//! court's electronic filing system prints along the top of every page it
//! generates (LAW-24).
//!
//! The stamp is the one piece of "who filed what, in which case, when"
//! that comes from the document itself rather than from whoever typed a
//! docket-entry description:
//!
//! ```text
//! Case 1:23-cv-04567-ABC Document 29 Filed 03/14/24 Page 1 of 12
//! ```
//!
//! This module only reads text a caller already extracted from the PDF
//! (`pdf::acroform::page_text`, or an equivalent) — it has no storage or
//! database dependency, so it is trivial to test against real stamp text
//! and safe to run on every upload.

/// The case number, entry (document) number, and filing date printed in
/// one CM/ECF stamp line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmEcfStamp {
    pub case_number: String,
    pub entry_number: String,
    /// Verbatim as stamped — typically `MM/DD/YY` or `MM/DD/YYYY`. Kept as
    /// the source text rather than parsed into a date, so a stamp this
    /// module does not fully understand is still recorded rather than
    /// dropped.
    pub filed_on: String,
}

/// Find the first CM/ECF stamp line in `text` and extract it.
///
/// Scans line by line (a stamp is printed once per page, so the first
/// match is enough) for the `Case … Document|Doc … Filed …` shape. Returns
/// `None` when no line matches — an ordinary, non-electronically-filed
/// document, or a stamp format this parser does not recognize. A caller
/// gets no stamp rather than a guessed one.
#[must_use]
pub fn extract(text: &str) -> Option<CmEcfStamp> {
    text.lines().find_map(parse_stamp_line)
}

fn parse_stamp_line(line: &str) -> Option<CmEcfStamp> {
    let tokens: Vec<&str> = line.split_whitespace().collect();

    let case_at = tokens.iter().position(|&t| t == "Case")?;
    let case_number = (*tokens.get(case_at + 1)?).to_string();

    let doc_at = tokens.iter().position(|&t| t == "Document" || t == "Doc")?;
    let entry_number = (*tokens.get(doc_at + 1)?).to_string();

    let filed_at = tokens.iter().position(|&t| t == "Filed")?;
    let filed_on = (*tokens.get(filed_at + 1)?).to_string();

    if !looks_like_date(&filed_on) {
        return None;
    }

    Some(CmEcfStamp {
        case_number,
        entry_number,
        filed_on,
    })
}

/// A loose `MM/DD/YY` or `MM/DD/YYYY` check — enough to reject a stray
/// "Filed" that isn't followed by a date (a motion titled "... Filed
/// Under Seal", say) without committing to strict calendar validation
/// this module has no use for.
fn looks_like_date(candidate: &str) -> bool {
    let digits_and_slashes = candidate.chars().all(|c| c.is_ascii_digit() || c == '/');
    digits_and_slashes && candidate.matches('/').count() == 2
}

#[cfg(test)]
mod tests {
    use super::{extract, CmEcfStamp};

    #[test]
    fn extracts_a_district_court_stamp() {
        let page = "\
            UNITED STATES DISTRICT COURT\n\
            Case 1:23-cv-04567-ABC Document 29 Filed 03/14/24 Page 1 of 12\n\
            \n\
            Plaintiff, v. Defendant.\n";

        assert_eq!(
            extract(page),
            Some(CmEcfStamp {
                case_number: "1:23-cv-04567-ABC".to_string(),
                entry_number: "29".to_string(),
                filed_on: "03/14/24".to_string(),
            })
        );
    }

    #[test]
    fn accepts_the_bankruptcy_courts_abbreviated_doc_and_a_four_digit_year() {
        let page = "Case 23-12345-XYZ Doc 29 Filed 03/14/2024 Entered 03/14/2024 10:15:22\n";

        assert_eq!(
            extract(page),
            Some(CmEcfStamp {
                case_number: "23-12345-XYZ".to_string(),
                entry_number: "29".to_string(),
                filed_on: "03/14/2024".to_string(),
            })
        );
    }

    #[test]
    fn returns_none_for_a_conformed_order_with_no_stamp() {
        let page = "IT IS SO ORDERED.\n\nSigned by Judge Jane Roe on March 14, 2024.\n";
        assert_eq!(extract(page), None);
    }

    #[test]
    fn a_filed_that_is_not_followed_by_a_date_does_not_false_positive() {
        let page = "Case 1:23-cv-04567-ABC Document 29 Filed Under Seal Page 1 of 1\n";
        assert_eq!(extract(page), None);
    }

    #[test]
    fn takes_the_first_stamp_when_the_text_spans_several_pages() {
        let text = "\
            Case 1:23-cv-04567-ABC Document 29 Filed 03/14/24 Page 1 of 2\n\
            body of page one\n\
            Case 1:23-cv-04567-ABC Document 29 Filed 03/14/24 Page 2 of 2\n";

        assert_eq!(
            extract(text).map(|s| s.case_number),
            Some("1:23-cv-04567-ABC".to_string())
        );
    }
}
