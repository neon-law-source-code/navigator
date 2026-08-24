//! `C001` — a published content page (blog post or
//! event) must declare a non-empty `title` in its frontmatter.
//!
//! The web content loader (`portal::blog`) deserializes
//! `title` into a non-optional `String`, so a
//! missing or blank title fails the page at load time. This rule catches
//! it at authoring time — the same shape as the notation-template title
//! rule `N101`, but for the content surfaces that are not templates.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct C001ContentTitle;

impl C001ContentTitle {
    pub const CODE: &'static str = "C001";
}

impl Rule for C001ContentTitle {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "Content pages must declare a non-empty `title`."
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
            return report("Missing frontmatter (a content page needs a `title`)");
        };

        match frontmatter::field(fm, "title") {
            Some(value) if !value.trim().is_empty() => Vec::new(),
            _ => report("A content page must declare a non-empty `title`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::C001ContentTitle;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("web/content/blog/20260625_x.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn passes_with_a_title() {
        let f = file("---\ntitle: Hello\ndescription: A post\n---\n\nBody.\n");
        assert!(C001ContentTitle.lint(&f).is_empty());
    }

    #[test]
    fn flags_a_missing_title() {
        let v = C001ContentTitle.lint(&file("---\ndescription: A post\n---\n\nBody.\n"));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "C001");
    }

    #[test]
    fn flags_a_blank_title() {
        let v = C001ContentTitle.lint(&file("---\ntitle: \"  \"\n---\n"));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn flags_a_file_without_frontmatter() {
        let v = C001ContentTitle.lint(&file("# Just a heading\n"));
        assert_eq!(v.len(), 1);
    }
}
