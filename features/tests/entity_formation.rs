//! Cucumber runner for `features/entity_formation.feature`.
//!
//! The profit-corporation and business-trust siblings of the Nevada LLC
//! journey (`nv_llc_formation.rs`): each template binds the state's own
//! formation packet (`form:` frontmatter), so the founder's answers
//! land on the official Secretary-of-State `AcroForm` via the form's
//! field map, the attorney reviews the filled packet, and the matter
//! ends at a recorded Secretary-of-State filing.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, when, World};
use features::journey::{answer_body, client, Journey};
use uuid::Uuid;
use workflows::{CompliancePayload, MachineKind, StateMachineRuntime, StateName};

const SIGNATURE_WAIT: &str = "sent_for_signature__pending";

#[derive(Default, World)]
#[world(init = Self::default)]
struct FormationWorld {
    journey: Option<Journey>,
    person_id: Option<Uuid>,
    person_email: String,
    notation_id: Option<Uuid>,
}

impl std::fmt::Debug for FormationWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormationWorld")
            .field("person_id", &self.person_id)
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl FormationWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation_id not captured")
    }

    async fn stored_packet(&self) -> Vec<u8> {
        let key = format!("notations/{}/document.pdf", self.notation_id());
        let stored = self
            .journey()
            .storage
            .get(&key)
            .await
            .expect("the rendered packet is persisted");
        assert!(stored.bytes.starts_with(b"%PDF"));
        stored.bytes
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut FormationWorld) {
    world.journey = Some(Journey::open("entity-formation").await);
}

#[given(regex = r#"^a client named "([^"]+)" <([^>]+)>$"#)]
async fn seed_client(world: &mut FormationWorld, name: String, email: String) {
    let person = client(&world.journey().surreal, &name, &email).await;
    world.person_id = Some(person.id);
    world.person_email = email;
}

