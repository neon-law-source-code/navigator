//! Hover text for a questionnaire state — the effective prompt a
//! respondent will see, plus where that wording comes from.
//!
//! This mirrors, statically, the same resolution order the intake runtime
//! (`workflows::notation_session::load_question`) applies: a template-scoped
//! override wins over the question bank's seeded wording. Keeping the
//! resolution here — shared by the LSP — means an
//! author sees exactly what a bank-backed state inherits (and can decide to
//! improve the *bank* prompt once, rather than re-overriding per template).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::f113::{bank_prompt, describe_question_type};
use crate::frontmatter;

/// Where a state's effective prompt comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptProvenance {
    /// A `prompts:` entry defines this one-off question.
    Custom,
    /// A `prompts:` entry overrides the bank wording for a bank-backed state.
    Overridden,
    /// No template wording — the question bank supplies it.
    Inherited,
}

impl PromptProvenance {
    fn label(self) -> &'static str {
        match self {
            Self::Custom => "custom question",
            Self::Overridden => "overridden by this template",
            Self::Inherited => "inherited from the question bank",
        }
    }
}

/// The resolved prompt for a questionnaire state and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrompt {
    pub prompt: String,
    pub provenance: PromptProvenance,
}

#[derive(Debug, Default, Deserialize)]
struct HoverFrontmatter {
    #[serde(default)]
    prompts: BTreeMap<String, String>,
}

/// The bank-override alias keys a bank-backed state accepts, mirroring
/// `workflows::notation_session::metadata_keys_for_state`. Most states look
/// up by their bare `__<role>` discriminator; a few carry a friendlier alias.
fn override_keys_for_state(ty: &str, role: &str) -> Vec<String> {
    match (ty, role) {
        ("person", "client") => vec!["client".into(), "client_name".into()],
        ("project", "engagement") => vec!["engagement".into(), "project_name".into()],
        ("entity", "company") => vec!["company".into(), "entity_name".into()],
        ("entity", "nonprofit") => vec!["nonprofit".into(), "nonprofit_legal_name".into()],
        ("person", "worker") => vec!["worker".into(), "worker_legal_name".into()],
        _ => vec![role.into()],
    }
}

/// Substitute the `{{for_label}}` / `{label}` placeholder in a bank prompt
/// with the state's role, rendered as words. Mirrors
/// `workflows::notation_session::localize_prompt_for_state`.
fn substitute_label(prompt: &str, role: &str) -> String {
    let label = role.replace('_', " ");
    prompt
        .replace("{{for_label}}", &label)
        .replace("{label}", &label)
}

/// Resolve the effective prompt for questionnaire `state` given a document's
/// frontmatter, or `None` if the state is not a typed `<type>__<role>` state
/// (bare states carry no type claim) or its type is unregistered.
#[must_use]
pub fn resolve_prompt(frontmatter_yaml: &str, state: &str) -> Option<ResolvedPrompt> {
    let (ty, role) = state.split_once("__")?;
    let parsed: HoverFrontmatter = serde_yaml::from_str(frontmatter_yaml).unwrap_or_default();

    if ty.starts_with("custom_") {
        if let Some(cq) = parsed.prompts.get(role) {
            if !cq.trim().is_empty() {
                return Some(ResolvedPrompt {
                    prompt: cq.clone(),
                    provenance: PromptProvenance::Custom,
                });
            }
        }
        // Undefined custom question: fall back to the bank's generic wording
        // so the author still sees something (N104 flags the missing entry).
        return Some(ResolvedPrompt {
            prompt: substitute_label(bank_prompt(ty)?, role),
            provenance: PromptProvenance::Inherited,
        });
    }

    for key in override_keys_for_state(ty, role) {
        if let Some(prompt) = parsed.prompts.get(&key) {
            return Some(ResolvedPrompt {
                prompt: prompt.clone(),
                provenance: PromptProvenance::Overridden,
            });
        }
    }
    Some(ResolvedPrompt {
        prompt: substitute_label(bank_prompt(ty)?, role),
        provenance: PromptProvenance::Inherited,
    })
}

/// The markdown hover body for a questionnaire `state` in `document`: the
/// effective prompt as a quote, a provenance line, and the type description.
/// `None` when the state is bare/unregistered (nothing prompt-shaped to show).
#[must_use]
pub fn hover_markdown(document: &str, state: &str) -> Option<String> {
    let fm = frontmatter::extract(document)?;
    let resolved = resolve_prompt(fm, state)?;
    let ty = state.split_once("__").map_or(state, |(t, _)| t);
    let type_line = describe_question_type(ty)
        .map(|d| format!("\n\n{d}"))
        .unwrap_or_default();
    Some(format!(
        "**Prompt** — _{}_\n\n> {}{}",
        resolved.provenance.label(),
        resolved.prompt,
        type_line,
    ))
}

