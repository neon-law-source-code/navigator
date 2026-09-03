//! `navigator dev docs ...` — command-line access to published workspace docs.
//!
//! The glossary is the same vocabulary the website publishes at
//! `/docs/glossary`: parsed from `docs/glossary.md` by
//! [`store::glossary::parse`] so the CLI cannot drift from the page.

use std::process::ExitCode;

use store::glossary::{parse, Term, GLOSSARY_MD};

use crate::palette;

#[must_use]
pub fn list() -> ExitCode {
    let docs = portal::docs::loader::bundled();
    for doc in docs.docs() {
        println!("/docs/{slug}\t{title}", slug = doc.slug, title = doc.title);
    }
    for entry in glossary_entries() {
        println!(
            "/docs/glossary#{slug}\tGlossary: {title}",
            slug = entry.slug,
            title = entry.title,
        );
    }
    ExitCode::SUCCESS
}

#[must_use]
pub fn glossary(term: Option<&str>) -> ExitCode {
    let entries = glossary_entries();
    let Some(needle) = term else {
        for entry in &entries {
            print_entry(entry);
        }
        return ExitCode::SUCCESS;
    };
    if let Some(entry) = entries.iter().find(|entry| matches_entry(entry, needle)) {
        print_entry(entry);
        ExitCode::SUCCESS
    } else {
        eprintln!("navigator: docs glossary: unknown term `{needle}`");
        eprintln!("Run `navigator dev docs list` to list every published docs page.");
        ExitCode::from(1)
    }
}

fn print_entry(entry: &Term) {
    println!("## {}", palette::header(&entry.title));
    println!();
    println!("{}", entry.body.trim());
    println!();
}

fn matches_entry(entry: &Term, needle: &str) -> bool {
    entry.title.eq_ignore_ascii_case(needle) || entry.slug == store::glossary::slugify(needle)
}

fn glossary_entries() -> Vec<Term> {
    parse(GLOSSARY_MD)
}

#[cfg(test)]
mod tests {
    use super::glossary_entries;
    use store::glossary::GLOSSARY_MD;

    #[test]
    fn parses_glossary_headings_as_entries() {
        let entries = glossary_entries();
        assert!(entries.iter().any(|entry| entry.title == "Project"));
        assert!(entries.iter().any(|entry| entry.title == "Lawyer Review"));
    }

    #[test]
    fn heading_slug_matches_published_docs_anchor_shape() {
        assert_eq!(store::glossary::slugify("Lawyer Review"), "lawyer-review");
        assert_eq!(
            store::glossary::slugify("Engagement / Retainer"),
            "engagement--retainer"
        );
    }

    #[test]
    fn glossary_headings_are_alphabetical() {
        let headings = GLOSSARY_MD
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .map(|heading| heading.trim().trim_matches('`'))
            .collect::<Vec<_>>();
        let mut alphabetical = headings.clone();
        alphabetical.sort_by_key(|heading| heading.to_lowercase());

        assert_eq!(
            headings, alphabetical,
            "glossary headings must be alphabetical"
        );
    }

    #[test]
    fn glossary_terms_match_the_published_docs_page() {
        let entries = glossary_entries();
        let docs = portal::docs::loader::bundled();
        let glossary = docs
            .find("glossary")
            .expect("glossary is published at /docs/glossary");
        assert!(
            !entries.is_empty(),
            "the CLI glossary must parse the authored vocabulary"
        );
        let mut html_ids = Vec::new();
        let mut rest = glossary.body_html.as_str();
        while let Some(at) = rest.find("<h2 id=\"") {
            rest = &rest[at + 8..];
            let Some(end) = rest.find('"') else {
                break;
            };
            html_ids.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        let slugs: Vec<String> = entries.iter().map(|entry| entry.slug.clone()).collect();
        assert_eq!(
            slugs, html_ids,
            "navigator dev docs glossary drifted from /docs/glossary"
        );
        for entry in &entries {
            assert!(
                glossary
                    .body_html
                    .contains(&format!("<h2 id=\"{}\">", entry.slug)),
                "published /docs/glossary missing heading for `{}`",
                entry.title
            );
        }
    }

    #[test]
    fn docs_list_includes_the_glossary_page_from_the_bundled_index() {
        let docs = portal::docs::loader::bundled();
        assert!(
            docs.docs().iter().any(|doc| doc.slug == "glossary"),
            "the published docs index must include /docs/glossary"
        );
        assert_eq!(
            docs.find("glossary").map(|doc| doc.title.as_str()),
            Some("Glossary")
        );
    }
}
