//! Workspace docs published verbatim at `/docs/:slug`, behind the shared
//! Navigator session boundary.
//!
//! The single source of truth is the workspace-root `docs/` tree.
//! There is **no forked copy** under `web/`: the docs tree is baked into
//! the binary at compile time (the prod
//! image builds from `web/`, so `docs/` is outside it and can't be read
//! at runtime — see [`loader`]). A git reader and a `/docs` visitor see
//! the same bytes.
//!
//! A doc is published only with `publish: true` in its leading YAML
//! frontmatter. This fail-closed boundary keeps contributor and
//! infrastructure documentation repo-local. Each public page carries
//! the informational-not-legal-advice disclaimer ([`views::pages::docs`]).
//!
//! Mirrors the [`crate::marketing`] / [`crate::workshops`] loader shape:
//! an `Arc`-backed index, looked up by slug at request time.

pub mod loader;

use std::sync::Arc;

/// One published doc, rendered to HTML at load time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// Route slug — the file stem (`glossary`, `notation`).
    pub slug: String,
    /// Page title, taken from the doc's leading `# ` heading (falls
    /// back to the slug when the file has none).
    pub title: String,
    /// Rendered HTML body (NOT raw markdown). Sibling `*.md` links are
    /// rewritten to `/docs/*` routes, repo-relative `../` links become
    /// GitHub source URLs, off-site anchors carry the up-right arrow,
    /// and headings carry GitHub-style anchor ids so in-page `#anchor`
    /// links resolve.
    pub body_html: String,
}

/// `Arc`-wrapped lookup shared as router state. Cheap to clone.
#[derive(Debug, Clone)]
pub struct DocsIndex {
    docs: Arc<Vec<Doc>>,
}

impl DocsIndex {
    #[must_use]
    pub fn new(docs: Vec<Doc>) -> Self {
        Self {
            docs: Arc::new(docs),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Every published doc, sorted by slug — for a `/docs` hub or tests.
    #[must_use]
    pub fn docs(&self) -> &[Doc] {
        &self.docs
    }

    #[must_use]
    pub fn find(&self, slug: &str) -> Option<&Doc> {
        self.docs.iter().find(|d| d.slug == slug)
    }
}

#[cfg(test)]
mod tests {
    use super::{Doc, DocsIndex};

    #[test]
    fn empty_index_finds_nothing() {
        let ix = DocsIndex::empty();
        assert!(ix.docs().is_empty());
        assert!(ix.find("glossary").is_none());
    }

    #[test]
    fn populated_index_returns_docs_by_slug() {
        let ix = DocsIndex::new(vec![Doc {
            slug: "glossary".to_string(),
            title: "Glossary".to_string(),
            body_html: "<p>Terms</p>".to_string(),
        }]);

        assert_eq!(ix.docs().len(), 1);
        assert_eq!(
            ix.find("glossary").map(|doc| doc.title.as_str()),
            Some("Glossary")
        );
        assert!(ix.find("missing").is_none());
    }
}
