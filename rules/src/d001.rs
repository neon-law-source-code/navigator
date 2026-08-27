//! `D001` — a section named in a matter dashboard's `lenses:` block must
//! be a recognized section type.
//!
//! An unrecognized name is a blocking error rather than a dropped
//! section: the renderer only ever sees registered sections, so a typo
//! would otherwise publish a page that silently lacks the thing the
//! attorney thought they put on it. If the shape genuinely does not
//! exist, the fix is a new section type in the registry (#888) — never a
//! hand-written page.

use crate::dashboard::Section;
use crate::{kind, line_byte_range, Rule, SourceFile, Violation};

/// `D001` — every section named in `lenses:` must be a known type.
pub struct D001UnknownSection;

impl D001UnknownSection {
    pub const CODE: &'static str = "D001";
}

impl Rule for D001UnknownSection {
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
        let Some(lenses) = crate::dashboard::declared_lenses(&file.contents) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (lens, sections) in lenses {
            for name in sections {
                if Section::parse(&name).is_some() {
                    continue;
                }
                let line = crate::dashboard::section_line(&file.contents, &name);
                out.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line,
                    range: line_byte_range(&file.contents, line),
                    message: format!(
                        "Lens `{lens}` names `{name}`, which is not a section type. \
                         Known sections: {}",
                        crate::dashboard::section_names().join(", "),
                    ),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::D001UnknownSection;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("hub.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn a_known_section_passes() {
        let body = "---\nkind: authority_library\nlenses:\n  lawyer: [authority_table]\n---\n";
        assert!(D001UnknownSection.lint(&file(body)).is_empty());
    }

    #[test]
    fn an_unknown_section_is_flagged_with_the_vocabulary() {
        let body = "---\nkind: authority_library\nlenses:\n  lawyer: [vibe_chart]\n---\n";
        let v = D001UnknownSection.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert_eq!(v[0].code, "D001");
        assert!(v[0].message.contains("`vibe_chart`"), "{}", v[0].message);
        assert!(
            v[0].message.contains("authority_table"),
            "the message must name the vocabulary: {}",
            v[0].message,
        );
        assert_eq!(v[0].line, 4, "underlines the offending lens line");
    }

    #[test]
    fn a_non_dashboard_kind_is_left_alone() {
        // A notation template with an unrelated `lenses:` key is not this
        // rule's business.
        let body = "---\nkind: onboarding\nlenses:\n  lawyer: [vibe_chart]\n---\n";
        assert!(D001UnknownSection.lint(&file(body)).is_empty());
        assert!(D001UnknownSection.lint(&file("plain prose")).is_empty());
    }
}
