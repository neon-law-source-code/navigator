//! `N115` — template body data paths and iterators must resolve against
//! the questionnaire's typed states.
//!
//! Two new body grammars are grounded here:
//!
//! - **Dotted data path** — `{{person__trustor.name}}` reads a field off a
//!   typed answer. The state before the dot must be a declared
//!   questionnaire state, and the field must belong to that type's shape (a
//!   custom primitive has no fields). Signature placeholders
//!   (`{{client.signature}}`) keep their `signer.field` shape and stay
//!   `N107`'s specialty — `N115` skips them.
//! - **Iterator** — `{{#for x in people__members}} … {{x.name}} … {{/for}}`
//!   walks an aggregate answer. The iterand must be a declared aggregate
//!   state; the loop variable's fields resolve against the aggregate's row
//!   shape; and every `#for` must close.
//!
//! Shares `f107`'s `{{ … }}` dotted-grammar scanning; the per-type shape is
//! the registry's (mirrored from `store::question_registry`).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

/// Signature field types — `N107`'s domain. A dotted path whose first
/// field is one of these is a signature placeholder, not questionnaire
/// data, regardless of the signer role.
const SIGNATURE_FIELDS: &[&str] = &["signature", "initials", "date"];

/// A person/`people` aggregate row's fields — the registry's
/// `PERSON_ROW_PARTS`.
const PERSON_FIELDS: &[&str] = &[
    "name", "email", "title", "phone", "street", "city", "state", "zip", "country",
];
/// An address row's fields.
const ADDRESS_FIELDS: &[&str] = &["street", "city", "state", "zip", "country"];
/// An entity's renderable field.
const ENTITY_FIELDS: &[&str] = &["name"];

/// The aggregate `<type>` tokens (mirror of
/// [`crate::AGGREGATE_QUESTION_TYPES`]) — the valid iterands of `#for`.
use crate::AGGREGATE_QUESTION_TYPES;

/// What fields a typed state exposes to a dotted path.
#[derive(Clone, Copy)]
enum Shape {
    /// A custom primitive: no dotted fields at all.
    Primitive,
    /// A known, closed field set.
    Fields(&'static [&'static str]),
    /// A type whose field set the lint does not enumerate — accept any
    /// field (the render evaluator resolves it at fill time).
    Any,
}

fn shape_for(type_token: &str) -> Shape {
    match type_token {
        "person" | "people" => Shape::Fields(PERSON_FIELDS),
        "address" | "addresses" => Shape::Fields(ADDRESS_FIELDS),
        "entity" | "entities" => Shape::Fields(ENTITY_FIELDS),
        t if t.starts_with("custom_") => Shape::Primitive,
        _ => Shape::Any,
    }
}

pub struct F115PathResolution;

