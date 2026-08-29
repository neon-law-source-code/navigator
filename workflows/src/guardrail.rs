//! Workflow-spec guardrails — structural invariants a spec must hold.
//!
//! Today: the **attorney-review gate** for filled government documents.
//! A `generate_pdf__*` step can fill a fillable government form
//! (AcroForm) from questionnaire answers, but the output is
//! *attorney-review-ready, never auto-filed*. N106 (and the firm's
//! competence / candor duties) require a licensed attorney to review
//! the specific filled document before it is mailed or filed with a
//! government office. [`lawyer_review_gates_filing`] proves the spec
//! cannot reach a submission state from a fill state without passing a
//! `lawyer_review` state in between — the guardrail in code, not prose.

use std::collections::BTreeSet;

use crate::spec::{StateName, WorkflowSpec};

/// A `generate_pdf__*` fill state.
#[must_use]
pub fn is_fill_state(state: &StateName) -> bool {
    state.as_str().starts_with("generate_pdf__")
}

/// A `lawyer_review*` review state (the mandatory human gate).
#[must_use]
pub fn is_review_state(state: &StateName) -> bool {
    state.prefix() == "lawyer_review"
}

/// A state that submits a document to the outside world — mail to a
/// party or a filing with a government office. Covers today's
/// `mailroom_send` / `mailroom_receive` and the filing prefixes Prompt 5
/// promotes to real handlers (`certified_mail`, `e_filing`, `filing__*`),
/// so the gate holds the moment those land.
#[must_use]
pub fn is_submission_state(state: &StateName) -> bool {
    matches!(
        state.prefix(),
        "mailroom_send" | "mailroom_receive" | "certified_mail" | "e_filing"
    ) || state.as_str().starts_with("filing__")
}

/// A `sent_for_signature__*` state — the assembled document goes out for
/// e-signature.
#[must_use]
pub fn is_signature_state(state: &StateName) -> bool {
    state.as_str().starts_with("sent_for_signature")
}

/// A fill→file path that skips the review gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateViolation {
    pub fill_state: String,
    pub submission_state: String,
}

