//! `N121` — a `sent_for_signature` must be preceded by a `generate_pdf`.
//!
//! There is nothing to send for signature until something has rendered a
//! PDF: `sent_for_signature__*` is the e-signature send
//! ([`workflows::step::STEP_PREFIXES`] — `StepKind::SentForSignature`), and
//! the only step that produces the bytes it sends is `generate_pdf__*`
//! (`StepKind::GeneratePdf`, `rules::workflow_steps::WORKFLOW_STEPS`).
//! On **every path from `BEGIN`**, a `generate_pdf` state must be reached
//! before any `sent_for_signature` state.
//!
//! This mirrors `N116`'s shape (a graph-reachability rule keyed to a
//! notation's `workflow:` block) but is a different claim: `N116` is about
//! attorney review gating an *outbound* act; this rule is about a
//! *sequencing* dependency between two `Implemented` steps — the render has
//! to exist before the send can have anything to send. Neither rule
//! substitutes for the other, so a workflow can violate one without the
//! other: `generate_pdf → lawyer_review → sent_for_signature` satisfies
//! this rule and has nothing for `N116` to check (`sent_for_signature` is
//! not an outbound-submission prefix); `sent_for_signature` reached with no
//! preceding `generate_pdf` violates only this rule.
//!
//! Files without frontmatter or without a `workflow:` key are silently
//! skipped, like the other workflow-shape rules.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{line_byte_range, Rule, SourceFile, Violation};

pub struct F121GeneratePdfPrecedesSignature;

impl F121GeneratePdfPrecedesSignature {
    pub const CODE: &'static str = "N121";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// The bare step prefix (the part before a `__discriminator`).
fn prefix_of(state: &str) -> &str {
    state.split_once("__").map_or(state, |(p, _)| p)
}

/// The e-signature send this rule requires a render ahead of.
fn is_signature_send(state: &str) -> bool {
    prefix_of(state) == "sent_for_signature"
}

/// The render step that produces the bytes a signature send needs.
fn is_generate_pdf(state: &str) -> bool {
    prefix_of(state) == "generate_pdf"
}

impl Rule for F121GeneratePdfPrecedesSignature {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = crate::frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let Ok(parsed) = serde_yaml::from_str::<FrontmatterShape>(fm) else {
            return Vec::new();
        };
        let Some(workflow) = parsed.workflow else {
            return Vec::new();
        };

        ungated_signature_sends(&workflow)
            .into_iter()
            .map(|state| {
                let line = workflow_state_line(&file.contents, &state);
                Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line,
                    range: line_byte_range(&file.contents, line),
                    message: format!(
                        "`{state}` is reachable from BEGIN without a preceding `generate_pdf` — \
                         there is nothing to send for signature until a step has rendered it"
                    ),
                }
            })
            .collect()
    }
}

/// 1-based line of the indented `workflow:` state key `state`, so the
/// squiggle lands on the offending step rather than the frontmatter
/// delimiter. Falls back to line 1 when the key can't be located.
fn workflow_state_line(contents: &str, state: &str) -> usize {
    let key = format!("{state}:");
    for (idx, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.len() < raw.len() && trimmed == key {
            return idx + 1;
        }
    }
    1
}

