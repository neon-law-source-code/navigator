//! Cucumber runner for `features/naturalization_federal.feature`.
//!
//! Neon Law Navigator's first immigration journey: one lawful permanent resident
//! (Maria Santos) through a whole Form N-400 naturalization, crossing the
//! admin intake walker (real HTTP), the post-intake signing workflow the
//! walker auto-drives, and the worker-shaped runtime that records the USCIS
//! filing, advances the biometrics / interview / oath milestones, and files
//! the issued Certificate of Naturalization (Form N-550) into the matter.
//! The `us__naturalization` template body renders the N-400 intake
//! summary; the questionnaire + workflow it carries are the contract here.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, when, World};
use features::journey::{answer_body, client, Journey};
use uuid::Uuid;
use workflows::{
    CompliancePayload, IntakeArtifact, IntakePayload, MachineKind, StateMachineRuntime, StateName,
};

const SIGNATURE_WAIT: &str = "sent_for_signature__pending";
const E_FILING: &str = "e_filing__uscis";
const BIOMETRICS: &str = "mailroom_receive__biometrics_notice";
const INTERVIEW: &str = "mailroom_receive__interview_notice";
const OATH: &str = "mailroom_receive__oath_notice";
const CERTIFICATE_INTAKE: &str = "document_intake__certificate_of_naturalization";

#[derive(Default, World)]
#[world(init = Self::default)]
struct NaturalizationWorld {
    journey: Option<Journey>,
    person_id: Option<Uuid>,
    person_email: String,
    notation_id: Option<Uuid>,
}

impl std::fmt::Debug for NaturalizationWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NaturalizationWorld")
            .field("person_id", &self.person_id)
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl NaturalizationWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation_id not captured")
    }

    /// Current workflow state for the matter, as a `String` for assertions.
    async fn workflow_state(&self) -> Option<String> {
        StateMachineRuntime::current_state(
            self.journey().runtime.as_ref(),
            MachineKind::Workflow,
            self.notation_id(),
        )
        .await
        .as_ref()
        .map(|s| s.as_str().to_string())
    }

    /// Signal the workflow as the lawyer-side worker and return the state it
    /// lands on. The worker shares the web app's journal, so a signal here
    /// is visible to the same matter the walker drove.
    async fn signal(&self, condition: &str, payload: Option<&str>) -> StateName {
        self.journey()
            .worker()
            .signal(
                MachineKind::Workflow,
                self.notation_id(),
                condition,
                payload,
            )
            .await
            .unwrap_or_else(|e| panic!("signal `{condition}` failed: {e}"))
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut NaturalizationWorld) {
    world.journey = Some(Journey::open("naturalization").await);
}

#[given(regex = r#"^a client named "([^"]+)" <([^>]+)>$"#)]
async fn seed_client(world: &mut NaturalizationWorld, name: String, email: String) {
    let person = client(&world.journey().surreal, &name, &email).await;
    world.person_id = Some(person.id);
    world.person_email = email;
}

#[when(regex = r#"^the firm opens the "([^"]+)" matter for the client$"#)]
async fn open_matter(world: &mut NaturalizationWorld, code: String) {
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

#[when("the applicant answers the naturalization questionnaire:")]
async fn answer_questionnaire(world: &mut NaturalizationWorld, step: &Step) {
    let table = step.table.as_ref().expect("scenario has a data table");
    let path = format!("/app/lawyer/notations/{}/step", world.notation_id());
    // First row is the `value` header; skip it. Every N-400 intake answer
    // is a scalar (string / date / radio choice / yes_no), so each posts as
    // the step form's single `value` field.
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        let resp = world.journey().lawyer_post(&path, answer_body(value)).await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "answering {value:?} returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
    }
}

#[when("the attorney approves and sends the document")]
async fn attorney_approves_and_sends(world: &mut NaturalizationWorld) {
    // Intake now parks at the `lawyer_review` human gate: no PDF is rendered
    // on the applicant's final answer. Lawyer approves (renders + persists
    // the PDF at `generate_pdf__*`) then sends (drives to the signature
    // wait). Both are deliberate steps, mirroring the durable prod pipeline.
    let id = world.notation_id();
    for action in ["approve-send", "send"] {
        let resp = world
            .journey()
            .lawyer_post(&format!("/app/lawyer/notations/{id}/{action}"), String::new())
            .await;
        assert!(
            resp.status.is_success() || resp.status.is_redirection(),
            "{action} returned {} — body:\n{}",
            resp.status,
            resp.body,
        );
    }
}

#[then("the application reaches the signature wait")]
async fn assert_signature_wait(world: &mut NaturalizationWorld) {
    assert_eq!(
        world.workflow_state().await.as_deref(),
        Some(SIGNATURE_WAIT),
        "the walker should drive the N-400 intake to the signature wait",
    );
}

