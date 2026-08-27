//! `N120` — a template body's undotted typed placeholders must be declared
//! questionnaire states.
//!
//! [`crate::F115PathResolution`] grounds the *dotted* half of the body
//! grammar: `{{person__trustor.name}}` reads a field off a typed answer, so
//! the state before the dot must be declared. This rule grounds the other
//! half — the bare `{{<type>__<role>}}` token that substitutes a whole
//! answer, with no field access.
//!
//! The two are the same claim about the same grammar. A body token written
//! in the typed-state form asserts that the questionnaire collects that
//! state; when it does not, nothing fills the token and it reaches the page
//! verbatim. The Markdown-to-Typst conversion marks such a token with a
//! highlight so a reader can see the blank, but a highlight is a signal to a
//! human, not a guarantee — the fix belongs at authoring time, where the
//! template and its questionnaire sit in the same file and can be compared.
//!
//! Scope, deliberately narrow so the rule cannot fight the other grammars:
//!
//! - **Dotted paths** (`{{person__client.name}}`) are `N115`'s, skipped here.
//! - **Signature placeholders** (`{{client.signature}}`) are `N107`'s; they
//!   are dotted, so the same skip covers them.
//! - **Iterators** (`{{#for x in people__members}}`, `{{/for}}`) are `N115`'s.
//! - **Untyped tokens** (`{{custom_clauses}}`, `{{for_label}}`) carry no
//!   `__` discriminator, are filled by mechanisms outside the questionnaire,
//!   and are not checked.
//!
//! What remains is exactly the token that claims to be a typed state, and
//! the frontmatter beside it says whether that claim holds.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F120BodyStateGrounding;

impl F120BodyStateGrounding {
    pub const CODE: &'static str = "N120";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// Every `{{ … }}` token in `body`, as `(inner trimmed text, 1-based line
/// within `body`)`.
fn tokens_with_lines(body: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        let mut rest = line;
        while let Some(start) = rest.find("{{") {
            let Some(end_rel) = rest[start + 2..].find("}}") else {
                break;
            };
            let end = start + 2 + end_rel;
            out.push((rest[start + 2..end].trim().to_string(), idx + 1));
            rest = &rest[end + 2..];
        }
    }
    out
}

/// The byte offset at which the body begins — everything after the
/// frontmatter's closing `---`. Mirrors `F115PathResolution`'s split so both
/// rules read the same region, and lets a body line map back to a file line.
fn body_offset(contents: &str) -> Option<usize> {
    let open = contents.strip_prefix("---")?;
    let close_rel = open.find("\n---")?;
    let after = open[close_rel + 4..].find('\n')?;
    Some(3 + close_rel + 4 + after + 1)
}

/// Translate a 1-based line within the body to a 1-based line within the file.
fn file_line(contents: &str, body_line: usize) -> usize {
    let Some(offset) = body_offset(contents) else {
        return 1;
    };
    let preceding = contents[..offset].lines().count();
    preceding + body_line
}

impl Rule for F120BodyStateGrounding {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let Ok(parsed) = serde_yaml::from_str::<FrontmatterShape>(fm) else {
            return Vec::new();
        };
        let Some(questionnaire) = parsed.questionnaire else {
            return Vec::new();
        };
        let declared: Vec<&str> = questionnaire
            .keys()
            .map(String::as_str)
            .filter(|s| *s != "BEGIN" && *s != "END")
            .collect();

        let Some(offset) = body_offset(&file.contents) else {
            return Vec::new();
        };
        let body = &file.contents[offset..];

