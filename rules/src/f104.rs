//! `N104` — questionnaire states must reference valid question codes
//! and workflow states must compose known workflow step prefixes.
//!
//! Questionnaire state name shape: `<question_code>__<discriminator>`
//! (the `__<discriminator>` part is optional). The prefix before the
//! first `__` must appear in the configured valid-codes set. Workflow
//! state names use the same discriminator convention, but their prefix
//! must come from the reusable workflow-step catalog. The sentinel
//! states `BEGIN` and `END` are exempt.
//!
//! Both the `questionnaire:` and `workflow:` maps in frontmatter
//! are validated; both must declare a `BEGIN` state and reach
//! `END` from at least one transition.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

use crate::{f107, frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F104FlowQuestionCodes {
    valid_codes: HashSet<String>,
    /// Whether a `workflow:` block is required. True for a legal notation,
    /// whose document only becomes real by advancing through review,
    /// signature, and filing. False for a `kind: github` notation, which
    /// renders its answers into an issue or pull request body and stops —
    /// it has no workflow to run, and demanding an unexecuted one would
    /// put a vestigial state machine in every file on that shelf.
    workflow_required: bool,
}

impl F104FlowQuestionCodes {
    pub const CODE: &'static str = "N104";

    #[must_use]
    pub fn new<I, S>(codes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            valid_codes: codes.into_iter().map(Into::into).collect(),
            workflow_required: true,
        }
    }

    /// The same rule with the `workflow:` requirement lifted — every
    /// questionnaire check still runs. See [`Self::workflow_required`].
    #[must_use]
    pub fn questionnaire_only<I, S>(codes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            workflow_required: false,
            ..Self::new(codes)
        }
    }
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    /// Attorney-drafted `letter` and `memo` blueprints may be body-only
    /// — but only when the body itself is prose-only; see
    /// [`body_is_binding_or_signable`]. Other notation kinds remain
    /// machine-driven and require both maps.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    prompts: BTreeMap<String, String>,
    #[serde(default)]
    choices: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// The two custom types that carry a one-off option list; every other
/// `custom_*` primitive takes a free value and must not define `choices`.
const CHOICE_CUSTOM_TYPES: &[&str] = &["custom_single_choice", "custom_multiple_choice"];

impl Rule for F104FlowQuestionCodes {
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

        let mut violations = Vec::new();
        let Some(questionnaire) = parsed.questionnaire else {
            if parsed.workflow.is_none()
                && attorney_drafted_kind(parsed.kind.as_deref())
                && !body_is_binding_or_signable(&file.contents)
            {
                return violations;
            }
            violations.push(violation(file, "Missing required `questionnaire` key"));
            return violations;
        };
        let workflow = match parsed.workflow {
            Some(workflow) => Some(workflow),
            None if self.workflow_required => {
                violations.push(violation(file, "Missing required `workflow` key"));
                return violations;
            }
            None => None,
        };
        self.validate_questionnaire(
            file,
            &questionnaire,
            &parsed.prompts,
            &parsed.choices,
            &mut violations,
        );
        // A declared `workflow:` is validated whether or not it was
        // required, so a GitHub notation that grows one later is still held
        // to the same shape.
        if let Some(workflow) = workflow {
            Self::validate_workflow(file, &workflow, &mut violations);
        }
        violations
    }
}

fn attorney_drafted_kind(kind: Option<&str>) -> bool {
    matches!(kind, Some("letter" | "memo"))
}

