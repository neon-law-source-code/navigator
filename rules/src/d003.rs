//! `D003` — a matter dashboard must carry its skeleton.
//!
//! Two checks, both structural:
//!
//! - **Every declared lens carries the universal sections.** A boundary
//!   note and a provenance statement appear on every surface in the
//!   surveyed corpus, and they are the two that say what the page is
//!   *not*. Leaving them to the author is how a client face ships
//!   asserting more certainty than the firm has, so they are part of the
//!   skeleton and this rule holds every lens to them.
//! - **The kind's spine appears somewhere.** A review queue workbench
//!   with no item rail is not a review queue workbench. The spine is
//!   required in at least one lens, not in all of them — a client face
//!   legitimately shows less than the firm's.

use crate::dashboard::{skeleton, Section, UNIVERSAL};
use crate::{kind, line_byte_range, Rule, SourceFile, Violation};

/// `D003` — required sections must be present.
pub struct D003RequiredSection;

impl D003RequiredSection {
    pub const CODE: &'static str = "D003";
}

impl Rule for D003RequiredSection {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(declared) = kind::declared(&file.contents) else {
            return Vec::new();
        };
        let Some(skeleton) = skeleton(declared) else {
            return Vec::new();
        };
        // A missing `lenses:` block is D004's report; without one there is
        // nothing here to check against.
        let Some(lenses) = crate::dashboard::declared_lenses(&file.contents) else {
            return Vec::new();
        };
        let line = crate::dashboard::lenses_line(&file.contents);
        let mut out = Vec::new();
        let violation = |message: String| Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line,
            range: line_byte_range(&file.contents, line),
            message,
        };

        for (lens, names) in &lenses {
            let sections: Vec<Section> = names.iter().filter_map(|n| Section::parse(n)).collect();
            for universal in UNIVERSAL {
                if !sections.contains(universal) {
                    out.push(violation(format!(
                        "Lens `{lens}` is missing `{}` — {}. Every lens of every dashboard \
                         kind carries it.",
                        universal.as_str(),
                        universal.describe().to_lowercase(),
                    )));
                }
            }
        }

        let declared_anywhere: Vec<Section> = lenses
            .iter()
            .flat_map(|(_, names)| names.iter())
            .filter_map(|n| Section::parse(n))
            .collect();
        for required in skeleton.required {
            if !declared_anywhere.contains(required) {
                out.push(violation(format!(
                    "`{}` requires `{}` in at least one lens — {}.",
                    declared.as_str(),
                    required.as_str(),
                    required.describe().to_lowercase(),
                )));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::D003RequiredSection;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("hub.md"),
            contents: body.to_string(),
        }
    }

    const COMPLETE: &str = "---\nkind: authority_library\nlenses:\n  \
lawyer: [authority_table, boundary_note, provenance_statement]\n  \
client: [boundary_note, provenance_statement]\n---\n";

    #[test]
    fn a_complete_composition_passes() {
        assert!(
            D003RequiredSection.lint(&file(COMPLETE)).is_empty(),
            "{:?}",
            D003RequiredSection.lint(&file(COMPLETE)),
        );
    }

    #[test]
    fn a_lens_without_a_boundary_note_is_flagged() {
        let body = "---\nkind: authority_library\nlenses:\n  \
lawyer: [authority_table, provenance_statement]\n---\n";
        let v = D003RequiredSection.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert_eq!(v[0].code, "D003");
        assert!(v[0].message.contains("boundary_note"), "{}", v[0].message);
        assert!(v[0].message.contains("`lawyer`"), "{}", v[0].message);
    }

    #[test]
    fn the_client_lens_is_held_to_the_universal_sections_too() {
        // The failure this rule exists to prevent: a client face that
        // never says what the page is not.
        let body = "---\nkind: authority_library\nlenses:\n  \
lawyer: [authority_table, boundary_note, provenance_statement]\n  \
client: [provenance_statement]\n---\n";
        let v = D003RequiredSection.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert!(v[0].message.contains("`client`"), "{}", v[0].message);
    }

    #[test]
    fn a_missing_spine_section_is_flagged() {
        let body = "---\nkind: review_queue_workbench\nlenses:\n  \
lawyer: [queue_rail, item_detail, boundary_note, provenance_statement]\n---\n";
        let v = D003RequiredSection.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert!(
            v[0].message.contains("item_status_setter"),
            "{}",
            v[0].message,
        );
    }

    #[test]
    fn the_spine_need_only_appear_in_one_lens() {
        let body = "---\nkind: authority_library\nlenses:\n  \
lawyer: [authority_table, boundary_note, provenance_statement]\n  \
client: [boundary_note, provenance_statement]\n---\n";
        assert!(D003RequiredSection.lint(&file(body)).is_empty());
    }

    #[test]
    fn a_non_dashboard_kind_is_left_alone() {
        assert!(D003RequiredSection
            .lint(&file("---\nkind: onboarding\nlenses:\n  lawyer: []\n---\n"))
            .is_empty());
    }
}
