//! Guard that `docs/validate.md` documents every rule code that actually ships.
//!
//! ENG-381 found 54 of 84 codes documented nowhere: the whole M-family, plus C003, E002, N112,
//! N116, S102, and Y001. This test enumerates every code the `rules` crate and `cli`'s own
//! seed-document pass emit and fails, naming each offender, when one has no entry in
//! `docs/validate.md` — so a new rule cannot merge without its row in the table, which is how the
//! original 54 accumulated (N120 and Y001 both shipped in the two days before this test existed,
//! neither with a doc change).

use std::collections::BTreeSet;
use std::fs;

/// `Y001` is the seed-document pass's rule code. It lives in `cli/src/main.rs`, not the `rules`
/// crate, because the pass it guards runs outside `ClassifiedRuleEngine` entirely.
const SEED_DOCUMENT_CODE: &str = "Y001";
const LOCALE_DOCUMENT_CODE: &str = "Y002";

fn all_shipped_codes() -> BTreeSet<&'static str> {
    let mut codes = BTreeSet::new();
    let rule_sets: Vec<Vec<Box<dyn rules::Rule>>> = vec![
        rules::engine::navigator_default_rules(),
        rules::engine::navigator_markdown_only_rules(),
        rules::engine::navigator_event_rules(),
        rules::engine::navigator_blog_rules(),
        rules::engine::navigator_workshop_rules(),
        rules::engine::navigator_github_rules(),
        rules::engine::navigator_dashboard_rules(),
    ];
    for rules in rule_sets {
        for rule in rules {
            codes.insert(rule.code());
        }
    }
    // N111 is a cross-file check (`rules::code_uniqueness_violations`), not a `Rule` impl, so no
    // rule-set above carries it.
    codes.insert("N111");
    codes.insert(SEED_DOCUMENT_CODE);
    codes.insert(LOCALE_DOCUMENT_CODE);
    codes
}

#[test]
fn every_shipped_code_has_an_entry_in_validate_docs() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let validate_md =
        fs::read_to_string(root.join("docs/validate.md")).expect("read docs/validate.md");

    let missing: Vec<&str> = all_shipped_codes()
        .into_iter()
        .filter(|code| !validate_md.contains(code))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/validate.md has no entry for: {}. Add a row to the matching family table.",
        missing.join(", ")
    );
}

/// Pin the exhaustive count so a rule addition or removal is a visible diff here, not a silent
/// change to how many codes the doc is supposed to cover.
#[test]
fn the_shipped_code_count_is_eighty_five() {
    assert_eq!(all_shipped_codes().len(), 85);
}