/// True when the document's own body marks it as binding or signable,
/// independent of what `kind:` it declares — a signature placeholder
/// (`{{<signer>.signature}}` / `{{<signer>.initials}}`, `N107`'s
/// grammar), a heading naming a signature block, or a manual
/// "By: ____" signing line. The shipped Nevada engagement letter looks
/// exactly like the last of these: real binding terms and a hand-signed
/// block, with no `{{ }}` placeholder at all.
///
/// The attorney-drafted bypass above is for a *prose-only* letter or
/// memo — one nobody signs. A letter that carries one of these markers
/// is not prose-only, so it may not use the bypass regardless of its
/// declared `kind:`: trusting `kind: letter` alone would let a
/// signable instrument validate clean the moment its
/// `questionnaire:`/`workflow:` metadata is stripped out, which is
/// exactly the failure mode this check exists to close.
fn body_is_binding_or_signable(contents: &str) -> bool {
    let body = frontmatter::split(contents).map_or(contents, |(_, body)| body);
    if f107::signature_placeholders(body)
        .iter()
        .any(|p| p.field == "signature" || p.field == "initials")
    {
        return true;
    }
    body.lines().any(|line| {
        let trimmed = line.trim();
        is_signature_heading(trimmed) || is_manual_signature_line(trimmed)
    })
}

fn is_signature_heading(line: &str) -> bool {
    line.starts_with('#') && line.to_ascii_lowercase().contains("signature")
}

fn is_manual_signature_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("by:") && line.contains("___")
}

impl F104FlowQuestionCodes {
    fn validate_common_shape(
        file: &SourceFile,
        map: &BTreeMap<String, BTreeMap<String, String>>,
        map_name: &str,
        violations: &mut Vec<Violation>,
    ) -> bool {
        if !map.contains_key("BEGIN") {
            violations.push(violation(
                file,
                format!("{map_name} is missing required BEGIN state"),
            ));
            return false;
        }
        let reaches_end = map.values().any(|t| t.values().any(|n| n == "END"));
        if !reaches_end {
            violations.push(violation(
                file,
                format!("{map_name} is missing required END state"),
            ));
            return false;
        }
        true
    }

    fn validate_questionnaire(
        &self,
        file: &SourceFile,
        map: &BTreeMap<String, BTreeMap<String, String>>,
        prompts: &BTreeMap<String, String>,
        choices: &BTreeMap<String, BTreeMap<String, String>>,
        violations: &mut Vec<Violation>,
    ) {
        if !Self::validate_common_shape(file, map, "questionnaire", violations) {
            return;
        }
        if self.valid_codes.is_empty() {
            // No registry provided — fall back to structural checks only
            // (BEGIN/END presence), matching the behavior callers get
            // when the default factory is used without supplying codes.
            return;
        }
        for state in map.keys() {
            if state == "BEGIN" || state == "END" {
                continue;
            }
            let prefix = state.split_once("__").map_or(state.as_str(), |(p, _)| p);
            if !self.valid_codes.contains(prefix) {
                violations.push(violation(
                    file,
                    format!("Invalid question code: `{prefix}` (from state `{state}`)"),
                ));
            }
            if prefix.starts_with("custom_") {
                Self::validate_custom_question(file, state, prefix, prompts, choices, violations);
            }
        }
    }

    /// A `custom_*` state gets its wording (and, for a choice type, its
    /// options) from `prompts.<prompt_key>` and `choices.<prompt_key>` — the bank
    /// supplies nothing for a one-off. Enforce that the entry exists and
    /// that its `choices` presence matches the type: choice types require
    /// options, every other custom primitive forbids them.
    fn validate_custom_question(
        file: &SourceFile,
        state: &str,
        prefix: &str,
        prompts: &BTreeMap<String, String>,
        choices: &BTreeMap<String, BTreeMap<String, String>>,
        violations: &mut Vec<Violation>,
    ) {
        let Some((_, prompt_key)) = state.split_once("__") else {
            violations.push(violation(
                file,
                format!(
                    "Custom question state `{state}` must use `custom_*__prompt_key` and define `prompts.<prompt_key>`"
                ),
            ));
            return;
        };
        let Some(prompt) = prompts.get(prompt_key) else {
            violations.push(violation(
                file,
                format!(
                    "Custom question state `{state}` is missing required `prompts.{prompt_key}`"
                ),
            ));
            return;
        };
        if prompt.trim().is_empty() {
            violations.push(violation(
                file,
                format!("Custom question `prompts.{prompt_key}` needs a non-empty `prompt`"),
            ));
        }
        let is_choice_type = CHOICE_CUSTOM_TYPES.contains(&prefix);
        if is_choice_type && choices.get(prompt_key).is_none_or(BTreeMap::is_empty) {
            violations.push(violation(
                file,
                format!(
                    "Custom question state `{state}` is a choice type and needs \
                     `choices.{prompt_key}`"
                ),
            ));
        } else if !is_choice_type && !choices.get(prompt_key).is_none_or(BTreeMap::is_empty) {
            violations.push(violation(
                file,
                format!(
                    "Custom question `prompts.{prompt_key}` (`{prefix}`) must not define \
                     `choices` — only custom_single_choice / custom_multiple_choice may"
                ),
            ));
        }
    }

