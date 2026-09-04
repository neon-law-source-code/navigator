//! Provenance guard for vendored government forms.
//!
//! The repository path is the public bucket path: each blank lives in
//! the assets bucket at `object_path`, and the repo keeps the diffable
//! text beside the markdown template — the `.fields.toml` map and the
//! `.sha256` pin the fill path verifies the pulled bytes against.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("forms crate sits one level under the workspace root")
        .to_path_buf()
}

#[test]
fn vendored_forms_pin_their_bucket_objects() {
    let forms = forms::registry().expect("registry loads");
    assert!(!forms.is_empty(), "registry lists no forms");

    for form in &forms {
        assert!(
            form.object_path.starts_with("forms/united_states/"),
            "{}: object path must be bucket-relative under forms/united_states",
            form.code
        );
        assert!(
            form.object_path.ends_with(&format!("{}.pdf", form.code)),
            "{}: object path must end with the form code stem",
            form.code
        );

        // The blank's bytes are NOT in the tree — only its pin is. The
        // pin file sits beside the markdown template at the
        // bucket-shaped repo path, and the registry compiles it in.
        let templates_path = workspace_root()
            .join("templates")
            .join("notations")
            .join(form.object_path);
        let pin_path = templates_path.with_extension("sha256");
        let pin_on_disk = std::fs::read_to_string(&pin_path).unwrap_or_else(|e| {
            panic!(
                "{}: cannot read pin file {}: {e}",
                form.code,
                pin_path.display()
            )
        });
        assert_eq!(
            pin_on_disk.trim(),
            form.pinned_sha256(),
            "{}: compiled-in pin diverges from {} — rebuild after `forms sync`",
            form.code,
            pin_path.display()
        );
        let pin = form.pinned_sha256();
        assert_eq!(pin.len(), 64, "{}: pin is not a sha-256 digest", form.code);
        assert!(
            pin.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{}: pin is not lowercase hex",
            form.code
        );

        assert!(
            form.origin_url.starts_with("https://") && form.origin_url.contains(".gov"),
            "{}: origin_url must be an HTTPS government URL, got `{}`",
            form.code,
            form.origin_url
        );
    }
}
