//! Workflow specs that the workspace ships. Loaded at compile time
//! via `include_str!` so binaries don't have to read template + spec
//! files off disk at boot — they're part of the crate.
//!
//! Each bundled template has a paired
//! `workflows/specs/<code>.yaml` carrying the *same* `workflow:` and
//! `questionnaire:` blocks that live in its markdown frontmatter
//! today. The standalone YAML is the format `cli scaffold` will
//! generate first; the template markdown keeps the rendering body
//! (and, for now, a mirrored copy of the spec that the integrity test
//! pins against).
//!
//! Adding a new workflow: drop a notation template under
//! `templates/forms/...` or `templates/neon_law/...`,
//! write the same `workflow:` + `questionnaire:` blocks into
//! `workflows/specs/<code>.yaml`, and add the file to
//! [`BUNDLED_SPEC_YAML`] below. Product-specific retainers may reuse the
//! shared retainer spec through [`catalog_spec_yaml`]. The coherence test in
//! `workflows/tests/spec_coherence.rs` catches any drift between standalone
//! YAML and its template frontmatter.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::spec::{QuestionnaireSpec, WorkflowSpec, WorkflowSpecError};

/// Raw markdown body for the onboarding-letter notation template. Used
/// by the rendering layer (`views::notation::render_filled_in`) and
/// the integrity / coherence tests; the workflow spec itself now
/// loads from [`RETAINER_INTAKE_SPEC_YAML`].
pub const RETAINER_INTAKE_TEMPLATE: &str =
    include_str!("../../templates/neon_law/shared/onboarding_letter.md");

/// Standalone YAML carrying both `questionnaire:` and `workflow:`
/// blocks for the onboarding-letter intake template.
pub const RETAINER_INTAKE_SPEC_YAML: &str = include_str!("../specs/onboarding__letter.yaml");

/// Shared questionnaire/workflow every *project-scoped* onboarding letter
/// (`onboarding__letter_*`) rides via [`catalog_spec_yaml`]. It differs from
/// the generic intake spec by the `custom_single_choice__governing_law`
/// question — the fillable governing-law clause every product letter now
/// carries (#363). Project-scoped letters vary only in their legal prose, so one
/// spec covers all of them rather than a per-letter copy.
pub const RETAINER_PRODUCT_SPEC_YAML: &str =
    include_str!("../specs/onboarding__letter_product.yaml");

/// Welcome-email workflow spec. Lives outside [`BUNDLED_SPEC_YAML`]
/// because the welcome flow is a notification, not a legal-document
/// notation — the N-family lint rules (lawyer_review required, state
/// names map to question codes) assume the latter and don't apply.
/// The worker reads this constant directly when handling the
/// `onboarding__welcome` notation.
pub const WELCOME_SPEC_YAML: &str = include_str!("../specs/onboarding__welcome.yaml");

/// Parsed welcome workflow spec.
#[must_use]
pub fn welcome_spec() -> WorkflowSpec {
    workflow_spec_from_yaml(WELCOME_SPEC_YAML)
        .expect("welcome spec is bundled; its workflow block must parse")
}

/// Workshop completion certificate workflow spec. Like the welcome flow
/// it lives outside [`BUNDLED_SPEC_YAML`] — it's a notification, not a
/// legal-document notation, so the N-family lint rules don't apply.
/// `BEGIN --requested--> email_send__certificate --email_sent--> END`.
pub const WORKSHOP_CERTIFICATE_SPEC_YAML: &str =
    include_str!("../specs/workshop__certificate.yaml");

/// Parsed workshop-certificate workflow spec.
#[must_use]
pub fn workshop_certificate_spec() -> WorkflowSpec {
    workflow_spec_from_yaml(WORKSHOP_CERTIFICATE_SPEC_YAML)
        .expect("workshop certificate spec is bundled; its workflow block must parse")
}

