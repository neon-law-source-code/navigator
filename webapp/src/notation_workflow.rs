//! `/notations/{slug}`'s "Workflow" section — an Airflow-Grid-flavored,
//! client-side-only sample of the template's declared `workflow:` state
//! machine: a handful of illustrative runs, each a row of per-state chips a
//! reader can open to see a fabricated log for that step.
//!
//! **This is not live data.** There is no Restate invocation, no
//! `store::notation_events` row, and no real matter behind any run shown
//! here — the same structural guarantee [`crate::notation_demo`] gives the
//! questionnaire section, for the same reason: this public page cannot
//! depend on `workflows` or `store` (`cli/tests/brand_crate_dependencies.rs`),
//! and a firm's real client activity has no business on a public marketing
//! page regardless. Every run and every log line below is *computed* from
//! the template's own declared graph — never read from anywhere real.
//!
//! [`WorkflowStateView`] is the plain-data mirror of
//! `views::workflow_preview::WorkflowState` that crosses the `neon` →
//! `webapp` boundary, the same pattern [`crate::notation_demo::DemoQuestion`]
//! uses for the questionnaire section.

use std::collections::HashSet;

use dioxus::prelude::*;

/// One state in the declared workflow, with its own outgoing `(event, to)`
/// transitions — the plain-data mirror of
/// `views::workflow_preview::WorkflowState`.
#[derive(Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkflowStateView {
    pub name: String,
    pub transitions: Vec<(String, String)>,
}

impl WorkflowStateView {
    fn is_terminal(&self) -> bool {
        self.transitions.is_empty()
    }
}

fn find<'a>(states: &'a [WorkflowStateView], name: &str) -> Option<&'a WorkflowStateView> {
    states.iter().find(|s| s.name == name)
}

/// Walk from `start`, always taking the `branch`-th declared transition out
/// of `start` itself and the first (`0`th) transition out of every state
/// after that, until a terminal state or a repeated state (cycle guard).
/// Returns the full path, `start` included.
fn walk(states: &[WorkflowStateView], start: &str, branch: usize) -> Vec<String> {
    let mut path = vec![start.to_string()];
    let mut visited: HashSet<String> = path.iter().cloned().collect();
    let mut current = start.to_string();
    let mut first_hop = true;
    while let Some(state) = find(states, &current) {
        let chosen = if first_hop {
            state.transitions.get(branch)
        } else {
            state.transitions.first()
        };
        first_hop = false;
        let Some((_, next)) = chosen else { break };
        if !visited.insert(next.clone()) {
            break;
        }
        path.push(next.clone());
        current = next.clone();
        if find(states, &current).is_some_and(WorkflowStateView::is_terminal) {
            break;
        }
    }
    path
}

/// The first state along `path` (searched from the end) with more than one
/// declared transition — where a second, untaken outcome exists.
fn first_branch_point<'a>(states: &'a [WorkflowStateView], path: &[String]) -> Option<&'a str> {
    path.iter().find_map(|name| {
        find(states, name)
            .filter(|s| s.transitions.len() > 1)
            .map(|s| s.name.as_str())
    })
}

/// Whether `path` ends on a terminal state (the run is over) rather than
/// stopping mid-flight.
fn reached_terminal(states: &[WorkflowStateView], path: &[String]) -> bool {
    path.last()
        .and_then(|name| find(states, name))
        .is_some_and(WorkflowStateView::is_terminal)
}

/// A task's status on one sample run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TaskStatus {
    Success,
    Running,
    Pending,
    Skipped,
}

impl TaskStatus {
    fn class_name(self) -> &'static str {
        match self {
            TaskStatus::Success => "success",
            TaskStatus::Running => "running",
            TaskStatus::Pending => "pending",
            TaskStatus::Skipped => "skipped",
        }
    }
}

/// One task chip on one sample run.
struct TaskCell {
    name: String,
    status: TaskStatus,
    log_lines: Vec<String>,
}