    fn validate_workflow(
        file: &SourceFile,
        map: &BTreeMap<String, BTreeMap<String, String>>,
        violations: &mut Vec<Violation>,
    ) {
        if !Self::validate_common_shape(file, map, "workflow", violations) {
            return;
        }
        for state in map.keys() {
            if state == "BEGIN" || state == "END" {
                continue;
            }
            let prefix = state.split_once("__").map_or(state.as_str(), |(p, _)| p);
            if !valid_workflow_step_prefix(prefix) {
                violations.push(violation(
                    file,
                    format!("Invalid workflow step prefix: `{prefix}` (from state `{state}`)"),
                ));
            }
        }
    }
}

/// Whether `prefix` is an allowed workflow-step prefix. Delegates to the
/// single source of truth, the [`crate::workflow_steps`] catalog (which
/// also handles the `_signature` / `_signatures` suffix family), so the
/// allow-list and the hover descriptions can never drift apart.
#[must_use]
pub fn valid_workflow_step_prefix(prefix: &str) -> bool {
    crate::workflow_steps::is_allowed_prefix(prefix)
}

fn violation(file: &SourceFile, message: impl Into<String>) -> Violation {
    Violation {
        code: F104FlowQuestionCodes::CODE,
        path: file.path.clone(),
        line: 1,
        range: line_byte_range(&file.contents, 1),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{valid_workflow_step_prefix, F104FlowQuestionCodes};
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    const VALID_CODES: &[&str] = &["trustee_name", "beneficiary_name"];

    fn rule() -> F104FlowQuestionCodes {
        F104FlowQuestionCodes::new(VALID_CODES.iter().copied())
    }

    #[test]
    fn passes_on_clean_questionnaire_and_workflow() {
        let body = "---
title: T
questionnaire:
  BEGIN:
    created: trustee_name
  trustee_name:
    answered: beneficiary_name
  beneficiary_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let violations = rule().lint(&file(body));
        assert!(violations.is_empty(), "got {violations:?}");
    }

    #[test]
    fn no_frontmatter_means_no_violation() {
        assert!(rule().lint(&file("just body")).is_empty());
    }

    #[test]
    fn attorney_drafted_kinds_may_omit_both_machines() {
        for kind in ["letter", "memo"] {
            let source = file(&format!(
                "---\nkind: {kind}\n---\n\nAttorney-drafted body.\n"
            ));
            assert!(
                rule().lint(&source).is_empty(),
                "{kind} may omit questionnaire and workflow"
            );
        }
    }

    #[test]
    fn attorney_drafted_letter_with_a_signature_block_still_requires_both_machines() {
        // A `letter` with real binding terms and a signature block is not
        // prose-only — the shipped Nevada engagement letter looks exactly
        // like this: numbered substantive sections, then a "Signatures"
        // heading with manual "By: ____  Date: ____" lines and no `{{ }}`
        // placeholder at all. Stripping its questionnaire/workflow
        // metadata must not let a copy of it validate clean.
        let body = "---\nkind: letter\n---\n\n## I. Terms\n\nThe Firm will represent you.\n\n\
                     ## IX. Signatures\n\nBy: ______________________________  Date: ____________\n";
        let violations = rule().lint(&file(body));
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("Missing required `questionnaire`")),
            "a signable letter body must still require the questionnaire machine: {violations:?}"
        );
    }

    #[test]
    fn attorney_drafted_kinds_must_omit_both_machines_together() {
        for machine in ["questionnaire", "workflow"] {
            let source = file(&format!(
                "---\nkind: memo\n{machine}:\n  BEGIN:\n    _: END\n  END: {{}}\n---\n"
            ));
            let violations = rule().lint(&source);
            assert_eq!(violations.len(), 1, "{machine}: {violations:?}");
            assert!(
                violations[0]
                    .message
                    .contains(if machine == "questionnaire" {
                        "workflow"
                    } else {
                        "questionnaire"
                    }),
                "{machine}: {violations:?}"
            );
        }
    }

    #[test]
    fn instrument_kinds_still_require_both_machines() {
        let source = file("---\nkind: agreement\n---\n");
        let violations = rule().lint(&source);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("questionnaire"));
    }

    #[test]
    fn custom_states_read_top_level_prompts_and_choices() {
        let source = file("---\nquestionnaire:\n  BEGIN: { _: custom_single_choice__delivery }\n  custom_single_choice__delivery: { _: END }\n  END: {}\nprompts:\n  delivery: How should it arrive?\nchoices:\n  delivery:\n    email: By email\n    post: By post\n---\n");
        let findings =
            F104FlowQuestionCodes::questionnaire_only(["custom_single_choice"]).lint(&source);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn missing_questionnaire_key_is_a_violation() {
        let body = "---\nworkflow:\n  BEGIN:\n    a: END\n  END: {}\n---\n";
        let v = rule().lint(&file(body));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("Missing required `questionnaire`"));
    }

    #[test]
    fn missing_workflow_key_is_a_violation() {
        let body = "---\nquestionnaire:\n  BEGIN:\n    a: END\n  END: {}\n---\n";
        let v = rule().lint(&file(body));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("Missing required `workflow`"));
    }

    #[test]
    fn missing_begin_state_in_either_map_is_a_violation() {
        let body = "---
questionnaire:
  trustee_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    a: END
  END: {}
---
";
        let v = rule().lint(&file(body));
        assert!(v
            .iter()
            .any(|x| x.message.contains("questionnaire") && x.message.contains("BEGIN")));
    }

    #[test]
    fn flags_state_referencing_unknown_question_code() {
        let body = "---
questionnaire:
  BEGIN:
    created: not_a_valid_code
  not_a_valid_code:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = rule().lint(&file(body));
        assert!(v.iter().any(|x| x.message.contains("Invalid question code")
            && x.message.contains("not_a_valid_code")));
    }

    #[test]
    fn double_underscore_suffix_is_stripped_for_code_lookup() {
        let body = "---
questionnaire:
  BEGIN:
    created: trustee_name__for_grantor
  trustee_name__for_grantor:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        assert!(rule().lint(&file(body)).is_empty());
    }

    #[test]
    fn custom_question_states_require_a_prompt() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_text__fundraising_activities
  custom_text__fundraising_activities:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = F104FlowQuestionCodes::new(["custom_text"]).lint(&file(body));
        assert!(v.iter().any(|x| x
            .message
            .contains("missing required `prompts.fundraising_activities`")));
    }

    #[test]
    fn custom_question_states_accept_a_matching_prompt() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_text__fundraising_activities
  custom_text__fundraising_activities:
    answered: END
  END: {}
