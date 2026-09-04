//! Parsed workflow spec — the shape that lives in a template's
//! YAML frontmatter under `workflow:`.
//!
//! Wire shape:
//!
//! ```yaml
//! workflow:
//!   BEGIN:
//!     created: lawyer_review__for_grantor
//!   lawyer_review__for_grantor:
//!     approve: notarization__for_grantor
//!     reject:  END
//!   notarization__for_grantor:
//!     signed:  mailroom_send__to_signer
//!     refused: END
//!   mailroom_send__to_signer:
//!     sent:    mailroom_receive__signed_copy
//!   mailroom_receive__signed_copy:
//!     received: END
//!   END: {}
//! ```
//!
//! State names use `<prefix>__<discriminator>` form; the prefix
//! selects the [`crate::StepKind`] (system / lawyer_review /
//! notarization / mailroom_send / mailroom_receive).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A workflow state name, e.g., `lawyer_review__for_trustee`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateName(pub String);

impl StateName {
    pub const BEGIN: &'static str = "BEGIN";
    pub const END: &'static str = "END";

    #[must_use]
    pub fn begin() -> Self {
        Self(Self::BEGIN.to_string())
    }

    #[must_use]
    pub fn end() -> Self {
        Self(Self::END.to_string())
    }

    /// Prefix used by [`crate::step_kind_for`] to pick the step
    /// type. For `lawyer_review__for_trustee` returns
    /// `"lawyer_review"`; for `BEGIN` or `END` returns the whole
    /// name verbatim.
    #[must_use]
    pub fn prefix(&self) -> &str {
        self.0.split_once("__").map_or(self.0.as_str(), |(p, _)| p)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for StateName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Transitions out of a state: `condition -> next state`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionMap(pub BTreeMap<String, StateName>);

impl TransitionMap {
    #[must_use]
    pub fn lookup(&self, condition: &str) -> Option<&StateName> {
        self.0.get(condition)
    }

    pub fn conditions(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Actor class allowed to transition out of a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorClass {
    /// Driven by the durable runtime — no human in the loop.
    System,
    /// A lawyer triggers the transition.
    Lawyer,
    /// The respondent (the person/entity the notation is for)
    /// triggers the transition.
    Respondent,
}

/// Which of a Notation's two state machines a runtime call targets.
///
/// A Notation runs *two* state machines back-to-back: a
/// [`QuestionnaireSpec`] walks the respondent through the
/// declared questions, then a [`WorkflowSpec`] drives the
/// resulting document to final disposition. Both share the same
/// runtime surface; this enum is the key partitioner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineKind {
    /// The questionnaire walker — asks one question per signal.
    Questionnaire,
    /// The post-intake workflow — drives lawyer review, signing,
    /// mailroom, etc.
    Workflow,
}

impl MachineKind {
    /// Stable lowercase token used in Restate handler URLs and
    /// glossary copy.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Questionnaire => "questionnaire",
            Self::Workflow => "workflow",
        }
    }
}

/// Full workflow spec parsed from frontmatter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowSpec {
    pub states: BTreeMap<StateName, TransitionMap>,
}

#[derive(Debug, Error)]
pub enum WorkflowSpecError {
    #[error("missing required state `BEGIN`")]
    MissingBegin,
    #[error("missing required state `END`")]
    MissingEnd,
    #[error("state `{from}` has transition `{condition}` to unknown state `{to}`")]
    DanglingTransition {
        from: String,
        condition: String,
        to: String,
    },
    #[error(
        "questionnaire state `{state}` has condition `{condition}` — `_` (\"the respondent \
         answered\") is the only questionnaire condition"
    )]
    QuestionnaireCondition { state: String, condition: String },
    #[error(
        "questionnaire state `{state}` is not on the `_` chain from BEGIN — the walker would \
         never ask it and the rendered step total would lie"
    )]
    QuestionnaireOffChain { state: String },
    #[error(
        "questionnaire `_` chain from BEGIN stops or cycles at `{state}` before reaching END — \
         the walker would strand the respondent there"
    )]
    QuestionnaireChainBroken { state: String },
    #[error("yaml parse error: {0}")]
    Yaml(String),
}