#[when(regex = r#"^the firm opens the "([^"]+)" matter for the client$"#)]
async fn open_matter(world: &mut FormationWorld, code: String) {
    let email = world.person_email.clone();
    let body = format!(
        "client_email={}&retainer_template_code={code}",
        features::form_encode(&email),
    );
    let resp = world
        .journey()
        .lawyer_post("/app/lawyer/retainers/new", body)
        .await;
    let location = resp.location.unwrap_or_else(|| {
        panic!(
            "opening the matter did not redirect (status {})",
            resp.status
        )
    });
    let id = location
        .strip_prefix("/app/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .unwrap_or_else(|| panic!("unexpected redirect target: {location}"));
    world.notation_id = Some(Uuid::parse_str(id).expect("notation id in redirect is a UUID"));
}

#[when("the founder answers the formation questionnaire:")]
async fn answer_questionnaire(world: &mut FormationWorld, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let path = format!("/app/lawyer/notations/{}/step", world.notation_id());
    // First row is the `value` header; skip it.
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        // A `people_list` answer is written in the table as semicolon
        // parts — `name; street; city; state; zip; country`, or with a
        // title second (`name; title; street; …`) for officer rows —
        // and posted as the widget's per-part inputs.
        let body = if value.contains(';') {
            let parts: Vec<&str> = value.split(';').map(str::trim).collect();
            let keys: &[&str] = if parts.len() == 7 {
                &["name", "title", "street", "city", "state", "zip", "country"]
            } else {
                &["name", "street", "city", "state", "zip", "country"]
            };
            keys.iter()
                .zip(parts.iter())
                .map(|(part, v)| format!("p0_{part}={}", features::form_encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        } else {
            answer_body(value)
        };
        let resp = world.journey().lawyer_post(&path, body).await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "answering {value:?} returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
    }
}

#[when("the attorney approves and sends the document")]
async fn attorney_approves_and_sends(world: &mut FormationWorld) {
    // Intake now parks at the `lawyer_review` human gate: no packet is
    // rendered on the founder's final answer. Lawyer approves (fills +
    // persists the SoS packet at `generate_pdf__*`) then sends.
    let id = world.notation_id();
    for action in ["approve-send", "send"] {
        let resp = world
            .journey()
            .lawyer_post(
                &format!("/app/lawyer/notations/{id}/{action}"),
                String::new(),
            )
            .await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "{action} returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
    }
}

#[then("the formation reaches the signature wait")]
async fn assert_signature_wait(world: &mut FormationWorld) {
    let state = StateMachineRuntime::current_state(
        world.journey().runtime.as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    assert_eq!(
        state.as_ref().map(StateName::as_str),
        Some(SIGNATURE_WAIT),
        "expected the walker to drive the workflow to the signature wait",
    );
}

/// The dispatch flattens the filled `AcroForm` to static content before
/// persisting, so answers are asserted through the page text — the same
/// artifact an attorney (or a filing clerk) actually reads.
fn assert_flattened_packet_carries(bytes: &[u8], expected: &[(&str, &str)]) {
    assert!(
        pdf::field_names(bytes)
            .expect("readable AcroForm")
            .is_empty(),
        "persisted packet must be flattened (no live AcroForm fields)",
    );
    let text = pdf::page_text(bytes).expect("readable page text");
    for (value, why) in expected {
        assert!(
            text.contains(value),
            "{why}: flattened page text must carry `{value}`; got: {text}",
        );
    }
}

#[then("the persisted corporation packet carries the founder's answers")]
async fn assert_corp_packet(world: &mut FormationWorld) {
    let bytes = world.stored_packet().await;
    assert_flattened_packet_carries(
        &bytes,
        &[
            (
                "Bright Star Inc",
                "entity name lands on the Articles of Incorporation",
            ),
            ("Libra", "the director fills board slot 1"),
            ("President", "the officer's title lands on the Initial List"),
            ("1000", "the authorized shares land on the Articles"),
        ],
    );
}

#[then("the persisted business-trust packet carries the founder's answers")]
async fn assert_business_trust_packet(world: &mut FormationWorld) {
    let bytes = world.stored_packet().await;
    assert_flattened_packet_carries(
        &bytes,
        &[
            (
                "Bright Star Holdings",
                "entity name lands on the Certificate of Business Trust",
            ),
            ("Libra", "the trustee fills slot 1 of the certificate"),
            ("Trustee", "the trustee title lands on the Initial List"),
        ],
    );
}

#[when("the attorney files the formation packet with the Nevada Secretary of State")]
async fn file_packet(world: &mut FormationWorld) {
    let worker = world.journey().worker();
    let notation_id = world.notation_id();
    let payload = serde_json::to_string(&CompliancePayload {
        office: "Nevada Secretary of State".into(),
        summary: "Formation packet".into(),
        reference: None,
    })
    .expect("serialize compliance payload");
    // The client's signature lands the workflow on `filing__nv_sos`,
    // where the worker records the `filings` row; `filed` then ends it.
    let landed = worker
        .signal(
            MachineKind::Workflow,
            notation_id,
            "signature_received",
            Some(&payload),
        )
        .await
        .expect("signature_received signal");
    assert_eq!(landed.as_str(), "filing__nv_sos");
    worker
        .signal(MachineKind::Workflow, notation_id, "filed", None)
        .await
        .expect("filed signal");
}

#[then("the formation workflow reaches END")]
async fn assert_workflow_end(world: &mut FormationWorld) {
    let state = StateMachineRuntime::current_state(
        world.journey().runtime.as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    assert_eq!(state, Some(StateName::end()), "workflow should be at END");
}

#[then(regex = r#"^a filing was recorded with the "([^"]+)"$"#)]
async fn assert_filing(world: &mut FormationWorld, office: String) {
    let rows = store::filings::for_notation(&world.journey().surreal, world.notation_id())
        .await
        .expect("query filings");
    assert_eq!(rows.len(), 1, "expected exactly one filing, got {rows:?}");
    assert_eq!(rows[0].office, office);
    assert_eq!(rows[0].kind, "filing");
}

#[tokio::main]
async fn main() {
    FormationWorld::cucumber()
        .run_and_exit("tests/features/entity_formation.feature")
        .await;
}
