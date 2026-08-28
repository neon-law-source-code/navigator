//! `navigator template narrate <file> --out <stage.html>` — write a
//! self-contained Harvard-outline narration stage.
//!
//! The file is parsed as Markdown (YAML frontmatter optional). Depth-1
//! headings numbered `I.` / `II.` (contracts) or `1.` / `2.` (motion
//! practice), plus `> **A.**` block-quote subsections, become highlightable
//! units. The HTML carries its CSS and script inline so a recorder can open
//! the file without a running site.

use std::path::Path;
use std::process::ExitCode;

use crate::palette;

const STAGE_CSS: &str = concat!(
    include_str!("../../server/public/css/tokens.css"),
    "\n",
    include_str!("../../server/public/css/harvard-outline.css"),
);

const STAGE_JS: &str = include_str!("../../server/public/js/harvard-outline-narrate.js");

/// Parse `path` and write a standalone stage to `out`.
#[must_use]
pub fn run(path: &Path, out: &Path) -> ExitCode {
    if !path.exists() {
        eprintln!("navigator: narrate: file not found: {}", path.display());
        return ExitCode::from(2);
    }
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("navigator: narrate: read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    println!(
        "{} {}...",
        palette::dim("Narrating"),
        palette::highlight(path.display())
    );
    let doc = views::harvard_outline::parse(&original);
    if doc.units.is_empty() {
        eprintln!("navigator: narrate: no outline units in {}", path.display());
        return ExitCode::from(2);
    }
    let html = views::harvard_outline::standalone_html(&doc, STAGE_CSS, STAGE_JS);
    if let Err(e) = std::fs::write(out, html) {
        eprintln!("navigator: narrate: write {}: {e}", out.display());
        return ExitCode::from(2);
    }
    println!(
        "{} {} units → {}",
        palette::header("✓"),
        doc.units.len(),
        palette::highlight(out.display())
    );
    ExitCode::SUCCESS
}