/// Every `sent_for_signature` state reachable from `BEGIN` without a
/// `generate_pdf` on the path before it. A depth-first graph walk over
/// `(state, seen_render)` pairs (the `Vec` stack is LIFO) — the render
/// flag is part of the visit key so a state reached both with and without
/// a prior render is explored under both, and a cycle can't loop forever.
fn ungated_signature_sends(
    workflow: &BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeSet<String> {
    let mut offending = BTreeSet::new();
    let mut visited: BTreeSet<(String, bool)> = BTreeSet::new();
    let mut queue: Vec<(String, bool)> = vec![("BEGIN".to_string(), false)];
    while let Some((node, seen_render)) = queue.pop() {
        if !visited.insert((node.clone(), seen_render)) {
            continue;
        }
        if !seen_render && node != "BEGIN" && is_signature_send(&node) {
            offending.insert(node.clone());
        }
        let downstream_seen = seen_render || is_generate_pdf(&node);
        if let Some(transitions) = workflow.get(&node) {
            for target in transitions.values() {
                queue.push((target.clone(), downstream_seen));
            }
        }
    }
    offending
}

#[cfg(test)]
mod tests {
    use super::F121GeneratePdfPrecedesSignature;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    #[test]
    fn flags_signature_send_reached_before_a_render() {
        let body = "---
workflow:
  BEGIN:
    intake_submitted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        let v = F121GeneratePdfPrecedesSignature.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N121");
        assert!(v[0].message.contains("sent_for_signature__pending"));
    }

    #[test]
    fn passes_when_a_render_precedes_the_signature_send() {
        let body = "---
workflow:
  BEGIN:
    intake_submitted: generate_pdf__retainer_pdf
  generate_pdf__retainer_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        assert!(F121GeneratePdfPrecedesSignature
            .lint(&file(body))
            .is_empty());
    }

    #[test]
    fn formation_shape_passes_signature_after_render_and_review() {
        // The shipped NV formation shape: render, review, sign, file.
        let body = "---
workflow:
  BEGIN:
    intake_submitted: intake_persisted__organizer
  intake_persisted__organizer:
    articles_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__articles_pdf
    rejected: END
  generate_pdf__articles_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: filing__nv_sos
    signature_declined: END
  filing__nv_sos:
    filed: END
  END: {}
---
";
        assert!(F121GeneratePdfPrecedesSignature
            .lint(&file(body))
            .is_empty());
    }

    #[test]
    fn review_and_submission_alone_are_not_a_substitute_for_a_render() {
        // N116 is satisfied by review gating the outbound act; N121 is a
        // distinct claim and must still flag the missing render.
        let body = "---
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: sent_for_signature__pending
    rejected: END
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        let v = F121GeneratePdfPrecedesSignature.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("sent_for_signature__pending"));
    }

    #[test]
    fn generate_pdf_with_no_signature_send_is_not_flagged() {
        // The mirror gap N116 documents: a render with no signature step at
        // all is not this rule's concern either.
        let body = "---
workflow:
  BEGIN:
    created: generate_pdf__memo_pdf
  generate_pdf__memo_pdf:
    pdf_persisted: END
  END: {}
---
";
        assert!(F121GeneratePdfPrecedesSignature
            .lint(&file(body))
            .is_empty());
    }

    #[test]
    fn flags_branch_that_bypasses_the_render() {
        // One branch renders then sends (ok); the other sends directly
        // (violation). The whole workflow must fail.
        let body = "---
workflow:
  BEGIN:
    slow: generate_pdf__form
    fast: sent_for_signature__urgent
  generate_pdf__form:
    rendered: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  sent_for_signature__urgent:
    signature_received: END
  END: {}
---
";
        let v = F121GeneratePdfPrecedesSignature.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("sent_for_signature__urgent"));
    }

    #[test]
    fn cycle_does_not_hang_and_still_flags() {
        let body = "---
workflow:
  BEGIN:
    a: loop
  loop:
    back: BEGIN
    out: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        let v = F121GeneratePdfPrecedesSignature.lint(&file(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("sent_for_signature__pending"));
    }

    #[test]
    fn no_frontmatter_or_workflow_means_no_violation() {
        assert!(F121GeneratePdfPrecedesSignature
            .lint(&file("just body"))
            .is_empty());
        let no_wf = "---\ntitle: T\n---\nbody\n";
        assert!(F121GeneratePdfPrecedesSignature
            .lint(&file(no_wf))
            .is_empty());
    }

    #[test]
    fn violation_points_at_the_offending_state_line() {
        let body = "---
workflow:
  BEGIN:
    go: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
  END: {}
---
";
        let v = F121GeneratePdfPrecedesSignature.lint(&file(body));
        assert_eq!(v.len(), 1);
        // `sent_for_signature__pending:` is line 5 of the body.
        assert_eq!(v[0].line, 5, "{v:?}");
    }

    #[test]
    fn is_error_severity() {
        use crate::{severity_for_code, Severity};
        assert_eq!(severity_for_code("N121"), Severity::Error);
    }
}
