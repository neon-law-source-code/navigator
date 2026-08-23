//! The front-matter-and-body shape every `.md` content file in this tree
//! parses into, and the parser that reads one.
//!
//! Front-matter declares the document's title, slug, and short description;
//! the body is rendered to HTML via pulldown-cmark at load time so a handler
//! ships the HTML verbatim. [`loader::parse`] is the single reader, shared by
//! [`crate::blog`] and the workshop loader, which is what keeps a post and a
//! slide resolving an author-written picture identically.

pub mod loader;

use std::collections::HashMap;

/// One marketing fragment.
///
/// `metadata` holds frontmatter keys that aren't one of the four
/// well-known fields (`title`, `slug`, `description`, body). Long-lived
/// content uses it for partner-org details on `/help` entries and
/// `bar_admissions` on `/about` bios — fields the page renderer reads
/// by name. Unknown keys round-trip so the loader stays decoupled
/// from the schema of any one content tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketingDoc {
    pub slug: String,
    pub title: String,
    pub description: String,
    /// Rendered HTML body (NOT raw markdown).
    pub body_html: String,
    pub metadata: HashMap<String, String>,
}
