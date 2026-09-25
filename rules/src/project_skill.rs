//! The Project Skill catalog — `skills/{jurisdiction_code_lowercase}/{practice_area}.md`
//! — and the parser/validator every entry is held to.
//!
//! A **Project Skill** is a practice playbook: a jurisdiction, a practice
//! area, a name, a version, and the [Notation](crate::kind) codes it bundles.
//! `navigator project skill use` (in the `cli` crate) pins one onto a
//! Project's `navigator.yaml`. This module owns only the offline shape of
//! the catalog entry — parsing one file's frontmatter into [`ProjectSkill`],
//! and [`catalog_violations`], the cross-file pass that walks `skills/` the
//! way [`crate::code_uniqueness_violations`] walks `templates/` for `N111`.
//!
//! Modeled on [`crate::kind`] and [`crate::s103`] for the parsing/validation
//! style — a closed, explicit `match` over frontmatter fields, a clear
//! `Display` error per failure mode — but this catalog is its own noun with
//! its own frontmatter schema, not a variant of either.
//!
//! # Jurisdiction is validated offline, not through `store::jurisdictions`
//!
//! The acceptance criteria describe resolving `jurisdiction` against the
//! seeded `jurisdiction` table (`store::jurisdictions::find_by_code`). This
//! crate cannot do that: `rules` has no `store` or `surrealdb` dependency
//! (by design — the same engine backs the LSP, `cli validate`, and CI, none
//! of which open a database connection), and `store::jurisdictions::find_by_code`
//! is async besides. `F110JurisdictionPath` (`N110`, notation template
//! jurisdictions) already answers the identical need offline, from
//! [`crate::f110::JURISDICTIONS`] — codes compiled in at build time from
//! `store/seeds/Jurisdiction.yaml`, the same source table `find_by_code`
//! reads at runtime. This module reuses that static rather than opening a
//! second embed of the same seed file.

use crate::f110::JURISDICTIONS;
use crate::{frontmatter, line_byte_range, FileFilter, Violation};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// `N126` — a `skills/` catalog entry's frontmatter is malformed.
pub const MALFORMED_CODE: &str = "N126";
/// `N127` — two catalog entries declare the same `(jurisdiction, practice_area)` pair.
pub const DUPLICATE_CODE: &str = "N127";

/// One parsed `skills/<jurisdiction>/<practice_area>.md` catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSkill {
    /// A seeded jurisdiction code (`NV`, `TX`, `US`) — see
    /// [`crate::f110::JURISDICTIONS`].
    pub jurisdiction: String,
    /// The practice area slug (`estates`, `probate`), lowercase by
    /// convention (it names the file: `skills/nv/estates.md`).
    pub practice_area: String,
    /// The human-readable playbook name.
    pub name: String,
    /// The version a Project pins when it runs `navigator project skill use`.
    pub version: String,
    /// Notation `code`s this skill bundles, scaffolded onto a Project that
    /// pins it.
    pub notations: Vec<String>,
    /// The markdown body below the frontmatter — the playbook itself.
    pub body: String,
}

/// Why a catalog entry failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSkillError {
    /// No leading `---`-fenced frontmatter block at all.
    MissingFrontmatter,
    /// The frontmatter block is not valid YAML.
    InvalidYaml(String),
    /// The frontmatter block is not a top-level mapping.
    NotAMapping,
    /// A required key is absent.
    Missing(&'static str),
    /// A required key is present but empty.
    Empty(&'static str),
    /// A required key is present but is not a scalar (string/number/bool).
    NotAScalar(&'static str),
    /// `notations:` is present but is not a sequence of strings.
    NotASequence(&'static str),
    /// `jurisdiction:` does not name a code seeded in
    /// `store/seeds/Jurisdiction.yaml` — a free-text value like `Nevada`
    /// lands here, not in [`Self::Missing`] or [`Self::Empty`].
    UnknownJurisdiction(String),
}

impl std::fmt::Display for ProjectSkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "missing YAML frontmatter"),
            Self::InvalidYaml(error) => write!(f, "frontmatter is not valid YAML: {error}"),
            Self::NotAMapping => write!(f, "frontmatter must be a mapping"),
            Self::Missing(key) => write!(f, "frontmatter is missing required `{key}:` field"),
            Self::Empty(key) => write!(f, "frontmatter `{key}:` is empty"),
            Self::NotAScalar(key) => write!(f, "frontmatter `{key}:` must be a scalar value"),
            Self::NotASequence(key) => write!(f, "frontmatter `{key}:` must be a list of strings"),
            Self::UnknownJurisdiction(value) => write!(
                f,
                "unknown jurisdiction `{value}`; expected a code seeded in \
                 store/seeds/Jurisdiction.yaml"
            ),
        }
    }
}