prompts:
  fundraising_activities: What are the fundraising activities?
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        assert!(F104FlowQuestionCodes::new(["custom_text"])
            .lint(&file(body))
            .is_empty());
    }

    #[test]
    fn custom_question_with_a_blank_prompt_is_flagged() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_text__note
  custom_text__note:
    answered: END
  END: {}
prompts:
  note: '   '
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = F104FlowQuestionCodes::new(["custom_text"]).lint(&file(body));
        assert!(
            v.iter()
                .any(|x| x.message.contains("needs a non-empty `prompt`")),
            "got {v:?}"
        );
    }

    #[test]
    fn choice_custom_type_requires_choices() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_single_choice__basis
  custom_single_choice__basis:
    answered: END
  END: {}
prompts:
  basis: Which basis applies?
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = F104FlowQuestionCodes::new(["custom_single_choice"]).lint(&file(body));
        assert!(
            v.iter()
                .any(|x| x.message.contains("is a choice type and needs")),
            "got {v:?}"
        );
    }

    #[test]
    fn non_choice_custom_type_forbids_choices() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_datetime__formation_date
  custom_datetime__formation_date:
    answered: END
  END: {}
prompts:
  formation_date: When was the formation date?
choices:
  formation_date:
    today: Today
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = F104FlowQuestionCodes::new(["custom_datetime"]).lint(&file(body));
        assert!(
            v.iter().any(|x| x.message.contains("must not define")),
            "got {v:?}"
        );
    }

    #[test]
    fn choice_custom_type_accepts_choices() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_single_choice__basis
  custom_single_choice__basis:
    answered: END
  END: {}
