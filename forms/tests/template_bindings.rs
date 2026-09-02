//! Every template `form:` binding must point at a vendored form.
//!
//! The `form:` frontmatter key selects the `AcroForm` rendering path in
//! the workflow walker; a binding to a form that was never vendored
//! would surface as a runtime error at lawyer-approve time. This guard
//! moves that failure to CI: walk `templates/**/*.md`, extract any
//! `form:` value, and assert the registry (and its field map) carries
//! it.

use std::path::{Path, PathBuf};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("forms sits under the workspace root")
        .join("templates")
}

fn walk_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read templates dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk_markdown(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Pull a top-level `form:` scalar out of the YAML frontmatter, if any.
fn form_binding(markdown: &str) -> Option<String> {
    let frontmatter = markdown.strip_prefix("---\n")?;
    let end = frontmatter.find("\n---\n")?;
    frontmatter[..end].lines().find_map(|line| {
        line.strip_prefix("form:")
            .map(|v| v.trim().trim_matches('"').to_string())
    })
}

#[test]
fn every_template_form_binding_points_at_a_vendored_form() {
    let mut files = Vec::new();
    walk_markdown(&templates_root(), &mut files);
    assert!(!files.is_empty(), "no templates found — wrong path?");

    let mut bound = 0;
    for path in &files {
        let markdown = std::fs::read_to_string(path).expect("read template");
        let Some(form_code) = form_binding(&markdown) else {
            continue;
        };
        bound += 1;
        assert!(
            forms::get(&form_code).expect("registry loads").is_some(),
            "{}: `form: {form_code}` is not in the vendored forms registry",
            path.display()
        );
        assert!(
            forms::manifest(&form_code).is_some(),
            "{}: `form: {form_code}` has no re-authored `.fields` manifest",
            path.display()
        );
    }
    assert!(
        bound >= 1,
        "expected at least one formation template to carry a form: binding"
    );
}
