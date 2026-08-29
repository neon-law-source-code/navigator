//! Guard: every Markdown-authored Neon Law template compiles through the
//! real `Markdown → Typst → PDF` pipeline.
//!
//! Notation bodies are authored in Markdown and reach Typst through
//! [`pdf::to_typst`], which escapes the prose sigils Typst treats as syntax
//! — `#`, `$`, `*`, and, the one that bit us, a bare `@`. Before that
//! conversion was wired into the retainer render path, `support@neonlaw.com`
//! in every product retainer was parsed as a Typst label reference
//! (`@neonlaw.com`) and the matter 500'd the moment lawyer approved. This
//! test compiles each template body so a Typst-hostile character can never
//! merge silently again.
//!
//! The bodies here carry no `{{#for}}` loops or tables, so they compile with
//! placeholders left verbatim (Typst renders an unfilled `{{token}}` as
//! literal text). If a future template needs answers to compile, render it
//! through the real answer context instead of adding it here.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// The render body is everything after the leading `---` YAML frontmatter
/// block — the same bytes `store::templates::body` persists and the render
/// path compiles.
fn body_after_frontmatter(contents: &str) -> &str {
    let rest = contents.strip_prefix("---\n").unwrap_or(contents);
    match rest.find("\n---\n") {
        Some(i) => &rest[i + "\n---\n".len()..],
        None => contents,
    }
}

fn neon_law_templates() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/neon_law");
    WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect()
}

#[test]
fn every_neon_law_template_compiles_through_typst() {
    let templates = neon_law_templates();
    assert!(
        templates.len() >= 2,
        "expected the sample onboarding and offboarding letters, found {}",
        templates.len()
    );
    for path in templates {
        let contents = std::fs::read_to_string(&path).unwrap();
        let body = body_after_frontmatter(&contents);
        let typst = pdf::to_typst(body);
        pdf::render(&typst).unwrap_or_else(|e| {
            panic!(
                "template {} must compile through the Markdown → Typst → PDF pipeline, but failed: {e}",
                path.display()
            )
        });
    }
}
