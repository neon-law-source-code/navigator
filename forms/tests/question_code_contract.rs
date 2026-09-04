//! The field-name = question-code contract, verified as a guard test.
//!
//! Issue #231's core claim is that a government form's fill map is not
//! trusted, it is *checked*: every field a packet fills must resolve to a
//! real question the questionnaire actually asks, and every question must
//! be one of the canonical seeded types (`store/seeds/Question.yaml`, via
//! `rules::canonical_question_codes()` — the same source of truth the
//! notation-template linter uses post-#233).
//!
//! Today the three NV blanks still carry a `<code>.fields.toml` that maps
//! their hostile `OmniForm` `/T` names onto question references; the human
//! re-authoring that makes the PDF `/T` names *be* question codes is a
//! sequenced follow-on (see `docs/gov-forms.md`). This guard
//! pins the layer that exists today: it fails CI if a `.fields.toml`
//! references a question the notation never declares, or if a notation
//! declares a state whose type is not canonical. Either way a mis-map
//! breaks loudly here, before it can mis-fill a filing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;

/// The notation frontmatter fields this guard reads.
#[derive(Debug, Deserialize)]
struct Notation {
    questionnaire: std::collections::BTreeMap<String, serde_yaml::Value>,
}

/// Read a vendored form's sibling notation `.md` and return its declared
/// questionnaire state names (excluding the `BEGIN` / `END` sentinels).
fn questionnaire_states(object_path: &str) -> Vec<String> {
    let md_rel = object_path.replace(".pdf", ".md");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("templates")
        .join("notations")
        .join(&md_rel);
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read notation {}: {e}", path.display()));
    let fm = frontmatter(&contents)
        .unwrap_or_else(|| panic!("{}: no `---` frontmatter block", path.display()));
    let notation: Notation = serde_yaml::from_str(fm)
        .unwrap_or_else(|e| panic!("{}: parse frontmatter: {e}", path.display()));
    notation
        .questionnaire
        .into_keys()
        .filter(|s| s != "BEGIN" && s != "END")
        .collect()
}

/// The YAML frontmatter block between the leading `---` and its closer.
fn frontmatter(contents: &str) -> Option<&str> {
    let rest = contents.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// The `<type>` a questionnaire state is built from — the segment before
/// `__` (`entity__company` → `entity`, `people__managing_members` →
/// `people`). A state with no `__` is its own type.
fn state_type(state: &str) -> &str {
    state.split_once("__").map_or(state, |(t, _)| t)
}

#[test]
fn every_notation_state_is_a_canonical_question_type() {
    let canonical: BTreeSet<String> = rules::canonical_question_codes().into_iter().collect();
    for form in forms::registry().expect("registry loads") {
        for state in questionnaire_states(form.object_path) {
            let ty = state_type(&state);
            assert!(
                canonical.contains(ty),
                "{}: questionnaire state `{state}` has type `{ty}`, which is not a \
                 canonical question code in store/seeds/Question.yaml",
                form.code
            );
        }
    }
}

/// The re-authored contract: every form carries a `.fields` manifest of
/// its blank's actual `/T` names, and every one of them either *is* a
/// declared questionnaire state path or sits in the reserved `unmapped__`
/// namespace — the assertion filed onto the names of the bytes we ship
/// (#256 item 1).
#[test]
fn every_reauthored_field_name_is_a_declared_state_path_or_unmapped() {
    let mut reauthored = 0usize;
    for form in forms::registry().expect("registry loads") {
        reauthored += 1;
        let manifest = forms::manifest(form.code)
            .unwrap_or_else(|| panic!("{}: form has no .fields manifest", form.code));
        let states = questionnaire_states(form.object_path);
        let states: BTreeSet<&str> = states.iter().map(String::as_str).collect();
        for name in manifest {
            if name.starts_with(forms::UNMAPPED_PREFIX) {
                continue;
            }
            let head = name.split('.').next().unwrap_or(name);
            assert!(
                states.contains(head),
                "{}: re-authored field `{name}` does not carry a declared \
                 questionnaire state — the blank was re-authored against a \
                 different notation, or the notation drifted",
                form.code
            );
        }
    }
    assert!(reauthored >= 1, "nv__llc_formation is re-authored");
}

/// The N-400 second slice (#311): the applicant's current legal name is
/// mapped to the structured client-name parts the `persons` record owns
/// (`person__client.family/given/middle` — the N-400 splits the name into
/// `P2_Line1_FamilyName/GivenName/MiddleName`, which one display string
/// can't fill faithfully), while the absences trip table (Part 8) and the
/// itemized good-moral-character questions (Part 9) stay `unmapped__` on
/// purpose — each maps to a *shape* the current intake does not yet carry,
/// so a lawyer completes them on the form until a finer typed slice lands.
#[test]
fn n400_maps_structured_legal_name_and_defers_absences_and_moral_character() {
    let manifest: BTreeSet<&str> = forms::manifest("us__naturalization")
        .expect("us__naturalization is re-authored")
        .into_iter()
        .collect();

    for part in [
        "person__client.family",
        "person__client.given",
        "person__client.middle",
    ] {
        assert!(
            manifest.contains(part),
            "N-400 legal name must map to `{part}`: the P2_Line1 name fields are \
             re-authored onto the structured client-name parts (#311)"
        );
    }

    // No P2_Line1 name field may survive in the unmapped namespace — the
    // legal-name mapping must cover both occurrences (Part 2 and the
    // Part 11 certification block).
    let leftover_name: Vec<&str> = manifest
        .iter()
        .filter(|n| n.starts_with(forms::UNMAPPED_PREFIX) && n.contains("P2_Line1_"))
        .copied()
        .collect();
    assert!(
        leftover_name.is_empty(),
        "these legal-name fields are still unmapped — the #311 mapping missed them: {leftover_name:?}"
    );

    // Absences (Part 8 trip table) and moral character (Part 9) remain
    // deferred to lawyer completion this slice.
    for (part, what) in [
        ("P8_Line", "Part 8 absences trip table"),
        ("P9_Line", "Part 9 moral-character items"),
    ] {
        assert!(
            manifest
                .iter()
                .any(|n| n.starts_with(forms::UNMAPPED_PREFIX) && n.contains(part)),
            "{what} must remain `unmapped__` (deferred to lawyer completion, #311)"
        );
    }
}