impl WorkflowSpec {
    /// Parse from a YAML document. Validates `BEGIN`/`END` presence
    /// and that every transition target exists.
    pub fn from_yaml(yaml: &str) -> Result<Self, WorkflowSpecError> {
        let spec: Self =
            serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), WorkflowSpecError> {
        if !self.states.contains_key(&StateName::begin()) {
            return Err(WorkflowSpecError::MissingBegin);
        }
        if !self.states.contains_key(&StateName::end()) {
            return Err(WorkflowSpecError::MissingEnd);
        }
        for (from, transitions) in &self.states {
            for (condition, target) in &transitions.0 {
                if !self.states.contains_key(target) {
                    return Err(WorkflowSpecError::DanglingTransition {
                        from: from.0.clone(),
                        condition: condition.clone(),
                        to: target.0.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Transitions out of `state`, or `None` if no such state.
    #[must_use]
    pub fn transitions_from(&self, state: &StateName) -> Option<&TransitionMap> {
        self.states.get(state)
    }

    #[must_use]
    pub fn is_terminal(&self, state: &StateName) -> bool {
        self.states.get(state).is_some_and(TransitionMap::is_empty)
    }
}

/// Parsed `questionnaire:` block from a notation template's
/// frontmatter. Same wire shape as [`WorkflowSpec`] — a graph of
/// named states keyed by transition condition — but distinct at
/// the type level so the application can't accidentally hand a
/// questionnaire spec to a workflow runtime call (or vice versa).
///
/// Wire shape (matches the retainer template's
/// [`templates/notations/neon_law/shared/onboarding_letter.md`](../../../templates/notations/neon_law/shared/onboarding_letter.md)
/// `questionnaire:` block):
///
/// ```yaml
/// questionnaire:
///   BEGIN:
///     _: client_name
///   client_name:
///     _: client_email
///   client_email:
///     _: END
///   END: {}
/// ```
///
/// State names are bare question codes (no `__discriminator`
/// suffix in practice — questionnaires only ever ask one
/// respondent), and `_` is the **only** transition condition since
/// the only signal that advances a questionnaire is "the respondent
/// answered." [`QuestionnaireSpec::validate`] enforces that shape at
/// parse time: a questionnaire is one linear `_` chain from `BEGIN`
/// to `END` covering every declared state, so the walker's
/// "step N of M" total and END-reachability hold by construction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionnaireSpec(pub WorkflowSpec);

impl QuestionnaireSpec {
    /// Parse from a YAML document. Applies [`WorkflowSpec`]'s
    /// validation (`BEGIN`/`END` required, every transition target
    /// resolves) plus questionnaire linearity — see
    /// [`QuestionnaireSpec::validate`].
    pub fn from_yaml(yaml: &str) -> Result<Self, WorkflowSpecError> {
        let spec = Self(WorkflowSpec::from_yaml(yaml)?);
        spec.validate()?;
        Ok(spec)
    }

    /// Validate the questionnaire shape on top of the base
    /// [`WorkflowSpec::validate`] checks:
    ///
    /// 1. `_` is the only transition condition anywhere;
    /// 2. the `_` chain out of `BEGIN` terminates at `END` (no dead
    ///    end, no cycle);
    /// 3. every declared state except `BEGIN`/`END` sits on that
    ///    chain.
    ///
    /// Deserializing the transparent serde shape bypasses this, so
    /// every constructor that accepts authored YAML must call it.
    ///
    /// # Errors
    /// The `Questionnaire*` variants of [`WorkflowSpecError`], plus
    /// anything [`WorkflowSpec::validate`] returns.
    pub fn validate(&self) -> Result<(), WorkflowSpecError> {
        self.0.validate()?;
        for (state, transitions) in &self.0.states {
            for condition in transitions.conditions() {
                if condition != "_" {
                    return Err(WorkflowSpecError::QuestionnaireCondition {
                        state: state.as_str().to_string(),
                        condition: condition.to_string(),
                    });
                }
            }
        }

        // Walk the `_` chain from BEGIN. Condition-uniqueness above
        // means at most one `_` per state, so the walk is
        // deterministic; collect the question states it visits.
        let mut on_chain: std::collections::BTreeSet<StateName> = std::collections::BTreeSet::new();
        let mut here = StateName::begin();
        loop {
            let Some(next) = self
                .0
                .transitions_from(&here)
                .and_then(|t| t.lookup("_"))
                .cloned()
            else {
                // Dead end before END — BEGIN itself when empty, or a
                // question with no outgoing `_`.
                return Err(WorkflowSpecError::QuestionnaireChainBroken {
                    state: here.as_str().to_string(),
                });
            };
            if next == StateName::end() {
                break;
            }
            if !on_chain.insert(next.clone()) {
                // Revisiting a state = a cycle that never reaches END.
                return Err(WorkflowSpecError::QuestionnaireChainBroken {
                    state: next.as_str().to_string(),
                });
            }
            here = next;
        }

        for state in self.0.states.keys() {
            if *state == StateName::begin() || *state == StateName::end() {
                continue;
            }
            if !on_chain.contains(state) {
                return Err(WorkflowSpecError::QuestionnaireOffChain {
                    state: state.as_str().to_string(),
                });
            }
        }
        Ok(())
    }

    /// Borrow the underlying [`WorkflowSpec`] — useful when a
    /// runtime trait method takes the canonical machine spec.
    #[must_use]
    pub fn inner(&self) -> &WorkflowSpec {
        &self.0
    }

    /// Consume into the underlying [`WorkflowSpec`].
    #[must_use]
    pub fn into_inner(self) -> WorkflowSpec {
        self.0
    }

    /// Transitions out of `state`, or `None` if no such state.
    #[must_use]
    pub fn transitions_from(&self, state: &StateName) -> Option<&TransitionMap> {
        self.0.transitions_from(state)
    }

    /// Whether `state` is terminal (no outgoing transitions).
    #[must_use]
    pub fn is_terminal(&self, state: &StateName) -> bool {
        self.0.is_terminal(state)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActorClass, MachineKind, QuestionnaireSpec, StateName, WorkflowSpec, WorkflowSpecError,
    };

    const TRUST_WORKFLOW: &str = r"
BEGIN:
  created: lawyer_review__for_grantor
lawyer_review__for_grantor:
  approve: notarization__for_grantor
  reject: END
notarization__for_grantor:
  signed: mailroom_send__to_signer
  refused: END
mailroom_send__to_signer:
  sent: mailroom_receive__signed_copy
mailroom_receive__signed_copy:
  received: END
END: {}
";

    #[test]
    fn parses_a_realistic_trust_workflow() {
        let spec = WorkflowSpec::from_yaml(TRUST_WORKFLOW).expect("valid spec");
        assert_eq!(spec.states.len(), 6);
        let begin = spec.transitions_from(&StateName::begin()).unwrap();
        assert_eq!(
            begin.lookup("created").unwrap().as_str(),
            "lawyer_review__for_grantor"
        );
        assert!(spec.is_terminal(&StateName::end()));
    }

    #[test]
    fn rejects_spec_without_begin_state() {
        let err = WorkflowSpec::from_yaml("END: {}\n").unwrap_err();
        assert!(matches!(err, WorkflowSpecError::MissingBegin));
    }

    #[test]
    fn rejects_spec_without_end_state() {
        let err = WorkflowSpec::from_yaml("BEGIN: {created: somewhere}\n").unwrap_err();
        assert!(matches!(err, WorkflowSpecError::MissingEnd));
    }

    #[test]
    fn rejects_dangling_transition_target() {
        let err = WorkflowSpec::from_yaml("BEGIN: {go: nowhere}\nEND: {}\n").unwrap_err();
        assert!(matches!(
            err,
            WorkflowSpecError::DanglingTransition { to, .. } if to == "nowhere"
        ));
    }

    #[test]
    fn state_name_prefix_strips_double_underscore_discriminator() {
        assert_eq!(
            StateName::from("lawyer_review__for_trustee").prefix(),
            "lawyer_review"
        );
        assert_eq!(StateName::begin().prefix(), "BEGIN");
        assert_eq!(StateName::from("notarization").prefix(), "notarization");
    }

    #[test]
    fn actor_class_serialization_matches_yaml_lowercase() {
        let yaml = serde_yaml::to_string(&ActorClass::Lawyer).unwrap();
        assert_eq!(yaml.trim(), "lawyer");
        let back: ActorClass = serde_yaml::from_str("system").unwrap();
        assert_eq!(back, ActorClass::System);
    }

    #[test]
    fn yaml_parse_error_surfaces_as_workflow_spec_error_yaml_variant() {
        // Plain string at the top level — fails to deserialize into
        // the spec's BTreeMap shape before validation runs.
        let err = WorkflowSpec::from_yaml("just_a_scalar_string\n").unwrap_err();
        assert!(matches!(err, WorkflowSpecError::Yaml(_)), "got {err:?}");
    }

    const RETAINER_QUESTIONNAIRE: &str = r"
BEGIN:
  _: client_name
client_name:
  _: client_email
client_email:
  _: project_name
project_name:
  _: product_description
product_description:
  _: END
END: {}
";

    #[test]
    fn questionnaire_spec_parses_the_retainer_questionnaire_block() {
        let q = QuestionnaireSpec::from_yaml(RETAINER_QUESTIONNAIRE).expect("valid");
        let first = q
            .transitions_from(&StateName::begin())
            .and_then(|t| t.lookup("_"))
            .map(StateName::as_str);
        assert_eq!(first, Some("client_name"));
        assert!(q.is_terminal(&StateName::end()));
    }

    #[test]
    fn questionnaire_spec_reuses_workflow_spec_validation_for_missing_begin() {
        let err = QuestionnaireSpec::from_yaml("END: {}\n").unwrap_err();
        assert!(matches!(err, WorkflowSpecError::MissingBegin));
    }

    #[test]
    fn questionnaire_rejects_any_condition_other_than_underscore() {
        let err = QuestionnaireSpec::from_yaml(
            "
BEGIN:
  _: client_name
client_name:
  skip: END
  _: END
END: {}
",
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                WorkflowSpecError::QuestionnaireCondition { state, condition }
                    if state == "client_name" && condition == "skip"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn questionnaire_rejects_a_state_off_the_underscore_chain() {
        // `orphan` is declared and dangling-free (it points at END) but
        // the walker would never ask it — the step total would lie.
        let err = QuestionnaireSpec::from_yaml(
            "
BEGIN:
  _: client_name
client_name:
  _: END
orphan:
  _: END
END: {}
",
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                WorkflowSpecError::QuestionnaireOffChain { state } if state == "orphan"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn questionnaire_rejects_a_chain_that_dead_ends_before_end() {
        let err = QuestionnaireSpec::from_yaml(
            "
BEGIN:
  _: client_name
client_name: {}
END: {}
",
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                WorkflowSpecError::QuestionnaireChainBroken { state } if state == "client_name"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn questionnaire_rejects_a_chain_that_cycles_before_end() {
        let err = QuestionnaireSpec::from_yaml(
            "
BEGIN:
  _: client_name
client_name:
  _: client_email
client_email:
  _: client_name
END: {}
",
        )
        .unwrap_err();
        assert!(
            matches!(&err, WorkflowSpecError::QuestionnaireChainBroken { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_empty_questionnaire_that_goes_straight_to_end_is_linear() {
        let q = QuestionnaireSpec::from_yaml("BEGIN:\n  _: END\nEND: {}\n").expect("valid");
        assert!(q.is_terminal(&StateName::end()));
    }

    #[test]
    fn questionnaire_spec_exposes_underlying_workflow_spec() {
        let q = QuestionnaireSpec::from_yaml(RETAINER_QUESTIONNAIRE).unwrap();
        // `inner()` borrows the same graph; `into_inner()` consumes.
        assert_eq!(q.inner().states.len(), 6);
        let unwrapped = q.into_inner();
        assert!(unwrapped.is_terminal(&StateName::end()));
    }

    #[test]
    fn machine_kind_serializes_as_lowercase_tokens() {
        let q = serde_yaml::to_string(&MachineKind::Questionnaire).unwrap();
        let w = serde_yaml::to_string(&MachineKind::Workflow).unwrap();
        assert_eq!(q.trim(), "questionnaire");
        assert_eq!(w.trim(), "workflow");
    }

    #[test]
    fn machine_kind_as_str_matches_serde_tokens() {
        assert_eq!(MachineKind::Questionnaire.as_str(), "questionnaire");
        assert_eq!(MachineKind::Workflow.as_str(), "workflow");
    }
}