impl std::error::Error for ProjectSkillError {}

/// Parse one catalog entry's frontmatter and body.
///
/// # Errors
///
/// See [`ProjectSkillError`] for every rejected shape: missing/empty/
/// non-scalar `jurisdiction`, `practice_area`, `name`, or `version`; a
/// `jurisdiction` that is not a seeded code; or a `notations:` that is not a
/// list of strings.
pub fn parse(contents: &str) -> Result<ProjectSkill, ProjectSkillError> {
    let Some((fm, body)) = frontmatter::split(contents) else {
        return Err(ProjectSkillError::MissingFrontmatter);
    };
    let document: serde_yaml::Value =
        serde_yaml::from_str(fm).map_err(|error| ProjectSkillError::InvalidYaml(error.to_string()))?;
    let mapping = document.as_mapping().ok_or(ProjectSkillError::NotAMapping)?;

    let scalar = |key: &'static str| -> Result<String, ProjectSkillError> {
        match mapping.get(key) {
            None => Err(ProjectSkillError::Missing(key)),
            Some(serde_yaml::Value::Null) => Err(ProjectSkillError::Empty(key)),
            Some(serde_yaml::Value::String(value)) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    Err(ProjectSkillError::Empty(key))
                } else {
                    Ok(trimmed.to_string())
                }
            }
            Some(_) => Err(ProjectSkillError::NotAScalar(key)),
        }
    };

    let jurisdiction = scalar("jurisdiction")?;
    let practice_area = scalar("practice_area")?;
    let name = scalar("name")?;
    let version = scalar("version")?;

    if !JURISDICTIONS.iter().any(|(code, _)| *code == jurisdiction) {
        return Err(ProjectSkillError::UnknownJurisdiction(jurisdiction));
    }

    let notations = match mapping.get("notations") {
        None => Vec::new(),
        Some(serde_yaml::Value::Sequence(items)) => {
            let mut codes = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(code) if !code.trim().is_empty() => codes.push(code.trim().to_string()),
                    _ => return Err(ProjectSkillError::NotASequence("notations")),
                }
            }
            codes
        }
        Some(_) => return Err(ProjectSkillError::NotASequence("notations")),
    };

    Ok(ProjectSkill {
        jurisdiction,
        practice_area,
        name,
        version,
        notations,
        body: body.to_string(),
    })
}

