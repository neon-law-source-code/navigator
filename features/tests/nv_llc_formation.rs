//! Cucumber runner for `features/nv_llc_formation.feature`.
//!
//! The first end-to-end *journey* spec: it follows one founder (Libra)
//! and one Neon Law attorney through a whole Nevada entity formation,
//! crossing three surfaces — the admin intake walker (real HTTP), the
//! post-intake signing workflow the walker auto-drives, and the
//! worker-shaped runtime that records the Secretary-of-State filing once
//! the client has signed. The `nv__llc_formation` template body is a stub;
//! the questionnaire + workflow it carries are the contract under test.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, when, World};
use features::journey::{answer_body, client, Journey};
use uuid::Uuid;
use workflows::{
    bundled_spec_yaml, is_dispatched_submission, workflow_spec_from_yaml, CompliancePayload,
    MachineKind, StateMachineRuntime, StateName,
};

const SIGNATURE_WAIT: &str = "sent_for_signature__pending";

#[derive(Default, World)]
#[world(init = Self::default)]
struct LlcFormationWorld {
    journey: Option<Journey>,
    person_id: Option<Uuid>,
    person_email: String,
    notation_id: Option<Uuid>,
}

impl std::fmt::Debug for LlcFormationWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlcFormationWorld")
            .field("person_id", &self.person_id)
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl LlcFormationWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation_id not captured")
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut LlcFormationWorld) {
    world.journey = Some(Journey::open("nv-llc-formation").await);
}

#[given(regex = r#"^a client named "([^"]+)" <([^>]+)>$"#)]
async fn seed_client(world: &mut LlcFormationWorld, name: String, email: String) {
    let person = client(&world.journey().surreal, &name, &email).await;
    world.person_id = Some(person.id);
    world.person_email = email;
}