/// Every bundled spec keyed by its template `code`. Wired up so
/// callers (and `cli scaffold`) can locate the YAML by code without
/// reaching into the filesystem.
pub const BUNDLED_SPEC_YAML: &[(&str, &str)] = &[
    ("onboarding__letter", RETAINER_INTAKE_SPEC_YAML),
    (
        "nv__llc_formation",
        include_str!("../specs/nv__llc_formation.yaml"),
    ),
    (
        "nv__profit_corp_formation",
        include_str!("../specs/nv__profit_corp_formation.yaml"),
    ),
    (
        "nv__business_trust_formation",
        include_str!("../specs/nv__business_trust_formation.yaml"),
    ),
    (
        "offboarding__letter",
        include_str!("../specs/offboarding__letter.yaml"),
    ),
    (
        "memo__contract_review",
        include_str!("../specs/memo__contract_review.yaml"),
    ),
    (
        "nv__dissolution",
        include_str!("../specs/nv__dissolution.yaml"),
    ),
    (
        "nv__annual_report",
        include_str!("../specs/nv__annual_report.yaml"),
    ),
    (
        "nv__modified_business_tax",
        include_str!("../specs/nv__modified_business_tax.yaml"),
    ),
    (
        "nv__nonprofit_501c3_formation",
        include_str!("../specs/nv__nonprofit_501c3_formation.yaml"),
    ),
    ("us__form_990", include_str!("../specs/us__form_990.yaml")),
    (
        "nv__charitable_solicitation_registration",
        include_str!("../specs/nv__charitable_solicitation_registration.yaml"),
    ),
    (
        "us__naturalization",
        include_str!("../specs/us__naturalization.yaml"),
    ),
];

/// Look up the bundled standalone YAML for `code`. Returns `None`
/// if no bundled spec carries that code.
#[must_use]
pub fn bundled_spec_yaml(code: &str) -> Option<&'static str> {
    BUNDLED_SPEC_YAML
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, y)| *y)
}

/// Resolve the questionnaire/workflow spec for a template in the seeded
/// catalog.
///
/// Most templates have their own standalone YAML in [`BUNDLED_SPEC_YAML`].
/// A project-scoped onboarding-letter variant (`onboarding__letter_<something>`)
/// intentionally shares the [`RETAINER_PRODUCT_SPEC_YAML`]
/// questionnaire/workflow: its legal prose varies per matter, but the intake,
/// fillable governing-law question, and review/signature path stay the same
/// until one needs a distinct questionnaire.
#[must_use]
pub fn catalog_spec_yaml(code: &str) -> Option<&'static str> {
    bundled_spec_yaml(code).or_else(|| {
        code.strip_prefix("onboarding__letter_")
            .map(|_| RETAINER_PRODUCT_SPEC_YAML)
    })
}

/// Parsed `retainer_intake` workflow spec, sourced from the
/// standalone YAML.
#[must_use]
pub fn retainer_intake_spec() -> WorkflowSpec {
    workflow_spec_from_yaml(RETAINER_INTAKE_SPEC_YAML)
        .expect("retainer spec is bundled; its workflow block must parse")
}

/// Parsed `retainer_intake` questionnaire spec, sourced from the
/// standalone YAML.
#[must_use]
pub fn retainer_intake_questionnaire() -> QuestionnaireSpec {
    questionnaire_spec_from_yaml(RETAINER_INTAKE_SPEC_YAML)
        .expect("retainer spec is bundled; its questionnaire block must parse")
}

/// Parse a standalone spec YAML (containing `workflow:` and
/// optionally `questionnaire:`) and return the workflow spec.
pub fn workflow_spec_from_yaml(yaml: &str) -> Result<WorkflowSpec, WorkflowSpecError> {
    let wrapper: WorkflowFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    wrapper.workflow.validate()?;
    Ok(wrapper.workflow)
}