#[cfg(test)]
mod tests {
    use super::{hover_markdown, resolve_prompt, PromptProvenance};

    const DOC: &str = "---
title: T
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_single_choice__management_structure
  custom_single_choice__management_structure:
    _: person__registered_agent
  person__registered_agent:
    _: END
  END: {}
prompts:
  client_name: What is the client's full legal name?
  management_structure: How will the company be managed?
choices:
  management_structure:
    members: Managed by its members
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---
Body.
";

    fn fm() -> &'static str {
        // The frontmatter body between the --- markers.
        crate::frontmatter::extract(DOC).unwrap()
    }

    #[test]
    fn custom_question_resolves_to_its_definition() {
        let r = resolve_prompt(fm(), "custom_single_choice__management_structure").unwrap();
        assert_eq!(r.prompt, "How will the company be managed?");
        assert_eq!(r.provenance, PromptProvenance::Custom);
    }

    #[test]
    fn bank_state_with_override_uses_the_override() {
        let r = resolve_prompt(fm(), "person__client").unwrap();
        assert_eq!(r.prompt, "What is the client's full legal name?");
        assert_eq!(r.provenance, PromptProvenance::Overridden);
    }

    #[test]
    fn bank_state_without_override_shows_inherited_bank_prompt() {
        let r = resolve_prompt(fm(), "person__registered_agent").unwrap();
        assert_eq!(r.prompt, "Who is registered agent?");
        assert_eq!(r.provenance, PromptProvenance::Inherited);
    }

    #[test]
    fn undefined_custom_question_falls_back_to_bank_wording() {
        let r = resolve_prompt(fm(), "custom_text__nowhere").unwrap();
        assert_eq!(r.prompt, "What text should be added for nowhere?");
        assert_eq!(r.provenance, PromptProvenance::Inherited);
    }

    #[test]
    fn custom_question_with_a_blank_prompt_falls_back_to_bank_wording() {
        // Defined but empty `prompt` — the resolver still shows the bank's
        // generic wording (N104 separately flags the empty entry).
        let doc = "---
prompts:
  blank: '   '
---
Body.
";
        let fm = crate::frontmatter::extract(doc).unwrap();
        let r = resolve_prompt(fm, "custom_text__blank").unwrap();
        assert_eq!(r.prompt, "What text should be added for blank?");
        assert_eq!(r.provenance, PromptProvenance::Inherited);
    }

    /// Every bank-override alias arm resolves to its friendlier key. Pins the
    /// table that mirrors the runtime's `metadata_keys_for_state`.
    #[test]
    fn each_override_alias_arm_resolves() {
        let doc = "---
prompts:
  entity_name: What is the legal name of your LLC?
  nonprofit_legal_name: What is the nonprofit's legal name?
  worker_legal_name: What is the worker's full legal name?
  project_name: What should we call this matter?
---
Body.
";
        let fm = crate::frontmatter::extract(doc).unwrap();
        for (state, expected) in [
            ("entity__company", "What is the legal name of your LLC?"),
            ("entity__nonprofit", "What is the nonprofit's legal name?"),
            ("person__worker", "What is the worker's full legal name?"),
            ("project__engagement", "What should we call this matter?"),
        ] {
            let r = resolve_prompt(fm, state).unwrap();
            assert_eq!(r.prompt, expected, "state {state}");
            assert_eq!(r.provenance, PromptProvenance::Overridden, "state {state}");
        }
    }

    #[test]
    fn hover_markdown_labels_custom_and_inherited_provenance() {
        let custom = hover_markdown(DOC, "custom_single_choice__management_structure").unwrap();
        assert!(custom.contains("custom question"), "{custom}");
        assert!(
            custom.contains("How will the company be managed?"),
            "{custom}"
        );

        let inherited = hover_markdown(DOC, "person__registered_agent").unwrap();
        assert!(
            inherited.contains("inherited from the question bank"),
            "{inherited}"
        );
        assert!(
            inherited.contains("Who is registered agent?"),
            "{inherited}"
        );
    }

    #[test]
    fn bare_state_has_no_prompt_hover() {
        assert!(resolve_prompt(fm(), "lawyer_review").is_none());
    }

    #[test]
    fn hover_markdown_shows_prompt_provenance_and_type() {
        let md = hover_markdown(DOC, "person__client").unwrap();
        assert!(md.contains("What is the client's full legal name?"), "{md}");
        assert!(md.contains("overridden by this template"), "{md}");
        assert!(md.contains("registered question type"), "{md}");
    }
}
