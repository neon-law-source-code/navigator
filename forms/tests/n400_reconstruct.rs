//! Provenance harness for the re-authored USCIS N-400 (#311).
//!
//! #308 vendored the re-authored `us__naturalization` blank and its pin
//! (`24f2b77…`) but never committed the `.fields.toml` that drove the
//! transform, so the re-authored bytes could not be regenerated. This
//! `#[ignore]` harness closes that gap: it re-applies the recorded
//! mapping (`RECONSTRUCTED_MAP` below) to the original USCIS source
//! blank and asserts the output is byte-identical to the shipped pin.
//! Hitting that sha proves the reconstruction matches #308 exactly —
//! radio letter→choice mapping and all — so the second slice (#311) can
//! add the legal-name rules as a controlled delta.
//!
//! Run with the source blank (USCIS 01/20/25 edition,
//! sha `8b33868…`):
//!
//! ```text
//! N400_SOURCE_PDF=/tmp/navigator-n400/n-400.pdf \
//!   cargo test -p forms --test n400_reconstruct -- --ignored --nocapture
//! ```

use forms::FieldMap;

/// The recorded 8-state mapping #308 applied, recovered from the shipped
/// manifest by elimination (every unmapped radio member / contact field
/// is named `unmapped__…` in the manifest; what is absent was mapped).
/// The radio `on_state` letters are the source export values read off
/// the blank (`field-info.tsv`); `checked_when` is the questionnaire
/// choice each maps to.
const RECONSTRUCTED_MAP: &str = r#"
form_code = "us__naturalization"

# --- structured legal name (#311): both the Part 2 occurrence and the
#     Part 11 certification-block reprint merge onto the client-name parts
#     the persons record owns. `person__client` is already a declared
#     state, so no questionnaire change is needed — only a richer answer. ---
[[field]]
name = "form1[0].#subform[0].P2_Line1_FamilyName[0]"
question = "person__client.family"
[[field]]
name = "form1[0].#subform[12].P2_Line1_FamilyName[1]"
question = "person__client.family"
[[field]]
name = "form1[0].#subform[0].P2_Line1_GivenName[0]"
question = "person__client.given"
[[field]]
name = "form1[0].#subform[12].P2_Line1_GivenName[1]"
question = "person__client.given"
[[field]]
name = "form1[0].#subform[0].P2_Line1_MiddleName[0]"
question = "person__client.middle"
[[field]]
name = "form1[0].#subform[12].P2_Line1_MiddleName[1]"
question = "person__client.middle"

# --- scalar text renames ---
[[field]]
name = "form1[0].#subform[1].P2_Line8_DateOfBirth[0]"
question = "custom_datetime__date_of_birth"

[[field]]
name = "form1[0].#subform[1].P2_Line9_DateBecamePermanentResident[0]"
question = "custom_datetime__lpr_since"

[[field]]
name = "form1[0].#subform[1].P2_Line10_CountryOfBirth[0]"
question = "country__of_birth.name"

[[field]]
name = "form1[0].#subform[1].P2_Line11_CountryOfNationality[0]"
question = "country__of_citizenship.name"

[[field]]
name = "form1[0].#subform[10].P12_Line5_Email[0]"
question = "person__client.email"

[[field]]
name = "form1[0].#subform[10].P12_Line3_Telephone[0]"
question = "custom_phone__daytime_phone"

# --- Part 1 eligibility radio (members in shipped /Kids order) ---
[[field]]
name = "form1[0].#subform[0].Part1_Eligibility[2]"
question = "custom_single_choice__eligibility_basis"
checked_when = "five_year"
on_state = "A"

[[field]]
name = "form1[0].#subform[0].Part1_Eligibility[1]"
question = "custom_single_choice__eligibility_basis"
checked_when = "three_year_marriage"
on_state = "B"

[[field]]
name = "form1[0].#subform[0].Part1_Eligibility[0]"
question = "custom_single_choice__eligibility_basis"
checked_when = "military"
on_state = "C"

# --- Part 10 marital-status radio (members in shipped /Kids order) ---
[[field]]
name = "form1[0].#subform[3].P10_Line1_MaritalStatus[1]"
question = "custom_single_choice__marital_status"
checked_when = "single"
on_state = "S"

