#![allow(clippy::doc_markdown)]
//! `N110` — notation templates must live in the private catalog and
//! declare their jurisdiction.
//!
//! The legal notation tree has two shelves:
//!
//! - `neon_law/` for firm-authored product templates.
//! - `forms/` for government form-backed templates whose repo path
//!   mirrors their public bucket key.
//!
//! Jurisdiction is explicit metadata, not a deep practice-area path. A
//! form template at `templates/forms/united_states/nevada/state/
//! nv__llc_formation.md` therefore declares `jurisdiction: NV` and
//! `code: nv__llc_formation`; the blank PDF lives in the public assets
//! bucket at the matching key `forms/united_states/nevada/state/
//! nv__llc_formation.pdf`, pinned by the sibling `.sha256` file.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};
use std::path::Path;

const ROOTS: &[&str] = &["neon_law", "forms"];
pub const JURISDICTIONS: &[(&str, &str)] = &[("NV", "nv"), ("CA", "ca"), ("US", "us")];

pub struct F110JurisdictionPath;

impl F110JurisdictionPath {
    pub const CODE: &'static str = "N110";

    fn segments_under_tree(path: &Path) -> Option<Vec<&str>> {
        let mut comps = path.components();
        let mut found = false;
        for c in comps.by_ref() {
            if let std::path::Component::Normal(seg) = c {
                if seg == "templates" {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return None;
        }
        Some(
            comps
                .filter_map(|c| match c {
                    std::path::Component::Normal(seg) => seg.to_str(),
                    _ => None,
                })
                .collect(),
        )
    }

    fn violation(file: &SourceFile, message: String) -> Violation {
        Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line: 1,
            range: line_byte_range(&file.contents, 1),
            message,
        }
    }
}

impl Rule for F110JurisdictionPath {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "Notation templates must live under `neon_law/` or `forms/` and declare jurisdiction"
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(rel) = Self::segments_under_tree(&file.path) else {
            return Vec::new();
        };
        let Some(first) = rel.first() else {
            return Vec::new();
        };
        if rel.len() == 1 && first.eq_ignore_ascii_case("README.md") {
            return Vec::new();
        }

        let mut violations = Vec::new();
        if !ROOTS.contains(first) {
            violations.push(Self::violation(
                file,
                format!(
                    "Template must live under `neon_law/` or `forms/`; found `{}`",
                    rel.join("/")
                ),
            ));
        }

        let Some(fm) = frontmatter::extract(&file.contents) else {
            violations.push(Self::violation(
                file,
                "Missing frontmatter (file must declare `jurisdiction:`)".to_string(),
            ));
            return violations;
        };
        let jurisdiction = frontmatter::field(fm, "jurisdiction");
        let Some(jurisdiction) = jurisdiction.filter(|j| !j.is_empty()) else {
            violations.push(Self::violation(
                file,
                "Frontmatter is missing required `jurisdiction:` field".to_string(),
            ));
            return violations;
        };
        let Some((_, prefix)) = JURISDICTIONS.iter().find(|(code, _)| *code == jurisdiction) else {
            violations.push(Self::violation(
                file,
                format!(
                    "Unknown jurisdiction `{jurisdiction}`; expected one of {:?}",
                    JURISDICTIONS
                        .iter()
                        .map(|(code, _)| *code)
                        .collect::<Vec<_>>()
                ),
            ));
            return violations;
        };

        if *first == "forms" {
            if !frontmatter::field(fm, "origin_url")
                .is_some_and(|url| url.starts_with("https://") && url.contains(".gov"))
            {
                violations.push(Self::violation(
                    file,
                    "Form templates must declare an HTTPS government `origin_url:`".to_string(),
                ));
            }
            let code = frontmatter::field(fm, "code");
            if let Some(code) = code {
                if !code.starts_with(&format!("{prefix}__")) {
                    violations.push(Self::violation(
                        file,
                        format!(
                            "Form code `{code}` must start with jurisdiction prefix `{prefix}__`"
                        ),
                    ));
                }
                let stem = file.path.file_stem().and_then(|s| s.to_str());
                if stem != Some(code.as_str()) {
                    violations.push(Self::violation(
                        file,
                        format!(
                            "Form filename stem `{}` must match code `{code}`",
                            stem.unwrap_or_default()
                        ),
                    ));
                }
            }
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::F110JurisdictionPath;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn at(path: &str, fm: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            contents: format!("---\n{fm}\n---\nbody\n"),
        }
    }

