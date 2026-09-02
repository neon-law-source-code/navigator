//! `N114` — a `__for_` child state must follow a role-matched parent.
//!
//! The `__for_<role>` grammar declares a state whose answer creates/selects
//! a child row that FKs to an earlier role-addressed parent — e.g.
//! `address__for_trustor` follows `person__trustor` (or `entity__trustor`)
//! and hangs off that answer's row. This rule enforces two things over the
//! questionnaire graph:
//!
//! 1. **Parent ordering** — every `<type>__for_<role>` state has a
//!    `person__<role>` or `entity__<role>` state that reaches it (is an
//!    ancestor) in the questionnaire graph, so the parent row exists before
//!    the child FKs to it. The present parent type picks the XOR FK column.
//! 2. **Aggregates barred** — an aggregate (plural) `<type>` may not use
//!    `__for_`; its children are inline in the array, not FK-linked.
//!
//! Reuses the `BTreeMap<StateName, TransitionMap>` questionnaire shape that
//! `N104`/`f104` parse.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

/// Aggregate (plural) `<type>` tokens — barred from the `__for_` grammar.
/// Mirrors `store::question_registry::QuestionType::aggregate_tokens()`,
/// grounded by the `cli` parity test so it can't drift.
pub const AGGREGATE_QUESTION_TYPES: &[&str] = &[
    "people",
    "entities",
    "addresses",
    "roles",
    "filings",
    "credentials",
    "disclosures",
    "issuances",
    "jurisdictions",
    "entity_types",
];

pub struct F114ForParentOrdering;

impl F114ForParentOrdering {
    pub const CODE: &'static str = "N114";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

impl Rule for F114ForParentOrdering {
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

        let mut violations = Vec::new();
        for state in questionnaire.keys() {
            let Some((ty, role)) = state.split_once("__") else {
                continue;
            };
            // The `__for_` child grammar: the role reads `for_<parent_role>`.
            let Some(parent_role) = role.strip_prefix("for_") else {
                continue;
            };
            if parent_role.is_empty() {
                continue;
            }

            if AGGREGATE_QUESTION_TYPES.contains(&ty) {
                violations.push(violation(
                    file,
                    format!(
                        "Aggregate type `{ty}` may not use the `__for_` grammar \
                         (state `{state}`) — an aggregate carries inline children"
                    ),
                ));
                continue;
            }

            let person_parent = format!("person__{parent_role}");
            let entity_parent = format!("entity__{parent_role}");
            let has_parent = [&person_parent, &entity_parent].into_iter().any(|parent| {
                questionnaire.contains_key(parent) && reaches(&questionnaire, parent, state)
            });
            if !has_parent {
                violations.push(violation(
                    file,
                    format!(
                        "`{state}` has no preceding `person__{parent_role}` or \
                         `entity__{parent_role}` reachable earlier in the questionnaire graph"
                    ),
                ));
            }
        }
        violations
    }
}

fn violation(file: &SourceFile, message: String) -> Violation {
    Violation {
        code: F114ForParentOrdering::CODE,
        path: file.path.clone(),
        line: 1,
        range: line_byte_range(&file.contents, 1),
        message,
    }
}

/// Whether `target` is reachable from `from` (strictly after it) by
/// following the questionnaire transitions — i.e. `from` is an ancestor of
/// `target`, so its answer is collected first.
fn reaches(graph: &BTreeMap<String, BTreeMap<String, String>>, from: &str, target: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&str> = graph
        .get(from)
        .into_iter()
        .flat_map(|t| t.values())
        .map(String::as_str)
        .collect();
    while let Some(state) = stack.pop() {
        if state == target {
            return true;
        }
        if !seen.insert(state) {
            continue;
        }
        if let Some(transitions) = graph.get(state) {
            stack.extend(transitions.values().map(String::as_str));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::F114ForParentOrdering;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn passes_when_a_person_parent_precedes_the_for_child() {
        let body = "---
questionnaire:
  BEGIN:
    _: person__trustor
  person__trustor:
    _: address__for_trustor
  address__for_trustor:
    _: END
  END: {}
---
";
        assert!(F114ForParentOrdering.lint(&file(body)).is_empty());
    }

    #[test]
    fn passes_with_an_entity_parent() {
        let body = "---
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: address__for_company
  address__for_company:
    _: END
  END: {}
---
";
        assert!(F114ForParentOrdering.lint(&file(body)).is_empty());
    }

    #[test]
    fn flags_a_for_child_with_no_parent() {
        let body = "---
questionnaire:
  BEGIN:
    _: address__for_trustor
  address__for_trustor:
    _: END
  END: {}
---
";
        let v = F114ForParentOrdering.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
        assert!(v[0].message.contains("no preceding `person__trustor`"));
    }

    #[test]
    fn flags_a_parent_that_only_follows_the_child() {
        // The parent exists but is not an ancestor — it comes after the
        // child, so the FK target would not exist yet.
        let body = "---
questionnaire:
  BEGIN:
    _: address__for_trustor
  address__for_trustor:
    _: person__trustor
  person__trustor:
    _: END
  END: {}
---
";
        let v = F114ForParentOrdering.lint(&file(body));
        assert_eq!(v.len(), 1, "got {v:?}");
    }

    #[test]
    fn bars_an_aggregate_from_the_for_grammar() {
        let body = "---
questionnaire:
  BEGIN:
    _: person__trustor
  person__trustor:
    _: people__for_trustor
  people__for_trustor:
    _: END
  END: {}
---
";
        let v = F114ForParentOrdering.lint(&file(body));
        assert!(v
            .iter()
            .any(|x| x.message.contains("Aggregate type `people`")));
    }

    #[test]
    fn ignores_non_for_states() {
        let body = "---
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: custom_text__mission
  custom_text__mission:
    _: END
  END: {}
---
";
        assert!(F114ForParentOrdering.lint(&file(body)).is_empty());
    }
}
