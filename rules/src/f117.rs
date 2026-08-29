//! `N117` — every `custom_text__*` state must be explicitly allowlisted.
//!
//! `custom_text` is an escape hatch for document-specific narrative prose,
//! not a way to model durable people, contact facts, legal actors,
//! countries, projects, or product scope outside the glossary-backed typed
//! states. The rule is a strict allowlist: a `custom_text__<role>` not in
//! [`ALLOWED_CUSTOM_TEXT_ROLES`] is an error, so a new free-text state
//! fails closed and forces the adjudication (type it, or allowlist it here
//! with its rationale) at authoring time. Roles that name a glossary-backed
//! noun (a person's name/email, a legal actor, a country, a phone) get a
//! pointed message and can never be allowlisted — a meta-test bars the
//! tokens from the allowlist itself.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F117GlossaryBackedCustomText;

impl F117GlossaryBackedCustomText {
    pub const CODE: &'static str = "N117";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// The adjudicated free-text primitives (issue #241). Each entry is a
/// role whose value is genuinely one-off narrative or a primitive the
/// registry has no better shape for — never a glossary noun.
pub const ALLOWED_CUSTOM_TEXT_ROLES: &[&str] = &[
    // Narrative free text — the answer is prose only a human can write.
    "alleged_account",    // creditor's account reference, verbatim
    "contractor_term",    // e.g. "until the project completes"
    "disputed_reason",    // client's own words for the dispute
    "dissolution_reason", // stated grounds for dissolution
    // The engagement letter's minimum scope — the one sentence saying what
    // the Firm is committing to right now. It cannot be typed: the Matter
    // does not exist as a record until this letter opens it, and the scope
    // is the lawyer's own prose about work, not a durable noun the registry
    // could carry. Everything else in that letter IS typed (the client
    // entity, its address, both DRIs, the project, the start date, the
    // governing law), which is what leaves this one field free.
    "engagement_scope",
    "file_retention",         // closing letter: how the file is handled
    "fundraising_activities", // narrative program description
    "matter_summary",         // closing letter: work performed
    "mission_statement",      // the nonprofit's own mission text
    "next_obligation",        // closing letter: what the client does next
    "disputed_item",          // report-specific item label, verbatim
    "report_error",           // client's description of the inaccuracy
    "reporting_agency",       // report issuer label, not a durable contact
    "revenue_strategy",       // Form 990 narrative synthesis
    "settlement_terms",       // negotiated terms, free-form
    "tradeline",              // credit-report line, verbatim
    "trust_property",         // schedule of trust corpus, free-form
    "worker_duties",          // duties description
    // Preservation-TRO narrative (Rule 65(d)(1)(C) specificity). Each is
    // prose the engineering custodian writes once for one motion; none
    // names a durable actor the registry could type.
    "custody_transfer",     // how control of the assets changed hands
    "protected_duties",     // enumerated duties and access an order preserves
    "repository_inventory", // per-repo name, host, owner, URL, commit range
    "threat_evidence",      // documents bearing on the risk of destruction
    // Primitives awaiting a better shape — counts and unit-carrying
    // amounts the registry has no numeric type for. The NV formation
    // pair is revisited by the #256 form re-authoring; they are charter
    // attributes of the articles, not `issuance` events.
    "annual_salary",           // amount + the client's format ("$85,000")
    "contractor_rate",         // amount + unit ("$95/hour")
    "par_value",               // per-share dollar figure (NV articles)
    "pay_schedule",            // single-choice once its client rendering ships
    "settlement_target",       // amount or formula ("50% of balance")
    "shares_authorized",       // count (NV articles)
    "termination_notice_days", // count of days
    "time_outside_us",         // approximate day count; the attorney
                               // verifies exact travel dates against the
                               // continuous-residence requirement
];

/// Role tokens that always denote a glossary-backed noun. A role
/// containing one gets the pointed "use a typed state" message, and the
/// meta-test bars these from [`ALLOWED_CUSTOM_TEXT_ROLES`] so the
/// allowlist can't erode the rule.
const GLOSSARY_BACKED_TOKENS: &[&str] = &[
    "agent",
    "beneficiary",
    "client",
    "country",
    "email",
    "executor",
    "guardian",
    "jurisdiction",
    "name",
    "phone",
    "testator",
    "trustee",
];

/// Whole roles that are glossary-backed without a matching token —
/// today, product scope belongs to the product/project render context.
const GLOSSARY_BACKED_ROLES: &[&str] = &["product_description"];

impl Rule for F117GlossaryBackedCustomText {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
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

