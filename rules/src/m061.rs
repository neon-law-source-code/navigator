//! `M061` — inside web-published markdown, relative links must be
//! web-portable.
//!
//! `docs/glossary/` publishes on one page at `/glossary`, so a link the
//! renderer can't rewrite becomes a dead link on the site. The glossary
//! renderer (`portal::glossary::rewrite_link`) maps a sibling term
//! `name.md` to the in-page `#name` and any `../` repo path (from
//! `docs/glossary/`) to a GitHub blob or tree URL. A relative link that is
//! neither — a same-directory `foo.rs`, or a `../` run climbing out of the
//! repository — has no page on the website.
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
                // opens the file in an editor, and the glossary renderer
                // rewrites a sibling `name.md` to its in-page `#name`. A
                // `../` path that stays inside the repository is
                // rewritten to a GitHub source URL. Only a relative path
                // the renderer cannot map has no page on the website.
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
                         (the glossary publishes at `/glossary`) and would 404; \
                         point it at a sibling term, a `../../` repo path the \
                         renderer can map to GitHub, or drop the link"
                    ),
                });
            }
        }
        violations
    }
}

/// How far `docs/glossary/` sits below the repository root: the most
/// `../` segments a link can climb and still name a repository path.
const GLOSSARY_DEPTH: usize = 2;

/// True when `file_part` climbs from `docs/glossary/` with `../` and
/// stays inside the repository — the renderer maps that onto Navigator's
/// GitHub tree. A deeper climb leaves the repository and still 404s.
fn docs_renderer_rewrites(file_part: &str) -> bool {
    let mut rest = file_part;
    let mut climbs = 0;
    while let Some(next) = rest.strip_prefix("../") {
        rest = next;
        climbs += 1;
    }
    (1..=GLOSSARY_DEPTH).contains(&climbs) && !rest.is_empty() && !rest.starts_with('/')
}

/// True when `path` is markdown that renders on the public website —
/// a file directly under `docs/glossary/`, which publishes on `/glossary`.
fn is_web_published(path: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(seg) => Some(seg),
            _ => None,
        })
        .collect();
    parts.len() >= 3
        && parts[parts.len() - 3].eq_ignore_ascii_case("docs")
        && parts[parts.len() - 2].eq_ignore_ascii_case("glossary")
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
    fn flags_a_same_directory_code_file_link_in_the_glossary() {
        let body = "The entity lives in [expunge_record.rs](expunge_record.rs).\n";
        let v = M061WebPortableLink.lint(&source("docs/glossary/expunge.md", body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "M061");
        assert!(v[0].message.contains("expunge_record.rs"));
    }

    #[test]
    fn allows_a_repo_relative_source_link_the_renderer_maps_to_github() {
        for body in [
            "Schema: [navigator.surql](../../store/src/schema/navigator.surql).\n",
            "Crate: [store](../../store/).\n",
            "Guide: [notation](../notation.md).\n",
        ] {
            assert!(
                M061WebPortableLink
                    .lint(&source("docs/glossary/person.md", body))
                    .is_empty(),
                "`../` inside the repository is rewritten to GitHub: {body}"
            );
        }
        let climbing = "See [outside](../../../outside.rs).\n";
        assert_eq!(
            M061WebPortableLink
                .lint(&source("docs/glossary/person.md", climbing))
                .len(),
            1,
            "`../../../` climbs out of the repository and still 404s"
        );
    }

    #[test]
    fn allows_sibling_term_links() {
        let body = "See [Matter](matter.md) and [Project](project.md).\n";
        assert!(
            M061WebPortableLink
                .lint(&source("docs/glossary/workshop.md", body))
                .is_empty(),
            "a sibling term renders as an in-page anchor"
        );
    }

    #[test]
    fn allows_absolute_canonical_url_escape_hatch() {
        let body = "See [the glossary](https://www.neonlaw.com/glossary).\n";
        assert!(M061WebPortableLink
            .lint(&source("docs/glossary/matter.md", body))
            .is_empty());
    }

    #[test]
    fn only_fires_on_the_published_glossary() {
        // The same unrewritable code-file link outside `docs/glossary/` is
        // not M061's concern — only the glossary renders on the website.
        let body = "Impl in [foo](foo.rs).\n";
        for unpublished in ["cli/README.md", "docs/notes.md", "docs/glossary/deep/x.md"] {
            assert!(
                M061WebPortableLink
                    .lint(&source(unpublished, body))
                    .is_empty(),
                "{unpublished} is not web-published"
            );
        }
        assert_eq!(
            M061WebPortableLink
                .lint(&source("docs/glossary/notes.md", body))
                .len(),
            1,
            "the glossary copy is flagged"
        );
    }

    #[test]
    fn skips_images_and_anchors() {
        let body = "![diagram](../../images/diagram.svg) and [top](#intro)\n";
        assert!(M061WebPortableLink
            .lint(&source("docs/glossary/notes.md", body))
            .is_empty());
    }
}
