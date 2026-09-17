//! The catalog's acceptance test: scaffolding every matter dashboard kind
//! produces a composition that validates (#896).
//!
//! This runs the real classified engine — the same selection
//! `navigator project gate` and `navigator-lsp` consume — rather than calling
//! the D-family rules directly. That is the point: the scaffold #895 hands
//! an attorney and the validator that then judges their file must agree on
//! the first keystroke, or the authoring loop starts red.

use rules::dashboard::{catalog, scaffold, skeleton, Section, UNIVERSAL};
use rules::{lint_source_classified, DocumentKind, Kind, SourceFile};
use std::path::PathBuf;

fn dashboard_kinds() -> Vec<Kind> {
    Kind::ALL
        .iter()
        .copied()
        .filter(|k| k.is_dashboard())
        .collect()
}

fn source(kind: Kind, contents: String) -> SourceFile {
    SourceFile {
        path: PathBuf::from(format!("{}.md", kind.as_str())),
        contents,
    }
}

#[test]
fn scaffolding_every_kind_produces_a_composition_that_validates() {
    assert!(!dashboard_kinds().is_empty(), "the catalog is not empty");
    for kind in dashboard_kinds() {
        let composition = scaffold(kind, "Homer v. Flanders").expect("a dashboard kind scaffolds");
        let file = source(kind, composition);
        let violations = lint_source_classified(&file);
        assert!(
            violations.is_empty(),
            "scaffolding `{}` produced a file that does not validate: {:#?}",
            kind.as_str(),
            violations,
        );
    }
}

#[test]
fn a_scaffolded_composition_classifies_as_a_matter_dashboard() {
    // Classification is what selects the D-family rules at all. If a kind
    // fell through to another family the test above would pass vacuously.
    for kind in dashboard_kinds() {
        let file = source(kind, scaffold(kind, "Homer v. Flanders").unwrap());
        assert_eq!(
            rules::classify_source(&file),
            DocumentKind::MatterDashboard,
            "{} classified wrong",
            kind.as_str(),
        );
    }
}

#[test]
fn the_dashboard_rule_set_actually_carries_the_d_family() {
    let file = source(
        Kind::AuthorityLibrary,
        scaffold(Kind::AuthorityLibrary, "Homer v. Flanders").unwrap(),
    );
    let codes: Vec<&str> = rules::navigator_classified_rules(&file)
        .iter()
        .map(|r| r.code())
        .collect();
    for code in ["D001", "D002", "D003", "D004"] {
        assert!(codes.contains(&code), "{code} is not in the rule set");
    }
    // A dashboard declares no questionnaire, so the legal notation rules
    // would report nothing but noise on it.
    assert!(
        !codes.iter().any(|c| c.starts_with('N')),
        "the notation family leaked into the dashboard rule set: {codes:?}",
    );
}

#[test]
fn every_scaffold_lists_only_sections_in_its_own_catalog() {
    for kind in dashboard_kinds() {
        let composition = scaffold(kind, "Homer v. Flanders").unwrap();
        let allowed = catalog(kind);
        for (lens, names) in rules::dashboard::declared_lenses(&composition).unwrap() {
            for name in names {
                let section = Section::parse(&name)
                    .unwrap_or_else(|| panic!("{} scaffolded unknown `{name}`", kind.as_str()));
                assert!(
                    allowed.contains(&section),
                    "{} lens `{lens}` scaffolded `{name}`, which is out of its catalog",
                    kind.as_str(),
                );
            }
        }
    }
}

#[test]
fn removing_a_universal_section_from_the_scaffold_fails_validation() {
    // The guard on the test above: prove the validator would have caught a
    // defect, so a green scaffold means something.
    let kind = Kind::AuthorityLibrary;
    let broken = scaffold(kind, "Homer v. Flanders")
        .unwrap()
        .replace(", boundary_note", "");
    let violations = lint_source_classified(&source(kind, broken));
    assert!(
        violations.iter().any(|v| v.code == "D003"),
        "dropping the boundary note must fail D003; got {violations:#?}",
    );
}

#[test]
fn an_out_of_catalog_section_fails_validation_end_to_end() {
    let kind = Kind::DeliverablePackage;
    let broken = scaffold(kind, "Homer v. Flanders")
        .unwrap()
        .replace("package_manifest,", "package_manifest, discovery_pairs,");
    let violations = lint_source_classified(&source(kind, broken));
    assert!(
        violations.iter().any(|v| v.code == "D002"),
        "a section from another kind must fail D002; got {violations:#?}",
    );
}

#[test]
fn the_universal_sections_reach_every_scaffolded_lens() {
    for kind in dashboard_kinds() {
        let composition = scaffold(kind, "Homer v. Flanders").unwrap();
        for (lens, names) in rules::dashboard::declared_lenses(&composition).unwrap() {
            for universal in UNIVERSAL {
                assert!(
                    names.iter().any(|n| n == universal.as_str()),
                    "{} lens `{lens}` scaffolded without `{}`",
                    kind.as_str(),
                    universal.as_str(),
                );
            }
        }
    }
}

#[test]
fn the_scaffold_writes_a_stub_for_every_section_it_declares_on_lawyer() {
    // The body is what the attorney fills in. A declared section with no
    // heading to write under is a scaffold that sent them to a blank file.
    for kind in dashboard_kinds() {
        let composition = scaffold(kind, "Homer v. Flanders").unwrap();
        let spine = skeleton(kind).unwrap().required;
        for section in spine.iter().chain(UNIVERSAL.iter()) {
            let heading = format!("## {}", section.heading());
            assert!(
                composition.contains(&heading),
                "{} scaffolded no `{heading}` stub",
                kind.as_str(),
            );
        }
    }
}
