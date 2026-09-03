//! Parse a template's declared questionnaire into an ordered preview list —
//! for the public "try answering this" demo on `/notations/{slug}`
//! (ENG-452), with no live Notation and no domain-crate dependency.
//!
//! The real questionnaire runtime lives in `workflows::notation_session`,
//! which `store` and `cloud` back it against a live Notation row. `neon` (and
//! every other brand crate) is forbidden from depending on `workflows` or
//! `store` at all — see `cli/tests/brand_crate_dependencies.rs` — so the
//! public preview page, which resolves entirely from `include_str!`'d
//! markdown with no database in the loop, needs its own small, pure reader of
//! the same `questionnaire:`/`custom_questions:` frontmatter shape. This
//! module owns exactly that: walk the linear `BEGIN → … → END` chain (already
//! guaranteed by the `N118` rule at authoring time — this reader stops rather
//! than loops on anything else) and pair each `custom_*` state with its own
//! declared prompt and choices.
//!
//! A non-`custom_*` state (`person__client`, `entity`, `signature__firm`, …)
//! answers by creating or linking a real database record — a Person, an
//! Entity, a `DocuSign` envelope — which a public demo cannot meaningfully
//! fake. [`PreviewQuestion::is_interactive`] is `false` for those; the demo
//! renders a short explanation instead of a control that pretends to work.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

/// One step of the demo, in declared order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreviewQuestion {
    /// The full state name, e.g. `custom_single_choice__governing_law`.
    pub code: String,
    /// The state's `<type>` prefix, e.g. `custom_single_choice` — or the
    /// whole code when it carries no `__role` suffix (`entity`).
    pub answer_type: String,
    /// The real prompt for a `custom_*` state; a mechanically-built
    /// explanation for every other (record/reference) state.
    pub prompt: String,
    /// `(value, label)` options — populated only for `custom_single_choice`.
    pub choices: Vec<(String, String)>,
}

impl PreviewQuestion {
    /// Whether this step has real free-entry data to demo. `false` for a
    /// record/reference state, which the page renders as an explanation
    /// instead of a fake control.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.answer_type.starts_with("custom_")
    }
}

#[derive(Debug, Default, Deserialize)]
struct QuestionnaireFrontmatter {
    #[serde(default)]
    questionnaire: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    custom_questions: BTreeMap<String, CustomQuestion>,
}

#[derive(Debug, Default, Deserialize)]
struct CustomQuestion {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    choices: BTreeMap<String, String>,
}

const BEGIN: &str = "BEGIN";
const END: &str = "END";
const UNCONDITIONAL: &str = "_";

/// Parse a template's YAML frontmatter into its ordered questionnaire
/// preview. Returns an empty list for frontmatter with no `questionnaire:`
/// block, malformed YAML, or a chain that doesn't resolve linearly — a bundled
/// public page degrades to "no demo" rather than panicking on a shape it
/// doesn't recognize.
#[must_use]
pub fn parse(frontmatter: &str) -> Vec<PreviewQuestion> {
    let Ok(doc) = serde_yaml::from_str::<QuestionnaireFrontmatter>(frontmatter) else {
        return Vec::new();
    };

    let mut ordered = Vec::new();
    let mut visited = HashSet::new();
    let mut current = BEGIN.to_string();
    while let Some(next) = doc
        .questionnaire
        .get(&current)
        .and_then(|transitions| transitions.get(UNCONDITIONAL))
    {
        if next == END || !visited.insert(next.clone()) {
            break;
        }
        ordered.push(question_for_state(next, &doc.custom_questions));
        current = next.clone();
    }
    ordered
}

/// Build one [`PreviewQuestion`] for `state`, pairing a `custom_*` state with
/// its declared prompt/choices and giving every other state a plain
/// explanation of what it does.
fn question_for_state(
    state: &str,
    custom_questions: &BTreeMap<String, CustomQuestion>,
) -> PreviewQuestion {
    let (answer_type, role) = state
        .split_once("__")
        .map_or((state, state), |(prefix, role)| (prefix, role));
    let custom = custom_questions.get(role);
    let readable_role = role.replace('_', " ");
    let prompt = custom.map_or_else(
        || default_prompt(answer_type, &readable_role),
        |question| question.prompt.clone(),
    );
    let choices = custom.map_or_else(Vec::new, |question| {
        question
            .choices
            .iter()
            .map(|(value, label)| (value.clone(), label.clone()))
            .collect()
    });
    PreviewQuestion {
        code: state.to_string(),
        answer_type: answer_type.to_string(),
        prompt,
        choices,
    }
}

