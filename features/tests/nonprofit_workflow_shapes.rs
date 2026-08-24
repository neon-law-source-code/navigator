//! Cucumber runner for `features/nonprofit_workflow_shapes.feature`.
//!
//! Composition-lock for the nonprofit side of the template tree:
//! 501(c)(3) formation, Form 990 annual report, and Nevada
//! charitable solicitation registration. Same pattern as
//! `legal_workflow_shapes.rs` and `compliance_filings_workflow_shapes.rs`.

#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, World};
use features::template_shapes::{strip_workflow_end, templates_root, walk_chain};
use workflows::{
    questionnaire_spec_from_template, workflow_spec_from_template, WorkflowSpec, WorkflowSpecError,
};

#[derive(Default, World)]
#[world(init = Self::default)]
struct ShapeWorld {
    markdown: Option<String>,
    parse_error: Option<WorkflowSpecError>,
}

impl std::fmt::Debug for ShapeWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShapeWorld")
            .field("has_markdown", &self.markdown.is_some())
            .field("parse_error", &self.parse_error)
            .finish()
    }
}

#[given(regex = r#"^the bundled template "([^"]+)"$"#)]
async fn load_template(world: &mut ShapeWorld, relpath: String) {
    let path = templates_root().join(&relpath);
    world.markdown = Some(
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    );
    world.parse_error = None;
}

#[given(regex = r#"^the bundled template "([^"]+)" with the workflow END declaration removed$"#)]
async fn load_template_without_end(world: &mut ShapeWorld, relpath: String) {
    let path = templates_root().join(&relpath);
    let original =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    world.markdown = Some(strip_workflow_end(&original));
    world.parse_error = None;
}

#[then("the questionnaire transitions, in BEGIN-first order, are:")]
async fn assert_questionnaire_chain(world: &mut ShapeWorld, step: &Step) {
    let md = world.markdown.as_ref().expect("template loaded");
    let q = questionnaire_spec_from_template(md).expect("questionnaire frontmatter parses");
    assert_chain_matches(q.inner(), step);
}

#[then("the workflow transitions, in BEGIN-first order, are:")]
async fn assert_workflow_chain(world: &mut ShapeWorld, step: &Step) {
    let md = world.markdown.as_ref().expect("template loaded");
    let w = workflow_spec_from_template(md).expect("workflow frontmatter parses");
    assert_chain_matches(&w, step);
}

#[then("parsing the workflow spec returns a MissingEnd error")]
async fn assert_missing_end(world: &mut ShapeWorld) {
    let md = world.markdown.as_ref().expect("template loaded");
    match workflow_spec_from_template(md) {
        Err(e @ WorkflowSpecError::MissingEnd) => world.parse_error = Some(e),
        Err(other) => panic!("expected MissingEnd, got {other:?}"),
        Ok(_) => panic!("expected parse failure but the spec parsed cleanly"),
    }
}

fn assert_chain_matches(spec: &WorkflowSpec, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let expected: Vec<(&str, &str)> = table
        .rows
        .iter()
        .skip(1)
        .map(|row| {
            (
                row.first().expect("from cell").as_str(),
                row.get(1).expect("to cell").as_str(),
            )
        })
        .collect();
    let chain = walk_chain(spec);
    let actual: Vec<(&str, &str)> = chain
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();
    assert_eq!(actual, expected, "transition chain mismatch");
}

#[tokio::main]
async fn main() {
    ShapeWorld::cucumber()
        .run_and_exit("tests/features/nonprofit_workflow_shapes.feature")
        .await;
}
