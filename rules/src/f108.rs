//! `N108` — notation templates must declare a stable `code:`.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F108TemplateCodeRequired;

impl F108TemplateCodeRequired {
    pub const CODE: &'static str = "N108";
}

impl Rule for F108TemplateCodeRequired {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let report = |message: &str| -> Vec<Violation> {
            vec![Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line: 1,
                range: line_byte_range(&file.contents, 1),
                message: message.to_string(),
            }]
        };

        let Some(fm) = frontmatter::extract(&file.contents) else {
            return report("Missing frontmatter (file must declare `code:`)");
        };
        match frontmatter::field(fm, "code") {
            None => report("Frontmatter is missing required `code:` field"),
            Some(value) if value.is_empty() => report("Frontmatter `code:` is empty"),
            Some(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::F108TemplateCodeRequired;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("template.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn passes_when_code_is_present() {
        let f = file("---\ntitle: T\ncode: onboarding__engagement_letter\n---\n");
        assert!(F108TemplateCodeRequired.lint(&f).is_empty());
    }

    #[test]
    fn flags_missing_code() {
        let v = F108TemplateCodeRequired.lint(&file("---\ntitle: T\n---\n"));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N108");
        assert!(v[0].message.contains("missing"));
    }

    #[test]
    fn flags_empty_code() {
        let v = F108TemplateCodeRequired.lint(&file("---\ncode:\n---\n"));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "N108");
        assert!(v[0].message.contains("empty"));
    }
}