/// An explanation for a record/reference state, which answers by creating or
/// linking a database row rather than by typing a value.
fn default_prompt(answer_type: &str, readable_role: &str) -> String {
    match answer_type {
        "signature" | "notarization" => format!(
            "This step sends the document to {readable_role} for {answer_type} — nothing to type here."
        ),
        _ => format!("This step records the {readable_role}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONBOARDING_LETTER_FRONTMATTER: &str = r"
custom_questions:
  engagement_scope:
    prompt: In a sentence or two, what is the minimum scope of this engagement.
  engagement_start_date:
    prompt: When does this engagement begin?
  governing_law:
    prompt: Which state's law governs this engagement?
    choices:
      nevada: Nevada
      california: California
      washington: Washington
questionnaire:
  BEGIN:
    _: entity
  entity:
    _: address__principal_office
  address__principal_office:
    _: person__client
  person__client:
    _: person__lawyer_dri
  person__lawyer_dri:
    _: project__engagement
  project__engagement:
    _: custom_datetime__engagement_start_date
  custom_datetime__engagement_start_date:
    _: custom_text__engagement_scope
  custom_text__engagement_scope:
    _: custom_single_choice__governing_law
  custom_single_choice__governing_law:
    _: END
  END: {}
";

    #[test]
    fn walks_the_linear_chain_in_declared_order() {
        let questions = parse(ONBOARDING_LETTER_FRONTMATTER);
        let codes: Vec<&str> = questions.iter().map(|q| q.code.as_str()).collect();
        assert_eq!(
            codes,
            vec![
                "entity",
                "address__principal_office",
                "person__client",
                "person__lawyer_dri",
                "project__engagement",
                "custom_datetime__engagement_start_date",
                "custom_text__engagement_scope",
                "custom_single_choice__governing_law",
            ]
        );
    }

    #[test]
    fn a_custom_state_carries_its_own_real_prompt_and_choices() {
        let questions = parse(ONBOARDING_LETTER_FRONTMATTER);
        let governing_law = questions
            .iter()
            .find(|q| q.code == "custom_single_choice__governing_law")
            .expect("the governing-law step is present");
        assert_eq!(governing_law.answer_type, "custom_single_choice");
        assert_eq!(
            governing_law.prompt,
            "Which state's law governs this engagement?"
        );
        assert_eq!(
            governing_law.choices,
            vec![
                ("california".to_string(), "California".to_string()),
                ("nevada".to_string(), "Nevada".to_string()),
                ("washington".to_string(), "Washington".to_string()),
            ]
        );
        assert!(governing_law.is_interactive());
    }

    #[test]
    fn a_record_state_carries_an_explanation_and_no_choices() {
        let questions = parse(ONBOARDING_LETTER_FRONTMATTER);
        let client = questions
            .iter()
            .find(|q| q.code == "person__client")
            .expect("the client step is present");
        assert_eq!(client.answer_type, "person");
        assert_eq!(client.prompt, "This step records the client.");
        assert!(client.choices.is_empty());
        assert!(!client.is_interactive());
    }

    #[test]
    fn a_signature_state_explains_the_hand_off_instead_of_asking_for_ink() {
        let frontmatter = r"
questionnaire:
  BEGIN:
    _: signature__client
  signature__client:
    _: END
  END: {}
";
        let questions = parse(frontmatter);
        assert_eq!(questions.len(), 1);
        assert_eq!(
            questions[0].prompt,
            "This step sends the document to client for signature — nothing to type here."
        );
        assert!(!questions[0].is_interactive());
    }

    #[test]
    fn frontmatter_with_no_questionnaire_block_yields_no_questions() {
        assert!(parse("title: Just a title\n").is_empty());
    }

    #[test]
    fn malformed_yaml_yields_no_questions_rather_than_panicking() {
        assert!(parse("not: [valid, yaml: at all").is_empty());
    }

    #[test]
    fn a_cyclic_chain_stops_instead_of_looping_forever() {
        let frontmatter = r"
questionnaire:
  BEGIN:
    _: a
  a:
    _: b
  b:
    _: a
";
        let questions = parse(frontmatter);
        assert_eq!(
            questions.len(),
            2,
            "stops once a state repeats: {questions:?}"
        );
    }
}
