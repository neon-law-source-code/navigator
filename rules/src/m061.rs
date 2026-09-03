//! `M061` — inside web-published markdown, relative links must be
//! web-portable.
//!
//! `docs/` publishes at `/docs/:slug`, so a link the renderer can't
//! rewrite becomes a dead link on the site. The docs renderer
//! ([`portal::docs::loader::rewrite_link`]) maps a sibling `name.md` to
//! `/docs/name` and a single `../` repo path (from `docs/`) to a GitHub
//! blob or tree URL. A relative link that is neither — a same-directory
//! `foo.rs`, or `../../` climbing out of the repository — has no page
//! on the website.
//!
//! Sibling and cross-tree `.md` links stay allowed (they still open in
//! an editor); an absolute `https://…` canonical URL is the escape
//! hatch for a genuine off-tree reference.
//!
//! Warning-severity: its sibling [`crate::M057RelativeLinkResolves`] is
//! the disk-resolution half (an error — a broken path is a bug). Both
//! check inline links only; image embeds route through the asset seam.

use std::path::{Component, Path};

use crate::links::{link_targets, relative_file_part};
use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct M061WebPortableLink;

impl M061WebPortableLink {
    pub const CODE: &'static str = "M061";
}

impl Rule for M061WebPortableLink {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        if !is_web_published(&file.path) {
            return Vec::new();
        }
        let mut violations = Vec::new();
        for (line_no, line) in frontmatter::body_lines(&file.contents) {
            let masked = frontmatter::mask_code_spans(line);
            for target in link_targets(&masked) {
                let Some(file_part) = relative_file_part(&target) else {
                    continue;
                };
                // A sibling or cross-tree `.md` link stays allowed: it
                // opens the file in an editor, and the docs renderer
                // rewrites a sibling `name.md` to its `/docs/name`
                // route. A single `../` from `docs/` is rewritten to a
                // GitHub source URL. Only a relative path the renderer
                // cannot map has no page on the website.
                if Path::new(file_part)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
                    || docs_renderer_rewrites(file_part)
                {
                    continue;
                }
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: line_no,
                    range: line_byte_range(&file.contents, line_no),
                    message: format!(
                        "Relative link `{target}` renders verbatim on the website \
                         (docs publish at `/docs/:slug`) and would 404; point it \
                         at a published `/docs/…` page, a `../` repo path the \
                         renderer can map to GitHub, or drop the link"
                    ),
                });
            }
        }
        violations
    }
}

/// True when `file_part` is a single `../` climb from `docs/` — the
/// renderer maps that onto Navigator's GitHub tree. `../../` leaves the
/// repository and still 404s on the site.
fn docs_renderer_rewrites(file_part: &str) -> bool {
    file_part
        .strip_prefix("../")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with("../") && !rest.starts_with('/'))
}

/// True when `path` is markdown that renders on the public website —
/// today, anything under a `docs/` directory, which publishes verbatim
/// at `/docs/:slug`.
fn is_web_published(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(seg) => seg.eq_ignore_ascii_case("docs"),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::M061WebPortableLink;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn source(path: &str, contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            contents: contents.to_string(),
        }
    }

    #[test]
    fn flags_a_same_directory_code_file_link_in_docs() {
        let body = "The entity lives in [expunge_record.rs](expunge_record.rs).\n";
        let v = M061WebPortableLink.lint(&source("docs/glossary.md", body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "M061");
        assert!(v[0].message.contains("expunge_record.rs"));
    }

    #[test]
    fn allows_a_repo_relative_source_link_the_renderer_maps_to_github() {
        let body = "Schema: [navigator.surql](../store/src/schema/navigator.surql).\n";
        assert!(
            M061WebPortableLink
                .lint(&source("docs/glossary.md", body))
                .is_empty(),
            "`../` from docs/ is rewritten to GitHub"
        );
        let climbing = "See [outside](../../outside.rs).\n";
        assert_eq!(
            M061WebPortableLink
                .lint(&source("docs/glossary.md", climbing))
                .len(),
            1,
            "`../../` climbs out of the repository and still 404s"
        );
    }

    #[test]
    fn allows_sibling_and_cross_tree_md_links() {
        // Sibling `.md` (renderer rewrites → /docs/name) and a
        // cross-tree `.md` (still opens in an editor) are both fine.
        let body =
            "See [glossary](glossary.md) and [mission](../web/content/marketing/mission.md).\n";
        assert!(
            M061WebPortableLink
                .lint(&source("docs/access-model.md", body))
                .is_empty(),
            "`.md` links stay allowed under the renderer's rewrite contract"
        );
    }

    #[test]
    fn allows_absolute_canonical_url_escape_hatch() {
        let body = "See [the glossary](https://www.neonlaw.com/docs/glossary).\n";
        assert!(M061WebPortableLink
            .lint(&source("docs/guide.md", body))
            .is_empty());
    }

    #[test]
    fn only_fires_on_web_published_docs() {
        // The same unrewritable code-file link outside `docs/` is not
        // M061's concern — only the docs tree renders at /docs/:slug.
        let body = "Impl in [foo](foo.rs).\n";
        assert!(
            M061WebPortableLink
                .lint(&source("cli/README.md", body))
                .is_empty(),
            "non-docs markdown is not web-published"
        );
        assert_eq!(
            M061WebPortableLink
                .lint(&source("docs/notes.md", body))
                .len(),
            1,
            "the docs copy is flagged"
        );
    }

    #[test]
    fn skips_images_and_anchors() {
        let body = "![erd](../images/erd.svg) and [top](#intro)\n";
        assert!(M061WebPortableLink
            .lint(&source("docs/erd.md", body))
            .is_empty());
    }
}
