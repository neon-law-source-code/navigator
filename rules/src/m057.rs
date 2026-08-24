//! `M057` — every relative link target must resolve to a file on disk.
//!
//! A markdown cross-reference has to survive in the repo (an editor or
//! GitHub, where a relative path opens the sibling file). This rule
//! resolves each inline-link target against the linking file's own
//! directory and flags the ones that point nowhere — catching typos and
//! dangling references anywhere in the tree. Its sibling
//! [`crate::M061WebPortableLink`] adds the *website* half: whether a
//! resolvable link is also renderable once published at `/docs/:slug`.
//!
//! Only inline links (`[text](target)`) are checked; image embeds
//! (`![alt](src)`) route through the asset seam, not the repo tree.

use std::path::Path;

use crate::links::{link_targets, relative_file_part};
use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct M057RelativeLinkResolves;

impl M057RelativeLinkResolves {
    pub const CODE: &'static str = "M057";
}

impl Rule for M057RelativeLinkResolves {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let dir = file.path.parent().unwrap_or_else(|| Path::new("."));
        let mut violations = Vec::new();
        for (line_no, line) in frontmatter::body_lines(&file.contents) {
            let masked = frontmatter::mask_code_spans(line);
            for target in link_targets(&masked) {
                let Some(file_part) = relative_file_part(&target) else {
                    continue;
                };
                if dir.join(file_part).exists() {
                    continue;
                }
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: line_no,
                    range: line_byte_range(&file.contents, line_no),
                    message: format!("Relative link `{target}` does not resolve to a file on disk"),
                });
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::M057RelativeLinkResolves;
    use crate::{Rule, SourceFile};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Write `rel` under `dir` with `contents`, creating parents.
    fn write(dir: &TempDir, rel: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn source(path: PathBuf, contents: &str) -> SourceFile {
        SourceFile {
            path,
            contents: contents.to_string(),
        }
    }

    #[test]
    fn flags_a_link_to_a_missing_sibling() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "docs/guide.md", "See [the setup](setup.md).\n");
        let v = M057RelativeLinkResolves.lint(&source(path, "See [the setup](setup.md).\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "M057");
        assert!(v[0].message.contains("setup.md"));
    }

    #[test]
    fn passes_when_the_sibling_exists() {
        let dir = TempDir::new().unwrap();
        write(&dir, "docs/setup.md", "# Setup\n");
        let path = write(&dir, "docs/guide.md", "See [setup](setup.md).\n");
        assert!(M057RelativeLinkResolves
            .lint(&source(path, "See [setup](setup.md).\n"))
            .is_empty());
    }

    #[test]
    fn resolves_parent_relative_paths() {
        let dir = TempDir::new().unwrap();
        write(&dir, "store/foo.rs", "// code\n");
        let body = "Impl in [foo](../store/foo.rs).\n";
        let path = write(&dir, "docs/guide.md", body);
        assert!(M057RelativeLinkResolves
            .lint(&source(path, body))
            .is_empty());
        // A typo in the same path is caught.
        let bad = "Impl in [foo](../store/typo.rs).\n";
        let path2 = write(&dir, "docs/guide2.md", bad);
        assert_eq!(M057RelativeLinkResolves.lint(&source(path2, bad)).len(), 1);
    }

    #[test]
    fn strips_anchor_before_resolving() {
        let dir = TempDir::new().unwrap();
        write(&dir, "docs/glossary.md", "# Glossary\n");
        let body = "See [term](glossary.md#lawyer-review).\n";
        let path = write(&dir, "docs/guide.md", body);
        assert!(M057RelativeLinkResolves
            .lint(&source(path, body))
            .is_empty());
    }

    #[test]
    fn ignores_absolute_urls_anchors_and_placeholders() {
        let dir = TempDir::new().unwrap();
        let body = "[site](https://www.neonlaw.com) [top](#intro) [mail](mailto:x@y.com) \
                    [abs](/docs/glossary) [tmpl]({{confirm_url}})\n";
        let path = write(&dir, "docs/guide.md", body);
        assert!(
            M057RelativeLinkResolves
                .lint(&source(path, body))
                .is_empty(),
            "non-repo-relative targets must not be disk-checked"
        );
    }

    #[test]
    fn skips_image_embeds() {
        // Image sources route through the asset seam, not the repo tree.
        let dir = TempDir::new().unwrap();
        let body = "![collage](img/thanks-apple/collage-6.jpg)\n";
        let path = write(&dir, "web/content/blog/post.md", body);
        assert!(
            M057RelativeLinkResolves
                .lint(&source(path, body))
                .is_empty(),
            "image embeds are resolved by the asset seam, not on disk"
        );
    }

    #[test]
    fn ignores_links_inside_code_fences_and_spans() {
        let dir = TempDir::new().unwrap();
        let body = "```\n[x](missing.md)\n```\n\nInline `[y](gone.md)` too.\n";
        let path = write(&dir, "docs/guide.md", body);
        assert!(M057RelativeLinkResolves
            .lint(&source(path, body))
            .is_empty());
    }
}
