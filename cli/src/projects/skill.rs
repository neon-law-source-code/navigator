//! `navigator project skill ...` — the Project Skill catalog
//! (`skills/<jurisdiction>/<practice_area>.md`, compiled into this binary)
//! and the pins a Project records against it in its own `navigator.yaml`.
//!
//! The catalog is embedded at compile time with [`include_dir`], exactly the
//! way [`crate::notations_preview`] embeds the notation catalog through
//! `portal::template_api::bundled_files`. `list`, `show`, and `use` therefore
//! need no network call and no database connection — the same bytes `cargo
//! build` linked in are what a Project pins, so a checkout's gate and a
//! Project's pin can never drift against two different catalogs.
//! `navigator site projects repository sync-skills` (Agent-skill sync, a
//! different feature entirely) is retired precisely because it fetched a
//! catalog over the network; this module deliberately does not reintroduce
//! that pattern.

use std::path::Path;
use std::process::ExitCode;

use include_dir::{include_dir, Dir};

use super::manifest;

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
            rules::project_skill::parse(contents)
                .ok()
                .map(|skill| CatalogEntry {
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
pub fn unresolved_message(
    entries: &[CatalogEntry],
    jurisdiction: &str,
    practice_area: &str,
) -> String {
    let asked = format!(
        "{}/{}",
        jurisdiction.to_uppercase(),
        practice_area.to_lowercase()
    );
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
        println!(
            "{}\t{}\t{}",
            entry.jurisdiction, entry.practice_area, entry.name
        );
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

/// `navigator project skill use <jurisdiction> <practice_area>` — pin the
/// resolved entry's version onto the Project rooted at `dir`, and scaffold
/// each Notation `code` it bundles.
#[must_use]
pub fn run_use(dir: &Path, jurisdiction: &str, practice_area: &str) -> ExitCode {
    let entries = catalog();
    let Some(entry) = find(&entries, jurisdiction, practice_area) else {
        eprintln!(
            "navigator: {}",
            unresolved_message(&entries, jurisdiction, practice_area)
        );
        return ExitCode::from(1);
    };

    let manifest_path = dir.join(manifest::FILE);
    let contents = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("navigator: read {}: {error}", manifest_path.display());
            return ExitCode::from(2);
        }
    };
    let (updated, changed) = match manifest::pin_skill(
        &contents,
        &entry.jurisdiction,
        &entry.practice_area,
        &entry.version,
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    };
    if changed {
        if let Err(error) = std::fs::write(&manifest_path, &updated) {
            eprintln!("navigator: write {}: {error}", manifest_path.display());
            return ExitCode::from(2);
        }
    }

    for code in &entry.notations {
        if let Err(error) = scaffold_notation(dir, code) {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    }

    if changed {
        println!(
            "pinned {}/{} at {}",
            entry.jurisdiction, entry.practice_area, entry.version
        );
    } else {
        println!(
            "{}/{} already pinned at {}",
            entry.jurisdiction, entry.practice_area, entry.version
        );
    }
    ExitCode::SUCCESS
}

/// Materialize one bundled Notation `code` into this Project's
/// `templates/<code>.md`, the flat layout `N110` requires of a Project
/// repository. A no-op when the file already exists — re-running `use`
/// never overwrites an attorney's edits to a scaffolded template. Reads the
/// bundled Notation catalog through [`portal::template_api::bundled_files`],
/// the same compiled-in reader `navigator notation preview`
/// (`crate::notations_preview::bundled_template`) already resolves a
/// template's body from, so the catalog is read in exactly one place rather
/// than embedded a second time.
fn scaffold_notation(dir: &Path, code: &str) -> Result<(), String> {
    let target = dir.join("templates").join(format!("{code}.md"));
    if target.is_file() {
        return Ok(());
    }
    // Matched by the template's own `code:` frontmatter, not its filename
    // stem: only the `notations/forms/` shelf holds stem == code (`N110`);
    // a `notations/neon_law/` template's stem (`onboarding.md`) commonly
    // differs from its stable `code` (`onboarding__letter`).
    let body = portal::template_api::bundled_files()
        .into_iter()
        .find(|(_, bytes)| {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(rules::frontmatter::extract)
                .and_then(|fm| rules::frontmatter::field(fm, "code"))
                .as_deref()
                == Some(code)
        })
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("bundled Notation `{code}` not found in this binary's catalog"))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::write(&target, body).map_err(|error| format!("write {}: {error}", target.display()))
}

/// `navigator project skill status` — every Project Skill this Project's
/// `navigator.yaml` pins, and whether it still resolves.
#[must_use]
pub fn run_status(dir: &Path) -> ExitCode {
    match resolve_pins(dir) {
        Ok(resolutions) => {
            for resolution in &resolutions {
                let state = if resolution.resolved {
                    "resolvable"
                } else {
                    "unresolvable"
                };
                println!(
                    "{}\t{}\t{}\t{state}",
                    resolution.jurisdiction, resolution.practice_area, resolution.pinned_version
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("navigator: {error}");
            ExitCode::from(2)
        }
    }
}

/// One pinned Project Skill, resolved against the compiled-in catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinResolution {
    pub jurisdiction: String,
    pub practice_area: String,
    pub pinned_version: String,
    /// `true` when the catalog still carries this `(jurisdiction,
    /// practice_area)` pair at exactly `pinned_version`.
    pub resolved: bool,
    /// The version the catalog currently carries for this pair, when the
    /// pair itself still resolves (even if the version has moved on).
    pub catalog_version: Option<String>,
}

/// Resolve every `skills:` pin in `<dir>/navigator.yaml` against the
/// compiled-in catalog. Shared by `navigator project gate --check` (ENG-879)
/// and `navigator project skill status` (ENG-880) so the two report the same
/// verdict for the same fixture rather than reimplementing the check twice.
///
/// # Errors
///
/// A string error if `navigator.yaml` cannot be read or does not parse.
pub fn resolve_pins(dir: &Path) -> Result<Vec<PinResolution>, String> {
    let manifest_path = dir.join(manifest::FILE);
    let contents = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let parsed = manifest::parse(&contents)?;
    let entries = catalog();
    Ok(parsed
        .skills
        .into_iter()
        .map(|pin| {
            let catalog_entry = find(&entries, &pin.jurisdiction, &pin.practice_area);
            let (resolved, catalog_version) = match catalog_entry {
                Some(entry) if entry.version == pin.version => (true, Some(entry.version.clone())),
                Some(entry) => (false, Some(entry.version.clone())),
                None => (false, None),
            };
            PinResolution {
                jurisdiction: pin.jurisdiction,
                practice_area: pin.practice_area,
                pinned_version: pin.version,
                resolved,
                catalog_version,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{catalog, find, resolve_pins, run_status, run_use, unresolved_message};

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

    fn scaffold(dir: &std::path::Path, yaml: &str) {
        std::fs::write(dir.join("navigator.yaml"), yaml).unwrap();
    }

    #[test]
    fn use_pins_the_entry_and_scaffolds_its_notations() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), "host: staging.neonlaw.com\nproject: acme\n");
        let code = run_use(dir.path(), "nv", "estates");
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        let manifest_contents = std::fs::read_to_string(dir.path().join("navigator.yaml")).unwrap();
        assert!(manifest_contents.contains("skills"), "{manifest_contents}");
        assert!(manifest_contents.contains("estates"), "{manifest_contents}");
        assert!(
            dir.path().join("templates/onboarding__letter.md").is_file(),
            "the bundled onboarding__letter notation should be scaffolded"
        );
    }

    #[test]
    fn a_second_use_of_the_same_pin_does_not_duplicate_it() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), "host: staging.neonlaw.com\nproject: acme\n");
        assert_eq!(
            run_use(dir.path(), "nv", "estates"),
            std::process::ExitCode::SUCCESS
        );
        assert_eq!(
            run_use(dir.path(), "NV", "estates"),
            std::process::ExitCode::SUCCESS
        );
        let resolutions = resolve_pins(dir.path()).unwrap();
        assert_eq!(resolutions.len(), 1, "{resolutions:?}");
    }

    #[test]
    fn use_of_an_unknown_pair_fails_without_touching_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = "host: staging.neonlaw.com\nproject: acme\n";
        scaffold(dir.path(), yaml);
        let code = run_use(dir.path(), "zz", "estates");
        assert_ne!(code, std::process::ExitCode::SUCCESS);
        let after = std::fs::read_to_string(dir.path().join("navigator.yaml")).unwrap();
        assert_eq!(after, yaml, "a failed use must not modify navigator.yaml");
    }

    #[test]
    fn resolve_pins_reports_a_pin_present_at_its_catalog_version() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(
            dir.path(),
            "host: staging.neonlaw.com\nproject: acme\nskills:\n  - jurisdiction: NV\n    practice_area: estates\n    version: \"1\"\n",
        );
        let resolutions = resolve_pins(dir.path()).unwrap();
        assert_eq!(resolutions.len(), 1);
        assert!(resolutions[0].resolved, "{resolutions:?}");
    }

    #[test]
    fn resolve_pins_reports_an_unknown_code_as_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(
            dir.path(),
            "host: staging.neonlaw.com\nproject: acme\nskills:\n  - jurisdiction: ZZ\n    practice_area: nowhere\n    version: \"1\"\n",
        );
        let resolutions = resolve_pins(dir.path()).unwrap();
        assert_eq!(resolutions.len(), 1);
        assert!(!resolutions[0].resolved);
        assert!(resolutions[0].catalog_version.is_none());
    }

    #[test]
    fn resolve_pins_reports_a_stale_version_as_unresolved_naming_both() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(
            dir.path(),
            "host: staging.neonlaw.com\nproject: acme\nskills:\n  - jurisdiction: NV\n    practice_area: estates\n    version: \"999\"\n",
        );
        let resolutions = resolve_pins(dir.path()).unwrap();
        assert_eq!(resolutions.len(), 1);
        assert!(!resolutions[0].resolved);
        assert_eq!(resolutions[0].pinned_version, "999");
        assert_eq!(resolutions[0].catalog_version.as_deref(), Some("1"));
    }

    const TWO_PIN_FIXTURE: &str = concat!(
        "host: staging.neonlaw.com\nproject: acme\nskills:\n",
        "  - jurisdiction: NV\n    practice_area: estates\n    version: \"1\"\n",
        "  - jurisdiction: TX\n    practice_area: probate\n    version: \"1\"\n",
    );

    const STALE_PIN_FIXTURE: &str = concat!(
        "host: staging.neonlaw.com\nproject: acme\nskills:\n",
        "  - jurisdiction: NV\n    practice_area: estates\n    version: \"999\"\n",
    );

    #[test]
    fn a_fixture_with_two_pins_reports_both_resolvable() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), TWO_PIN_FIXTURE);
        let resolutions = resolve_pins(dir.path()).unwrap();
        assert_eq!(resolutions.len(), 2, "{resolutions:?}");
        assert!(resolutions.iter().all(|r| r.resolved), "{resolutions:?}");
    }

    #[test]
    fn status_and_gate_check_agree_on_the_same_fixture() {
        // `navigator project skill status` (`run_status`, this test) and
        // `navigator project gate --check` (`crate::append_skill_pin_findings`
        // in `cli/src/main.rs`) both read their verdict from
        // `resolve_pins` — this is the shared-fixture proof ENG-880 asks for:
        // a two-pin fixture reports both resolvable, and a stale pin reports
        // unresolved, the same way on both surfaces because there is only one
        // resolution function.
        let resolvable = tempfile::tempdir().unwrap();
        scaffold(resolvable.path(), TWO_PIN_FIXTURE);
        assert_eq!(
            run_status(resolvable.path()),
            std::process::ExitCode::SUCCESS
        );
        assert!(resolve_pins(resolvable.path())
            .unwrap()
            .iter()
            .all(|r| r.resolved));

        let stale = tempfile::tempdir().unwrap();
        scaffold(stale.path(), STALE_PIN_FIXTURE);
        assert_eq!(run_status(stale.path()), std::process::ExitCode::SUCCESS);
        assert!(!resolve_pins(stale.path()).unwrap()[0].resolved);
    }
}