/// Parse a standalone spec YAML and return the questionnaire spec.
/// Applies the full questionnaire validation
/// ([`QuestionnaireSpec::validate`]): base shape plus the
/// linear-`_`-chain invariant.
pub fn questionnaire_spec_from_yaml(yaml: &str) -> Result<QuestionnaireSpec, WorkflowSpecError> {
    let wrapper: QuestionnaireFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    wrapper.questionnaire.validate()?;
    Ok(wrapper.questionnaire)
}

/// Parse the optional `prompts:` map from a standalone spec YAML.
pub fn prompt_overrides_from_yaml(
    yaml: &str,
) -> Result<BTreeMap<String, String>, WorkflowSpecError> {
    let wrapper: PromptFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    Ok(wrapper.prompts)
}

/// Parse the optional `audiences:` map from a standalone spec YAML.
pub fn audiences_from_yaml(yaml: &str) -> Result<BTreeMap<String, String>, WorkflowSpecError> {
    let wrapper: AudienceFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    Ok(wrapper.audiences)
}

/// Parse the optional `choices:` map from a standalone spec YAML.
pub fn choices_from_yaml(
    yaml: &str,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, WorkflowSpecError> {
    let wrapper: ChoiceFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    Ok(wrapper.choices)
}

/// A single `custom_*` question defined by the template: its wording and,
/// for a choice type, the one-off options. This is the canonical home for
/// a custom question — the bank supplies the wording for every non-`custom_`
/// state, so those never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomQuestion {
    /// The prompt shown to the respondent for this custom state.
    pub prompt: String,
    /// The one-off options, keyed `value: label`. Empty for the non-choice
    /// custom types (`custom_text`, `custom_datetime`, …).
    #[serde(default)]
    pub choices: BTreeMap<String, String>,
}

/// Parse the optional `custom_questions:` map from a standalone spec YAML.
/// Keyed by the state's `__<prompt_key>` discriminator (e.g. the entry
/// `management_structure` describes `custom_single_choice__management_structure`).
pub fn custom_questions_from_yaml(
    yaml: &str,
) -> Result<BTreeMap<String, CustomQuestion>, WorkflowSpecError> {
    let wrapper: CustomQuestionsFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| WorkflowSpecError::Yaml(e.to_string()))?;
    Ok(wrapper.custom_questions)
}

/// Extract the optional `custom_questions:` map from a notation template's
/// YAML frontmatter.
pub fn custom_questions_from_template(
    markdown: &str,
) -> Result<BTreeMap<String, CustomQuestion>, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    custom_questions_from_yaml(frontmatter)
}

/// Fold a `custom_questions:` map into the flat `prompts` and `choices`
/// maps the questionnaire runtime resolves against. Each custom question's
/// wording lands in `prompts` under its key, and its options (if any) in
/// `choices`. The custom-question entry is canonical: it overwrites any
/// stray flat entry sharing the key.
pub fn merge_custom_questions(
    custom_questions: &BTreeMap<String, CustomQuestion>,
    prompts: &mut BTreeMap<String, String>,
    choices: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    for (key, question) in custom_questions {
        prompts.insert(key.clone(), question.prompt.clone());
        // The custom question is canonical for its key's choices too: a
        // non-choice custom type clears any stray flat entry so no retired
        // option metadata survives behind it.
        if question.choices.is_empty() {
            choices.remove(key);
        } else {
            choices.insert(key.clone(), question.choices.clone());
        }
    }
}

/// Extract the `workflow:` block from a notation template's YAML
/// frontmatter and parse it as a [`WorkflowSpec`]. Used by the
/// integrity / shape-lock tests, which validate that every template's
/// frontmatter is structurally coherent regardless of whether
/// production code reads from the markdown or the standalone YAML.
pub fn workflow_spec_from_template(markdown: &str) -> Result<WorkflowSpec, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    workflow_spec_from_yaml(frontmatter)
}