        questionnaire
            .keys()
            .filter_map(|state| custom_text_role(state))
            .filter(|role| !ALLOWED_CUSTOM_TEXT_ROLES.contains(role))
            .map(|role| {
                let state = format!("custom_text__{role}");
                let line = questionnaire_state_line(&file.contents, &state);
                let message = if is_glossary_backed_role(role) {
                    format!(
                        "`{state}` models glossary-backed vocabulary through `custom_text`; \
                         use a typed questionnaire state such as `person__*`, `entity__*`, \
                         `country__*`, `custom_phone__*`, or product/project render context \
                         instead"
                    )
                } else {
                    format!(
                        "`{state}` is not an allowlisted free-text primitive; use a typed \
                         questionnaire state, or add the role to the rules crate's \
                         `ALLOWED_CUSTOM_TEXT_ROLES` with its rationale"
                    )
                };
                Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line,
                    range: line_byte_range(&file.contents, line),
                    message,
                }
            })
            .collect()
    }
}

fn custom_text_role(state: &str) -> Option<&str> {
    state.strip_prefix("custom_text__")
}

fn is_glossary_backed_role(role: &str) -> bool {
    GLOSSARY_BACKED_ROLES.contains(&role)
        || role
            .split('_')
            .any(|token| GLOSSARY_BACKED_TOKENS.contains(&token))
}

fn questionnaire_state_line(contents: &str, state: &str) -> usize {
    let key = format!("{state}:");
    for (idx, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.len() < raw.len() && trimmed == key {
            return idx + 1;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::{is_glossary_backed_role, F117GlossaryBackedCustomText, ALLOWED_CUSTOM_TEXT_ROLES};
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn flags_custom_text_name_and_email_shapes() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__client_name
  custom_text__client_name:
    _: custom_text__client_email
  custom_text__client_email:
    _: END
  END: {}
---
";
        let v = F117GlossaryBackedCustomText.lint(&file(body));
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v.iter().any(|v| v.message.contains("client_name")));
        assert!(v.iter().any(|v| v.message.contains("client_email")));
    }

    #[test]
    fn flags_country_and_phone_shapes_with_typed_state_guidance() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__country_of_birth
  custom_text__country_of_birth:
    _: custom_text__daytime_phone
  custom_text__daytime_phone:
    _: END
  END: {}
---
";
        let v = F117GlossaryBackedCustomText.lint(&file(body));
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v.iter().all(|v| v.message.contains("glossary-backed")));
        assert!(v.iter().any(|v| v.message.contains("country_of_birth")));
        assert!(v.iter().any(|v| v.message.contains("daytime_phone")));
    }

    #[test]
    fn flags_any_role_missing_from_the_allowlist() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__favorite_color
  custom_text__favorite_color:
    _: END
  END: {}
---
";
        let v = F117GlossaryBackedCustomText.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("not an allowlisted"), "{v:?}");
        assert!(v[0].message.contains("ALLOWED_CUSTOM_TEXT_ROLES"));
    }

    #[test]
    fn flags_product_description_as_render_context() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__product_description
  custom_text__product_description:
    _: END
  END: {}
---
";
        let v = F117GlossaryBackedCustomText.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("product/project render context"));
    }

    #[test]
    fn allowlisted_free_text_primitives_pass() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__settlement_terms
  custom_text__settlement_terms:
    _: custom_text__disputed_reason
  custom_text__disputed_reason:
    _: END
  END: {}
---
";
        assert!(F117GlossaryBackedCustomText.lint(&file(body)).is_empty());
    }

    /// The allowlist can never erode the rule: no entry may contain a
    /// glossary-backed token, so a glossary noun can't be allowlisted.
    #[test]
    fn no_allowlist_entry_contains_a_glossary_backed_token() {
        for role in ALLOWED_CUSTOM_TEXT_ROLES {
            assert!(
                !is_glossary_backed_role(role),
                "`{role}` is glossary-backed and must not be allowlisted"
            );
        }
    }

    #[test]
    fn violation_points_at_the_offending_questionnaire_state_line() {
        let body = "---
questionnaire:
  BEGIN:
    _: custom_text__registered_agent
  custom_text__registered_agent:
    _: END
  END: {}
---
";
        let v = F117GlossaryBackedCustomText.lint(&file(body));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].line, 5, "{v:?}");
    }

    #[test]
    fn no_frontmatter_or_questionnaire_means_no_violation() {
        assert!(F117GlossaryBackedCustomText
            .lint(&file("just body"))
            .is_empty());
        assert!(F117GlossaryBackedCustomText
            .lint(&file("---\ntitle: T\n---\nbody\n"))
            .is_empty());
    }

    #[test]
    fn is_error_severity() {
        use crate::{severity_for_code, Severity};
        assert_eq!(severity_for_code("N117"), Severity::Error);
    }
}
