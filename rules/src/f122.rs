//! `N122` — every questionnaire state a notation declares must be read by
//! its body.
//!
//! [`crate::F115PathResolution`] and [`crate::F120BodyStateGrounding`] walk
//! the body and ask whether each placeholder has a state behind it. This
//! rule walks the other way: it takes each declared state and asks whether
//! anything renders it. Both directions describe one contract — a
//! questionnaire exists to fill a document — and only together do they hold
//! it. A template can lose its `{{person__client.name}}` in a rewrite and
//! keep `person__client` in the state machine: the respondent is still
//! asked who the client is, and the answer reaches nothing. That is an
//! error rather than a warning, because a tree held at zero errors is the
//! only place a real finding stays visible.
//!
//! What counts as a read, mirroring the grammars the two body rules ground:
//!
//! - **A bare token** — `{{custom_text__scope}}` substitutes the whole
//!   answer.
//! - **A dotted path** — `{{person__client.name}}` reads a field off the
//!   answer, and so does `.email`; either one reads `person__client`.
//! - **An iterator source** — `{{#for m in people__members}}` walks the
//!   aggregate, so the iterand is read. The loop variable is a binding, not
//!   a state, and is never treated as one.
//! - **A signature block** — `{{client.signature}}` is `N107`'s grammar and
//!   renders from the signer's person state, which `N115` requires to be
//!   `person__client`. The block is that state's reader; without this, the
//!   two rules would contradict each other on the same template.
//!
//! `BEGIN` and `END` are the chain's endpoints, not questions, and are
//! exempt. Render context that no questionnaire declares — `{{custom_clauses}}`,
//! a project or product token — fills no state, so it is not a read; this
//! rule only ever names states the frontmatter itself declares, so context
//! cannot be mistaken for one in either direction.
//!
//! **A form notation is exempt.** `output: form` (equivalently, a `form:`
//! key — `N109` binds the two) says the answers fill a named `AcroForm`'s
//! fields, and the body beside them is an intake summary rather than the
//! document. The field map lives in the `forms` crate, which this one does
//! not depend on, so the rule cannot see that consumer and does not guess:
//! `templates/notations/forms/united_states/federal/uscis/us__naturalization.md`
//! asks a good-moral-character question that reaches a lawyer, not the
//! summary, and it is not a defect.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

/// Signature field types — `N107`'s domain, mirrored from
/// [`crate::f115`]. A dotted path whose first field is one of these is a
/// signature block, and it renders from the signer's `person__<role>`
/// state.
const SIGNATURE_FIELDS: &[&str] = &["signature", "initials", "date"];

pub struct F122QuestionnaireStateIsRead;

impl F122QuestionnaireStateIsRead {
    pub const CODE: &'static str = "N122";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    form: Option<String>,
}

/// One `{{ … }}` token's inner trimmed text, in body order.
fn tokens(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let Some(end_rel) = rest[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end_rel;
        out.push(rest[start + 2..end].trim().to_string());
        rest = &rest[end + 2..];
    }
    out
}

/// Every questionnaire state the body renders.
fn states_read(body: &str) -> BTreeSet<String> {
    let mut read = BTreeSet::new();
    for token in tokens(body) {
        if let Some(rest) = token.strip_prefix("#for ") {
            if let Some((_, state)) = rest.split_once(" in ") {
                read.insert(state.trim().to_string());
            }
            continue;
        }
        if token == "/for" {
            continue;
        }
        let Some((head, tail)) = token.split_once('.') else {
            read.insert(token);
            continue;
        };
        let (head, field) = (head.trim(), tail.split('.').next().unwrap_or(tail));
        if SIGNATURE_FIELDS.contains(&field) {
            read.insert(format!("person__{head}"));
        }
        read.insert(head.to_string());
    }
    read
}

/// The 1-based line on which `state` is declared inside the
/// `questionnaire:` block, so the diagnostic points at the question rather
/// than at the file's first line.
fn declaration_line(contents: &str, state: &str) -> usize {
    let mut inside = false;
    for (index, line) in contents.lines().enumerate() {
        if line.trim_end() == "questionnaire:" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if !line.trim().is_empty() && !line.starts_with(char::is_whitespace) {
            break;
        }
        if line.trim_start().starts_with(&format!("{state}:")) {
            return index + 1;
        }
    }
    1
}

impl Rule for F122QuestionnaireStateIsRead {
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
        if parsed.output.as_deref() == Some("form") || parsed.form.is_some() {
            return Vec::new();
        }
        let Some(questionnaire) = parsed.questionnaire else {
            return Vec::new();
        };

        // Only the body carries render placeholders; scan past frontmatter.
        let body = file
            .contents
            .split_once("\n---")
            .map_or(file.contents.as_str(), |(_, rest)| rest);
        let read = states_read(body);

