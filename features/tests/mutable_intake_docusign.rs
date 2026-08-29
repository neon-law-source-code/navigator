//! Cucumber runner for `features/mutable_intake_docusign.feature`.
//!
//! The end-to-end journey for the mutable, two-sided intake: lawyer and
//! client co-fill one notation, a custom clause forces it back through
//! attorney review, and the exact reviewed document — body + interleaved
//! answers + clause — goes out for signature client-then-firm. Proves the
//! review gate and the generalized assemble→DocuSign send together.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, when, World};
use features::journey::Journey;
use uuid::Uuid;
use workflows::{MachineKind, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__engagement_letter";

#[derive(Default, World)]
#[world(init = Self::default)]
struct MutableWorld {
    journey: Option<Journey>,
    notation_id: Option<Uuid>,
    project_code: Option<String>,
    client: Option<store::persons::Person>,
    last_body: String,
}

impl std::fmt::Debug for MutableWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutableWorld")
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl MutableWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }
    fn client(&self) -> &store::persons::Person {
        self.client.as_ref().expect("client not resolved")
    }
    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation not captured")
    }
    fn intake_path(&self) -> String {
        format!(
            "/app/projects/{}/intake/{}",
            self.project_code.as_deref().expect("project"),
            self.notation_id(),
        )
    }
}

#[given(regex = r#"^a retainer matter opened for "[^"]+" <([^>]+)>$"#)]
async fn open_matter(world: &mut MutableWorld, email: String) {
    let journey = Journey::open("mutable-intake").await;
    let body = format!(
        "client_email={}&retainer_template_code={TEMPLATE_CODE}",
        features::form_encode(&email),
    );
    let resp = journey.lawyer_post("/lawyer/retainers/new", body).await;
    let location = resp
        .location
        .unwrap_or_else(|| panic!("matter-open did not redirect (status {})", resp.status));
    let id = location
        .strip_prefix("/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .unwrap_or_else(|| panic!("unexpected redirect target: {location}"));
    let notation_id = Uuid::parse_str(id).expect("notation id is a UUID");
    let notation = store::notations::find_by_id(&journey.surreal, notation_id)
        .await
        .expect("query notation")
        .expect("notation row");
    let person = store::persons::find_by_email_ci(&journey.surreal, &email)
        .await
        .expect("query person")
        .expect("matter-open created the client person");

    let project = store::projects::find_by_id(&journey.surreal, notation.project_id)
        .await
        .expect("query project")
        .expect("the matter the notation belongs to");
    world.project_code = Some(project.code);
    world.notation_id = Some(notation_id);
    world.client = Some(person);
    world.journey = Some(journey);
}

#[when("the client answers their part of the intake:")]
async fn client_answers_part(world: &mut MutableWorld, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let path = world.intake_path();
    let client = world.client().clone();
    // First row is the `value` header; skip it. Each post advances the
    // client to the next client-facing question.
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        let resp = world
            .journey()
            .client_post(
                &client,
                &path,
                &format!("value={}", features::form_encode(value)),
            )
            .await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "client answer returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
    }
}

#[when(regex = r#"^lawyers add the custom clause "([^"]+)"$"#)]
async fn lawyer_add_clause(world: &mut MutableWorld, clause: String) {
    let path = format!("/lawyer/notations/{}/clauses", world.notation_id());
    let resp = world
        .journey()
        .lawyer_post(&path, format!("body={}", features::form_encode(&clause)))
        .await;
    assert!(
        resp.status.is_success() || resp.status.is_redirection(),
        "adding a clause returned {}",
        resp.status,
    );
}

#[when("lawyers finish the intake walk:")]
async fn lawyer_finish_walk(world: &mut MutableWorld, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let path = format!("/lawyer/notations/{}/step", world.notation_id());
    let mut last_body = String::new();
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        let resp = world
            .journey()
            .lawyer_post(&path, format!("value={}", features::form_encode(value)))
            .await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "walking {value:?} returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
        last_body = resp.body;
    }
    world.last_body = last_body;
}

#[then("the matter is awaiting attorney review")]
async fn awaiting_review(world: &mut MutableWorld) {
    let state = StateMachineRuntime::current_state(
        world.journey().runtime.as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    assert_eq!(state.map(|name| name.0).as_deref(), Some("lawyer_review"));
}

#[then("the matter has no signature request yet")]
async fn no_signature_yet(world: &mut MutableWorld) {
    assert!(
        store::signatures::request_id_for_notation(&world.journey().surreal, world.notation_id())
            .await
            .unwrap()
            .is_none(),
        "a notation carrying custom content must not be sent before review",
    );
}

#[when("the attorney approves and sends the document")]
async fn attorney_approves(world: &mut MutableWorld) {
    // Approve renders + parks the PDF at generate_pdf__retainer_pdf; the
    // separate send dispatches the envelope once the PDF is present. Under
    // the in-process runtime the render is synchronous, so the send that
    // follows finds the PDF ready.
    let id = world.notation_id();
    let approve = world
        .journey()
        .lawyer_post(
            &format!("/lawyer/notations/{id}/approve-send"),
            String::new(),
        )
        .await;
    assert!(
        approve.status.is_success() || approve.status.is_redirection(),
        "approve-send returned {} — body:\n{}",
        approve.status,
        approve.body,
    );
    let send = world
        .journey()
        .lawyer_post(&format!("/lawyer/notations/{id}/send"), String::new())
        .await;
    assert!(
        send.status.is_success() || send.status.is_redirection(),
        "send returned {} — body:\n{}",
        send.status,
        send.body,
    );
    // Both actions are post/redirect/get onto the review screen, which is where
    // the assembled document is rendered. Follow it so the assertions below read
    // the document rather than an empty redirect body.
    world.last_body = world
        .journey()
        .lawyer_get(&format!("/lawyer/notations/{id}/review"))
        .await
        .body;
}

#[then("the matter has a signature request")]
async fn has_signature(world: &mut MutableWorld) {
    assert!(
        store::signatures::request_id_for_notation(&world.journey().surreal, world.notation_id())
            .await
            .unwrap()
            .is_some(),
        "the approved document must have been sent for signature",
    );
}

#[then("the signature envelope routes the client before the firm")]
async fn envelope_routing(world: &mut MutableWorld) {
    let calls = world.journey().signature.calls();
    assert_eq!(
        calls.len(),
        1,
        "expected exactly one envelope, got {calls:?}"
    );
    let recipients = &calls[0].manifest.recipients;
    let client = recipients
        .iter()
        .find(|r| r.role == "client")
        .expect("envelope has a client recipient");
    let firm = recipients
        .iter()
        .find(|r| r.role == "firm")
        .expect("envelope has a firm recipient");
    assert_eq!(client.routing_order, 1, "the client signs first");
    assert_eq!(firm.routing_order, 2, "the firm countersigns");
    // The captive client signs embedded; the firm is emailed.
    assert!(
        client.client_user_id.is_some(),
        "the client is captive (embedded signing)",
    );
}

#[then("the sent document carries the custom clause")]
async fn document_carries_clause(world: &mut MutableWorld) {
    assert!(
        world.last_body.contains("governed by Nevada law"),
        "the approved-and-sent document must carry the custom clause, got:\n{}",
        world.last_body,
    );
}

#[tokio::main]
async fn main() {
    MutableWorld::cucumber()
        .run_and_exit("tests/features/mutable_intake_docusign.feature")
        .await;
}