#[when(regex = r#"^the firm opens the "([^"]+)" matter for the client$"#)]
async fn open_matter(world: &mut LlcFormationWorld, code: String) {
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
    // `/app/lawyer/notations/<uuid>/step`
    let id = location
        .strip_prefix("/app/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .unwrap_or_else(|| panic!("unexpected redirect target: {location}"));
    world.notation_id = Some(Uuid::parse_str(id).expect("notation id in redirect is a UUID"));
}

#[when("the founder answers the formation questionnaire:")]
async fn answer_questionnaire(world: &mut LlcFormationWorld, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let path = format!("/app/lawyer/notations/{}/step", world.notation_id());
    // First row is the `value` header; skip it.
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        // A `people_list` answer is written in the table as
        // `name; street; city; state; zip; country` and posted as the
        // widget's per-part inputs, exactly as the browser form would.
        let body = if value.contains(';') {
            let parts: Vec<&str> = value.split(';').map(str::trim).collect();
            ["name", "street", "city", "state", "zip", "country"]
                .iter()
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
async fn attorney_approves_and_sends(world: &mut LlcFormationWorld) {
    // Intake now parks at the `lawyer_review` human gate: no packet is
    // rendered on the founder's final answer. Lawyer approves (fills +
    // persists the official SoS form at `generate_pdf__*`) then sends.
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

#[then("the generated packet is filed as a document in the matter")]
async fn assert_generated_document_filed(world: &mut LlcFormationWorld) {
    // The attorney's approve rendered + persisted the packet at the
    // `generate_pdf` step, which files it into the matter as a
    // project-scoped `documents` row backed by a content-addressed blob —
    // the auditable intermediary artifact, filed through the real web
    // dispatch path.
    let notation = store::notations::find_by_id(&world.journey().surreal, world.notation_id())
        .await
        .unwrap()
        .expect("seeded notation");
    let filed: Vec<_> = store::assets::for_project(&world.journey().surreal, notation.project_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|a| a.source.as_deref() == Some(store::documents::source::GENERATED))
        .collect();
    assert!(
        !filed.is_empty(),
        "the generated packet must be filed as a `generated` document on the project",
    );
    assert!(
        !filed[0].storage_key.is_empty(),
        "the filed document points at a real storage object",
    );
}

#[then("the formation reaches the signature wait")]
async fn assert_signature_wait(world: &mut LlcFormationWorld) {
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

#[then("the persisted packet is the official SoS form carrying the founder's answers")]
async fn assert_filled_packet(world: &mut LlcFormationWorld) {
    let key = format!("notations/{}/document.pdf", world.notation_id());
    let stored = world
        .journey()
        .storage
        .get(&key)
        .await
        .expect("the rendered packet is persisted");
    assert!(stored.bytes.starts_with(b"%PDF"));
    // The artifact came through the AcroForm fill (sha-pin-verified pull
    // from the assets lane), then `pdf::flatten` past lawyer review: no
    // interactive field survives for a downstream tool to re-edit, and
    // the founder's answers read back as static page content.
    assert!(
        pdf::field_names(&stored.bytes)
            .expect("field names readable")
            .is_empty(),
        "the filed packet is flattened — no interactive fields survive lawyer review"
    );
    let text = pdf::page_text(&stored.bytes).expect("extract flattened page text");
    assert!(
        text.contains("Bright Star Ventures"),
        "entity name lands as static content:\n{text}"
    );
    assert!(
        text.contains("Libra"),
        "the managing member lands as static content:\n{text}"
    );
}

#[when("the attorney files the Articles with the Nevada Secretary of State")]
async fn file_articles(world: &mut LlcFormationWorld) {
    let worker = world.journey().worker();
    let notation_id = world.notation_id();
    let payload = serde_json::to_string(&CompliancePayload {
        office: "Nevada Secretary of State".into(),
        summary: "Articles of Organization".into(),
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
async fn assert_workflow_end(world: &mut LlcFormationWorld) {
    let state = StateMachineRuntime::current_state(
        world.journey().runtime.as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    assert_eq!(state, Some(StateName::end()), "workflow should be at END");
}

#[then(regex = r#"^a filing was recorded with the "([^"]+)"$"#)]
async fn assert_filing(world: &mut LlcFormationWorld, office: String) {
    let rows = store::filings::for_notation(&world.journey().surreal, world.notation_id())
        .await
        .expect("query filings");
    assert_eq!(rows.len(), 1, "expected exactly one filing, got {rows:?}");
    assert_eq!(rows[0].office, office);
    assert_eq!(rows[0].kind, "filing");
}

#[then("the founder's six onboarding answers are on file")]
async fn assert_answers(world: &mut LlcFormationWorld) {
    let person_id = world.person_id.expect("person seeded");
    let rows: Vec<_> = store::answers::list_all(&world.journey().surreal)
        .await
        .expect("query answers")
        .into_iter()
        .filter(|a| a.person_id == person_id)
        .collect();
    assert_eq!(rows.len(), 6, "expected six onboarding answers");
}

#[then(regex = r#"^the "([^"]+)" workflow ends at a Secretary-of-State filing$"#)]
async fn assert_recurring_obligation(_world: &mut LlcFormationWorld, code: String) {
    let yaml = bundled_spec_yaml(&code).unwrap_or_else(|| panic!("no bundled spec for {code}"));
    let spec = workflow_spec_from_yaml(yaml).expect("workflow spec parses");
    // The recurring obligation is "visible" as a workflow that ends at a
    // dispatched government submission (the annual report goes to the
    // Secretary of State via the mailroom).
    let ends_at_submission = spec.states.iter().any(|(state, transitions)| {
        is_dispatched_submission(state)
            && transitions
                .0
                .values()
                .any(|to| to.as_str() == StateName::END)
    });
    assert!(
        ends_at_submission,
        "expected {code} to end at a dispatched Secretary-of-State submission",
    );
}

#[tokio::main]
async fn main() {
    LlcFormationWorld::cucumber()
        .run_and_exit("tests/features/nv_llc_formation.feature")
        .await;
}
