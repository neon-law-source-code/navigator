//! Pure structural-validation tests — no database. These mirror what an
//! editor/LSP or a dry-run CLI would surface before any write.

use import::{canonical_url, parse, validate, Severity};

fn errors(payload: &import::Payload) -> Vec<String> {
    validate(payload)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.pointer, d.message))
        .collect()
}

#[test]
fn canonical_url_upgrades_strips_and_lowercases() {
    assert_eq!(
        canonical_url("http://Example.ORG/?utm_source=x#frag").unwrap(),
        "https://example.org"
    );
    assert_eq!(
        canonical_url("https://justice.example/").unwrap(),
        "https://justice.example"
    );
    assert_eq!(
        canonical_url("https://legalaidchicago.org/about").unwrap(),
        "https://legalaidchicago.org/about"
    );
}

#[test]
fn canonical_url_rejects_schemeless_and_non_http() {
    assert!(canonical_url("justice.example").is_err());
    assert!(canonical_url("ftp://files.example.org").is_err());
}

#[test]
fn valid_payload_has_no_errors() {
    let payload = parse(SAMPLE).expect("parse sample");
    assert!(
        errors(&payload).is_empty(),
        "unexpected: {:?}",
        errors(&payload)
    );
}

#[test]
fn duplicate_email_is_an_error() {
    let dup = SAMPLE.replace("mgordon@mylegalaid.org", "mmumgaard@mylegalaid.org");
    let payload = parse(&dup).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("duplicate email")));
}

#[test]
fn unknown_organization_reference_is_an_error() {
    let bad = SAMPLE.replace("\"organization\": \"ejp\"", "\"organization\": \"ghost\"");
    let payload = parse(&bad).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("organization `ghost`")));
}

#[test]
fn bad_jurisdiction_code_is_an_error() {
    let bad = SAMPLE.replace(
        "\"jurisdiction\": \"WA\"",
        "\"jurisdiction\": \"Washington\"",
    );
    let payload = parse(&bad).expect("parse");
    assert!(errors(&payload).iter().any(|e| e.contains("jurisdiction")));
}

#[test]
fn noncanonical_url_is_a_warning_not_an_error() {
    let raw = SAMPLE.replace("https://justice.example", "http://justice.example/?ref=x");
    let payload = parse(&raw).expect("parse");
    assert!(errors(&payload).is_empty());
    assert!(validate(&payload)
        .iter()
        .any(|d| d.severity == Severity::Warning && d.message.contains("canonicalized")));
}

#[test]
fn unsupported_version_is_an_error() {
    let mut payload = parse(SAMPLE).expect("parse sample");
    payload.version = 99;
    assert!(errors(&payload)
        .iter()
        .any(|e| e.starts_with("version:") && e.contains("unsupported")));
}

#[test]
fn empty_and_duplicate_organization_keys_are_errors() {
    let empty_key = SAMPLE.replace("\"key\": \"ejp\"", "\"key\": \"  \"");
    let payload = parse(&empty_key).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("organizations[0].key") && e.contains("must not be empty")));

    let dup = SAMPLE.replace("\"key\": \"mmla\"", "\"key\": \"ejp\"");
    let payload = parse(&dup).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("duplicate organization key")));
}

#[test]
fn empty_organization_name_type_and_person_fields_are_errors() {
    let no_name = SAMPLE.replace("\"name\": \"Example Justice Project\"", "\"name\": \" \"");
    let payload = parse(&no_name).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("organizations[0].name")));

    let no_type = SAMPLE.replace(
        "\"entity_type\": \"501(c)(3) Non-Profit\"",
        "\"entity_type\": \" \"",
    );
    let payload = parse(&no_type).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("organizations[0].entity_type")));

    let no_person_key = SAMPLE.replace("\"key\": \"ada-counsel\"", "\"key\": \"\"");
    let payload = parse(&no_person_key).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("people[0].key") && e.contains("must not be empty")));

    let dup_person = SAMPLE.replace("\"key\": \"milo-mumgaard\"", "\"key\": \"ada-counsel\"");
    let payload = parse(&dup_person).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("duplicate person key")));

    let no_person_name = SAMPLE.replace("\"name\": \"Ada Counsel\"", "\"name\": \" \"");
    let payload = parse(&no_person_name).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("people[0].name")));

    let bad_email = SAMPLE.replace("acounsel@justice.example", "not-an-email");
    let payload = parse(&bad_email).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("people[0].email")));

    let empty_role = SAMPLE.replace(
        "\"title\": \"Executive Director\"",
        "\"entity_role\": \" \", \"title\": \"Executive Director\"",
    );
    let payload = parse(&empty_role).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("people[0].entity_role")));

    let empty_org = SAMPLE.replace("\"organization\": \"ejp\"", "\"organization\": \" \"");
    let payload = parse(&empty_org).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("people[0].organization") && e.contains("must reference")));
}

#[test]
fn canonical_url_keeps_an_explicit_port_and_rejects_a_hostless_url() {
    assert_eq!(
        canonical_url("https://justice.example:8443/about/").unwrap(),
        "https://justice.example:8443/about"
    );
    assert!(canonical_url("https://").is_err());
}

#[test]
fn a_malformed_organization_url_is_an_error() {
    let bad = SAMPLE.replace(
        "\"url\": \"https://justice.example\"",
        "\"url\": \"mailto:ops@example.com\"",
    );
    let payload = parse(&bad).expect("parse");
    assert!(errors(&payload)
        .iter()
        .any(|e| e.contains("organizations[0].url")));
}

/// The six contacts from the original outreach list, four organizations.
const SAMPLE: &str = r#"{
  "version": 1,
  "source": "partner-outreach-2026-06",
  "organizations": [
    { "key": "ejp", "name": "Example Justice Project", "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "WA", "phone": "206-555-0142", "url": "https://justice.example" },
    { "key": "mmla", "name": "Mid-Minnesota Legal Aid", "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "MN", "phone": "612-332-1441", "url": "https://mylegalaid.org" },
    { "key": "lac", "name": "Legal Aid Chicago", "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "IL", "phone": "312-341-1070", "url": "https://legalaidchicago.org" },
    { "key": "lsnyc", "name": "Legal Services NYC", "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "NY", "phone": "646-442-3600", "url": "https://lsnyc.org" }
  ],
  "people": [
    { "key": "ada-counsel", "name": "Ada Counsel", "email": "acounsel@justice.example", "title": "Executive Director", "phone": "206-555-0142", "organization": "ejp" },
    { "key": "milo-mumgaard", "name": "Milo Mumgaard", "email": "mmumgaard@mylegalaid.org", "title": "Executive Director", "phone": "612-332-1441", "organization": "mmla" },
    { "key": "marv-gordon", "name": "Marv Gordon", "email": "mgordon@mylegalaid.org", "title": "IT Director", "phone": "612-332-1441", "organization": "mmla" },
    { "key": "katherine-shank", "name": "Katherine W. Shank", "email": "kshank@legalaidchicago.org", "title": "CEO and Executive Director", "phone": "312-341-1070", "organization": "lac" },
    { "key": "shervon-small", "name": "Shervon M. Small", "email": "ssmall@lsnyc.org", "title": "Executive Director", "phone": "646-442-3600", "organization": "lsnyc" },
    { "key": "dilip-kulkarni", "name": "Dilip Kulkarni", "email": "dkulkarni@lsnyc.org", "title": "Chief Information Officer", "phone": "646-442-3600", "organization": "lsnyc" }
  ]
}"#;