impl F115PathResolution {
    pub const CODE: &'static str = "N115";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// One `{{ … }}` token: its inner trimmed text.
fn tokens(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(rel) = body[i + 2..].find("}}") {
                let end = i + 2 + rel;
                out.push(body[i + 2..end].trim().to_string());
                i = end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

impl Rule for F115PathResolution {
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
            let mut violations = Vec::new();
            check_declared_signer_backing(file, &file.contents, &BTreeMap::new(), &mut violations);
            return violations;
        };
        // state name → its `<type>` token.
        let state_type: BTreeMap<&str, &str> = questionnaire
            .keys()
            .filter(|s| s.as_str() != "BEGIN" && s.as_str() != "END")
            .map(|s| {
                (
                    s.as_str(),
                    s.split_once("__").map_or(s.as_str(), |(t, _)| t),
                )
            })
            .collect();

        // Only the body carries render placeholders; scan past frontmatter.
        let body = file
            .contents
            .split_once("\n---")
            .map_or(file.contents.as_str(), |(_, rest)| rest);

        let mut violations = Vec::new();
        // Active `#for` loop bindings: var → aggregate `<type>` token.
        let mut loops: Vec<(String, &'static str)> = Vec::new();
        for tok in tokens(body) {
            if let Some(rest) = tok.strip_prefix("#for ") {
                handle_for(rest, &state_type, &mut loops, file, &mut violations);
            } else if tok == "/for" {
                if loops.pop().is_none() {
                    violations.push(violation(file, "`{{/for}}` without a matching `{{#for}}`"));
                }
            } else if let Some((head, tail)) = tok.split_once('.') {
                handle_path(head, tail, &state_type, &loops, file, &mut violations);
            }
        }
        if !loops.is_empty() {
            violations.push(violation(file, "`{{#for}}` is not closed by `{{/for}}`"));
        }
        check_declared_signer_backing(file, &file.contents, &state_type, &mut violations);
        violations
    }
}

fn handle_for(
    rest: &str,
    state_type: &BTreeMap<&str, &str>,
    loops: &mut Vec<(String, &'static str)>,
    file: &SourceFile,
    violations: &mut Vec<Violation>,
) {
    let Some((var, state)) = rest.split_once(" in ") else {
        violations.push(violation(
            file,
            format!("Malformed iterator `{{{{#for {rest}}}}}` — expected `#for <var> in <state>`"),
        ));
        return;
    };
    let (var, state) = (var.trim(), state.trim());
    let Some(&ty) = state_type.get(state) else {
        violations.push(violation(
            file,
            format!(
                "Iterator `#for {var} in {state}` names undeclared questionnaire state `{state}`"
            ),
        ));
        loops.push((var.to_string(), "")); // still track so `/for` balances
        return;
    };
    // Ground the aggregate token to a static str for the binding.
    let Some(agg) = AGGREGATE_QUESTION_TYPES.iter().find(|a| **a == ty) else {
        violations.push(violation(
            file,
            format!("Iterator `#for {var} in {state}` is not an aggregate (`{ty}` is singular)"),
        ));
        loops.push((var.to_string(), ""));
        return;
    };
    loops.push((var.to_string(), agg));
}

fn handle_path(
    head: &str,
    tail: &str,
    state_type: &BTreeMap<&str, &str>,
    loops: &[(String, &'static str)],
    file: &SourceFile,
    violations: &mut Vec<Violation>,
) {
    // Signature blocks (`client.signature`) are N107's domain.
    let field = tail.split('.').next().unwrap_or(tail);
    if SIGNATURE_FIELDS.contains(&field) {
        return;
    }
    // A loop variable resolves against its aggregate row shape.
    if let Some((_, agg)) = loops.iter().find(|(v, _)| v == head) {
        if agg.is_empty() {
            return; // the `#for` was already flagged; don't double-report
        }
        check_field(shape_for(agg), field, head, file, violations);
        return;
    }
    // Otherwise `head` must be a declared questionnaire state.
    let Some(&ty) = state_type.get(head) else {
        violations.push(violation(
            file,
            format!(
                "Data path `{{{{{head}.{tail}}}}}` names undeclared questionnaire state `{head}`"
            ),
        ));
        return;
    };
    check_field(shape_for(ty), field, head, file, violations);
}

fn check_declared_signer_backing(
    file: &SourceFile,
    contents: &str,
    state_type: &BTreeMap<&str, &str>,
    violations: &mut Vec<Violation>,
) {
    let Ok(set) = crate::f107::signer_set(contents) else {
        return;
    };
    if !set.explicit {
        return;
    }
    for role in &set.roles {
        if role == "firm" {
            // The firm countersigns from the configured signer, not a
            // questionnaire state.
            continue;
        }
        let state = format!("person__{role}");
        let Some(&ty) = state_type.get(state.as_str()) else {
            violations.push(violation(
                file,
                format!(
                    "declared signer role `{role}` has no questionnaire state \
                     `{state}` carrying at least a name and an email"
                ),
            ));
            continue;
        };
        if ty != "person" {
            violations.push(violation(
                file,
                format!(
                    "declared signer role `{role}` must be backed by a `person` \
                     state (`{state}`) carrying at least a name and an email"
                ),
            ));
        }
    }
}

fn check_field(
    shape: Shape,
    field: &str,
    owner: &str,
    file: &SourceFile,
    violations: &mut Vec<Violation>,
) {
    match shape {
        Shape::Primitive => violations.push(violation(
            file,
            format!("`{owner}` is a custom primitive and has no dotted fields (`.{field}`)"),
        )),
        Shape::Fields(fields) if !fields.contains(&field) => violations.push(violation(
            file,
            format!("`{owner}` has no field `{field}` (expected one of {fields:?})"),
        )),
        _ => {}
    }
}

fn violation(file: &SourceFile, message: impl Into<String>) -> Violation {
    Violation {
        code: F115PathResolution::CODE,
        path: file.path.clone(),
        line: 1,
        range: line_byte_range(&file.contents, 1),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::F115PathResolution;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    fn tmpl(states: &str, body: &str) -> String {
        format!("---\nquestionnaire:\n{states}---\n\n{body}\n")
    }

    #[test]
    fn resolves_a_valid_dotted_person_field() {
        let body = tmpl(
            "  BEGIN:\n    _: person__trustor\n  person__trustor:\n    _: END\n  END: {}\n",
            "The trustor is {{person__trustor.name}}.",
        );
        assert!(F115PathResolution.lint(&file(&body)).is_empty());
    }

    #[test]
    fn skips_signature_blocks() {
        let body = tmpl(
            "  BEGIN:\n    _: person__trustor\n  person__trustor:\n    _: END\n  END: {}\n",
            "Sign here: {{client.signature}} {{firm.date}}",
        );
        assert!(F115PathResolution.lint(&file(&body)).is_empty());
    }

    #[test]
    fn flags_an_unknown_person_field() {
        let body = tmpl(
            "  BEGIN:\n    _: person__trustor\n  person__trustor:\n    _: END\n  END: {}\n",
            "{{person__trustor.middle_initial}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter()
                .any(|x| x.message.contains("no field `middle_initial`")),
            "{v:?}"
        );
    }

    #[test]
    fn flags_a_dotted_path_on_a_custom_primitive() {
        let body = tmpl(
            "  BEGIN:\n    _: custom_text__mission\n  custom_text__mission:\n    _: END\n  END: {}\n",
            "{{custom_text__mission.value}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter().any(|x| x.message.contains("custom primitive")),
            "{v:?}"
        );
    }

    #[test]
    fn flags_a_path_to_an_undeclared_state() {
        let body = tmpl(
            "  BEGIN:\n    _: person__trustor\n  person__trustor:\n    _: END\n  END: {}\n",
            "{{person__grantor.name}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter().any(|x| x
                .message
                .contains("undeclared questionnaire state `person__grantor`")),
            "{v:?}"
        );
    }

    #[test]
    fn resolves_a_for_loop_over_an_aggregate() {
        let body = tmpl(
            "  BEGIN:\n    _: people__members\n  people__members:\n    _: END\n  END: {}\n",
            "{{#for m in people__members}}{{m.name}}, {{m.city}}\n{{/for}}",
        );
        assert!(F115PathResolution.lint(&file(&body)).is_empty());
    }

    #[test]
    fn flags_a_for_over_a_singular_state() {
        let body = tmpl(
            "  BEGIN:\n    _: person__trustor\n  person__trustor:\n    _: END\n  END: {}\n",
            "{{#for p in person__trustor}}{{p.name}}{{/for}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter().any(|x| x.message.contains("is not an aggregate")),
            "{v:?}"
        );
    }

    #[test]
    fn flags_an_unclosed_for() {
        let body = tmpl(
            "  BEGIN:\n    _: people__members\n  people__members:\n    _: END\n  END: {}\n",
            "{{#for m in people__members}}{{m.name}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(v.iter().any(|x| x.message.contains("not closed")), "{v:?}");
    }

    #[test]
    fn flags_a_bad_loop_var_field() {
        let body = tmpl(
            "  BEGIN:\n    _: people__members\n  people__members:\n    _: END\n  END: {}\n",
            "{{#for m in people__members}}{{m.ssn}}{{/for}}",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter().any(|x| x.message.contains("no field `ssn`")),
            "{v:?}"
        );
    }

    #[test]
    fn no_frontmatter_means_no_violation() {
        assert!(F115PathResolution
            .lint(&file("{{a.b}} just body"))
            .is_empty());
    }

    fn three_party(signers: &str, questionnaire: &str) -> String {
        format!(
            "---\ntitle: Assignment\n{signers}questionnaire:\n{questionnaire}workflow:\n  BEGIN:\n    \
             created: sent_for_signature__pending\n  sent_for_signature__pending:\n    \
             signature_received: END\n  END: {{}}\n---\n\
             {{{{client.signature}}}}\n{{{{firm.signature}}}}\n{{{{confirming_assignor.signature}}}}\n"
        )
    }

    #[test]
    fn declared_three_party_roles_resolve_against_person_states() {
        let body = three_party(
            "signers:\n  - client\n  - firm\n  - confirming_assignor\n",
            "  BEGIN:\n    _: person__client\n  person__client:\n    _: person__confirming_assignor\n  \
             person__confirming_assignor:\n    _: END\n  END: {}\n",
        );
        assert!(
            F115PathResolution.lint(&file(&body)).is_empty(),
            "client and confirming_assignor are person states with name and email: {:?}",
            F115PathResolution.lint(&file(&body)),
        );
    }

    #[test]
    fn declared_role_without_questionnaire_state_fails() {
        let body = three_party(
            "signers:\n  - client\n  - firm\n  - confirming_assignor\n",
            "  BEGIN:\n    _: person__client\n  person__client:\n    _: END\n  END: {}\n",
        );
        let v = F115PathResolution.lint(&file(&body));
        assert!(
            v.iter().any(|x| {
                x.code == "N115"
                    && x.message.contains("confirming_assignor")
                    && (x.message.contains("questionnaire")
                        || x.message.contains("name and an email"))
            }),
            "a declared role with no person state must fail N115; got {v:?}"
        );
    }

    #[test]
    fn shipped_onboarding_letter_needs_no_signers_key() {
        let body = include_str!("../../templates/notations/neon_law/shared/onboarding_letter.md");
        assert!(
            F115PathResolution.lint(&file(body)).is_empty(),
            "{:?}",
            F115PathResolution.lint(&file(body)),
        );
    }

    #[test]
    fn shipped_offboarding_letter_needs_no_signers_key() {
        let body = include_str!("../../templates/notations/neon_law/shared/offboarding_letter.md");
        assert!(
            F115PathResolution.lint(&file(body)).is_empty(),
            "{:?}",
            F115PathResolution.lint(&file(body)),
        );
    }
}
