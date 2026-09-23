//! `/notations/{slug}`'s "Workflow" section — a client-side-only graph of
//! the template's declared `workflow:` state machine. Every state becomes a
//! node and every declared event becomes a labelled edge.
//!
//! **This is not live data.** There is no Restate invocation, no
//! `store::notation_events` row, and no real matter behind the graph shown
//! here — the same structural guarantee [`crate::notation_demo`] gives the
//! questionnaire section, for the same reason: this public page cannot
//! depend on `workflows` or `store` (`cli/tests/brand_crate_dependencies.rs`),
//! and a firm's real client activity has no business on a public marketing
//! page regardless. The topology below is computed from the template's own
//! declared graph — never read from anywhere real.
//!
//! [`WorkflowStateView`] is the plain-data mirror of
//! `views::workflow_preview::WorkflowState` that crosses the `neon` →
//! `webapp` boundary, the same pattern [`crate::notation_demo::DemoQuestion`]
//! uses for the questionnaire section.

use std::collections::{HashMap, VecDeque};

use dioxus::prelude::*;

/// One state in the declared workflow, with its own outgoing `(event, to)`
/// transitions — the plain-data mirror of
/// `views::workflow_preview::WorkflowState`.
#[derive(Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkflowStateView {
    pub name: String,
    pub transitions: Vec<(String, String)>,
}

#[derive(Debug)]
struct GraphNode {
    name: String,
    x: f64,
    y: f64,
    terminal: bool,
}

#[derive(Debug)]
struct GraphEdge {
    event: String,
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
}

#[derive(Debug)]
struct GraphLayout {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    width: f64,
    height: f64,
}

fn graph_layout(states: &[WorkflowStateView]) -> Option<GraphLayout> {
    let begin = states.iter().position(|state| state.name == "BEGIN")?;
    let by_name: HashMap<&str, usize> = states
        .iter()
        .enumerate()
        .map(|(index, state)| (state.name.as_str(), index))
        .collect();
    let mut depths = vec![usize::MAX; states.len()];
    depths[begin] = 0;
    let mut queue = VecDeque::from([begin]);
    while let Some(index) = queue.pop_front() {
        let next_depth = depths[index].saturating_add(1);
        for (_, target) in &states[index].transitions {
            if let Some(&target_index) = by_name.get(target.as_str()) {
                if depths[target_index] > next_depth {
                    depths[target_index] = next_depth;
                    queue.push_back(target_index);
                }
            }
        }
    }
    let fallback = depths
        .iter()
        .copied()
        .filter(|depth| *depth != usize::MAX)
        .max()
        .unwrap_or(0)
        + 1;
    for depth in &mut depths {
        if *depth == usize::MAX {
            *depth = fallback;
        }
    }
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    let mut levels = vec![Vec::new(); max_depth + 1];
    for (index, depth) in depths.iter().copied().enumerate() {
        levels[depth].push(index);
    }
    let max_across = levels.iter().map(Vec::len).max().unwrap_or(1);
    let width = (f64::from(u32::try_from(max_across).ok()?) * 250.0).max(240.0);
    let height = (f64::from(u32::try_from(levels.len()).ok()?) * 130.0).max(100.0);
    let mut positions = vec![(0.0, 0.0); states.len()];
    for (depth, level) in levels.iter().enumerate() {
        let level_width = f64::from(u32::try_from(level.len()).ok()?) * 250.0;
        let offset = (width - level_width) / 2.0;
        for (column, index) in level.iter().copied().enumerate() {
            positions[index] = (
                offset + f64::from(u32::try_from(column).ok()?) * 250.0 + 125.0,
                f64::from(u32::try_from(depth).ok()?) * 130.0 + 42.0,
            );
        }
    }
    let nodes = states
        .iter()
        .enumerate()
        .map(|(index, state)| GraphNode {
            name: state.name.clone(),
            x: positions[index].0,
            y: positions[index].1,
            terminal: state.is_terminal(),
        })
        .collect();
    let edges = states
        .iter()
        .enumerate()
        .flat_map(|(index, state)| {
            let positions = &positions;
            let by_name = &by_name;
            state.transitions.iter().filter_map(move |(event, target)| {
                let target_index = *by_name.get(target.as_str())?;
                Some(GraphEdge {
                    event: event.clone(),
                    from_x: positions[index].0,
                    from_y: positions[index].1 + 30.0,
                    to_x: positions[target_index].0,
                    to_y: positions[target_index].1 - 30.0,
                })
            })
        })
        .collect();
    Some(GraphLayout {
        nodes,
        edges,
        width,
        height,
    })
}

