//! The shared public marketing catalog: the one place a sentence that both
//! Navigator and `navigator-ux` publish is authored.
//!
//! Navigator's page catalogs under `locales/en/<brand-key>/` stay the typed
//! documents their renderers take. This module holds the *shared* subset —
//! the prose both repositories put in front of the same visitor — as stable
//! keys, so a page catalog references a sentence instead of repeating it and
//! the other repository consumes the exported catalog instead of keeping a
//! second copy.
//!
//! Three rules make that safe to depend on:
//!
//! - **Version.** [`SharedCatalog::catalog_version`] is an explicit integer.
//!   A consumer built against a version it does not support must refuse the
//!   document rather than render half of it.
//! - **Required copy.** [`REQUIRED_KEYS`] is the copy a page cannot render
//!   without. A catalog missing one fails validation; it never falls back to
//!   an empty string.
//! - **Fallback.** A brand may override an entry under [`SharedCatalog::brands`].
//!   Lookup reads that brand's override and otherwise the shared default —
//!   never another brand's override. An override of a key the default does
//!   not define is refused, so a brand cannot smuggle a key into the contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The catalog version this build authors and understands.
pub const SUPPORTED_CATALOG_VERSION: u32 = 1;

/// The stem a shared catalog file carries: `locales/en/shared.yaml`.
pub const SHARED_CATALOG_STEM: &str = "shared";

/// The brand placeholders a catalog value may carry. These are the same two
/// [`crate::locales::interpolate`] substitutes, and the list is closed so a
/// typo becomes a validation error rather than a literal brace on the page.
pub const SUPPORTED_PLACEHOLDERS: &[&str] = &["site_name", "firm_email"];

/// The keys a published page cannot render without.
///
/// Absence is a validation failure, not a fallback: a missing headline is a
/// blank page, and a blank page is worse than a failed build.
pub const REQUIRED_KEYS: &[&str] = &[
    "fractional_gc.eyebrow",
    "fractional_gc.lede",
    "fractional_gc.price",
    "fractional_gc.title",
    "home.mission_heading",
    "home.mission_north_star",
    "home.mission_promise",
    "home.need_prompt",
    "litigation.cta",
    "litigation.eyebrow",
    "litigation.lede",
    "litigation.title",
    "personal_plan.eyebrow",
    "personal_plan.price",
    "personal_plan.title",
    "services.catalog_heading",
    "services.eyebrow",
    "services.lede",
    "services.subscriptions_heading",
    "services.title",
];

/// The shared catalog document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedCatalog {
    /// The contract version. See [`SUPPORTED_CATALOG_VERSION`].
    pub catalog_version: u32,
    /// The shared default wording, by stable key.
    pub entries: BTreeMap<String, String>,
    /// Per-brand overrides. A brand with no entry here reads the defaults.
    #[serde(default)]
    pub brands: BTreeMap<String, BTreeMap<String, String>>,
}

impl SharedCatalog {
    /// Deserialize and validate one shared catalog document.
    ///
    /// The checks are the contract both consumers rely on: a supported
    /// version, every required key, values that are safe to substitute, and
    /// only placeholders this build knows how to fill.
    pub fn parse(yaml: &str) -> Result<Self, String> {
        let catalog: Self = serde_yaml::from_str(yaml).map_err(|err| format!("shared: {err}"))?;
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), String> {
        if self.catalog_version != SUPPORTED_CATALOG_VERSION {
            return Err(format!(
                "shared: catalog version {} is not supported; this build authors version {SUPPORTED_CATALOG_VERSION}",
                self.catalog_version
            ));
        }
        for key in REQUIRED_KEYS {
            if !self.entries.contains_key(*key) {
                return Err(format!("shared: required key `{key}` is missing"));
            }
        }
        for (key, value) in &self.entries {
            check_value(key, value)?;
        }
        for (brand, overrides) in &self.brands {
            for (key, value) in overrides {
                if !self.entries.contains_key(key) {
                    return Err(format!(
                        "shared: brand `{brand}` overrides `{key}`, which the shared defaults do not define"
                    ));
                }
                check_value(key, value)?;
            }
        }
        Ok(())
    }