/// Whether a notation template's markdown frontmatter carries a
/// `questionnaire:` block at all — distinct from whether that block is
/// *valid*. A project-scoped template may override only the document body
/// (frontmatter with no `questionnaire:`, or no frontmatter), in which case
/// the bundled questionnaire still drives intake; a scoped template that does
/// carry a `questionnaire:` block owns its own intake, and a malformed one
/// still surfaces its parse error rather than silently falling back.
#[must_use]
pub fn template_has_questionnaire(markdown: &str) -> bool {
    let Some(frontmatter) = extract_frontmatter(markdown) else {
        return false;
    };
    serde_yaml::from_str::<serde_yaml::Value>(frontmatter)
        .is_ok_and(|value| value.get("questionnaire").is_some())
}

/// Extract the `questionnaire:` block from a notation template's
/// YAML frontmatter and parse it as a [`QuestionnaireSpec`].
pub fn questionnaire_spec_from_template(
    markdown: &str,
) -> Result<QuestionnaireSpec, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    questionnaire_spec_from_yaml(frontmatter)
}

/// Extract the optional `prompts:` map from a notation template's
/// YAML frontmatter.
pub fn prompt_overrides_from_template(
    markdown: &str,
) -> Result<BTreeMap<String, String>, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    prompt_overrides_from_yaml(frontmatter)
}

/// Extract the optional `audiences:` map from a notation template's
/// YAML frontmatter.
pub fn audiences_from_template(
    markdown: &str,
) -> Result<BTreeMap<String, String>, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    audiences_from_yaml(frontmatter)
}