/// One illustrative sample run: a label, an outcome, and one [`TaskCell`] per
/// task the template declares.
struct SampleRun {
    label: String,
    outcome: &'static str,
    cells: Vec<TaskCell>,
}

/// Every state that represents work done, rather than the run's start or its
/// end — a terminal state (zero outgoing transitions, whatever it is named)
/// marks that the run is over rather than describing a step performed.
fn tasks(states: &[WorkflowStateView]) -> Vec<&WorkflowStateView> {
    states
        .iter()
        .filter(|s| s.name != "BEGIN" && !s.is_terminal())
        .collect()
}

/// The transition, if any, that led into `state_name` from `prev`.
fn edge_into<'a>(states: &'a [WorkflowStateView], prev: &str, state_name: &str) -> Option<&'a str> {
    find(states, prev)?
        .transitions
        .iter()
        .find(|(_, to)| to == state_name)
        .map(|(event, _)| event.as_str())
}

/// The log line describing how a run arrived at `path[index]` — the event
/// declared on the transition out of the previous step, or the entry line
/// for the very first task after `BEGIN`.
fn entry_line(states: &[WorkflowStateView], path: &[String], index: usize) -> String {
    index
        .checked_sub(1)
        .and_then(|prev_index| {
            edge_into(states, &path[prev_index], &path[index])
                .map(|event| format!("Received event `{event}` from `{}`.", path[prev_index]))
        })
        .unwrap_or_else(|| "Entered from BEGIN.".to_string())
}

/// Build one [`TaskCell`] for `task` given the run's `path` and whether that
/// path is still reachable further along the primary path (`remaining`, used
/// only for the in-progress run's not-yet-reached tasks).
fn cell_for(
    states: &[WorkflowStateView],
    task: &WorkflowStateView,
    path: &[String],
    remaining: &HashSet<&str>,
) -> TaskCell {
    let position = path.iter().position(|name| name == &task.name);
    let (task_status, log_lines) = match position {
        Some(index) if index + 1 < path.len() => {
            let entry = entry_line(states, path, index);
            let next = &path[index + 1];
            let event = edge_into(states, &task.name, next).unwrap_or("_");
            (
                TaskStatus::Success,
                vec![
                    entry,
                    format!("Event `{event}` — transitioned to `{next}`."),
                ],
            )
        }
        Some(index) if reached_terminal(states, path) => (
            TaskStatus::Success,
            vec![entry_line(states, path, index), "Run complete.".to_string()],
        ),
        Some(index) => (
            TaskStatus::Running,
            vec![
                entry_line(states, path, index),
                "Awaiting its next event.".to_string(),
            ],
        ),
        None if remaining.contains(task.name.as_str()) => (
            TaskStatus::Pending,
            vec!["Not yet reached — still ahead on this run.".to_string()],
        ),
        None => (
            TaskStatus::Skipped,
            vec!["Not reached — this run took a different branch.".to_string()],
        ),
    };
    TaskCell {
        name: task.name.clone(),
        status: task_status,
        log_lines,
    }
}

fn build_run(
    states: &[WorkflowStateView],
    all_tasks: &[&WorkflowStateView],
    label: &str,
    outcome: &'static str,
    path: &[String],
    remaining: &HashSet<&str>,
) -> SampleRun {
    SampleRun {
        label: label.to_string(),
        outcome,
        cells: all_tasks
            .iter()
            .map(|task| cell_for(states, task, path, remaining))
            .collect(),
    }
}