    /// The wording `brand_key` publishes for `key`.
    ///
    /// The brand's own override wins; otherwise the shared default. A brand
    /// never reads another brand's override, so a missing translation shows
    /// the shared sentence rather than a competitor's.
    #[must_use]
    pub fn lookup(&self, brand_key: &str, key: &str) -> Option<&str> {
        self.brands
            .get(brand_key)
            .and_then(|overrides| overrides.get(key))
            .or_else(|| self.entries.get(key))
            .map(String::as_str)
    }

    /// Every key this catalog defines, in stable order.
    #[must_use]
    pub fn keys(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// The bytes an integrity digest covers: compact JSON with every object
    /// key sorted. Both the Rust exporter and the TypeScript consumer can
    /// reproduce this exactly, which is what lets a consumer prove the file
    /// it loaded is the file that was exported.
    #[must_use]
    pub fn canonical_payload(&self, source: &Provenance) -> String {
        let payload = ExportPayload {
            catalog_version: self.catalog_version,
            source: source.clone(),
            entries: self.entries.clone(),
            brands: self.brands.clone(),
        };
        serde_json::to_string(&payload).expect("invariant: the export payload is plain JSON data")
    }
}

/// Where an exported catalog came from, recorded in the export itself so a
/// consumer's pin names an immutable revision rather than a moving branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The catalog's path inside that repository.
    pub path: String,
    /// The producing repository, e.g. `neon-law-source-code/navigator`.
    pub repository: String,
    /// The immutable commit the catalog was read at.
    pub revision: String,
}

/// The digest-covered half of an export.
#[derive(Debug, Serialize)]
struct ExportPayload {
    brands: BTreeMap<String, BTreeMap<String, String>>,
    catalog_version: u32,
    entries: BTreeMap<String, String>,
    source: Provenance,
}

fn check_value(key: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("shared: `{key}` is empty"));
    }
    if value.contains('\n') {
        return Err(format!(
            "shared: `{key}` spans more than one line; a shared value is substituted into a quoted scalar"
        ));
    }
    if value.contains('"') {
        return Err(format!(
            "shared: `{key}` contains a double quote, which cannot be substituted into a quoted scalar"
        ));
    }
    for placeholder in placeholders(value) {
        if !SUPPORTED_PLACEHOLDERS.contains(&placeholder.as_str()) {
            return Err(format!(
                "shared: `{key}` uses unsupported placeholder `{{{placeholder}}}`; expected one of {}",
                SUPPORTED_PLACEHOLDERS.join(", ")
            ));
        }
    }
    Ok(())
}

/// Every `{token}` in `raw`, in order of appearance.
#[must_use]
pub fn placeholders(raw: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = raw.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        let Some(end) = raw[index + 1..].find('}') else {
            break;
        };
        found.push(raw[index + 1..index + 1 + end].to_string());
        index += end + 2;
    }
    found
}

/// The prefix a page catalog uses to reference a shared sentence.
const REFERENCE_PREFIX: &str = "shared:";