        let mut violations = Vec::new();
        let mut reported: Vec<String> = Vec::new();
        for (tok, body_line) in tokens_with_lines(body) {
            // `N115` owns dotted paths and both iterator forms.
            if tok.contains('.') || tok.starts_with("#for ") || tok == "/for" {
                continue;
            }
            // Only the typed-state grammar makes a claim this rule can check.
            if !tok.contains("__") {
                continue;
            }
            if declared.contains(&tok.as_str()) || reported.contains(&tok) {
                continue;
            }
            reported.push(tok.clone());
            let line = file_line(&file.contents, body_line);
            violations.push(Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line,
                range: line_byte_range(&file.contents, line),
                message: format!(
                    "Placeholder `{{{{{tok}}}}}` names undeclared questionnaire state `{tok}`; \
                     nothing fills it, so the token renders verbatim in the document"
                ),
            });
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::F120BodyStateGrounding;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: contents.to_string(),
        }
    }

    /// A questionnaire declaring `states`, then `body`.
    fn template(states: &[&str], body: &str) -> String {
        let first = states.first().copied().unwrap_or("END");
        let mut lines = vec![
            "---".to_string(),
            "kind: onboarding".to_string(),
            "title: T".to_string(),
            "questionnaire:".to_string(),
            "  BEGIN:".to_string(),
            format!("    _: {first}"),
        ];
        for (i, s) in states.iter().enumerate() {
            let next = states.get(i + 1).copied().unwrap_or("END");
            lines.push(format!("  {s}:"));
            lines.push(format!("    _: {next}"));
        }
        lines.push("  END: {}".to_string());
        lines.push("---".to_string());
        lines.push(String::new());
        format!("{}\n{body}", lines.join("\n"))
    }

    #[test]
    fn a_declared_state_placeholder_passes() {
        let f = file(&template(
            &["custom_text__fee_basis"],
            "Fees:\n\n{{custom_text__fee_basis}}\n",
        ));
        assert!(F120BodyStateGrounding.lint(&f).is_empty());
    }

    #[test]
    fn an_undeclared_state_placeholder_is_flagged() {
        let f = file(&template(
            &["person__client"],
            "Fees:\n\n{{custom_text__fee_basis}}\n",
        ));
        let v = F120BodyStateGrounding.lint(&f);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N120");
        assert!(v[0].message.contains("custom_text__fee_basis"), "{v:?}");
    }

    #[test]
    fn the_violation_points_at_the_body_line_carrying_the_token() {
        let contents = template(&["person__client"], "One\n\nTwo\n\n{{custom_text__x}}\n");
        let f = file(&contents);
        let v = F120BodyStateGrounding.lint(&f);
        assert_eq!(v.len(), 1, "{v:?}");
        let line = contents
            .lines()
            .position(|l| l.contains("{{custom_text__x}}"))
            .expect("the token is in the fixture")
            + 1;
        assert_eq!(v[0].line, line, "{v:?}");
    }

    #[test]
    fn a_dotted_path_is_left_to_n115() {
        let f = file(&template(
            &["person__client"],
            "{{person__undeclared.name}}\n",
        ));
        assert!(
            F120BodyStateGrounding.lint(&f).is_empty(),
            "dotted paths belong to N115"
        );
    }

    #[test]
    fn a_signature_placeholder_is_left_to_n107() {
        let f = file(&template(&["person__client"], "{{client.signature}}\n"));
        assert!(F120BodyStateGrounding.lint(&f).is_empty());
    }

    #[test]
    fn iterator_tokens_are_left_to_n115() {
        let f = file(&template(
            &["people__members"],
            "{{#for m in people__members}}{{m.name}}{{/for}}\n",
        ));
        assert!(F120BodyStateGrounding.lint(&f).is_empty());
    }

    #[test]
    fn an_untyped_token_is_not_checked() {
        // `custom_clauses` is spliced by the notation-clause mechanism, not
        // the questionnaire, and carries no `__` discriminator.
        let f = file(&template(&["person__client"], "{{custom_clauses}}\n"));
        assert!(F120BodyStateGrounding.lint(&f).is_empty());
    }

    #[test]
    fn a_repeated_undeclared_token_is_reported_once() {
        let f = file(&template(
            &["person__client"],
            "{{custom_text__x}}\n\n{{custom_text__x}}\n",
        ));
        assert_eq!(F120BodyStateGrounding.lint(&f).len(), 1);
    }

    #[test]
    fn a_file_without_frontmatter_is_skipped() {
        assert!(F120BodyStateGrounding
            .lint(&file("Just prose {{custom_text__x}}.\n"))
            .is_empty());
    }

    #[test]
    fn a_frontmatter_without_a_questionnaire_is_skipped() {
        assert!(F120BodyStateGrounding
            .lint(&file(
                "---\nkind: onboarding\ntitle: T\n---\n\n{{custom_text__x}}\n"
            ))
            .is_empty());
    }

    #[test]
    fn a_placeholder_in_the_frontmatter_is_not_a_body_placeholder() {
        // A prompt may quote a token; only the body renders.
        let contents = "---\nkind: onboarding\ntitle: T\ncustom_questions:\n  x:\n    prompt: \
                        \"say {{custom_text__x}}\"\nquestionnaire:\n  BEGIN:\n    _: END\n  \
                        END: {}\n---\n\nBody.\n";
        assert!(F120BodyStateGrounding.lint(&file(contents)).is_empty());
    }
}