/// Cross-file catalog checks (`N126`/`N127`): walk `<dir>/skills/`, parse
/// every `.md` file, and report each parse failure plus every duplicate
/// `(jurisdiction, practice_area)` pair. A repository with no `skills/`
/// directory simply finds nothing — mirrors
/// [`crate::code_uniqueness_violations`] and [`crate::service_template_violations`].
///
/// # Errors
///
/// An [`io::Error`] if the directory cannot be walked or a file cannot be
/// read.
pub fn catalog_violations(dir: &Path, filter: &dyn FileFilter) -> io::Result<Vec<Violation>> {
    let root = dir.join("skills");
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<(PathBuf, String)> = Vec::new();
    for entry in WalkDir::new(&root).follow_links(false).into_iter().filter_entry(|e| {
        if e.file_type().is_dir() && e.depth() > 0 {
            filter.include_dir(e.path())
        } else {
            true
        }
    }) {
        let entry = entry.map_err(io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        if !filter.include_file(path) {
            continue;
        }
        entries.push((path.to_path_buf(), fs::read_to_string(path)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut violations = Vec::new();
    let mut first_seen: HashMap<(String, String), PathBuf> = HashMap::new();
    for (path, contents) in entries {
        match parse(&contents) {
            Ok(skill) => {
                let pair = (skill.jurisdiction, skill.practice_area);
                if let Some(prev) = first_seen.get(&pair) {
                    violations.push(Violation {
                        code: DUPLICATE_CODE,
                        path: path.clone(),
                        line: 1,
                        range: line_byte_range(&contents, 1),
                        message: format!(
                            "Duplicate Project Skill `{}`/`{}`; already declared in `{}`",
                            pair.0,
                            pair.1,
                            prev.display()
                        ),
                    });
                } else {
                    first_seen.insert(pair, path);
                }
            }
            Err(error) => violations.push(Violation {
                code: MALFORMED_CODE,
                path: path.clone(),
                line: 1,
                range: line_byte_range(&contents, 1),
                message: format!("Project Skill catalog entry is invalid: {error}"),
            }),
        }
    }
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::{parse, ProjectSkillError};

    const ESTATES: &str = "---\njurisdiction: NV\npractice_area: estates\nname: Nevada Estates Playbook\nversion: \"1\"\nnotations:\n  - onboarding__letter\n---\n\nSynthetic playbook body.\n";
    const PROBATE: &str = "---\njurisdiction: TX\npractice_area: probate\nname: Texas Probate Playbook\nversion: \"1\"\nnotations:\n  - onboarding__letter\n  - offboarding__letter\n---\n\nSynthetic playbook body.\n";

    #[test]
    fn parses_the_nevada_estates_seed() {
        let skill = parse(ESTATES).expect("parses");
        assert_eq!(skill.jurisdiction, "NV");
        assert_eq!(skill.practice_area, "estates");
        assert_eq!(skill.name, "Nevada Estates Playbook");
        assert_eq!(skill.version, "1");
        assert_eq!(skill.notations, vec!["onboarding__letter".to_string()]);
        assert!(skill.body.contains("Synthetic playbook body."));
    }

    #[test]
    fn parses_the_texas_probate_seed() {
        let skill = parse(PROBATE).expect("parses");
        assert_eq!(skill.jurisdiction, "TX");
        assert_eq!(skill.practice_area, "probate");
        assert_eq!(
            skill.notations,
            vec!["onboarding__letter".to_string(), "offboarding__letter".to_string()]
        );
    }

    #[test]
    fn missing_jurisdiction_fails_validation() {
        let body = "---\npractice_area: estates\nname: N\nversion: \"1\"\n---\nbody\n";
        assert_eq!(parse(body), Err(ProjectSkillError::Missing("jurisdiction")));
    }

    #[test]
    fn missing_practice_area_fails_validation() {
        let body = "---\njurisdiction: NV\nname: N\nversion: \"1\"\n---\nbody\n";
        assert_eq!(parse(body), Err(ProjectSkillError::Missing("practice_area")));
    }

    #[test]
    fn a_free_text_jurisdiction_fails_validation() {
        let body =
            "---\njurisdiction: Nevada\npractice_area: estates\nname: N\nversion: \"1\"\n---\nbody\n";
        assert_eq!(
            parse(body),
            Err(ProjectSkillError::UnknownJurisdiction("Nevada".to_string()))
        );
    }

    #[test]
    fn an_unrecognized_jurisdiction_code_fails_validation() {
        let body =
            "---\njurisdiction: ZZ\npractice_area: estates\nname: N\nversion: \"1\"\n---\nbody\n";
        assert_eq!(
            parse(body),
            Err(ProjectSkillError::UnknownJurisdiction("ZZ".to_string()))
        );
    }

    #[test]
    fn empty_name_fails_validation() {
        let body = "---\njurisdiction: NV\npractice_area: estates\nname:\nversion: \"1\"\n---\nbody\n";
        assert_eq!(parse(body), Err(ProjectSkillError::Empty("name")));
    }

    #[test]
    fn missing_frontmatter_fails_validation() {
        assert_eq!(parse("no frontmatter here\n"), Err(ProjectSkillError::MissingFrontmatter));
    }

    #[test]
    fn notations_may_be_absent() {
        let body = "---\njurisdiction: NV\npractice_area: estates\nname: N\nversion: \"1\"\n---\nbody\n";
        assert_eq!(parse(body).unwrap().notations, Vec::<String>::new());
    }

    #[test]
    fn a_non_string_notations_entry_fails_validation() {
        let body = "---\njurisdiction: NV\npractice_area: estates\nname: N\nversion: \"1\"\nnotations:\n  - 1\n---\nbody\n";
        assert_eq!(parse(body), Err(ProjectSkillError::NotASequence("notations")));
    }

    #[test]
    fn every_seeded_jurisdiction_code_is_accepted() {
        for (code, _prefix) in crate::f110::JURISDICTIONS.iter() {
            let body =
                format!("---\njurisdiction: {code}\npractice_area: estates\nname: N\nversion: \"1\"\n---\nbody\n");
            assert!(parse(&body).is_ok(), "jurisdiction `{code}` should parse");
        }
    }
}
