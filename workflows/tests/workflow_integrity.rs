//! Integrity test for every notation template under `templates/`.
//!
//! Parses the workflow + questionnaire frontmatter of every `.md`
//! file in the workspace's `templates/` tree and asserts a small set
//! of invariants that a half-finished hand-edit could easily
//! violate:
//!
//! 1. The frontmatter parses (both `workflow:` and, when declared,
//!    `questionnaire:` — the questionnaire parse alone carries its whole
//!    invariant set, since `QuestionnaireSpec::validate` enforces
//!    BEGIN/END, resolving targets, and the linear `_` chain at parse
//!    time).
//! 2. The workflow machine has both `BEGIN` and `END`.
//! 3. `END` is reachable from `BEGIN` via the transition graph.
//! 4. Every transition target appears as a state in the machine.
//! 5. Every non-`END` workflow state's prefix resolves to a known
//!    `StepKind` (no silent "unrouted" states).
//!
//! Failures point at the offending file + state so the next agent
//! to hand-author a workflow knows exactly what to fix.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use workflows::{
    questionnaire_spec_from_template, step_kind_for, workflow_spec_from_template, StateName,
    WorkflowSpec,
};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workflows crate lives one level below the workspace root")
        .join("templates")
}

fn walk_markdown(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// BFS the workflow graph from BEGIN; return every reachable state.
fn reachable_from_begin(spec: &WorkflowSpec) -> BTreeSet<&StateName> {
    let mut seen: BTreeSet<&StateName> = BTreeSet::new();
    let mut queue: VecDeque<&StateName> = VecDeque::new();
    let begin = spec
        .states
        .keys()
        .find(|s| s.as_str() == "BEGIN")
        .expect("checked elsewhere");
    queue.push_back(begin);
    seen.insert(begin);
    while let Some(state) = queue.pop_front() {
        if let Some(transitions) = spec.transitions_from(state) {
            for next in transitions.0.values() {
                if let Some(canonical) = spec.states.keys().find(|s| s.as_str() == next.as_str()) {
                    if seen.insert(canonical) {
                        queue.push_back(canonical);
                    }
                }
            }
        }
    }
    seen
}

fn check_machine_invariants(
    label: &str,
    template_path: &Path,
    states: impl Iterator<Item = String>,
    spec: &WorkflowSpec,
) {
    let state_names: BTreeSet<String> = states.collect();
    assert!(
        state_names.contains("BEGIN"),
        "{label} in {} is missing BEGIN",
        template_path.display(),
    );
    assert!(
        state_names.contains("END"),
        "{label} in {} is missing END",
        template_path.display(),
    );

    // Every transition target must be a real state.
    for (from, transitions) in &spec.states {
        for (cond, to) in &transitions.0 {
            assert!(
                state_names.contains(to.as_str()),
                "{label} in {}: transition `{from:?} --{cond}--> {to:?}` points at \
                 unknown target state `{}`",
                template_path.display(),
                to.as_str(),
            );
        }
    }

    // END must be reachable from BEGIN.
    let reachable = reachable_from_begin(spec);
    assert!(
        reachable.iter().any(|s| s.as_str() == "END"),
        "{label} in {}: END is not reachable from BEGIN (reached only: {:?})",
        template_path.display(),
        reachable.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );

    // Workflow states must resolve to a known step kind so the
    // runtime knows which actor drives each transition.
    for state in spec.states.keys() {
        if state.as_str() == "END" {
            continue;
        }
        assert!(
            step_kind_for(state).is_some(),
            "{label} in {}: state `{}` has no StepKind (prefix `{}` is unrouted — \
             add it to `workflows::step::step_kind_for` or rename the state)",
            template_path.display(),
            state.as_str(),
            state.prefix(),
        );
    }
}

#[test]
fn every_bundled_template_has_a_coherent_workflow_and_questionnaire() {
    let root = templates_root();
    let files = walk_markdown(&root);
    assert!(
        !files.is_empty(),
        "no template files found under {} — wrong path?",
        root.display(),
    );
    for path in &files {
        let markdown = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        // Skip non-notation files (templates/README.md and similar
        // prose docs that share the directory). Real notation
        // templates carry YAML frontmatter delimited by `---`.
        if !markdown.starts_with("---\n") {
            continue;
        }

        // Skip document-fragment templates: frontmatter-bearing bodies
        // that have no `workflow:` block because they are rendered as
        // part of another matter's workflow rather than driving one of
        // their own. A fragment is defined by the absence of a
        // `workflow:` block.
        if !markdown.contains("workflow:") {
            continue;
        }

        let workflow = workflow_spec_from_template(&markdown)
            .unwrap_or_else(|e| panic!("workflow in {} did not parse: {e}", path.display()));
        check_machine_invariants(
            "workflow",
            path,
            workflow.states.keys().map(|s| s.as_str().to_string()),
            &workflow,
        );

        // A questionnaire block only needs to *parse*: the strict parse
        // itself now carries the whole invariant set — BEGIN/END,
        // resolving targets, and the linear `_` chain covering every
        // state (`QuestionnaireSpec::validate`) — so parsing the corpus
        // is the guard.
        if markdown.contains("questionnaire:") {
            questionnaire_spec_from_template(&markdown).unwrap_or_else(|e| {
                panic!("questionnaire in {} did not parse: {e}", path.display())
            });
        }
    }
}