/// Verify that no path from a `generate_pdf__*` (fill) state reaches a
/// submission state without passing a `lawyer_review` state in between.
///
/// # Errors
///
/// Returns every offending `(fill → submission)` pair when the gate is
/// missing on some path.
pub fn lawyer_review_gates_filing(spec: &WorkflowSpec) -> Result<(), Vec<GateViolation>> {
    let mut violations = Vec::new();
    for fill in spec.states.keys().filter(|s| is_fill_state(s)) {
        if let Some(submission) = reaches_submission_without_review(spec, fill) {
            violations.push(GateViolation {
                fill_state: fill.as_str().to_string(),
                submission_state: submission,
            });
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Verify that no path from `BEGIN` reaches a submission state without
/// passing a `lawyer_review` state in between. The Prompt 5 generalization
/// of [`lawyer_review_gates_filing`]: every outbound submission
/// (`mailroom_send`, `certified_mail`, `e_filing`, `filing__*`) must be
/// attorney-reviewed first, whether or not a `generate_pdf__*` fill is
/// involved. The submission step's side effect (a `filings` row) fires
/// only on entering the submission state, so this proves no mail/filing
/// side effect can fire before the review gate.
///
/// # Errors
///
/// Returns the first submission state reachable from `BEGIN` without a
/// review, if any.
pub fn lawyer_review_precedes_submission(spec: &WorkflowSpec) -> Result<(), GateViolation> {
    let begin = StateName::begin();
    if let Some(submission) = reaches_submission_without_review(spec, &begin) {
        return Err(GateViolation {
            fill_state: begin.as_str().to_string(),
            submission_state: submission,
        });
    }
    Ok(())
}

/// Verify that no path from `BEGIN` reaches a `sent_for_signature__*`
/// state without passing a `lawyer_review` state first. The signing
/// analogue of [`lawyer_review_precedes_submission`]: the bytes that go out
/// for e-signature are the bytes an attorney approved, so a notation
/// carrying a custom clause or a client-entered answer is reviewed before
/// it can be signed (the render-once-persist-send invariant in the spec,
/// not just the code).
///
/// # Errors
///
/// Returns the first signature state reachable from `BEGIN` without a
/// review, if any.
pub fn lawyer_review_precedes_signature(spec: &WorkflowSpec) -> Result<(), GateViolation> {
    let begin = StateName::begin();
    if let Some(signature) = reaches_target_without_review(spec, &begin, is_signature_state) {
        return Err(GateViolation {
            fill_state: begin.as_str().to_string(),
            submission_state: signature,
        });
    }
    Ok(())
}

/// BFS from `start`, tracking whether a `lawyer_review` has been crossed.
/// Returns the name of the first submission state reachable without one.
fn reaches_submission_without_review(spec: &WorkflowSpec, start: &StateName) -> Option<String> {
    reaches_target_without_review(spec, start, is_submission_state)
}

/// BFS from `start`, tracking whether a `lawyer_review` has been crossed.
/// Returns the name of the first state matching `is_target` reachable
/// without one. Shared by the submission and signature gates.
fn reaches_target_without_review(
    spec: &WorkflowSpec,
    start: &StateName,
    is_target: fn(&StateName) -> bool,
) -> Option<String> {
    // (state, seen_review). Pairs visited once; the review flag is part
    // of the key so a state reached both with and without review is
    // explored under both.
    let mut visited: BTreeSet<(StateName, bool)> = BTreeSet::new();
    let mut queue: Vec<(StateName, bool)> = vec![(start.clone(), false)];

    while let Some((node, seen_review)) = queue.pop() {
        if !visited.insert((node.clone(), seen_review)) {
            continue;
        }
        // A target reached without a review on the path is the violation.
        // The start node itself never counts (BEGIN / a fill state is not
        // a target).
        if !seen_review && &node != start && is_target(&node) {
            return Some(node.as_str().to_string());
        }
        let downstream_seen = seen_review || is_review_state(&node);
        if let Some(transitions) = spec.states.get(&node) {
            for target in transitions.0.values() {
                queue.push((target.clone(), downstream_seen));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::lawyer_review_gates_filing;
    use crate::spec::WorkflowSpec;

    #[test]
    fn gate_holds_when_lawyer_review_sits_between_fill_and_filing() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: generate_pdf__nv_articles
generate_pdf__nv_articles:
  filled: lawyer_review__articles
lawyer_review__articles:
  approved: mailroom_send__nv_sos
  rejected: END
mailroom_send__nv_sos:
  mailed: END
END: {}
",
        )
        .unwrap();
        assert!(lawyer_review_gates_filing(&spec).is_ok());
    }

    #[test]
    fn gate_is_violated_when_fill_reaches_filing_without_review() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: generate_pdf__nv_articles
generate_pdf__nv_articles:
  done: mailroom_send__nv_sos
mailroom_send__nv_sos:
  mailed: END
END: {}
",
        )
        .unwrap();
        let err = lawyer_review_gates_filing(&spec).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].fill_state, "generate_pdf__nv_articles");
        assert_eq!(err[0].submission_state, "mailroom_send__nv_sos");
    }

    #[test]
    fn gate_catches_review_on_only_one_of_two_branches() {
        // One branch goes fill → review → file (ok); the other goes
        // fill → file directly (violation). The whole spec must fail.
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: generate_pdf__form
generate_pdf__form:
  reviewed: lawyer_review__form
  rushed: filing__nv_sos
lawyer_review__form:
  approved: filing__nv_sos
filing__nv_sos:
  filed: END
END: {}
",
        )
        .unwrap();
        let err = lawyer_review_gates_filing(&spec).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].submission_state, "filing__nv_sos");
    }

    #[test]
    fn a_spec_with_no_fill_state_passes_vacuously() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: lawyer_review
lawyer_review:
  approved: mailroom_send__sos
mailroom_send__sos:
  mailed: END
