//! `navigator project skill ...` — the Project Skill catalog
//! (`skills/<jurisdiction>/<practice_area>.md`, compiled into this binary).
//!
//! The catalog is embedded at compile time with [`include_dir`], exactly the
//! way [`crate::notations_preview`] embeds the notation catalog through
//! `portal::template_api::bundled_files`. `list` and `show` therefore need no
//! network call and no database connection — the same bytes `cargo build`
//! linked in are what a Project will later pin, so a checkout's gate and a
//! Project's pin can never drift against two different catalogs.

use std::process::ExitCode;

use include_dir::{include_dir, Dir};

static SKILLS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../skills");

/// One catalog entry: a parsed `skills/<jurisdiction>/<practice_area>.md`.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub jurisdiction: String,
    pub practice_area: String,
    pub name: String,
    pub version: String,
    pub notations: Vec<String>,
    pub body: String,
}

/// Every catalog entry compiled into this binary, sorted by
/// `(jurisdiction, practice_area)`.
///
/// A file that fails to parse is skipped rather than panicking the CLI:
/// `navigator project gate` (`rules::project_skill_catalog_violations`,
/// `N126`/`N127`) is what refuses a catalog entry that does not parse, so a
/// released binary always carries a clean catalog. Skipping here, rather
/// than propagating the error, keeps `list`/`show` usable even if that
/// invariant were ever violated — the offending entry is simply invisible
/// instead of crashing every Project Skill command.
#[must_use]
pub fn catalog() -> Vec<CatalogEntry> {
    let mut files = Vec::new();
    collect(&SKILLS, &mut files);
    let mut entries: Vec<CatalogEntry> = files
        .into_iter()
        .filter(|file| file.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .filter_map(|file| {
            let contents = file.contents_utf8()?;
            rules::project_skill::parse(contents).ok().map(|skill| CatalogEntry {
                jurisdiction: skill.jurisdiction,
                practice_area: skill.practice_area,
                name: skill.name,
                version: skill.version,
                notations: skill.notations,
                body: skill.body,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        (a.jurisdiction.as_str(), a.practice_area.as_str())
            .cmp(&(b.jurisdiction.as_str(), b.practice_area.as_str()))
    });
    entries
}

fn collect(dir: &'static Dir<'static>, out: &mut Vec<&'static include_dir::File<'static>>) {
    for file in dir.files() {
        out.push(file);
    }
    for child in dir.dirs() {
        collect(child, out);
    }
}

/// Resolve one catalog entry, matching `jurisdiction` and `practice_area`
/// case-insensitively (`show nv estates` and `show NV estates` are the same
/// lookup).
#[must_use]
pub fn find<'a>(
    entries: &'a [CatalogEntry],
    jurisdiction: &str,
    practice_area: &str,
) -> Option<&'a CatalogEntry> {
    entries.iter().find(|entry| {
        entry.jurisdiction.eq_ignore_ascii_case(jurisdiction)
            && entry.practice_area.eq_ignore_ascii_case(practice_area)
    })
}

/// The message `show` (and, later, `use`) reports when a `(jurisdiction,
/// practice_area)` pair does not resolve: names what was asked for and the
/// catalog entries closest to it by edit distance, so a typo is correctable
/// without dumping the whole catalog.
#[must_use]
pub fn unresolved_message(entries: &[CatalogEntry], jurisdiction: &str, practice_area: &str) -> String {
    let asked = format!("{}/{}", jurisdiction.to_uppercase(), practice_area.to_lowercase());
    let mut ranked: Vec<(usize, String)> = entries
        .iter()
        .map(|entry| {
            let pair = format!("{}/{}", entry.jurisdiction, entry.practice_area);
            (levenshtein(&asked, &pair), pair)
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if ranked.is_empty() {
        return format!(
            "no Project Skill for jurisdiction `{jurisdiction}` practice area `{practice_area}`; \
             the catalog is empty"
        );
    }
    let closest: Vec<String> = ranked.into_iter().take(3).map(|(_, pair)| pair).collect();
    format!(
        "no Project Skill for jurisdiction `{jurisdiction}` practice area `{practice_area}`; \
         closest catalog entries: {}",
        closest.join(", ")
    )
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, &ac) in a.iter().enumerate() {
        let mut curr = vec![i + 1; b.len() + 1];
        curr[0] = i + 1;
        for (j, &bc) in b.iter().enumerate() {
            curr[j + 1] = if ac == bc {
                prev[j]
            } else {
                1 + prev[j].min(prev[j + 1]).min(curr[j])
            };
        }
        prev = curr;
    }
    prev[b.len()]
}

/// `navigator project skill list` — every catalog entry's `jurisdiction`,
/// `practice_area`, and `name`.
#[must_use]
pub fn run_list() -> ExitCode {
    let entries = catalog();
    for entry in &entries {
        println!("{}\t{}\t{}", entry.jurisdiction, entry.practice_area, entry.name);
    }
    ExitCode::SUCCESS
}

/// `navigator project skill show <jurisdiction> <practice_area>` — the
/// resolved entry's full body.
#[must_use]
pub fn run_show(jurisdiction: &str, practice_area: &str) -> ExitCode {
    let entries = catalog();
    match find(&entries, jurisdiction, practice_area) {
        Some(entry) => {
            println!("{}", entry.body.trim_end());
            ExitCode::SUCCESS
        }
        None => {
            eprintln!(
                "navigator: {}",
                unresolved_message(&entries, jurisdiction, practice_area)
            );
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{catalog, find, unresolved_message};

    #[test]
    fn the_catalog_carries_both_seeded_entries() {
        let entries = catalog();
        assert!(
            find(&entries, "NV", "estates").is_some(),
            "nv/estates should be in the catalog: {entries:?}"
        );
        assert!(
            find(&entries, "TX", "probate").is_some(),
            "tx/probate should be in the catalog: {entries:?}"
        );
    }

    #[test]
    fn find_is_case_insensitive_on_jurisdiction() {
        let entries = catalog();
        let upper = find(&entries, "NV", "estates").expect("NV estates resolves");
        let lower = find(&entries, "nv", "estates").expect("nv estates resolves");
        assert_eq!(upper.name, lower.name);
    }

    #[test]
    fn an_unrecognized_jurisdiction_names_the_closest_entries() {
        let entries = catalog();
        assert!(find(&entries, "zz", "estates").is_none());
        let message = unresolved_message(&entries, "zz", "estates");
        assert!(message.contains("zz"), "{message}");
        assert!(message.contains("estates"), "{message}");
    }
}
