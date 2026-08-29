//! Integration test for the inbound contract-review workflow.
//!
//! The product-neutral spec lives in
//! `workflows/specs/memo__contract_review.yaml`. The first test pins the parsed
//! state-machine shape; the second drives a notation through the happy path on
//! the in-memory runtime (upload → analysis → attorney approval → memo → END);
//! the third confirms the attorney can reject at `lawyer_review` and
//! short-circuit to END.

use uuid::Uuid;
use workflows::{
    workflow_spec_from_yaml, InMemoryRuntime, MachineKind, StateMachineRuntime, StateName,
    WorkflowSpec,
};

const SPEC: &str = include_str!("../specs/memo__contract_review.yaml");

const KIND: MachineKind = MachineKind::Workflow;
const ID1: Uuid = Uuid::from_u128(201);
const ID2: Uuid = Uuid::from_u128(202);

fn load_spec() -> WorkflowSpec {
    workflow_spec_from_yaml(SPEC).expect("memo__contract_review workflow spec must parse")
}

#[test]
fn contract_review_spec_has_expected_state_machine() {
    let spec = load_spec();

    let begin = spec
        .transitions_from(&StateName::begin())
        .expect("BEGIN state");
    assert_eq!(
        begin.lookup("contract_uploaded").map(StateName::as_str),
        Some("document_intake__inbound_contract"),
    );

    // The uploaded contract is filed into the matter by the reusable
    // document-intake step, then analysis runs.
    let intake = spec
        .transitions_from(&StateName::from("document_intake__inbound_contract"))
        .expect("document_intake__inbound_contract state");
    assert_eq!(
        intake.lookup("intake_filed").map(StateName::as_str),
        Some("analysis__contract_deviations"),
    );

    // Analysis is a System seam web drives; it advances to the attorney gate.
    let analysis = spec
        .transitions_from(&StateName::from("analysis__contract_deviations"))
        .expect("analysis__contract_deviations state");
    assert_eq!(
        analysis.lookup("analysis_ready").map(StateName::as_str),
        Some("lawyer_review"),
    );

    // The attorney gate fans out: approve renders the memo, reject ends it.
    let reviewed = spec
        .transitions_from(&StateName::from("lawyer_review"))
        .expect("lawyer_review state");
    assert_eq!(
        reviewed.lookup("approved").map(StateName::as_str),
        Some("generate_pdf__review_memo"),
    );
    assert_eq!(reviewed.lookup("rejected"), Some(&StateName::end()));

    // The memo render is the template-OUT tail of a review-IN head.
    let memo = spec
        .transitions_from(&StateName::from("generate_pdf__review_memo"))
        .expect("generate_pdf__review_memo state");
    assert_eq!(memo.lookup("memo_rendered"), Some(&StateName::end()));

    assert!(spec.is_terminal(&StateName::end()));
}

#[tokio::test]
async fn contract_review_runs_upload_to_memo_on_in_memory_runtime() {
    let spec = load_spec();
    let rt = InMemoryRuntime::new();
    StateMachineRuntime::start(&rt, KIND, ID1, &spec)
        .await
        .unwrap();

    for (condition, expected) in [
        ("contract_uploaded", "document_intake__inbound_contract"),
        ("intake_filed", "analysis__contract_deviations"),
        ("analysis_ready", "lawyer_review"),
        ("approved", "generate_pdf__review_memo"),
    ] {
        let s = StateMachineRuntime::signal(&rt, KIND, ID1, condition, None)
            .await
            .unwrap();
        assert_eq!(s.as_str(), expected, "after {condition}");
    }

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "memo_rendered", None)
        .await
        .unwrap();
    assert_eq!(s, StateName::end());

    let events = StateMachineRuntime::events(&rt, KIND, ID1).await;
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].condition, "contract_uploaded");
    assert_eq!(events.last().unwrap().condition, "memo_rendered");
}

#[tokio::test]
async fn attorney_rejection_ends_the_review_without_a_memo() {
    let spec = load_spec();
    let rt = InMemoryRuntime::new();
    StateMachineRuntime::start(&rt, KIND, ID2, &spec)
        .await
        .unwrap();
    for condition in ["contract_uploaded", "intake_filed", "analysis_ready"] {
        StateMachineRuntime::signal(&rt, KIND, ID2, condition, None)
            .await
            .unwrap();
    }
    let s = StateMachineRuntime::signal(&rt, KIND, ID2, "rejected", None)
        .await
        .unwrap();
    assert_eq!(s, StateName::end());
    let events = StateMachineRuntime::events(&rt, KIND, ID2).await;
    assert_eq!(events.last().unwrap().condition, "rejected");
}
