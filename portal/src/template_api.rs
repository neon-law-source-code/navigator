//! `GET /app/api/templates/*path` — raw template markdown, served
//! inline so a reader on neonlaw.com sees the same bytes a git reader
//! sees. This backs the repository README's template links (e.g.
//! `templates/forms/united_states/nevada/state/nv__llc_formation.md`)
//! without the `templates/` tree leaving the workspace root:
//! it is still `include_str!`-d by `store::seed` and
//! scanned by `cli validate`. Here `web` embeds the whole tree a second
//! time, read-only, purely to serve it over HTTP.
//!
//! Only templates whose frontmatter explicitly declares
//! `confidential: false` are served. The bulk of the tree is
//! `confidential: true` — client-data-bearing onboarding and engagement
//! bodies — and those return 404. The check **fails closed**: a template
//! with no `confidential` key is treated as confidential, mirroring the
//! curated gallery's allow-list stance (`template_gallery`).

use include_dir::{include_dir, Dir};

use crate::template_paths::kebab_path_eq;

/// The repository `templates/` tree, embedded at build time. The path is
/// resolved against `web`'s manifest dir, so it tracks the dir in place
/// at the workspace root.
static TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../templates");

const LEGACY_ALIASES: &[(&str, &str)] = &[
    (
        "annual_report/nevada",
        "forms/united_states/nevada/state/nv__annual_report",
    ),
    (
        "nonprofit/form990_annual_report",
        "forms/united_states/federal/irs/us__form_990",
    ),
    (
        "nonprofit/nevada_501c3_formation",
        "forms/united_states/nevada/state/nv__nonprofit_501c3_formation",
    ),
    (
        "nonprofit/nevada_charitable_solicitation_registration",
        "forms/united_states/nevada/state/nv__charitable_solicitation_registration",
    ),
    ("onboarding/retainer", "neon_law/shared/retainer"),
];

/// Canonical destination for old public links. Values are repository
/// template paths without `.md`.
#[must_use]
pub fn legacy_alias(path: &str) -> Option<&'static str> {
    LEGACY_ALIASES
        .iter()
        .find_map(|(old, new)| kebab_path_eq(path, old).then_some(*new))
}

/// Raw markdown for a non-confidential template, or `None` when the path
/// is unknown, the template is confidential, or the path could be a
/// traversal attempt.
#[must_use]
pub fn find_raw_path(path: &str) -> Option<&'static str> {
    let path = legacy_alias(path).unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    if !safe_parts(&parts) {
        return None;
    }

    // URLs are kebab-case; the embedded tree keeps the on-disk
    // underscore names. Match by comparing the canonical kebab form of
    // each real path segment rather than guessing an underscore filename
    // from the URL.
    let (stem, dirs) = parts.split_last()?;
    let mut dir = &TEMPLATES;
    for want in dirs {
        dir = dir.dirs().find(|d| {
            d.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| views::slug::to_url(n) == views::slug::to_url(want))
        })?;
    }
    let file = dir.files().find(|f| {
        f.path().extension().and_then(|e| e.to_str()) == Some("md")
            && f.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| views::slug::to_url(s) == views::slug::to_url(stem))
    })?;
    let raw = file.contents_utf8()?;
    is_public(raw).then_some(raw)
}

/// Compatibility wrapper for older two-segment callers.
#[must_use]
pub fn find_raw(category: &str, name: &str) -> Option<&'static str> {
    find_raw_path(&format!("{category}/{name}"))
}

fn safe_parts(parts: &[&str]) -> bool {
    !parts.is_empty()
        && parts
            .iter()
            .all(|s| !s.is_empty() && !s.contains(['\\', '.']))
}

/// Just the `confidential` flag of a template's frontmatter.
#[derive(serde::Deserialize)]
struct ConfidentialFlag {
    confidential: Option<bool>,
}

/// True only when the template's frontmatter carries an explicit
/// `confidential: false`. Absent or `true` → not public (fail closed).
fn is_public(raw: &str) -> bool {
    let Some(frontmatter) = frontmatter_block(raw) else {
        return false;
    };
    matches!(
        serde_yaml::from_str::<ConfidentialFlag>(frontmatter),
        Ok(ConfidentialFlag {
            confidential: Some(false)
        })
    )
}

