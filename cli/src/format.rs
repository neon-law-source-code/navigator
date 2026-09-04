//! `navigator notations format <file>` — normalize whitespace and bullet style
//! in a Markdown notation, preserving YAML frontmatter.
//!
//! Column wrapping is intentionally out of scope: pandoc was the only
//! viable wrapper and it broke on hosts without it, while the rest of
//! the workspace ships no shell-outs. Run
//! `pandoc -f markdown -t markdown --columns=120 file.md` manually if
//! column wrapping is needed.
//!
//! What the command does, in order:
//!
//! 1. Read the file (UTF-8).
//! 2. Split the YAML frontmatter from the body (preserves it
//!    verbatim — frontmatter is the linter's territory, not the
//!    formatter's).
//! 3. Convert `- ` list markers in the body to `* ` (M005 in the
//!    rule engine).
//! 4. Trim trailing whitespace on every body line (M009).
//! 5. Atomically write the result back.

use std::path::Path;
use std::process::ExitCode;

use crate::palette;

/// Run the formatter on `path`. Returns `ExitCode::SUCCESS` on a
/// successful (idempotent or actual) reformat; `ExitCode::from(2)`
/// on any I/O failure with the reason printed to stderr.
#[must_use]
pub fn run(path: &Path) -> ExitCode {
    if !path.exists() {
        eprintln!("navigator: format: file not found: {}", path.display());
        return ExitCode::from(2);
    }
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("navigator: format: read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    println!(
        "{} {}...",
        palette::dim("Formatting"),
        palette::highlight(path.display())
    );
    let formatted = format_string(&original);
    if formatted == original {
        println!(
            "{} already clean: {}",
            palette::header("✓"),
            palette::highlight(path.display())
        );
        return ExitCode::SUCCESS;
    }
    if let Err(e) = std::fs::write(path, &formatted) {
        eprintln!("navigator: format: write {}: {e}", path.display());
        return ExitCode::from(2);
    }
    println!(
        "{} formatted {}",
        palette::header("✓"),
        palette::highlight(path.display())
    );
    ExitCode::SUCCESS
}

/// Apply the format transforms to a Markdown string. Pure — exposed
/// for unit testing.
#[must_use]
pub fn format_string(content: &str) -> String {
    let (frontmatter, body) = split_frontmatter(content);
    let body = normalize_bullets(body);
    let body = trim_trailing_whitespace(&body);
    match frontmatter {
        Some(fm) => format!("{fm}{body}"),
        None => body,
    }
}

/// Split off the leading YAML frontmatter (including both `---`
/// markers and the terminating newline) so the formatter never
/// touches it. Returns `(frontmatter_with_markers, body)` — the
/// frontmatter slice is `None` when the input has no leading `---\n`
/// block.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let Some(after_open) = content.strip_prefix("---\n") else {
        return (None, content);
    };
    // Closer with trailing newline: split right after the `\n` so
    // the body starts at the first body byte.
    if let Some(end) = after_open.find("\n---\n") {
        let frontmatter_end = "---\n".len() + end + "\n---\n".len();
        return (
            Some(&content[..frontmatter_end]),
            &content[frontmatter_end..],
        );
    }
    // Closer at EOF, no trailing newline → no body.
    if after_open.ends_with("\n---") {
        return (Some(content), "");
    }
    (None, content)
}

/// Replace `- ` list markers with `* `, preserving the leading
/// indentation. Lines that don't look like a bullet pass through
/// untouched. Bullet-style preference matches the M005 rule in the
/// rule engine, so `validate` won't disagree with `format`.
fn normalize_bullets(body: &str) -> String {
    body.split('\n')
        .map(|line| {
            let indent_len = line.len() - line.trim_start().len();
            let (indent, rest) = line.split_at(indent_len);
            if let Some(after) = rest.strip_prefix("- ") {
                format!("{indent}* {after}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip trailing spaces and tabs from every line. M009 in the rule
/// engine fires on the same characters.
fn trim_trailing_whitespace(body: &str) -> String {
    body.split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{format_string, split_frontmatter};

    #[test]
    fn split_frontmatter_returns_none_when_no_leading_marker() {
        let (fm, body) = split_frontmatter("hello\n");
        assert!(fm.is_none());
        assert_eq!(body, "hello\n");
    }

    #[test]
    fn split_frontmatter_returns_markers_and_body_when_present() {
        let src = "---\ntitle: x\n---\n# Body\n";
        let (fm, body) = split_frontmatter(src);
        assert_eq!(fm, Some("---\ntitle: x\n---\n"));
        assert_eq!(body, "# Body\n");
    }

    #[test]
    fn split_frontmatter_handles_eof_closer_without_trailing_newline() {
        let src = "---\ntitle: x\n---";
        let (fm, body) = split_frontmatter(src);
        assert_eq!(fm, Some("---\ntitle: x\n---"));
        assert_eq!(body, "");
    }

    #[test]
    fn format_converts_dash_bullets_to_star_bullets() {
        let out = format_string("- a\n- b\n");
        assert_eq!(out, "* a\n* b\n");
    }

    #[test]
    fn format_preserves_indentation_under_nested_bullets() {
        let out = format_string("- top\n  - nested\n");
        assert_eq!(out, "* top\n  * nested\n");
    }

    #[test]
    fn format_does_not_touch_in_word_dashes() {
        let out = format_string("a-b is fine\n");
        assert_eq!(out, "a-b is fine\n");
    }

    #[test]
    fn format_does_not_touch_horizontal_rules() {
        // `---` as an HR sits inside body content. Bullet conversion
        // only fires on `- ` (dash + space); HR keeps its three
        // dashes untouched.
        let out = format_string("para\n\n---\n\npara\n");
        assert_eq!(out, "para\n\n---\n\npara\n");
    }

    #[test]
    fn format_trims_trailing_spaces_and_tabs() {
        let out = format_string("hello   \nworld\t\n");
        assert_eq!(out, "hello\nworld\n");
    }

    #[test]
    fn format_preserves_frontmatter_verbatim() {
        let src = "---\ntitle: x   \n---\n- item   \n";
        let out = format_string(src);
        // Trailing whitespace inside frontmatter is left alone.
        assert!(out.starts_with("---\ntitle: x   \n---\n"));
        // Body is normalized.
        assert!(out.ends_with("* item\n"));
    }

    #[test]
    fn format_is_idempotent() {
        let src = "---\ntitle: x\n---\n* a\n* b\n";
        let first = format_string(src);
        let second = format_string(&first);
        assert_eq!(first, second);
    }

    #[test]
    fn format_does_not_change_clean_input() {
        let clean = "# Heading\n\nA paragraph.\n\n* one\n* two\n";
        assert_eq!(format_string(clean), clean);
    }
}
