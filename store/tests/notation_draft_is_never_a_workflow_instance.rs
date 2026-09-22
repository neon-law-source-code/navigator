//! LAW-29: a notation draft is a stored, addressable preview — **not** an
//! executed instrument. Creating one must never create a
//! [`store::notations::Notation`] row, journal a
//! [`store::notation_events`] event, or start a workflow instance.

use store::notation_drafts::{create, find_live, NewNotationDraft};
use store::surreal::test_support::mem;

const TEMPLATE_SOURCE: &str = "---\ntitle: Sample Letter\ncode: sample__letter\nquestionnaire:\n  \
    BEGIN:\n    _: custom_text__client_name\n  \
    custom_text__client_name:\n    _: END\n  END: \
    {}\nprompts:\n  client_name: What \
    is your name?\nworkflow:\n  BEGIN:\n    intake_submitted: \
    lawyer_review\n  lawyer_review:\n    approved: END\n  END: \
    {}\n---\n\n# Sample Letter\n\nBody prose.\n";

#[tokio::test]
async fn creating_a_draft_creates_no_notation_row_and_starts_no_workflow_instance() {
    let db = &mem().await;
    let project_id = store::test_support::seed_project_surreal(db, "draft-project").await;

    assert!(
        !store::notations::exists_for_project(db, project_id)
            .await
            .expect("exists check"),
        "no notation exists before the draft is created"
    );

    let draft = create(
        db,
        &NewNotationDraft {
            project_id,
            slug: "sample-letter",
            title: "Sample Letter",
            source: TEMPLATE_SOURCE,
        },
    )
    .await
    .expect("draft is stored");

    assert_eq!(draft.slug, "sample-letter");
    assert_eq!(draft.project_id, project_id);

    // The whole point of LAW-29's draft door: a preview artifact that is
    // stored and addressable, but explicitly not run. No `notation` row —
    // and therefore no workflow-machine instance, since a machine is
    // always keyed by `(kind, notation_id)` and there is no notation_id.
    assert!(
        !store::notations::exists_for_project(db, project_id)
            .await
            .expect("exists check"),
        "creating a draft must never create a notation row"
    );
    assert!(
        store::notations::list_all(db)
            .await
            .expect("list all notations")
            .is_empty(),
        "no notation exists anywhere in the store after a draft is created"
    );
}

#[tokio::test]
async fn a_stored_draft_is_addressable_by_id() {
    let db = &mem().await;
    let project_id = store::test_support::seed_project_surreal(db, "draft-lookup").await;

    let draft = create(
        db,
        &NewNotationDraft {
            project_id,
            slug: "sample-letter",
            title: "Sample Letter",
            source: TEMPLATE_SOURCE,
        },
    )
    .await
    .expect("draft is stored");

    let found = find_live(db, draft.id)
        .await
        .expect("lookup")
        .expect("the draft is reachable by its id");
    assert_eq!(found.id, draft.id);
    assert_eq!(found.source, TEMPLATE_SOURCE);

    assert!(
        find_live(db, uuid::Uuid::now_v7())
            .await
            .expect("lookup")
            .is_none(),
        "an unknown id finds nothing"
    );
}