/// The YAML between the opening `---` and the next `---`, or `None` when
/// the document has no frontmatter fence.
fn frontmatter_block(raw: &str) -> Option<&str> {
    let after = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let end = after.find("\n---")?;
    Some(&after[..end])
}

#[cfg(test)]
mod tests {
    use super::{find_raw, find_raw_path, is_public, legacy_alias};

    #[test]
    fn serves_a_non_confidential_template_verbatim() {
        let raw = find_raw_path("forms/united-states/nevada/state/nv--llc-formation")
            .expect("Nevada LLC formation is public");
        assert!(raw.starts_with("---\n"), "served the raw markdown file");
        assert!(
            raw.contains("Nevada"),
            "served the actual Nevada LLC formation template"
        );
    }

    #[test]
    fn resolves_the_kebab_url_form_to_underscore_filenames() {
        // The route serves kebab-case URLs; the embedded tree keeps the
        // on-disk underscore names. A kebab `name` segment must resolve to
        // its underscore file…
        let by_kebab = find_raw_path("forms/united-states/federal/irs/us--form-990")
            .expect("kebab name resolves to us__form_990.md");
        assert!(by_kebab.contains("Form 990"));
        assert!(find_raw_path("forms/united_states/federal/irs/us__form_990").is_some());
    }

    #[test]
    fn refuses_a_confidential_template() {
        // The retainer is `confidential: true` and must never be served
        // over the public API even though the path is valid.
        assert!(
            find_raw_path("neon_law/shared/retainer").is_none(),
            "confidential templates must 404"
        );
    }

    #[test]
    fn unknown_path_is_none() {
        assert!(find_raw("nope", "missing").is_none());
    }

    #[test]
    fn rejects_path_traversal_segments() {
        assert!(find_raw_path("../nevada").is_none());
        assert!(find_raw_path("forms/../onboarding/retainer").is_none());
        assert!(find_raw_path("forms/..").is_none());
        assert!(find_raw_path("").is_none());
    }

    #[test]
    fn legacy_two_segment_links_resolve_to_canonical_paths() {
        assert_eq!(
            legacy_alias("annual_report/nevada"),
            Some("forms/united_states/nevada/state/nv__annual_report")
        );
        assert!(find_raw("annual_report", "nevada").is_some());
    }

    #[test]
    fn no_two_templates_in_a_directory_collide_under_kebab() {
        // The load-bearing invariant behind `find_raw`: the `_`→`-` URL
        // mapping is lossy, so two files in the same directory whose stems
        // differ only by `_` vs `-` (`a_b.md` and `a-b.md`) would map to
        // one URL — `find_raw` would silently serve the first and the
        // other would be unreachable. Walk the whole embedded tree and
        // fail the build if that ever ships, rather than serving the wrong
        // bytes in production.
        use super::TEMPLATES;
        use std::collections::HashMap;

        let mut stack = vec![&TEMPLATES];
        while let Some(dir) = stack.pop() {
            let mut seen: HashMap<String, &str> = HashMap::new();
            for file in dir.files() {
                if file.path().extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let Some(stem) = file.path().file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let kebab = views::slug::to_url(stem);
                if let Some(prev) = seen.insert(kebab.clone(), stem) {
                    panic!(
                        "templates `{}` and `{}` in {} both map to the kebab URL stem `{}` — \
                         rename one so every templates URL is unambiguous",
                        prev,
                        stem,
                        dir.path().display(),
                        kebab,
                    );
                }
            }
            stack.extend(dir.dirs());
        }
    }

    #[test]
    fn is_public_fails_closed_without_the_key() {
        assert!(!is_public("---\ntitle: X\n---\nbody"));
        assert!(!is_public("no frontmatter at all"));
        assert!(is_public("---\nconfidential: false\n---\nbody"));
        assert!(!is_public("---\nconfidential: true\n---\nbody"));
    }
}
