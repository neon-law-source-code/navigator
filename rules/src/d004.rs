//! `D004` — a matter dashboard must declare its per-lens composition.
//!
//! The client, lawyer, and clerk section lists live in **one file** (#690),
//! which is what keeps a dashboard's faces from drifting apart across
//! separate documents. A composition with no `lenses:` block has no face
//! at all, and a lens named something outside the vocabulary is a face
//! nothing will ever render.

use crate::dashboard::Lens;
use crate::{kind, line_byte_range, Rule, SourceFile, Violation};

/// `D004` — `lenses:` must be present and name only known lenses.
pub struct D004LensComposition;

impl D004LensComposition {
    pub const CODE: &'static str = "D004";
}

impl Rule for D004LensComposition {
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
        if !declared.is_dashboard() {
            return Vec::new();
        }
        let known: Vec<&str> = Lens::ALL.iter().map(|l| l.as_str()).collect();
        let line = crate::dashboard::lenses_line(&file.contents);
        let violation = |message: String| Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line,
            range: line_byte_range(&file.contents, line),
            message,
        };

        let Some(lenses) = crate::dashboard::declared_lenses(&file.contents) else {
            return vec![violation(format!(
                "`{}` must declare a `lenses:` mapping naming at least one of: {}",
                declared.as_str(),
                known.join(", "),
            ))];
        };
        if lenses.is_empty() {
            return vec![violation(format!(
                "`lenses:` is empty — declare at least one of: {}",
                known.join(", "),
            ))];
        }
        lenses
            .iter()
            .filter(|(name, _)| Lens::parse(name).is_none())
            .map(|(name, _)| {
                violation(format!(
                    "`{name}` is not a lens (expected one of: {})",
                    known.join(", "),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::D004LensComposition;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("hub.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn known_lenses_pass() {
        let body = "---\nkind: authority_library\nlenses:\n  lawyer: [authority_table]\n  \
client: []\n  clerk: []\n---\n";
        assert!(D004LensComposition.lint(&file(body)).is_empty());
    }

    #[test]
    fn a_missing_lenses_block_is_flagged() {
        let v = D004LensComposition.lint(&file("---\nkind: authority_library\n---\n"));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert_eq!(v[0].code, "D004");
        assert!(v[0].message.contains("lenses"), "{}", v[0].message);
    }

    #[test]
    fn an_empty_lenses_block_is_flagged() {
        let v = D004LensComposition.lint(&file("---\nkind: authority_library\nlenses: {}\n---\n"));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert!(v[0].message.contains("empty"), "{}", v[0].message);
    }

    #[test]
    fn an_unknown_lens_is_flagged() {
        let body = "---\nkind: authority_library\nlenses:\n  lawyer: []\n  partner: []\n---\n";
        let v = D004LensComposition.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert!(v[0].message.contains("`partner`"), "{}", v[0].message);
        assert!(v[0].message.contains("clerk"), "{}", v[0].message);
    }

    #[test]
    fn a_non_dashboard_kind_is_left_alone() {
        assert!(D004LensComposition
            .lint(&file("---\nkind: onboarding\n---\n"))
            .is_empty());
    }
}