        questionnaire
            .keys()
            .filter(|state| state.as_str() != "BEGIN" && state.as_str() != "END")
            .filter(|state| !read.contains(state.as_str()))
            .map(|state| {
                let line = declaration_line(&file.contents, state);
                Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line,
                    range: line_byte_range(&file.contents, line),
                    message: format!(
                        "Questionnaire state `{state}` is never read by the body — no \
                         `{{{{{state}}}}}`, `{{{{{state}.<field>}}}}`, or `{{{{#for … in {state}}}}}` \
                         renders it, so the respondent is asked a question whose answer reaches \
                         no document"
                    ),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::F122QuestionnaireStateIsRead;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: contents.to_string(),
        }
    }

    fn tmpl(states: &str, body: &str) -> String {
        format!("---\ntitle: T\nquestionnaire:\n{states}---\n\n{body}\n")
    }

    const CLIENT_CHAIN: &str =
        "  BEGIN:\n    _: person__client\n  person__client:\n    _: END\n  END: {}\n";

    #[test]
    fn flags_a_declared_state_the_body_never_reads() {
        let source = tmpl(CLIENT_CHAIN, "The letter says nothing about anyone.");
        let violations = F122QuestionnaireStateIsRead.lint(&file(&source));
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N122" && v.message.contains("person__client")),
            "an unread state must fail N122; got {violations:?}"
        );
    }

    #[test]
    fn a_bare_token_counts_as_a_read() {
        let source = tmpl(CLIENT_CHAIN, "Signed by {{person__client}}.");
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(&source)).is_empty(),
            "{:?}",
            F122QuestionnaireStateIsRead.lint(&file(&source))
        );
    }

    #[test]
    fn a_dotted_field_counts_as_a_read() {
        let source = tmpl(CLIENT_CHAIN, "Attn: {{person__client.name}}.");
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(&source)).is_empty(),
            "{:?}",
            F122QuestionnaireStateIsRead.lint(&file(&source))
        );
    }

    #[test]
    fn an_iterator_source_counts_as_a_read() {
        let source = tmpl(
            "  BEGIN:\n    _: people__members\n  people__members:\n    _: END\n  END: {}\n",
            "{{#for m in people__members}}{{m.name}}\n{{/for}}",
        );
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(&source)).is_empty(),
            "{:?}",
            F122QuestionnaireStateIsRead.lint(&file(&source))
        );
    }

    #[test]
    fn a_signature_block_reads_its_backing_person_state() {
        let source = tmpl(
            CLIENT_CHAIN,
            "Sign here: {{client.signature}} {{client.date}}",
        );
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(&source)).is_empty(),
            "a signer's person state is read by its signature block; got {:?}",
            F122QuestionnaireStateIsRead.lint(&file(&source))
        );
    }

    #[test]
    fn begin_and_end_are_exempt() {
        let source = tmpl(CLIENT_CHAIN, "Attn: {{person__client.name}}.");
        let violations = F122QuestionnaireStateIsRead.lint(&file(&source));
        assert!(
            !violations
                .iter()
                .any(|v| v.message.contains("BEGIN") || v.message.contains("END")),
            "BEGIN and END are not questions; got {violations:?}"
        );
    }

    #[test]
    fn a_form_notation_is_exempt_because_its_answers_fill_an_acroform() {
        let source = format!(
            "---\ntitle: T\noutput: form\nform: us__naturalization\nquestionnaire:\n{CLIENT_CHAIN}---\n\nIntake summary.\n"
        );
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(&source)).is_empty(),
            "a form notation's questionnaire feeds the field map, not the body; got {:?}",
            F122QuestionnaireStateIsRead.lint(&file(&source))
        );
    }

    #[test]
    fn a_context_token_is_not_mistaken_for_a_state_read() {
        let source = tmpl(CLIENT_CHAIN, "{{custom_clauses}} {{project.name}}");
        let violations = F122QuestionnaireStateIsRead.lint(&file(&source));
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("person__client")),
            "render context fills no questionnaire state; got {violations:?}"
        );
    }

    #[test]
    fn the_violation_points_at_the_declaration_line() {
        let source = tmpl(CLIENT_CHAIN, "Nothing read here.");
        let violations = F122QuestionnaireStateIsRead.lint(&file(&source));
        let declared = source
            .lines()
            .position(|l| l.trim_end() == "  person__client:")
            .expect("the state is declared")
            + 1;
        assert_eq!(violations[0].line, declared, "{violations:?}");
    }

    #[test]
    fn no_frontmatter_means_no_violation() {
        assert!(F122QuestionnaireStateIsRead
            .lint(&file("Just a body line.\n"))
            .is_empty());
    }

    #[test]
    fn no_questionnaire_means_no_violation() {
        assert!(F122QuestionnaireStateIsRead
            .lint(&file("---\ntitle: T\n---\n\nBody.\n"))
            .is_empty());
    }

    #[test]
    fn the_shipped_onboarding_letter_reads_every_state_it_asks() {
        let body = include_str!("../../templates/notations/neon_law/shared/onboarding_letter.md");
        assert!(
            F122QuestionnaireStateIsRead.lint(&file(body)).is_empty(),
            "{:?}",
            F122QuestionnaireStateIsRead.lint(&file(body))
        );
    }
}