    #[test]
    fn accepts_a_product_template_with_jurisdiction() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/neon_law/shared/onboarding_letter.md",
            "title: T\ncode: nest__retainer\njurisdiction: NV",
        ));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn accepts_a_form_template_matching_code_and_origin() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/forms/united_states/nevada/state/nv__llc_formation.md",
            "title: T\ncode: nv__llc_formation\njurisdiction: NV\norigin_url: https://www.nvsos.gov/forms",
        ));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn flags_old_top_level_roots() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/united_states/nevada/state/x.md",
            "title: T\ncode: x\njurisdiction: NV",
        ));
        assert_eq!(v[0].code, "N110");
        assert!(v[0].message.contains("neon_law"));
    }

    #[test]
    fn rejects_the_retired_public_template_shelf() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/open_source/mutual_nda.md",
            "title: T\ncode: mutual_nda\njurisdiction: US",
        ));
        assert_eq!(v[0].code, "N110");
        assert!(v[0].message.contains("open_source"), "{v:?}");
    }

    #[test]
    fn flags_missing_jurisdiction() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/neon_law/shared/onboarding_letter.md",
            "title: T\ncode: nest__retainer",
        ));
        assert_eq!(v[0].code, "N110");
        assert!(v[0].message.contains("jurisdiction"));
    }

    #[test]
    fn flags_form_code_that_disagrees_with_filename() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/forms/united_states/nevada/state/nv__llc_formation.md",
            "title: T\ncode: nv__profit_corp_formation\njurisdiction: NV\norigin_url: https://www.nvsos.gov/forms",
        ));
        assert!(v.iter().any(|v| v.message.contains("filename stem")));
    }

    #[test]
    fn flags_unknown_jurisdiction() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/neon_law/shared/onboarding_letter.md",
            "title: T\ncode: nest__retainer\njurisdiction: TX",
        ));
        assert_eq!(v[0].code, "N110");
        assert!(v[0].message.contains("Unknown jurisdiction `TX`"));
    }

    #[test]
    fn flags_form_template_without_government_origin_url() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/forms/united_states/nevada/state/nv__llc_formation.md",
            "title: T\ncode: nv__llc_formation\njurisdiction: NV\norigin_url: http://example.com",
        ));
        assert!(v
            .iter()
            .any(|v| v.message.contains("government `origin_url:`")));
    }

    #[test]
    fn flags_form_code_missing_jurisdiction_prefix() {
        let v = F110JurisdictionPath.lint(&at(
            "templates/forms/united_states/nevada/state/llc_formation.md",
            "title: T\ncode: llc_formation\njurisdiction: NV\norigin_url: https://www.nvsos.gov/forms",
        ));
        assert!(v.iter().any(|v| v
            .message
            .contains("must start with jurisdiction prefix `nv__`")));
    }

    #[test]
    fn flags_missing_frontmatter() {
        let v = F110JurisdictionPath.lint(&SourceFile {
            path: PathBuf::from("templates/neon_law/shared/onboarding_letter.md"),
            contents: "no frontmatter here\n".to_string(),
        });
        assert_eq!(v[0].code, "N110");
        assert!(v[0].message.contains("Missing frontmatter"));
    }

    #[test]
    fn skips_the_catalog_readme() {
        let v = F110JurisdictionPath.lint(&SourceFile {
            path: PathBuf::from("templates/README.md"),
            contents: "# Notation templates\n\nNo frontmatter, and that is fine.\n".to_string(),
        });
        assert!(v.is_empty(), "{v:?}");
    }
}