prompts:
  basis: Which basis applies?
choices:
  basis:
    a: Option A
    b: Option B
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        assert!(F104FlowQuestionCodes::new(["custom_single_choice"])
            .lint(&file(body))
            .is_empty());
    }

    #[test]
    fn bare_custom_question_state_is_invalid_without_a_prompt_discriminator() {
        let body = "---
questionnaire:
  BEGIN:
    created: custom_text
  custom_text:
    answered: END
  END: {}
prompts:
  custom_text: What custom text?
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        let v = F104FlowQuestionCodes::new(["custom_text"]).lint(&file(body));
        assert!(v
            .iter()
            .any(|x| x.message.contains("must use `custom_*__prompt_key`")));
    }

    #[test]
    fn workflow_states_are_validated_against_step_prefixes_not_question_codes() {
        let body = "---
title: T
questionnaire:
  BEGIN:
    created: trustee_name
  trustee_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: generate_pdf__trust_pdf
  generate_pdf__trust_pdf:
    persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        assert!(rule().lint(&file(body)).is_empty());
    }

    #[test]
    fn workflow_signature_suffixes_are_known_steps() {
        let body = "---
title: T
questionnaire:
  BEGIN:
    created: trustee_name
  trustee_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: member_signatures
  member_signatures:
    signed: lawyer_review
  lawyer_review:
    approved: END
  END: {}
---
";
        assert!(rule().lint(&file(body)).is_empty());
    }

    #[test]
    fn flags_workflow_states_outside_the_step_catalog() {
        let body = "---
title: T
questionnaire:
  BEGIN:
    created: trustee_name
  trustee_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: bespoke_magic
  bespoke_magic:
    done: END
  END: {}
---
";
        let v = rule().lint(&file(body));
        assert!(v.iter().any(|x| x
            .message
            .contains("Invalid workflow step prefix: `bespoke_magic`")));
    }

    #[test]
    fn retired_document_open_prefix_is_rejected() {
        // The step was renamed `document_open` → `generate_pdf` and the
        // old prefix removed outright (no alias). A template that still
        // authors the retired prefix must fail N104, so no notation can
        // reach the engine with a step the dispatcher no longer knows.
        assert!(!valid_workflow_step_prefix("document_open"));
        let body = "---
title: T
questionnaire:
  BEGIN:
    created: trustee_name
  trustee_name:
    answered: END
  END: {}
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: document_open__trust_pdf
  document_open__trust_pdf:
    persisted: END
  END: {}
---
";
        let v = rule().lint(&file(body));
        assert!(
            v.iter().any(|x| x
                .message
                .contains("Invalid workflow step prefix: `document_open`")),
            "got {v:?}"
        );
    }

    #[test]
    fn n104_accepts_every_engine_step_prefix() {
        // N104's allow-list is the workflow_steps catalog, which the
        // catalog's own drift test pins to workflows::step::STEP_PREFIXES.
        // This guards the N104 entry point specifically, including the
        // `_signature` suffix family.
        for (prefix, _) in workflows::step::STEP_PREFIXES {
            if *prefix == "_signature" {
                assert!(
                    valid_workflow_step_prefix("member_signatures"),
                    "signature suffix family should be accepted by N104",
                );
                continue;
            }
            assert!(
                valid_workflow_step_prefix(prefix),
                "workflow engine prefix `{prefix}` is not accepted by N104",
            );
        }
    }
}
