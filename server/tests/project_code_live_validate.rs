//! Guard: the matter-open form's live code preview describes the code that
//! `store::projects::open_matter` actually stores.
//!
//! `/app/projects/new` loads `server/public/js/project-code-live-validate.js`
//! to give shape feedback as the lawyer types. The script also previews the
//! resulting code, and that preview is copy a lawyer reads before committing
//! a value they can never change — so it has to agree with the contract in
//! `docs/glossary.md#project`: the code is stored exactly as supplied, with
//! nothing generated or appended. The server-rendered help text is covered by
//! `webapp::project_new::tests`; that test renders HTML only, so the
//! client-side script is the one surface that can carry retired wording
//! unnoticed. This test reads the asset itself.

use std::path::PathBuf;

fn live_validate_js() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("public")
        .join("js")
        .join("project-code-live-validate.js");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The preview must not describe a generated suffix: `open_matter` stores the
/// supplied code verbatim (`store/tests/project_code_storage.rs`), so a
/// preview that promises `<code>-xxxxxxxx` misstates the code the lawyer is
/// about to commit to.
#[test]
fn the_live_preview_promises_no_generated_suffix() {
    let js = live_validate_js();
    for retired in [
        "a1b2c3d4",
        "EXAMPLE_SUFFIX",
        "generated suffix",
        "generates the real suffix",
        "Suffix note",
        "appends a generated",
        "code_from_name",
    ] {
        assert!(
            !js.contains(retired),
            "project-code-live-validate.js still describes a generated suffix (`{retired}`): \
             the stored code is the supplied code, verbatim",
        );
    }
}

/// The preview names the exact code and carries the same consequence the
/// field's help text does: chosen once, never changed.
#[test]
fn the_live_preview_names_the_exact_code_and_that_it_is_immutable() {
    let js = live_validate_js();
    assert!(
        js.contains("Your matter's code will be `\" + value + \"`"),
        "the preview does not name the supplied code as the matter's code: {js}"
    );
    assert!(
        js.contains("chosen once and never changed"),
        "the preview does not say the code is immutable: {js}"
    );
}
