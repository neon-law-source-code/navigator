//! The `/notations` kind catalog (LAW-53): every `kind:` value `S103`
//! accepts, projected straight from [`rules::kind::Kind`] — the same enum
//! `S103` validates a declared `kind:` against — so the docs page and the
//! template gallery's kind/category facet cannot drift from the gate.

use rules::kind::{Kind, Lane};

/// One kind-specific structural lint rule bound to a [`KindEntry`], as a
/// `(code, one-sentence requirement)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindRule {
    pub code: String,
    pub note: String,
}

/// One `kind:` value's catalog entry: its definition, its category, and the
/// structural rules bound to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindEntry {
    /// The frontmatter value (`Kind::as_str`), e.g. `"agreement"`.
    pub kind: String,
    /// A title-cased display label, e.g. `"Agreement"` or
    /// `"Review Queue Workbench"`.
    pub label: String,
    /// The category's human-readable label, e.g. `"Instrument"`.
    pub category: String,
    /// The category's URL-safe slug, e.g. `"instrument"`.
    pub category_slug: String,
    /// A one-sentence definition of the kind ([`Kind::describe`]).
    pub definition: String,
    /// The kind-specific structural rules bound to it. Empty for a kind
    /// bound to no rule beyond the universal Markdown rules every file gets
    /// regardless of kind.
    pub rules: Vec<KindRule>,
}

/// Every kind `S103` accepts, in [`Kind::ALL`] order — the same order (and
/// the same 24-value set) as [`rules::kind::VALID`], proven by
/// `entries_match_the_s103_accepted_vocabulary` below.
#[must_use]
pub fn entries() -> Vec<KindEntry> {
    Kind::ALL
        .iter()
        .filter(|k| k.valid_for(Lane::Template))
        .map(|k| KindEntry {
            kind: k.as_str().to_string(),
            label: title_case(k.as_str()),
            category: k.category().label().to_string(),
            category_slug: k.category().slug().to_string(),
            definition: k.describe().to_string(),
            rules: k
                .structural_rules()
                .into_iter()
                .map(|(code, note)| KindRule {
                    code: code.to_string(),
                    note,
                })
                .collect(),
        })
        .collect()
}

/// The declared `kind:` value in `contents`' frontmatter, or `None` when
/// absent or unrecognized. A thin passthrough of
/// [`rules::kind::declared`], so a caller held to the brand-crate dependency
/// rule (`cli/tests/brand_crate_dependencies.rs`, which admits `views` but
/// not `rules`) can derive a bundled template's real kind — the same
/// classification `S103` and the LSP completion use — without adding
/// `rules` to its own `Cargo.toml`.
#[must_use]
pub fn declared_kind(contents: &str) -> Option<String> {
    rules::kind::declared(contents).map(|k| k.as_str().to_string())
}

/// `"review_queue_workbench"` -> `"Review Queue Workbench"`.
fn title_case(value: &str) -> String {
    value
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{declared_kind, entries, title_case};

    #[test]
    fn entries_match_the_s103_accepted_vocabulary() {
        let kinds: Vec<String> = entries().into_iter().map(|e| e.kind).collect();
        let expected: Vec<String> = rules::kind::VALID.iter().map(ToString::to_string).collect();
        assert_eq!(kinds, expected);
    }

    #[test]
    fn title_case_splits_and_capitalizes_each_word() {
        assert_eq!(title_case("agreement"), "Agreement");
        assert_eq!(
            title_case("review_queue_workbench"),
            "Review Queue Workbench"
        );
    }

    #[test]
    fn agreement_is_an_instrument_bound_to_n123() {
        let agreement = entries()
            .into_iter()
            .find(|e| e.kind == "agreement")
            .expect("agreement is in the catalog");
        assert_eq!(agreement.label, "Agreement");
        assert_eq!(agreement.category, "Instrument");
        assert_eq!(agreement.category_slug, "instrument");
        assert!(agreement.rules.iter().any(|r| r.code == "N123"));
    }

    #[test]
    fn letter_carries_no_kind_specific_rule() {
        let letter = entries()
            .into_iter()
            .find(|e| e.kind == "letter")
            .expect("letter is in the catalog");
        assert_eq!(letter.category_slug, "correspondence");
        assert!(letter.rules.is_empty());
    }

    #[test]
    fn a_matter_dashboard_kind_is_labeled_and_grouped() {
        let workbench = entries()
            .into_iter()
            .find(|e| e.kind == "review_queue_workbench")
            .expect("review_queue_workbench is in the catalog");
        assert_eq!(workbench.label, "Review Queue Workbench");
        assert_eq!(workbench.category_slug, "matter-dashboard");
    }

    #[test]
    fn declared_kind_reads_frontmatter_through_the_rules_classifier() {
        assert_eq!(
            declared_kind("---\ntitle: T\nkind: onboarding\n---\n"),
            Some("onboarding".to_string())
        );
        assert_eq!(declared_kind("---\ntitle: T\nkind: bogus\n---\n"), None);
        assert_eq!(declared_kind("no frontmatter"), None);
    }
}
