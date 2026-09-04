//! Parse a template's declared `workflow:` state machine into an ordered,
//! branching preview — for the public "workflow" diagram on
//! `/notations/{slug}`, with no live Restate invocation and no domain-crate
//! dependency.
//!
//! The real workflow runtime is `workflows::StateMachineRuntime`, backed in
//! production by Restate and journaled into `store::notation_events` against
//! a live Notation. `neon` (and every other brand crate) is forbidden from
//! depending on `workflows` or `store` at all — see
//! `cli/tests/brand_crate_dependencies.rs` — so the public preview page,
//! which resolves entirely from `include_str!`'d markdown with no database or
//! durable-execution engine in the loop, needs its own small, pure reader of
//! the same `workflow:` frontmatter shape [`crate::questionnaire_preview`]
//! already has for `questionnaire:`.
//!
//! Unlike the questionnaire's linear `BEGIN → … → END` chain, a workflow
//! branches: a state can declare more than one named event, each leading
//! somewhere different (`lawyer_review`'s `approved` and `rejected`, say).
//! [`parse`] walks the whole reachable graph breadth-first from `BEGIN` and
//! returns every reachable state once, each carrying its own declared
//! transitions — the shape a diagram (or a demo run generator) needs, not
//! just one path through it.

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde::Deserialize;

/// One named edge out of a [`WorkflowState`]: the event that fires it, and
/// the state it leads to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowTransition {
    pub event: String,
    pub to: String,
}

/// One state in the declared workflow, with its own outgoing transitions in
/// declared (alphabetical-by-event) order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowState {
    pub name: String,
    pub transitions: Vec<WorkflowTransition>,
}

impl WorkflowState {
    /// A state with no declared transitions — nothing leads out of it,
    /// either because it is `END` or because the frontmatter never gave it
    /// one.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.transitions.is_empty()
    }
}

#[derive(Debug, Default, Deserialize)]
struct WorkflowFrontmatter {
    #[serde(default)]
    workflow: BTreeMap<String, BTreeMap<String, String>>,
}

const BEGIN: &str = "BEGIN";

/// Parse a template's YAML frontmatter into every state reachable from
/// `BEGIN`, breadth-first. Returns an empty list for frontmatter with no
/// `workflow:` block or malformed YAML — a bundled public page degrades to
/// "no diagram" rather than panicking on a shape it doesn't recognize.
#[must_use]
pub fn parse(frontmatter: &str) -> Vec<WorkflowState> {
    let Ok(doc) = serde_yaml::from_str::<WorkflowFrontmatter>(frontmatter) else {
        return Vec::new();
    };
    if doc.workflow.is_empty() {
        return Vec::new();
    }

    let mut ordered = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(BEGIN.to_string());
    visited.insert(BEGIN.to_string());

    while let Some(name) = queue.pop_front() {
        let transitions: Vec<WorkflowTransition> = doc
            .workflow
            .get(&name)
            .into_iter()
            .flat_map(BTreeMap::iter)
            .map(|(event, to)| WorkflowTransition {
                event: event.clone(),
                to: to.clone(),
            })
            .collect();
        for transition in &transitions {
            if visited.insert(transition.to.clone()) {
                queue.push_back(transition.to.clone());
            }
        }
        ordered.push(WorkflowState { name, transitions });
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATURALIZATION_WORKFLOW: &str = r"
workflow:
  BEGIN:
    intake_submitted: intake_persisted__applicant
  intake_persisted__applicant:
    application_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__n400_summary
    rejected: END
  generate_pdf__n400_summary:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: e_filing__uscis
    signature_declined: END
  e_filing__uscis:
    filed: END
  END: {}
";

    #[test]
    fn walks_every_reachable_state_breadth_first() {
        let states = parse(NATURALIZATION_WORKFLOW);
        let names: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "BEGIN",
                "intake_persisted__applicant",
                "lawyer_review",
                "generate_pdf__n400_summary",
                "END",
                "sent_for_signature__pending",
                "e_filing__uscis",
            ],
            "breadth-first from BEGIN, each state exactly once: {names:?}"
        );
    }

    #[test]
    fn a_branching_state_carries_every_one_of_its_named_transitions() {
        let states = parse(NATURALIZATION_WORKFLOW);
        let lawyer_review = states
            .iter()
            .find(|s| s.name == "lawyer_review")
            .expect("lawyer_review is reachable");
        assert_eq!(
            lawyer_review.transitions,
            vec![
                WorkflowTransition {
                    event: "approved".to_string(),
                    to: "generate_pdf__n400_summary".to_string(),
                },
                WorkflowTransition {
                    event: "rejected".to_string(),
                    to: "END".to_string(),
                },
            ]
        );
    }

    #[test]
    fn end_is_terminal() {
        let states = parse(NATURALIZATION_WORKFLOW);
        let end = states
            .iter()
            .find(|s| s.name == "END")
            .expect("END is reachable");
        assert!(end.is_terminal());
    }

    #[test]
    fn frontmatter_with_no_workflow_block_yields_no_states() {
        assert!(parse("title: Just a title\n").is_empty());
    }

    #[test]
    fn malformed_yaml_yields_no_states_rather_than_panicking() {
        assert!(parse("not: [valid, yaml: at all").is_empty());
    }

    #[test]
    fn a_cycle_is_visited_once_rather_than_looping_forever() {
        let frontmatter = r"
workflow:
  BEGIN:
    go: a
  a:
    go: b
  b:
    go: a
";
        let states = parse(frontmatter);
        let names: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["BEGIN", "a", "b"], "each state once: {names:?}");
    }
}
