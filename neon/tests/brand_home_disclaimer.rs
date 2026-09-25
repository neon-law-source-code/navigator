use std::{fs, path::PathBuf};

use views::brand::{BrandKey, DEFAULT_BRANDING};

const DISCLAIMER_SENTENCE: &str =
    "Nothing here is legal advice without a signed retainer for an active project.";

fn home_lead(key: BrandKey) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("locales")
        .join("en")
        .join(key.as_str())
        .join("home.yaml");
    let yaml = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let document: serde_yaml::Value = serde_yaml::from_str(&yaml)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    document["lead"]
        .as_str()
        .unwrap_or_else(|| panic!("{} has no string lead", path.display()))
        .to_string()
}

#[test]
fn affected_brand_home_leads_keep_the_disclaimer_in_the_footer() {
    for key in BrandKey::ALL
        .iter()
        .copied()
        .filter(|key| matches!(key, BrandKey::Misericordia | BrandKey::Abhaya))
    {
        let branding = key.resolve_branding(&DEFAULT_BRANDING);
        assert!(
            branding.firm_disclaimer.contains(DISCLAIMER_SENTENCE),
            "{} footer lost the shared disclaimer: {}",
            key.as_str(),
            branding.firm_disclaimer
        );
        let lead = home_lead(key);
        assert!(
            !lead.contains(DISCLAIMER_SENTENCE),
            "{} home lead carries the footer disclaimer: {lead}",
            key.as_str()
        );
    }
}