/// Replace every `{shared:<key>}` reference in `raw` with `brand_key`'s wording.
///
/// This runs over the raw YAML before it is deserialized, exactly as
/// [`crate::locales::interpolate`] does, so a reference works in any string
/// field without the page schema knowing about it. An unknown key is an
/// error: a page that references copy nobody authored must fail loudly rather
/// than render a brace.
pub fn resolve_references(
    raw: &str,
    catalog: &SharedCatalog,
    brand_key: &str,
) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start + 1..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        let token = &rest[start + 1..start + 1 + end];
        if let Some(key) = token.strip_prefix(REFERENCE_PREFIX) {
            let value = catalog.lookup(brand_key, key).ok_or_else(|| {
                format!("shared: `{{shared:{key}}}` names a key the catalog does not define")
            })?;
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(token);
            out.push('}');
        }
        rest = &rest[start + 1 + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Every shared key `raw` references.
#[must_use]
pub fn referenced_keys(raw: &str) -> Vec<String> {
    placeholders(raw)
        .into_iter()
        .filter_map(|token| {
            token
                .strip_prefix(REFERENCE_PREFIX)
                .map(std::string::ToString::to_string)
        })
        .collect()
}

/// Refuse a placeholder a brand crate cannot fill.
///
/// A page catalog may carry the two brand placeholders and any shared
/// reference. Anything else is a typo that would otherwise reach the page as
/// a literal brace, so `navigator validate` rejects it.
pub fn check_page_placeholders(stem: &str, raw: &str) -> Result<(), String> {
    for placeholder in placeholders(raw) {
        if placeholder.starts_with(REFERENCE_PREFIX)
            || SUPPORTED_PLACEHOLDERS.contains(&placeholder.as_str())
        {
            continue;
        }
        return Err(format!(
            "{stem}: unsupported placeholder `{{{placeholder}}}`; expected one of {}, or `{{shared:<key>}}`",
            SUPPORTED_PLACEHOLDERS.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    /// A catalog with every required key, used as the base a test mutates.
    fn fixture() -> String {
        let mut yaml = String::from("catalog_version: 1\nentries:\n");
        for key in REQUIRED_KEYS {
            writeln!(yaml, "  {key}: Words for {key}.").expect("write to a String");
        }
        yaml
    }

    #[test]
    fn a_complete_catalog_parses() {
        let catalog = SharedCatalog::parse(&fixture()).expect("complete catalog");
        assert_eq!(catalog.catalog_version, SUPPORTED_CATALOG_VERSION);
        assert_eq!(catalog.keys().len(), REQUIRED_KEYS.len());
    }

    /// Required copy is required. A page cannot render a headline that was
    /// never authored, so the build stops rather than shipping a gap.
    #[test]
    fn a_missing_required_key_is_refused() {
        let yaml = fixture().replace("  litigation.title:", "  litigation.other:");
        let err = SharedCatalog::parse(&yaml).expect_err("missing required key");
        assert!(
            err.contains("required key `litigation.title` is missing"),
            "{err}"
        );
    }

    #[test]
    fn an_unsupported_version_is_refused() {
        let yaml = fixture().replace("catalog_version: 1", "catalog_version: 7");
        let err = SharedCatalog::parse(&yaml).expect_err("unsupported version");
        assert!(err.contains("catalog version 7 is not supported"), "{err}");
    }

    #[test]
    fn an_unsupported_placeholder_is_refused() {
        let yaml = fixture().replace(
            "  litigation.cta: Words for litigation.cta.",
            "  litigation.cta: Write to {support_email}.",
        );
        let err = SharedCatalog::parse(&yaml).expect_err("unsupported placeholder");
        assert!(
            err.contains("unsupported placeholder `{support_email}`"),
            "{err}"
        );
    }

    #[test]
    fn the_two_brand_placeholders_are_supported() {
        let yaml = fixture().replace(
            "  litigation.cta: Words for litigation.cta.",
            "  litigation.cta: Write to {site_name} at {firm_email}.",
        );
        SharedCatalog::parse(&yaml).expect("brand placeholders");
    }

    #[test]
    fn a_value_that_cannot_be_substituted_is_refused() {
        let quoted = fixture().replace(
            "  litigation.cta: Words for litigation.cta.",
            "  litigation.cta: 'Say \"hello\" first.'",
        );
        let err = SharedCatalog::parse(&quoted).expect_err("quoted value");
        assert!(err.contains("contains a double quote"), "{err}");

        let empty = fixture().replace(
            "  litigation.cta: Words for litigation.cta.",
            "  litigation.cta: ''",
        );
        let err = SharedCatalog::parse(&empty).expect_err("empty value");
        assert!(err.contains("is empty"), "{err}");
    }

    /// A brand reads its own override and otherwise the shared default. It
    /// never reads a different brand's override — that would put one brand's
    /// wording on another brand's page.
    #[test]
    fn a_brand_falls_back_to_the_shared_default_not_to_another_brand() {
        let yaml = format!(
            "{}brands:\n  delete-your-data:\n    litigation.title: Ask them to delete it.\n",
            fixture()
        );
        let catalog = SharedCatalog::parse(&yaml).expect("catalog with an override");
        assert_eq!(
            catalog.lookup("delete-your-data", "litigation.title"),
            Some("Ask them to delete it.")
        );
        assert_eq!(
            catalog.lookup("neon", "litigation.title"),
            Some("Words for litigation.title.")
        );
        assert_eq!(
            catalog.lookup("lawyer-shook", "litigation.title"),
            Some("Words for litigation.title.")
        );
        assert_eq!(catalog.lookup("neon", "nobody.authored.this"), None);
    }

    #[test]
    fn a_brand_cannot_override_a_key_the_defaults_do_not_define() {
        let yaml = format!(
            "{}brands:\n  neon:\n    litigation.invented: Something new.\n",
            fixture()
        );
        let err = SharedCatalog::parse(&yaml).expect_err("unknown override");
        assert!(
            err.contains(
                "overrides `litigation.invented`, which the shared defaults do not define"
            ),
            "{err}"
        );
    }

    #[test]
    fn references_resolve_against_the_requested_brand() {
        let yaml = format!(
            "{}brands:\n  delete-your-data:\n    litigation.title: Ask them to delete it.\n",
            fixture()
        );
        let catalog = SharedCatalog::parse(&yaml).expect("catalog");
        let raw = "heading: \"{shared:litigation.title}\"\nlead: \"{site_name} helps\"\n";
        assert_eq!(
            resolve_references(raw, &catalog, "neon").expect("neon"),
            "heading: \"Words for litigation.title.\"\nlead: \"{site_name} helps\"\n"
        );
        assert_eq!(
            resolve_references(raw, &catalog, "delete-your-data").expect("dyd"),
            "heading: \"Ask them to delete it.\"\nlead: \"{site_name} helps\"\n"
        );
    }

    #[test]
    fn an_unknown_reference_is_an_error_rather_than_a_brace_on_the_page() {
        let catalog = SharedCatalog::parse(&fixture()).expect("catalog");
        let err = resolve_references("heading: \"{shared:home.nothing}\"\n", &catalog, "neon")
            .expect_err("unknown key");
        assert!(err.contains("`{shared:home.nothing}` names a key"), "{err}");
    }

    #[test]
    fn referenced_keys_reads_only_the_shared_references() {
        assert_eq!(
            referenced_keys("a {shared:one} b {site_name} c {shared:two}"),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn a_page_may_only_carry_brand_placeholders_and_shared_references() {
        check_page_placeholders(
            "home",
            "a: \"{site_name} {firm_email} {shared:home.need_prompt}\"",
        )
        .expect("supported placeholders");
        let err = check_page_placeholders("home", "a: \"{sitename}\"").expect_err("typo");
        assert!(
            err.contains("unsupported placeholder `{sitename}`"),
            "{err}"
        );
    }

    /// Exporting the same catalog twice must produce the same bytes: the
    /// consumer pins a digest, and a digest that moves on its own is not a pin.
    #[test]
    fn the_canonical_payload_is_deterministic_and_sorted() {
        let catalog = SharedCatalog::parse(&fixture()).expect("catalog");
        let source = Provenance {
            path: "neon/locales/en/shared.yaml".to_string(),
            repository: "neon-law-source-code/navigator".to_string(),
            revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
        };
        let once = catalog.canonical_payload(&source);
        assert_eq!(once, catalog.canonical_payload(&source));
        assert!(
            once.starts_with(r#"{"brands":{},"catalog_version":1,"entries":{"#),
            "{once}"
        );
        assert!(
            once.contains(r#""source":{"path":"neon/locales/en/shared.yaml","repository":"neon-law-source-code/navigator","revision":"0123456789abcdef0123456789abcdef01234567"}"#),
            "{once}"
        );
        // Entry keys appear in sorted order, which is what lets the
        // TypeScript consumer reproduce these bytes byte for byte.
        let entries = once.split_once(r#""entries":{"#).expect("entries").1;
        let mut sorted = REQUIRED_KEYS.to_vec();
        sorted.sort_unstable();
        let mut cursor = 0usize;
        for key in sorted {
            let needle = format!("\"{key}\":");
            let at = entries[cursor..].find(&needle).expect("key in payload");
            cursor += at + needle.len();
        }
    }
}