[[field]]
name = "form1[0].#subform[3].P10_Line1_MaritalStatus[3]"
question = "custom_single_choice__marital_status"
checked_when = "married"
on_state = "M"

[[field]]
name = "form1[0].#subform[3].P10_Line1_MaritalStatus[0]"
question = "custom_single_choice__marital_status"
checked_when = "divorced"
on_state = "D"

[[field]]
name = "form1[0].#subform[3].P10_Line1_MaritalStatus[2]"
question = "custom_single_choice__marital_status"
checked_when = "widowed"
on_state = "W"
"#;

/// The questionnaire states `us__naturalization.md` declares (the
/// resolution target for every map question reference).
fn states() -> Vec<String> {
    [
        "person__client",
        "custom_datetime__date_of_birth",
        "country__of_birth",
        "country__of_citizenship",
        "custom_datetime__lpr_since",
        "custom_phone__daytime_phone",
        "custom_single_choice__eligibility_basis",
        "custom_single_choice__marital_status",
        "custom_text__time_outside_us",
        "custom_yes_no__good_moral_character",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// Re-author `source` bytes with `map` — the same transform
/// `cli::forms_sync::reauthor_bytes` runs, replicated here through the
/// public `pdf` + `forms` API so this harness needs no CLI internals.
fn reauthor(map: &FieldMap, source: &[u8]) -> Vec<u8> {
    let stripped = pdf::strip_static_xfa(source).expect("strip static XFA");
    let names = pdf::field_names(&stripped).expect("field names");
    let plan = forms::reauthor::plan(map, &names, &states()).expect("plan");
    let spec = pdf::ReauthorSpec {
        renames: plan.renames,
        radios: plan
            .radios
            .into_iter()
            .map(|(name, members)| pdf::RadioMergeSpec {
                name,
                members: members
                    .into_iter()
                    .map(|m| pdf::RadioMergeMember {
                        field: m.field,
                        source_export: m.source_export,
                        final_export: m.final_export,
                    })
                    .collect(),
            })
            .collect(),
        literals: plan.literals,
    };
    pdf::reauthor(&stripped, &spec).expect("reauthor")
}

/// The committed manifest of the re-authored blank — the field-name
/// topology this map must reproduce.
const COMMITTED_MANIFEST: &str = include_str!(
    "../../templates/notations/forms/united_states/federal/uscis/us__naturalization.fields"
);

#[test]
#[ignore = "requires N400_SOURCE_PDF=/path/to/original/n-400.pdf (USCIS 01/20/25, sha 8b33868…)"]
fn reconstructed_map_reproduces_the_committed_manifest() {
    let path = std::env::var("N400_SOURCE_PDF").expect("set N400_SOURCE_PDF");
    let source = std::fs::read(&path).expect("read source N-400");
    let map: FieldMap = toml::from_str(RECONSTRUCTED_MAP).expect("parse reconstructed map");

    // Determinism: the transform is byte-stable, so the pin it mints is
    // reproducible from the source — the guarantee #308 asserted but
    // could not commit without the map.
    let reauthored = reauthor(&map, &source);
    assert_eq!(
        forms::sha256_hex(&reauthored),
        forms::sha256_hex(&reauthor(&map, &source)),
        "re-author is not deterministic"
    );

    // Topology: the map regenerates the committed `.fields` names exactly
    // — no field added, dropped, renamed, or wrongly left `unmapped__`.
    // (The absolute pin is a serialization-level artifact of the lopdf
    // version at generation time; the field-name set is the contract the
    // fill path actually reads, and re-vendoring re-pins the bytes.)
    let mut manifest = pdf::field_names(&reauthored).expect("manifest names");
    manifest.sort();
    std::fs::write("/tmp/navigator-n400/reconstructed.pdf", &reauthored)
        .expect("stage reconstructed pdf for visual QA");
    std::fs::write(
        "/tmp/navigator-n400/reconstructed.fields",
        manifest.join("\n") + "\n",
    )
    .expect("stage reconstructed manifest");
    eprintln!("re-authored pin = {}", forms::sha256_hex(&reauthored));
    assert_eq!(
        manifest.join("\n") + "\n",
        COMMITTED_MANIFEST,
        "reconstructed re-author diverges from the committed manifest — the recovered \
         map no longer matches the vendored blank"
    );
}