/// Every sample run to show for this workflow, computed entirely from its
/// declared graph. Empty for a template with no `workflow:` block (or one
/// `views::workflow_preview::parse` couldn't read) — [`WorkflowDiagram`]
/// renders no section at all in that case.
fn sample_runs(states: &[WorkflowStateView]) -> Vec<SampleRun> {
    let Some(begin) = find(states, "BEGIN") else {
        return Vec::new();
    };
    if begin.transitions.is_empty() {
        return Vec::new();
    }
    let all_tasks = tasks(states);
    let happy_path = walk(states, "BEGIN", 0);
    let empty: HashSet<&str> = HashSet::new();

    let mut runs = vec![build_run(
        states,
        &all_tasks,
        "Sample run 1",
        "Completed",
        &happy_path,
        &empty,
    )];

    if let Some(branch_state) = first_branch_point(states, &happy_path) {
        let branch_from = happy_path
            .iter()
            .position(|name| name == branch_state)
            .unwrap_or(0);
        let mut alternate = happy_path[..=branch_from].to_vec();
        alternate.extend(walk(states, branch_state, 1).into_iter().skip(1));
        if alternate.len() < happy_path.len() && reached_terminal(states, &alternate) {
            runs.push(build_run(
                states,
                &all_tasks,
                "Sample run 2",
                "Ended early",
                &alternate,
                &empty,
            ));
        }
    }

    if happy_path.len() > 2 {
        let cut = (happy_path.len() / 2).max(1).min(happy_path.len() - 1);
        let in_progress = happy_path[..=cut].to_vec();
        let remaining: HashSet<&str> = happy_path[cut + 1..].iter().map(String::as_str).collect();
        runs.push(build_run(
            states,
            &all_tasks,
            "Sample run 3",
            "In progress",
            &in_progress,
            &remaining,
        ));
    }

    runs
}

/// The CSS modifier for a run's outcome badge. A plain lookup over the fixed
/// set [`sample_runs`] produces, rather than a generic slugify, since that
/// set is closed and known here.
fn outcome_class(outcome: &str) -> &'static str {
    match outcome {
        "Completed" => "completed",
        "Ended early" => "ended-early",
        "In progress" => "in-progress",
        _ => "unknown",
    }
}