#[then("the persisted N-400 is the filled USCIS AcroForm carrying the applicant's answers")]
async fn assert_rendered_pdf(world: &mut NaturalizationWorld) {
    let key = format!("notations/{}/document.pdf", world.notation_id());
    let stored = world
        .journey()
        .storage
        .get(&key)
        .await
        .expect("the filled N-400 packet is persisted");
    assert!(stored.bytes.starts_with(b"%PDF"));
    // With `form: us__naturalization` wired, the document is the USCIS
    // AcroForm — the answers land on the government form via its field
    // manifest — not the old one-page Typst summary. It is flattened past
    // lawyer review, so no interactive field survives and the applicant's
    // data reads back as static content.
    assert!(
        pdf::field_names(&stored.bytes)
            .expect("field names readable")
            .is_empty(),
        "the filed N-400 is flattened — no interactive field survives lawyer review",
    );
    let text = pdf::page_text(&stored.bytes).expect("extract flattened page text");
    // The applicant's email fills the N-400 — proof the
    // `{{person__client.email}}` merge now resolves from the notation's
    // Person row (the N-400 questionnaire asks for the name, never the
    // email), the first of the two render bugs this pass fixes.
    assert!(
        text.contains("maria@example.com"),
        "the applicant email fills the N-400 from the Person row:\n{text}",
    );
}

#[when("the applicant signs and the firm e-files the Form N-400 with USCIS")]
async fn sign_and_file(world: &mut NaturalizationWorld) {
    // The signature lands the workflow on `e_filing__uscis`, where the
    // worker records the USCIS submission in `filings`; `filed` then moves
    // it to the first USCIS milestone.
    let compliance = serde_json::to_string(&CompliancePayload {
        office: "USCIS".into(),
        summary: "Form N-400 Application for Naturalization".into(),
        reference: None,
    })
    .expect("serialize compliance payload");
    let landed = world.signal("signature_received", Some(&compliance)).await;
    assert_eq!(landed.as_str(), E_FILING, "signature e-files with USCIS");
    let landed = world.signal("filed", None).await;
    assert_eq!(
        landed.as_str(),
        BIOMETRICS,
        "filing opens the USCIS timeline"
    );
}

#[then("the naturalization workflow reaches the biometrics milestone")]
async fn assert_biometrics(world: &mut NaturalizationWorld) {
    assert_eq!(world.workflow_state().await.as_deref(), Some(BIOMETRICS));
}

#[when("USCIS sends the biometrics, interview, and oath notices")]
async fn uscis_milestones(world: &mut NaturalizationWorld) {
    let landed = world.signal("received", None).await;
    assert_eq!(landed.as_str(), INTERVIEW, "biometrics → interview notice");
    let landed = world.signal("received", None).await;
    assert_eq!(landed.as_str(), OATH, "interview → oath notice");
}

#[then("the naturalization workflow awaits the Certificate of Naturalization")]
async fn assert_awaiting_certificate(world: &mut NaturalizationWorld) {
    assert_eq!(world.workflow_state().await.as_deref(), Some(OATH));
}

#[when("USCIS issues the Certificate of Naturalization")]
async fn issue_certificate(world: &mut NaturalizationWorld) {
    // The issued N-550 arrives as the document-intake artifact; landing on
    // the intake state files it into the matter, then `certificate_filed`
    // closes the workflow.
    let intake = serde_json::to_string(&IntakePayload {
        kind: "certificate_of_naturalization".into(),
        filename: "certificate-of-naturalization.txt".into(),
        artifact: IntakeArtifact::Text {
            text: "Certificate of Naturalization (Form N-550) issued by USCIS.".into(),
        },
    })
    .expect("serialize intake payload");
    let landed = world.signal("certificate_received", Some(&intake)).await;
    assert_eq!(
        landed.as_str(),
        CERTIFICATE_INTAKE,
        "the issued certificate is filed into the matter",
    );
    let landed = world.signal("certificate_filed", None).await;
    assert_eq!(landed, StateName::end(), "the matter concludes");
}

#[then("the naturalization workflow reaches END")]
async fn assert_workflow_end(world: &mut NaturalizationWorld) {
    let state = StateMachineRuntime::current_state(
        world.journey().runtime.as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    assert_eq!(state, Some(StateName::end()), "workflow should be at END");
}

#[then("a USCIS filing was recorded")]
async fn assert_filing(world: &mut NaturalizationWorld) {
    let rows = store::filings::for_notation(&world.journey().surreal, world.notation_id())
        .await
        .expect("query filings");
    assert_eq!(rows.len(), 1, "expected exactly one filing, got {rows:?}");
    assert_eq!(rows[0].office, "USCIS");
    assert_eq!(rows[0].kind, "e_filing");
}

#[then("the issued Certificate of Naturalization is filed in the matter")]
async fn assert_certificate_document(world: &mut NaturalizationWorld) {
    let notation = store::notations::find_by_id(&world.journey().surreal, world.notation_id())
        .await
        .expect("query notation")
        .expect("notation exists");
    let docs: Vec<_> = store::assets::for_project(&world.journey().surreal, notation.project_id)
        .await
        .expect("query documents")
        .into_iter()
        .filter(|a| a.kind.as_deref() == Some("certificate_of_naturalization"))
        .collect();
    assert_eq!(
        docs.len(),
        1,
        "expected the issued N-550 filed once, got {docs:?}",
    );
}

#[then("the applicant's ten intake answers are on file")]
async fn assert_answers(world: &mut NaturalizationWorld) {
    let person_id = world.person_id.expect("person seeded");
    let rows: Vec<_> = store::answers::list_all(&world.journey().surreal)
        .await
        .expect("query answers")
        .into_iter()
        .filter(|a| a.person_id == person_id)
        .collect();
    assert_eq!(rows.len(), 10, "expected ten N-400 intake answers");
}

#[tokio::main]
async fn main() {
    NaturalizationWorld::cucumber()
        .run_and_exit("tests/features/naturalization_federal.feature")
        .await;
}