END: {}
",
        )
        .unwrap();
        assert!(lawyer_review_gates_filing(&spec).is_ok());
    }

    #[test]
    fn submission_gate_holds_from_begin_when_review_precedes_mail() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: lawyer_review
lawyer_review:
  approved: mailroom_send
mailroom_send:
  mailed: END
END: {}
",
        )
        .unwrap();
        assert!(super::lawyer_review_precedes_submission(&spec).is_ok());
    }

    #[test]
    fn submission_gate_is_violated_when_begin_reaches_mail_without_review() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: mailroom_send
mailroom_send:
  mailed: END
END: {}
",
        )
        .unwrap();
        let err = super::lawyer_review_precedes_submission(&spec).unwrap_err();
        assert_eq!(err.submission_state, "mailroom_send");
    }

    #[test]
    fn every_bundled_compliance_spec_gates_submission_behind_review() {
        // Lock every shipped spec: no path from BEGIN reaches a mail /
        // filing step without crossing lawyer_review. A future edit that
        // wires a submission step in ahead of review fails here.
        for (code, yaml) in crate::specs::BUNDLED_SPEC_YAML {
            let spec = crate::workflow_spec_from_yaml(yaml)
                .unwrap_or_else(|e| panic!("spec `{code}` workflow block must parse: {e}"));
            assert!(
                super::lawyer_review_precedes_submission(&spec).is_ok(),
                "spec `{code}` lets a submission fire before lawyer_review",
            );
        }
    }

    #[test]
    fn signature_gate_holds_when_review_precedes_signing() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: lawyer_review
lawyer_review:
  approved: generate_pdf__retainer_pdf
generate_pdf__retainer_pdf:
  pdf_persisted: sent_for_signature__pending
sent_for_signature__pending:
  signature_received: END
END: {}
",
        )
        .unwrap();
        assert!(super::lawyer_review_precedes_signature(&spec).is_ok());
    }

    #[test]
    fn signature_gate_is_violated_when_begin_reaches_signing_without_review() {
        let spec = WorkflowSpec::from_yaml(
            r"
BEGIN:
  start: sent_for_signature__pending
sent_for_signature__pending:
  signature_received: END
END: {}
",
        )
        .unwrap();
        let err = super::lawyer_review_precedes_signature(&spec).unwrap_err();
        assert_eq!(err.submission_state, "sent_for_signature__pending");
    }

    #[test]
    fn every_engagement_template_gates_signature_behind_review() {
        // The mutable two-sided intake generalizes across the
        // engagement-document templates: the assembled document
        // (template body + interleaved answers + custom clauses) goes out
        // for the client's binding signature only *after* lawyer_review, so
        // the bytes the attorney approved are the bytes that get signed. A
        // custom clause or a client-entered answer therefore cannot reach
        // the bytes the attorney approved are the bytes that get signed.
        for code in [
            "onboarding__engagement_letter",
            "onboarding__engagement_letter_transcript",
        ] {
            // Product retainers (e.g. `onboarding__engagement_letter_nest`) resolve
            // through the shared product fallback, not a bundled spec — so
            // gate on the *resolved* spec via `catalog_spec_yaml`.
            let yaml = crate::catalog_spec_yaml(code)
                .unwrap_or_else(|| panic!("engagement template `{code}` has a resolvable spec"));
            let spec = crate::workflow_spec_from_yaml(yaml)
                .unwrap_or_else(|e| panic!("spec `{code}` workflow block must parse: {e}"));
            assert!(
                super::lawyer_review_precedes_signature(&spec).is_ok(),
                "engagement spec `{code}` lets a document reach signature before lawyer_review",
            );
        }
    }

    #[test]
    fn the_retainer_spec_has_no_fill_to_filing_path() {
        // The shipped retainer spec fills the retainer PDF then waits
        // for signature — it never reaches a filing state, so the gate
        // is trivially satisfied. Guards against a future edit wiring a
        // filing step in without a review.
        let spec = crate::retainer_intake_spec();
        assert!(lawyer_review_gates_filing(&spec).is_ok());
    }
}