/// Extract the optional `choices:` map from a notation template's YAML
/// frontmatter.
pub fn choices_from_template(
    markdown: &str,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, WorkflowSpecError> {
    let frontmatter = extract_frontmatter(markdown)
        .ok_or_else(|| WorkflowSpecError::Yaml("template has no YAML frontmatter".into()))?;
    choices_from_yaml(frontmatter)
}

#[derive(Deserialize)]
struct WorkflowFrontmatter {
    workflow: WorkflowSpec,
}

#[derive(Deserialize)]
struct QuestionnaireFrontmatter {
    questionnaire: QuestionnaireSpec,
}

#[derive(Deserialize)]
struct PromptFrontmatter {
    #[serde(default)]
    prompts: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct AudienceFrontmatter {
    #[serde(default)]
    audiences: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ChoiceFrontmatter {
    #[serde(default)]
    choices: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct CustomQuestionsFrontmatter {
    #[serde(default)]
    custom_questions: BTreeMap<String, CustomQuestion>,
}

fn extract_frontmatter(contents: &str) -> Option<&str> {
    let after_open = contents.strip_prefix("---\n")?;
    if let Some(end) = after_open.find("\n---\n") {
        return Some(&after_open[..end]);
    }
    after_open.strip_suffix("\n---")
}

#[cfg(test)]
mod tests {
    use super::{
        bundled_spec_yaml, catalog_spec_yaml, custom_questions_from_template,
        custom_questions_from_yaml, merge_custom_questions, questionnaire_spec_from_template,
        questionnaire_spec_from_yaml, retainer_intake_questionnaire, retainer_intake_spec,
        template_has_questionnaire, workflow_spec_from_template, workflow_spec_from_yaml,
        RETAINER_INTAKE_SPEC_YAML, RETAINER_PRODUCT_SPEC_YAML,
    };
    use crate::spec::StateName;
    use std::collections::BTreeMap;

    #[test]
    fn custom_questions_parse_prompt_and_choices() {
        let yaml = "\
custom_questions:
  management_structure:
    prompt: How will the company be managed?
    choices:
      members: Managed by its members
      managers: Managed by appointed managers
  formation_date:
    prompt: When was the formation date?
";
        let cq = custom_questions_from_yaml(yaml).expect("parses");
        assert_eq!(cq.len(), 2);
        assert_eq!(
            cq["management_structure"].prompt,
            "How will the company be managed?"
        );
        assert_eq!(cq["management_structure"].choices.len(), 2);
        assert!(cq["formation_date"].choices.is_empty());
    }

    #[test]
    fn custom_questions_absent_yields_empty_map() {
        let cq = custom_questions_from_yaml("questionnaire:\n  BEGIN:\n    _: END\n  END: {}\n")
            .expect("parses");
        assert!(cq.is_empty());
    }

    #[test]
    fn merge_folds_prompts_and_choices_custom_wins() {
        let cq = custom_questions_from_yaml(
            "custom_questions:\n  fee_status:\n    prompt: What is the fee status?\n    choices:\n      paid: Paid in full\n",
        )
        .expect("parses");
        let mut prompts: BTreeMap<String, String> =
            BTreeMap::from([("fee_status".into(), "stale prompt".into())]);
        let mut choices: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        merge_custom_questions(&cq, &mut prompts, &mut choices);
        assert_eq!(prompts["fee_status"], "What is the fee status?");
        assert_eq!(choices["fee_status"]["paid"], "Paid in full");
    }

    #[test]
    fn merge_clears_stale_flat_choices_for_a_non_choice_custom_question() {
        // `fee_status` is now a non-choice custom question, but a retired
        // flat `choices.fee_status` lingers — the merge must drop it so no
        // stale option metadata survives behind the canonical definition.
        let cq = custom_questions_from_yaml(
            "custom_questions:\n  fee_status:\n    prompt: What is the fee status?\n",
        )
        .expect("parses");
        let mut prompts: BTreeMap<String, String> = BTreeMap::new();
        let mut choices: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::from([(
            "fee_status".into(),
            BTreeMap::from([("paid".into(), "Paid in full".into())]),
        )]);
        merge_custom_questions(&cq, &mut prompts, &mut choices);
        assert_eq!(prompts["fee_status"], "What is the fee status?");
        assert!(
            !choices.contains_key("fee_status"),
            "stale flat choices should be cleared, got {choices:?}"
        );
    }

    #[test]
    fn template_has_questionnaire_detects_the_block_not_its_validity() {
        // Frontmatter with a `questionnaire:` block → present.
        assert!(template_has_questionnaire(
            "---\nquestionnaire:\n  BEGIN:\n    _: END\n  END: {}\n---\nBody.\n"
        ));
        // A `questionnaire:` block that is present but malformed still reads as
        // present — so the caller surfaces the parse error rather than silently
        // falling back.
        assert!(template_has_questionnaire(
            "---\nquestionnaire: not-a-map\n---\nBody.\n"
        ));
        // Frontmatter overriding only the body (no `questionnaire:`) → absent.
        assert!(!template_has_questionnaire(
            "---\ntitle: Scoped\n---\n# Body only\n"
        ));
        // No frontmatter at all → absent.
        assert!(!template_has_questionnaire("# Just a body\n"));
    }

    #[test]
    fn retainer_intake_spec_parses_from_bundled_yaml() {
        let spec = retainer_intake_spec();
        assert!(spec
            .transitions_from(&StateName::begin())
            .and_then(|t| t.lookup("intake_submitted"))
            .is_some());
    }

    #[test]
    fn retainer_intake_questionnaire_walks_client_to_project_engagement() {
        let q = retainer_intake_questionnaire();
        // BEGIN → entity → principal office → client → firm DRI →
        // engagement → start date → scope → governing law → END. Walk via
        // the `_` condition.
        let mut here = StateName::begin();
        let order = [
            "entity",
            "address__principal_office",
            "person__client",
            "person__lawyer_dri",
            "project__engagement",
            "custom_datetime__engagement_start_date",
            "custom_text__engagement_scope",
            "custom_single_choice__governing_law",
            "END",
        ];
        for expected in order {
            let next = q
                .transitions_from(&here)
                .and_then(|t| t.lookup("_"))
                .cloned()
                .expect("each non-terminal state must have an `_` transition");
            assert_eq!(next.as_str(), expected, "from {here:?}");
            here = next;
        }
        assert!(q.is_terminal(&StateName::end()));
    }

    #[test]
    fn retainer_lawyer_review_reask_loop_never_dead_ends() {
        // A rejected review no longer terminates the matter: lawyer_review
        // routes `changes_requested` to a `reask__client` state that
        // re-collects the flagged answers and loops `intake_resubmitted`
        // back to review. `rejected -> END` survives only for a genuine
        // withdrawal. The signature gate (N116) still holds because the
        // loop passes back through lawyer_review before any binding step.
        let spec = retainer_intake_spec();
        let review = StateName::from("lawyer_review");
        let reask = spec
            .transitions_from(&review)
            .and_then(|t| t.lookup("changes_requested"))
            .cloned()
            .expect("lawyer_review must offer changes_requested");
        assert_eq!(reask.as_str(), "reask__client");
        let back_to_review = spec
            .transitions_from(&reask)
            .and_then(|t| t.lookup("intake_resubmitted"))
            .cloned()
            .expect("reask__client must resubmit for review");
        assert_eq!(back_to_review, review);
        // rejected still ends the matter (genuine withdrawal only).
        assert_eq!(
            spec.transitions_from(&review)
                .and_then(|t| t.lookup("rejected"))
                .map(StateName::as_str),
            Some("END"),
        );
        // The re-ask loop does not weaken the signature gate.
        assert!(crate::guardrail::lawyer_review_precedes_signature(&spec).is_ok());
    }

    #[test]
    fn retainer_signature_wait_ends_on_both_received_and_declined() {
        // A signed envelope and a declined/voided one both leave the
        // wait state for END; the journal records which condition fired.
        let spec = retainer_intake_spec();
        let wait = StateName::from("sent_for_signature__pending");
        let received = spec
            .transitions_from(&wait)
            .and_then(|t| t.lookup("signature_received"))
            .expect("signature_received edge exists");
        let declined = spec
            .transitions_from(&wait)
            .and_then(|t| t.lookup("signature_declined"))
            .expect("signature_declined edge exists");
        assert_eq!(received.as_str(), "END");
        assert_eq!(declined.as_str(), "END");
    }

    #[test]
    fn seeded_catalog_template_codes_resolve_to_walkable_questionnaires() {
        let codes = store::seed::seeded_template_codes().expect("seeded template codes");
        // A floor, not a comparison against `BUNDLED_SPEC_YAML`: the product-letter
        // fallback spec is bundled with no seeded template of its own, because it
        // serves project-scoped variants rather than a catalog row. The loop below
        // is the real assertion — every seeded code resolves to a walkable spec.
        assert!(
            codes.len() >= 10,
            "expected the seeded catalog, got {} codes",
            codes.len()
        );
        for code in codes {
            let yaml = catalog_spec_yaml(&code)
                .unwrap_or_else(|| panic!("seeded template `{code}` has no catalog spec"));
            questionnaire_spec_from_yaml(yaml)
                .unwrap_or_else(|e| panic!("catalog spec for `{code}` must parse: {e}"));
        }
    }

    #[test]
    fn bundled_spec_yaml_returns_none_for_unknown_code() {
        assert!(bundled_spec_yaml("does__not_exist").is_none());
    }

    #[test]
    fn catalog_spec_yaml_reuses_the_shared_retainer_for_retainer_variants() {
        // A retainer variant rides the shared spec (fillable governing law)
        // rather than needing a registered copy of its own. The seeded
        // catalog carries no such variant now that the twelve
        // service-specific retainers are retired, but a project-scoped
        // template may still be saved under the prefix — that is what this
        // fallback serves.
        assert_eq!(
            catalog_spec_yaml("onboarding__letter_transcript"),
            Some(RETAINER_PRODUCT_SPEC_YAML)
        );
        assert_eq!(
            catalog_spec_yaml("onboarding__letter_anything"),
            Some(RETAINER_PRODUCT_SPEC_YAML)
        );
        assert!(catalog_spec_yaml("does__not_exist").is_none());
        // The generic onboarding letter resolves to its own registered spec, not the
        // fallback.
        assert_eq!(
            catalog_spec_yaml("onboarding__letter"),
            Some(RETAINER_INTAKE_SPEC_YAML)
        );
    }

    #[test]
    fn product_retainer_fallback_matches_seeded_frontmatter() {
        let shared_questionnaire = questionnaire_spec_from_yaml(RETAINER_PRODUCT_SPEC_YAML)
            .expect("shared product retainer questionnaire");
        let shared_workflow = workflow_spec_from_yaml(RETAINER_PRODUCT_SPEC_YAML)
            .expect("shared product retainer workflow");
        let mut fallback_variants = 0;

        for template in store::seed::SEEDED_TEMPLATES {
            let frontmatter = super::extract_frontmatter(template.markdown)
                .unwrap_or_else(|| panic!("{} has no frontmatter", template.label));
            let yaml = serde_yaml::from_str::<serde_yaml::Value>(frontmatter)
                .unwrap_or_else(|e| panic!("{} frontmatter parses: {e}", template.label));
            let code = yaml
                .get("code")
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or_else(|| panic!("{} has no code", template.label));

            if !code.starts_with("onboarding__letter_") || bundled_spec_yaml(code).is_some() {
                continue;
            }

            fallback_variants += 1;
            assert_eq!(catalog_spec_yaml(code), Some(RETAINER_PRODUCT_SPEC_YAML));
            assert_eq!(
                questionnaire_spec_from_template(template.markdown)
                    .unwrap_or_else(|e| panic!("{code} questionnaire parses: {e}")),
                shared_questionnaire,
                "{code} changed its seeded questionnaire; register a standalone spec instead of using the shared retainer fallback"
            );
            assert_eq!(
                workflow_spec_from_template(template.markdown)
                    .unwrap_or_else(|e| panic!("{code} workflow parses: {e}")),
                shared_workflow,
                "{code} changed its seeded workflow; register a standalone spec instead of using the shared retainer fallback"
            );
            // Every product retainer carries the fillable governing-law
            // question the shared product spec declares (#363).
            assert_eq!(
                custom_questions_from_template(template.markdown)
                    .unwrap_or_else(|e| panic!("{code} custom_questions parse: {e}")),
                custom_questions_from_yaml(RETAINER_PRODUCT_SPEC_YAML)
                    .expect("shared product custom_questions"),
                "{code} must carry the shared governing_law question"
            );
        }

        // The seeded catalog carries no `onboarding__letter_*` variant, so this
        // loop legitimately covers nothing. It stays because the fallback
        // still serves project-scoped variants, and a future seeded one must
        // match the shared spec rather than quietly diverging from it.
        assert_eq!(
            fallback_variants, 0,
            "no seeded retainer variant is expected; if one is added it must match the shared spec"
        );
    }

    #[test]
    fn welcome_spec_drives_signup_through_email_send_to_end() {
        let spec = super::welcome_spec();
        // BEGIN --signup_recorded--> email_send__welcome --email_sent--> END
        let after_begin = spec
            .transitions_from(&StateName::begin())
            .and_then(|t| t.lookup("signup_recorded"))
            .cloned()
            .expect("BEGIN must transition on signup_recorded");
        assert_eq!(after_begin.as_str(), "email_send__welcome");
        let after_send = spec
            .transitions_from(&after_begin)
            .and_then(|t| t.lookup("email_sent"))
            .cloned()
            .expect("email_send state must transition on email_sent");
        assert_eq!(after_send.as_str(), "END");
    }
}