impl WorkflowStateView {
    fn is_terminal(&self) -> bool {
        self.transitions.is_empty()
    }
}

/// The client-side-only workflow definition graph. Renders nothing for a
/// notation with no declared `workflow:` block.
#[component]
pub fn WorkflowDiagram(states: Vec<WorkflowStateView>) -> Element {
    let Some(graph) = graph_layout(&states) else {
        return rsx! {};
    };
    let view_box = format!("0 0 {} {}", graph.width, graph.height);
    rsx! {
        section { class: "notation-workflow", "aria-label": "Workflow graph",
            p { class: "nav-muted",
                "Definition graph — generated directly from this notation's workflow."
            }
            svg {
                class: "notation-workflow__graph",
                view_box,
                role: "img",
                "aria-label": "Workflow states and transitions",
                defs {
                    marker {
                        id: "notation-workflow-arrow",
                        view_box: "0 0 10 10",
                        ref_x: "9",
                        ref_y: "5",
                        marker_width: "6",
                        marker_height: "6",
                        orient: "auto-start-reverse",
                        path { d: "M 0 0 L 10 5 L 0 10 z", class: "notation-workflow__arrow" }
                    }
                }
                for edge in graph.edges.iter() {
                    g { class: "notation-workflow__edge",
                        line {
                            x1: edge.from_x,
                            y1: edge.from_y,
                            x2: edge.to_x,
                            y2: edge.to_y,
                            marker_end: "url(#notation-workflow-arrow)",
                        }
                        text {
                            x: edge.from_x.midpoint(edge.to_x) + 6.0,
                            y: edge.from_y.midpoint(edge.to_y) - 6.0,
                            "{edge.event}"
                        }
                    }
                }
                for node in graph.nodes.iter() {
                    g {
                        class: if node.name == "BEGIN" { "notation-workflow__node notation-workflow__node--start" } else if node.terminal { "notation-workflow__node notation-workflow__node--end" } else { "notation-workflow__node" },
                        transform: "translate({node.x}, {node.y})",
                        rect { x: -100, y: -30, width: 200, height: 60, rx: 12 }
                        text { x: 0, y: 5, text_anchor: "middle", "{node.name}" }
                        if node.name == "BEGIN" {
                            text { class: "notation-workflow__node-role", x: 0, y: -40, text_anchor: "middle", "Start" }
                        } else if node.terminal {
                            text { class: "notation-workflow__node-role", x: 0, y: -40, text_anchor: "middle", "End" }
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
        assert!(graph_layout(&[]).is_none());
        assert!(!render(Vec::new()).contains("notation-workflow"));
    }

    #[test]
    fn graph_contains_every_definition_node_and_transition() {
        let workflow = naturalization();
        let graph = graph_layout(&workflow).expect("BEGIN makes a graph");
        assert_eq!(graph.nodes.len(), workflow.len());
        assert_eq!(
            graph.edges.len(),
            workflow
                .iter()
                .map(|state| state.transitions.len())
                .sum::<usize>()
        );
        let out = render(workflow);
        assert!(out.contains("notation-workflow__graph"), "{out}");
        assert!(out.contains("notation-workflow__node--start"), "{out}");
        assert!(out.contains("notation-workflow__node--end"), "{out}");
        assert!(out.contains("lawyer_review"), "{out}");
        assert!(out.contains("signature_declined"), "{out}");
        assert!(out.contains("generated directly"), "{out}");
    }
}