/// The client-side-only sample workflow diagram. Renders nothing for a
/// notation with no declared `workflow:` block.
#[component]
pub fn WorkflowDiagram(states: Vec<WorkflowStateView>) -> Element {
    let runs = sample_runs(&states);
    if runs.is_empty() {
        return rsx! {};
    }
    rsx! {
        section { class: "notation-workflow", "aria-label": "Sample workflow runs",
            p { class: "nav-muted",
                "Sample runs — illustrative only. This notation's real activity is never shown on a public page."
            }
            for run in runs.iter() {
                div { class: "notation-workflow__run", key: "{run.label}",
                    div { class: "notation-workflow__run-header",
                        span { class: "notation-workflow__run-label", "{run.label}" }
                        span {
                            class: "notation-workflow__run-outcome notation-workflow__run-outcome--{outcome_class(run.outcome)}",
                            "{run.outcome}"
                        }
                    }
                    div { class: "notation-workflow__tasks",
                        for cell in run.cells.iter() {
                            details {
                                class: "notation-workflow__task notation-workflow__task--{cell.status.class_name()}",
                                key: "{cell.name}",
                                summary { class: "notation-workflow__task-toggle", "{cell.name}" }
                                div { class: "notation-workflow__task-log",
                                    for line in cell.log_lines.iter() {
                                        p { "{line}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, transitions: &[(&str, &str)]) -> WorkflowStateView {
        WorkflowStateView {
            name: name.to_string(),
            transitions: transitions
                .iter()
                .map(|(e, t)| (e.to_string(), t.to_string()))
                .collect(),
        }
    }

    /// The naturalization workflow, in the shape
    /// `views::workflow_preview::parse` would hand back.
    fn naturalization() -> Vec<WorkflowStateView> {
        vec![
            state(
                "BEGIN",
                &[("intake_submitted", "intake_persisted__applicant")],
            ),
            state(
                "intake_persisted__applicant",
                &[("application_rendered", "lawyer_review")],
            ),
            state(
                "lawyer_review",
                &[
                    ("approved", "generate_pdf__n400_summary"),
                    ("rejected", "END"),
                ],
            ),
            state(
                "generate_pdf__n400_summary",
                &[("pdf_persisted", "sent_for_signature__pending")],
            ),
            state(
                "sent_for_signature__pending",
                &[
                    ("signature_received", "e_filing__uscis"),
                    ("signature_declined", "END"),
                ],
            ),
            state("e_filing__uscis", &[("filed", "END")]),
            state("END", &[]),
        ]
    }

    fn render(states: Vec<WorkflowStateView>) -> String {
        let mut dom = VirtualDom::new_with_props(WorkflowDiagram, WorkflowDiagramProps { states });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn a_notation_with_no_workflow_renders_nothing() {
        assert_eq!(sample_runs(&[]).len(), 0);
        assert!(!render(Vec::new()).contains("notation-workflow"));
    }

    #[test]
    fn a_begin_with_no_transitions_renders_nothing() {
        assert!(sample_runs(&[state("BEGIN", &[])]).is_empty());
    }

    #[test]
    fn the_happy_path_run_walks_every_first_choice_to_a_terminal_state() {
        let runs = sample_runs(&naturalization());
        let completed = &runs[0];
        assert_eq!(completed.label, "Sample run 1");
        assert_eq!(completed.outcome, "Completed");
        let names: Vec<&str> = completed.cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "intake_persisted__applicant",
                "lawyer_review",
                "generate_pdf__n400_summary",
                "sent_for_signature__pending",
                "e_filing__uscis",
            ],
            "END and BEGIN are not tasks: {names:?}"
        );
        assert!(
            completed
                .cells
                .iter()
                .all(|c| c.status == TaskStatus::Success),
            "every task on the happy path succeeded"
        );
    }

    #[test]
    fn a_branch_run_marks_the_untaken_branchs_tasks_as_skipped() {
        let runs = sample_runs(&naturalization());
        let ended_early = runs
            .iter()
            .find(|r| r.outcome == "Ended early")
            .expect("lawyer_review branches, so an early-ending run exists");
        let lawyer_review = ended_early
            .cells
            .iter()
            .find(|c| c.name == "lawyer_review")
            .expect("present");
        assert_eq!(lawyer_review.status, TaskStatus::Success);
        let generate_pdf = ended_early
            .cells
            .iter()
            .find(|c| c.name == "generate_pdf__n400_summary")
            .expect("present");
        assert_eq!(
            generate_pdf.status,
            TaskStatus::Skipped,
            "the rejected branch never reaches PDF generation"
        );
    }

    #[test]
    fn an_in_progress_run_marks_its_current_task_running_and_the_rest_pending() {
        let runs = sample_runs(&naturalization());
        let in_progress = runs
            .iter()
            .find(|r| r.outcome == "In progress")
            .expect("a long enough chain yields an in-progress run");
        assert_eq!(
            in_progress
                .cells
                .iter()
                .filter(|c| c.status == TaskStatus::Running)
                .count(),
            1,
            "exactly one task is currently running"
        );
        assert!(
            in_progress
                .cells
                .iter()
                .any(|c| c.status == TaskStatus::Pending),
            "a task still ahead on the happy path is pending, not skipped"
        );
    }

    #[test]
    fn a_purely_linear_workflow_yields_no_branch_run() {
        let linear = vec![
            state("BEGIN", &[("go", "only_step")]),
            state("only_step", &[("done", "END")]),
            state("END", &[]),
        ];
        let runs = sample_runs(&linear);
        assert!(!runs.iter().any(|r| r.outcome == "Ended early"));
    }

    #[test]
    fn a_cycle_does_not_hang_the_walk() {
        let cyclic = vec![
            state("BEGIN", &[("go", "a")]),
            state("a", &[("go", "b")]),
            state("b", &[("go", "a")]),
        ];
        let runs = sample_runs(&cyclic);
        assert!(!runs.is_empty(), "still produces a run rather than hanging");
    }

    #[test]
    fn renders_every_run_as_openable_task_chips() {
        let out = render(naturalization());
        assert!(out.contains("notation-workflow"), "{out}");
        assert!(out.contains("Sample run 1"), "{out}");
        assert!(out.contains("<details"), "openable chips: {out}");
        assert!(out.contains("lawyer_review"), "{out}");
        assert!(
            out.contains("illustrative only"),
            "the demo names itself as illustrative: {out}"
        );
    }
}
