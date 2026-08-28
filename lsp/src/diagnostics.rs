//! Run the Neon Law Navigator rule set over a buffer and produce LSP
//! `Diagnostic`s. The default rule selection matches classified
//! `cli validate`: prose markdown gets markdown-only rules, while
//! notation templates get frontmatter diagnostics too.

use std::path::PathBuf;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use rules::{description_for_code, severity_for_code, Rule, Severity, SourceFile, Violation};

use crate::position::range_to_lsp_range;

/// Lint `text` with the classified rule set and return both the raw
/// violations (so callers can wire `fix()` later) and the LSP
/// diagnostic projection.
#[must_use]
pub fn lint_buffer(path: PathBuf, text: String) -> (SourceFile, Vec<Violation>) {
    let file = SourceFile {
        path,
        contents: text,
    };
    let rule_set: Vec<Box<dyn Rule>> = rules::navigator_classified_rules(&file);
    let mut violations = Vec::new();
    for rule in &rule_set {
        violations.extend(rule.lint(&file));
    }
    (file, violations)
}

/// Project a single `Violation` onto the LSP diagnostic shape.
/// `text` is the source the violation was computed against and is
/// used to map byte offsets to UTF-16 positions.
///
/// The LSP severity mirrors the gate severity from
/// [`rules::severity_for_code`]: a blocking error (anything that fails
/// `navigator validate`) renders as a **red** squiggle, while a
/// non-blocking advisory (e.g. `N112`, "step allowed but not built yet")
/// renders as a **yellow** one — the same red/yellow split a lawyer sees
/// described in `docs/frontmatter.md`.
#[must_use]
pub fn violation_to_diagnostic(text: &str, v: &Violation) -> Diagnostic {
    let severity = match severity_for_code(v.code) {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    };
    Diagnostic {
        range: range_to_lsp_range(text, &v.range),
        severity: Some(severity),
        code: Some(NumberOrString::String(v.code.to_string())),
        code_description: None,
        source: Some("navigator".to_string()),
        message: format!("{} — {}", description_for_code(v.code), v.message),
        related_information: None,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{lint_buffer, violation_to_diagnostic};
    use lsp_types::{DiagnosticSeverity, NumberOrString};

    #[test]
    fn lint_buffer_flags_a_hard_tab() {
        let (_file, violations) = lint_buffer(
            std::path::PathBuf::from("t.md"),
            "ok\n\thard tab\n".to_string(),
        );
        assert!(
            violations.iter().any(|v| v.code == "M010"),
            "expected M010 violation, got: {:?}",
            violations.iter().map(|v| v.code).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn lint_buffer_keeps_code_only_frontmatter_in_markdown_mode() {
        let (_file, violations) = lint_buffer(
            std::path::PathBuf::from("web/content/marketing/service.md"),
            "---\ntitle: Service\ncode: sample\n---\n\nBody.\n".to_string(),
        );
        assert!(
            violations.iter().all(|v| !v.code.starts_with('N')),
            "code-only content frontmatter should not trigger notation diagnostics: {violations:?}",
        );
    }

    #[test]
    fn lint_buffer_flags_notation_template_frontmatter() {
        // A declared `kind:` classifies the buffer as a notation template,
        // so the N-family fires (here: N108 requires a `code`).
        let (_file, violations) = lint_buffer(
            std::path::PathBuf::from("draft.md"),
            "---\nkind: onboarding\ntitle: Draft\nworkflow:\n  BEGIN:\n    created: END\n---\n"
                .to_string(),
        );
        assert!(
            violations.iter().any(|v| v.code == "N108"),
            "notation template should require code, got {:?}",
            violations.iter().map(|v| v.code).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn violation_to_diagnostic_renders_a_blocking_error_as_red() {
        // A gate-blocking rule (M010 hard tab) is an `Error`, so it
        // surfaces as a red `ERROR` squiggle, not a yellow warning.
        let text = "ok\n\thard tab\n";
        let (_file, violations) = lint_buffer(std::path::PathBuf::from("t.md"), text.to_string());
        let m010 = violations.iter().find(|v| v.code == "M010").unwrap();
        let diag = violation_to_diagnostic(text, m010);
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert!(matches!(diag.code, Some(NumberOrString::String(ref s)) if s == "M010"));
        assert_eq!(diag.source.as_deref(), Some("navigator"));
        // Line 2 (1-based in source → 0-based in LSP).
        assert_eq!(diag.range.start.line, 1);
    }

    #[test]
    fn violation_to_diagnostic_renders_an_advisory_as_yellow() {
        // The "allowed but not built yet" advisory (N112) is a
        // `Warning`, so it stays a yellow `WARNING` squiggle.
        let advisory = rules::Violation {
            code: "N112",
            path: std::path::PathBuf::from("t.md"),
            line: 1,
            range: 0..1,
            message: "step not built yet".to_string(),
        };
        let diag = violation_to_diagnostic("x\n", &advisory);
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
    }
}
