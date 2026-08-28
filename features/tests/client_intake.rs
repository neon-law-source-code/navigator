//! Cucumber runner for `features/client_intake.feature`.
//!
//! Drives the client self-serve intake surface
//! (`/app/projects/:project_code/intake/:notation_id`) over real HTTP as the
//! client, against a retainer matter opened through the admin walker. The
//! demand-side mirror of `retainer_intake.rs`.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{given, then, when, World};
use features::decode_character_references;
use features::journey::{answer_body, client, Journey};
use uuid::Uuid;
use workflows::{bundled_spec_yaml, prompt_overrides_from_yaml};

const TEMPLATE_CODE: &str = "onboarding__letter";

#[derive(Default, World)]
#[world(init = Self::default)]
struct IntakeWorld {
    journey: Option<Journey>,
    notation_id: Option<Uuid>,
    project_code: Option<String>,
    client: Option<store::persons::Person>,
    last_status: Option<u16>,
    last_body: String,
}

impl std::fmt::Debug for IntakeWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntakeWorld")
            .field("notation_id", &self.notation_id)
            .field("project_code", &self.project_code)
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl IntakeWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn client(&self) -> &store::persons::Person {
        self.client.as_ref().expect("client not resolved")
    }

    fn intake_path(&self) -> String {
        format!(
            "/app/projects/{}/intake/{}",
            self.project_code.as_deref().expect("project not resolved"),
            self.notation_id.expect("notation not captured"),
        )
    }
}

