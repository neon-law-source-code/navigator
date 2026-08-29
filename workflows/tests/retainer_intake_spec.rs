//! Integration test for the retainer-intake workflow.
//!
//! The spec lives in the frontmatter of
//! `templates/neon_law/shared/onboarding_letter.md` so both the template and this
//! test fail together if either drifts. The first test asserts the
//! parsed state-machine shape; the second drives a notation through
//! every transition on the in-memory runtime to confirm it reaches
//! END with the right event log.

use uuid::Uuid;
use workflows::{
    workflow_spec_from_template, InMemoryRuntime, MachineKind, StateMachineRuntime, StateName,
    WorkflowSpec,
};

const TEMPLATE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../templates/neon_law/shared/onboarding_letter.md",
);

const KIND: MachineKind = MachineKind::Workflow;
const ID1: Uuid = Uuid::from_u128(1);
const ID2: Uuid = Uuid::from_u128(2);

fn load_spec() -> WorkflowSpec {
    let markdown = std::fs::read_to_string(TEMPLATE_PATH)
        .unwrap_or_else(|e| panic!("read {TEMPLATE_PATH}: {e}"));
    workflow_spec_from_template(&markdown).expect("retainer.md workflow frontmatter must parse")
}

#[test]
fn retainer_intake_spec_has_expected_state_machine() {
    let spec = load_spec();

    let begin = spec
        .transitions_from(&StateName::begin())
        .expect("BEGIN state");
    assert_eq!(
        begin.lookup("intake_submitted").map(StateName::as_str),
        Some("intake_persisted__client"),
    );

    let persisted = spec
        .transitions_from(&StateName::from("intake_persisted__client"))
        .expect("intake_persisted__client state");
    assert_eq!(
        persisted.lookup("retainer_rendered").map(StateName::as_str),
        Some("lawyer_review"),
    );

    let reviewed = spec
        .transitions_from(&StateName::from("lawyer_review"))
        .expect("lawyer_review state");
    assert_eq!(
        reviewed.lookup("approved").map(StateName::as_str),
        Some("generate_pdf__retainer_pdf"),
    );
    assert_eq!(reviewed.lookup("rejected"), Some(&StateName::end()));

    let opened = spec
        .transitions_from(&StateName::from("generate_pdf__retainer_pdf"))
        .expect("generate_pdf__retainer_pdf state");
    assert_eq!(
        opened.lookup("pdf_persisted").map(StateName::as_str),
        Some("sent_for_signature__pending"),
    );

    let sent = spec
        .transitions_from(&StateName::from("sent_for_signature__pending"))
        .expect("sent_for_signature__pending state");
    assert_eq!(sent.lookup("signature_received"), Some(&StateName::end()),);

    assert!(spec.is_terminal(&StateName::end()));
}

#[tokio::test]
async fn retainer_intake_workflow_runs_to_end_on_in_memory_runtime() {
    let spec = load_spec();
    let rt = InMemoryRuntime::new();

    StateMachineRuntime::start(&rt, KIND, ID1, &spec)
        .await
        .unwrap();
    assert_eq!(
        StateMachineRuntime::current_state(&rt, KIND, ID1).await,
        Some(StateName::begin())
    );

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "intake_submitted", None)
        .await
        .unwrap();
    assert_eq!(s.as_str(), "intake_persisted__client");

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "retainer_rendered", None)
        .await
        .unwrap();
    assert_eq!(s.as_str(), "lawyer_review");

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "approved", None)
        .await
        .unwrap();
    assert_eq!(s.as_str(), "generate_pdf__retainer_pdf");

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "pdf_persisted", None)
        .await
        .unwrap();
    assert_eq!(s.as_str(), "sent_for_signature__pending");

    let s = StateMachineRuntime::signal(&rt, KIND, ID1, "signature_received", None)
        .await
        .unwrap();
    assert_eq!(s, StateName::end());

    let events = StateMachineRuntime::events(&rt, KIND, ID1).await;
    assert_eq!(events.len(), 5);
    assert_eq!(events[0].condition, "intake_submitted");
    assert_eq!(events[4].condition, "signature_received");
}

#[tokio::test]
async fn rejected_branch_short_circuits_to_end_without_signing() {
    let spec = load_spec();
    let rt = InMemoryRuntime::new();
    StateMachineRuntime::start(&rt, KIND, ID2, &spec)
        .await
        .unwrap();
    StateMachineRuntime::signal(&rt, KIND, ID2, "intake_submitted", None)
        .await
        .unwrap();
    StateMachineRuntime::signal(&rt, KIND, ID2, "retainer_rendered", None)
        .await
        .unwrap();
    let s = StateMachineRuntime::signal(&rt, KIND, ID2, "rejected", None)
        .await
        .unwrap();
    assert_eq!(s, StateName::end());
    let events = StateMachineRuntime::events(&rt, KIND, ID2).await;
    assert_eq!(events.last().unwrap().condition, "rejected");
}
