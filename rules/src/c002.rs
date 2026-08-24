//! `C002` — a published content page (blog post or
//! event) must declare a non-empty `description`.
//!
//! The `description` is not decoration: `portal::blog` renders it into the
//! index blurb and the per-post `<meta name="description">`, and the
//! events loader requires it the same way. A page that
//! ships without one renders with an empty social/search summary, so the
//! rule requires it at authoring time.

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct C002ContentDescription;

impl C002ContentDescription {
    pub const CODE: &'static str = "C002";
}

impl Rule for C002ContentDescription {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "Content pages must declare a non-empty `description`."
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
            return report("Missing frontmatter (a content page needs a `description`)");
        };

        match frontmatter::field(fm, "description") {
            Some(value) if !value.trim().is_empty() => Vec::new(),
            _ => report(
                "A content page must declare a non-empty `description` \
                 (it becomes the index blurb and the page's meta description)",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::C002ContentDescription;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("web/content/blog/20260625_x.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn passes_with_a_description() {
        let f = file("---\ntitle: Hello\ndescription: A post\n---\n\nBody.\n");
        assert!(C002ContentDescription.lint(&f).is_empty());
    }

    #[test]
    fn passes_with_a_folded_description() {
        // `>` folded block scalars are the idiom for wrapping a long
        // description across lines; it parses to a non-empty string.
        let f = file("---\ntitle: Hi\ndescription: >\n  One sentence\n  wrapped.\n---\n");
        assert!(C002ContentDescription.lint(&f).is_empty());
    }

    #[test]
    fn flags_a_missing_description() {
        let v = C002ContentDescription.lint(&file("---\ntitle: Hello\n---\n\nBody.\n"));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "C002");
    }

    #[test]
    fn flags_a_blank_description() {
        let v = C002ContentDescription.lint(&file("---\ntitle: Hi\ndescription:\n---\n"));
        assert_eq!(v.len(), 1);
    }
}