#[given(regex = r#"^a retainer matter opened for "[^"]+" <([^>]+)>$"#)]
async fn open_matter(world: &mut IntakeWorld, email: String) {
    let journey = Journey::open("client-intake").await;
    let body = format!(
        "client_email={}&retainer_template_code={TEMPLATE_CODE}",
        features::form_encode(&email),
    );
    let resp = journey.lawyer_post("/lawyer/retainers/new", body).await;
    let location = resp
        .location
        .unwrap_or_else(|| panic!("matter-open did not redirect (status {})", resp.status));
    // `/lawyer/notations/<uuid>/step`
    let id = location
        .strip_prefix("/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .unwrap_or_else(|| panic!("unexpected redirect target: {location}"));
    let notation_id = Uuid::parse_str(id).expect("notation id in redirect is a UUID");

    let notation = store::notations::find_by_id(&journey.surreal, notation_id)
        .await
        .expect("query notation")
        .expect("notation row exists");
    let person = store::persons::find_by_email_ci(&journey.surreal, &email)
        .await
        .expect("query person")
        .expect("matter-open created the client person");

    // The intake link a client follows names the matter by its code, so the
    // world carries the code the URL is built from rather than the row id.
    let project = store::projects::find_by_id(&journey.surreal, notation.project_id)
        .await
        .expect("query project")
        .expect("the matter the notation belongs to");

    world.project_code = Some(project.code);
    world.notation_id = Some(notation_id);
    world.client = Some(person);
    world.journey = Some(journey);
}

#[given(regex = r#"^a lawyer pre-filled the client's name as "([^"]+)"$"#)]
async fn lawyer_prefill(world: &mut IntakeWorld, value: String) {
    // The walker's first question is person__client; lawyer answering it
    // records a lawyer-sourced answer the client will later confirm.
    let path = format!(
        "/lawyer/notations/{}/step",
        world.notation_id.expect("notation"),
    );
    let resp = world
        .journey()
        .lawyer_post(&path, answer_body(&value))
        .await;
    assert!(
        resp.status.is_success() || resp.status.is_redirection(),
        "lawyer pre-fill returned {}",
        resp.status,
    );
}

#[when("the client opens their intake")]
async fn client_opens_intake(world: &mut IntakeWorld) {
    let path = world.intake_path();
    let client = world.client().clone();
    let resp = world.journey().client_get(&client, &path).await;
    world.last_status = Some(resp.status.as_u16());
    world.last_body = resp.body;
}

#[when(regex = r#"^the client answers "([^"]+)"$"#)]
async fn client_answers(world: &mut IntakeWorld, value: String) {
    let path = world.intake_path();
    let client = world.client().clone();
    let resp = world
        .journey()
        .client_post(
            &client,
            &path,
            &format!("value={}", features::form_encode(&value)),
        )
        .await;
    assert!(
        resp.status.is_success() || resp.status.is_redirection(),
        "client answer returned {} — body:\n{}",
        resp.status,
        resp.body,
    );
}

#[when("a stranger opens the client's intake")]
async fn stranger_opens_intake(world: &mut IntakeWorld) {
    // A client with no participation row for this matter.
    let stranger = client(&world.journey().surreal, "Aries", "aries@example.com").await;
    let path = world.intake_path();
    let resp = world.journey().client_get(&stranger, &path).await;
    world.last_status = Some(resp.status.as_u16());
    world.last_body = resp.body;
}

#[then(regex = r#"^the intake asks the "([^"]+)" question$"#)]
async fn asks_question(world: &mut IntakeWorld, code: String) {
    // A `custom_*__<role>` state renders its template prompt override
    // (keyed by the `<role>` after `__`), not the canonical registry
    // prompt — so resolve the override the walker shows and assert the
    // rendered intake body carries it.
    let yaml = bundled_spec_yaml(TEMPLATE_CODE).expect("retainer bundled spec");
    let overrides = prompt_overrides_from_yaml(yaml).expect("parse prompt overrides");
    let role = match code.as_str() {
        "person__client" => "client_name",
        "project__engagement" => "project_name",
        _ => code
            .split_once("__")
            .map_or(code.as_str(), |(_, role)| role),
    };
    let prompt = overrides
        .get(role)
        .unwrap_or_else(|| panic!("no prompt override for state {code}"));
    let body = decode_character_references(&world.last_body);
    assert!(
        body.contains(prompt),
        "intake body did not ask the {code} question (prompt {prompt:?}), got:\n{}",
        world.last_body,
    );
}

#[then(regex = r#"^the intake is pre-filled with "([^"]+)"$"#)]
async fn prefilled_with(world: &mut IntakeWorld, value: String) {
    assert!(
        world.last_body.contains(&value),
        "intake body was not pre-filled with {value:?}",
    );
}

#[then("the client's part of the intake is complete")]
async fn intake_complete(world: &mut IntakeWorld) {
    assert!(
        world.last_body.contains("your part is done"),
        "expected the completion landing, got:\n{}",
        world.last_body,
    );
}

#[then(regex = r#"^the client's name answer on file is "([^"]+)" from the client$"#)]
async fn name_answer_on_file(world: &mut IntakeWorld, value: String) {
    // The `person__client` state resolves to the canonical `person`
    // registry question; the per-state `state_name` column pins the
    // answer to this role specifically.
    let surreal = world.journey().surreal.clone();
    let q = store::questions::find_by_code(&surreal, "person")
        .await
        .expect("query question")
        .expect("person seeded");
    let latest = store::answers::latest_for_state(
        &surreal,
        q.id,
        "person__client",
        world.client().id,
        world.notation_id.expect("a notation is under walk"),
    )
    .await
    .expect("query answers")
    .expect("a person__client answer exists");
    assert_eq!(store::answers::display_value(&latest.value), value);
    assert_eq!(latest.source, store::answers::SOURCE_CLIENT);
    assert_eq!(latest.authored_by_person_id, Some(world.client().id));
}

#[then(regex = r"^the intake response status is (\d+)$")]
async fn response_status(world: &mut IntakeWorld, status: u16) {
    assert_eq!(world.last_status, Some(status));
}

#[tokio::main]
async fn main() {
    IntakeWorld::cucumber()
        .run_and_exit("tests/features/client_intake.feature")
        .await;
}
