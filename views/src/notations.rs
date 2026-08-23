//! The `templates/README.md` body that `/notations` publishes.
//!
//! The page stays tied to the repository instructions: the README is baked in
//! at compile time, and its relative Markdown links are rewritten to the paths
//! the running site actually serves. Rendering lives here beside the rest of the
//! Markdown layer; the page itself is a Dioxus component in `webapp`.

use crate::markdown::render_with_link_rewrite;

const README: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../templates/README.md"
));

const REPO_BLOB_BASE: &str =
    "https://github.com/neon-law-source-code/navigator/blob/main/templates/";
const REPO_ROOT_BLOB_BASE: &str = "https://github.com/neon-law-source-code/navigator/blob/main/";

/// The rendered README body, with its links rewritten to site routes.
///
/// The page's hero owns the title, so the README's leading `# Notations`
/// heading is stripped to avoid a duplicate `<h1>`.
#[must_use]
pub fn readme_html() -> String {
    render_with_link_rewrite(strip_leading_h1(README), rewrite_link)
}

/// Drop the leading top-level `# ...` heading line (and the blank lines after
/// it) so the hero band, not the body, carries the page title.
fn strip_leading_h1(md: &str) -> &str {
    match md.split_once('\n') {
        Some((first, rest)) if first.starts_with("# ") => rest.trim_start_matches('\n'),
        _ => md,
    }
}

/// Map one of the README's relative Markdown links onto the URL that serves it:
/// a workspace doc to `/docs/...`, a template to its raw `/app/api/templates/...`
/// route, and anything else to the file on GitHub. Absolute and in-page links
/// pass through untouched.
fn rewrite_link(dest: &str) -> String {
    if dest.starts_with("http://")
        || dest.starts_with("https://")
        || dest.starts_with("mailto:")
        || dest.starts_with('#')
    {
        return dest.to_string();
    }
    let (path, anchor) = match dest.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (dest, None),
    };
    if let Some(stem) = path
        .strip_prefix("../docs/")
        .and_then(|rest| rest.strip_suffix(".md"))
    {
        if !stem.contains('/') {
            return with_anchor(&format!("/docs/{}", crate::slug::to_url(stem)), anchor);
        }
    }
    if path == "../README.md" {
        return with_anchor(&format!("{REPO_ROOT_BLOB_BASE}README.md"), anchor);
    }
    if let Some(stem) = path.strip_suffix(".md") {
        return with_anchor(
            &format!("/app/api/templates/{}", crate::slug::to_url(stem)),
            anchor,
        );
    }
    with_anchor(&format!("{REPO_BLOB_BASE}{path}"), anchor)
}

fn with_anchor(base: &str, anchor: Option<&str>) -> String {
    match anchor {
        Some(a) => format!("{base}#{a}"),
        None => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{readme_html, rewrite_link, strip_leading_h1, README};

    #[test]
    fn strip_leading_h1_drops_only_the_first_heading_line() {
        assert_eq!(
            strip_leading_h1("# Notations\n\nBody first line.\n"),
            "Body first line.\n"
        );
        assert_eq!(strip_leading_h1("No heading here.\n"), "No heading here.\n");
    }

    #[test]
    fn the_page_is_tied_to_the_readme() {
        assert!(README.starts_with("# Notations"));
        assert!(README.contains("## Naming convention"));
    }

    /// The hero owns the title, so the rendered body must not repeat the
    /// README's own `# Notations` heading.
    #[test]
    fn the_rendered_body_drops_the_readme_title() {
        let html = readme_html();
        assert!(
            !html.contains(">Notations</h1>"),
            "the README H1 must be stripped: {html}"
        );
        assert!(
            html.contains("Every notation has YAML frontmatter"),
            "the README body still renders: {html}"
        );
    }

    #[test]
    fn doc_links_map_to_site_routes() {
        assert_eq!(
            rewrite_link("../docs/notation.md#template"),
            "/docs/notation#template"
        );
        assert_eq!(rewrite_link("../docs/glossary.md"), "/docs/glossary");
    }

    #[test]
    fn root_readme_link_maps_to_the_repository_source() {
        assert_eq!(
            rewrite_link("../README.md#trademarks"),
            "https://github.com/neon-law-source-code/navigator/blob/main/README.md#trademarks"
        );
    }

    #[test]
    fn template_links_map_to_the_raw_api() {
        assert_eq!(
            rewrite_link("forms/united_states/nevada/state/nv__llc_formation.md"),
            "/app/api/templates/forms/united-states/nevada/state/nv--llc-formation"
        );
        assert_eq!(
            rewrite_link("forms/united_states/nevada/state/nv__annual_report.md"),
            "/app/api/templates/forms/united-states/nevada/state/nv--annual-report"
        );
    }

    #[test]
    fn other_relative_links_point_at_the_github_source() {
        assert_eq!(
            rewrite_link("forms/united_states/nevada/state/nv__llc_formation.fields.toml"),
            "https://github.com/neon-law-source-code/navigator/blob/main/templates/forms/united_states/nevada/state/nv__llc_formation.fields.toml"
        );
    }
}
