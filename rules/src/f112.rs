//! `N112` — a workflow step is allowed but its automation is deferred.
//!
//! This is an *advisory* ([`crate::Severity::Warning`], yellow in the
//! editor), not a blocker: the step is a legitimate member of the
//! workflow-step catalog ([`crate::workflow_steps::WORKFLOW_STEPS`]),
//! but the real side effect behind it is stubbed out
//! ([`crate::workflow_steps::StepStatus::Scaffolded`]), so a notation
//! that reaches it advances no further than the stub records. The
//! companion red error is `N104` (a step that isn't in the catalog at
//! all).
//!
//! The status comes from the catalog itself, not a second hand-kept
//! list: as a step's automation lands, its catalog entry moves off
//! `Scaffolded` and the yellow squiggle disappears with it — there is
//! nothing here to update. A step the catalog marks
//! [`StepStatus::Human`] (`lawyer_review`, `client_review`, `reask`,
//! `notarization`, a `_signature` state, `witnesses`) never earns this
//! advisory: pausing for a human decision is the step's whole job, not
//! unbuilt automation, and the catalog's own summary says so.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::workflow_steps::{self, StepStatus};
use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct F112WorkflowStepNotBuilt;

impl F112WorkflowStepNotBuilt {
    pub const CODE: &'static str = "N112";
}

#[derive(Debug, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// True when `prefix` names a catalog step whose real side effect is
/// still deferred behind a stub ([`StepStatus::Scaffolded`]). A step the
/// catalog does not know, or knows as `Implemented`, `Seam`, or `Human`,
/// is not "not built" in the sense this advisory means.
#[must_use]
pub fn workflow_step_not_built(prefix: &str) -> bool {
    matches!(
        workflow_steps::lookup(prefix).map(|step| step.status),
        Some(StepStatus::Scaffolded)
    )
}

impl Rule for F112WorkflowStepNotBuilt {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        "Workflow step is allowed but its automation is not built yet"
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let Some(fm) = frontmatter::extract(&file.contents) else {
            return Vec::new();
        };
        let Ok(parsed) = serde_yaml::from_str::<FrontmatterShape>(fm) else {
            return Vec::new();
        };
        let Some(workflow) = parsed.workflow else {
            return Vec::new();
        };

        let mut violations = Vec::new();
        for state in workflow.keys() {
            if state == "BEGIN" || state == "END" {
                continue;
            }
            let prefix = state.split_once("__").map_or(state.as_str(), |(p, _)| p);
            if workflow_step_not_built(prefix) {
                let line = workflow_state_line(&file.contents, state);
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line,
                    range: line_byte_range(&file.contents, line),
                    message: format!(
                        "workflow step `{prefix}` is allowed but its automation is not built yet \
                         (from state `{state}`)"
                    ),
                });
            }
        }
        violations
    }
}

/// 1-based line of the indented `workflow:` state key `state` in the raw
/// source, so the squiggle lands on the step itself rather than the
/// frontmatter delimiter. Falls back to line 1 if the key can't be
/// located (e.g. an unusual indentation the parser still accepted).
fn workflow_state_line(contents: &str, state: &str) -> usize {
    let key = format!("{state}:");
    for (idx, raw) in contents.lines().enumerate() {
        // A state key is nested (indented) under `workflow:`, never at
        // column zero — that guard avoids matching a same-named line
        // that isn't a mapping key.
        let trimmed = raw.trim_start();
        if trimmed.len() < raw.len() && trimmed == key {
            return idx + 1;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::F112WorkflowStepNotBuilt;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(body: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: body.to_string(),
        }
    }

    const LAWYER_REVIEW_WORKFLOW: &str = "---
title: T
workflow:
  BEGIN:
    intake_submitted: lawyer_review
  lawyer_review:
    approved: END
    rejected: END
  END: {}
---
";

    const ONCHAIN_WORKFLOW: &str = "---
title: T
workflow:
  BEGIN:
    attested: onchain
  onchain:
    recorded: END
  END: {}
---
";

    /// `lawyer_review` is a mandatory human gate
    /// ([`crate::workflow_steps::StepStatus::Human`]), not deferred
    /// automation, so the smallest legitimate workflow a notation can
    /// carry must not greet its author with a warning.
    #[test]
    fn does_not_warn_on_a_lawyer_review_state() {
        assert!(F112WorkflowStepNotBuilt
            .lint(&file(LAWYER_REVIEW_WORKFLOW))
            .is_empty());
    }

    #[test]
    fn does_not_warn_on_a_discriminated_lawyer_review_state() {
        let body = "---
title: T
workflow:
  BEGIN:
    created: lawyer_review__for_grantor
  lawyer_review__for_grantor:
    approved: END
  END: {}
---
";
        assert!(F112WorkflowStepNotBuilt.lint(&file(body)).is_empty());
    }

    #[test]
    fn warns_on_a_scaffolded_state() {
        let v = F112WorkflowStepNotBuilt.lint(&file(ONCHAIN_WORKFLOW));
        assert_eq!(v.len(), 1, "exactly one not-built advisory, got {v:?}");
        assert_eq!(v[0].code, "N112");
        assert!(v[0].message.contains("onchain"));
        assert!(v[0].message.contains("not built"));
    }

    #[test]
    fn advisory_points_at_the_scaffolded_step_line_not_line_one() {
        let v = F112WorkflowStepNotBuilt.lint(&file(ONCHAIN_WORKFLOW));
        // `onchain:` is the 6th line of the body above.
        assert_eq!(v[0].line, 6, "squiggle should land on the step, got {v:?}");
    }

    #[test]
    fn does_not_warn_on_built_steps() {
        // generate_pdf / sent_for_signature are implemented steps: no
        // advisory.
        let body = "---
title: T
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
        assert!(F112WorkflowStepNotBuilt.lint(&file(body)).is_empty());
    }

    #[test]
    fn warns_on_a_discriminated_scaffolded_state() {
        let body = "---
title: T
workflow:
  BEGIN:
    attested: onchain__trust_deed
  onchain__trust_deed:
    recorded: END
  END: {}
---
";
        let v = F112WorkflowStepNotBuilt.lint(&file(body));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("onchain__trust_deed"));
    }

    #[test]
    fn no_workflow_means_no_advisory() {
        assert!(F112WorkflowStepNotBuilt.lint(&file("just body")).is_empty());
        let only_q = "---\nquestionnaire:\n  BEGIN:\n    a: END\n  END: {}\n---\n";
        assert!(F112WorkflowStepNotBuilt.lint(&file(only_q)).is_empty());
    }

    #[test]
    fn advisory_is_warning_severity() {
        use crate::{severity_for_code, Severity};
        assert_eq!(severity_for_code("N112"), Severity::Warning);
    }
}
