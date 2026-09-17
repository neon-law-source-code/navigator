//! Cucumber runner for `features/start_door.feature`.
//!
//! Drives the public service door through a real client session, then walks
//! the newly opened questionnaire to the mandatory lawyer-review state.

#![allow(clippy::unused_async)]

use cucumber::{gherkin::Step, given, then, when, World};
use features::journey::{client, Journey};
use uuid::Uuid;
use workflows::notation_session::ClientIntakeStep;

#[derive(Default, World)]
#[world(init = Self::default)]
struct StartDoorWorld {
    journey: Option<Journey>,
    client: Option<store::persons::Person>,
    project_code: Option<String>,
    notation_id: Option<Uuid>,
    last_status: Option<u16>,
    last_location: Option<String>,
}

impl std::fmt::Debug for StartDoorWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartDoorWorld")
            .field("project_code", &self.project_code)
            .field("notation_id", &self.notation_id)
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl StartDoorWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn client(&self) -> &store::persons::Person {
        self.client.as_ref().expect("client not built")
    }

    /// Where the client-facing questionnaire currently stands.
    async fn intake_step(&self) -> ClientIntakeStep {
        workflows::notation_session::client_intake_step(
            &self.journey().surreal,
            Some(&self.journey().storage),
            self.notation_id.expect("notation id not captured"),
        )
        .await
        .expect("read the client intake step")
    }

    fn intake_path(&self) -> String {
        format!(
            "/app/projects/{}/intake/{}",
            self.project_code
                .as_deref()
                .expect("project code not captured"),
            self.notation_id.expect("notation id not captured"),
        )
    }
}

#[given("a client ready to start a mapped service")]
async fn client_ready(world: &mut StartDoorWorld) {
    let journey = Journey::open_start_door("start-door").await;
    let person = client(
        &journey.surreal,
        "Start Door Client",
        "start-client@example.com",
    )
    .await;
    world.journey = Some(journey);
    world.client = Some(person);
}

#[when(regex = r#"^the client starts the "([^"]+)" service$"#)]
async fn start_service(world: &mut StartDoorWorld, service_id: String) {
    let response = world
        .journey()
        .client_post(
            world.client(),
            &format!("/start/{service_id}"),
            "template=client-supplied-template",
        )
        .await;
    world.last_status = Some(response.status.as_u16());
    world.last_location = response.location;
    let location = world
        .last_location
        .as_deref()
        .expect("start response location");
    let mut parts = location
        .strip_prefix("/app/projects/")
        .expect("start redirects into the client portal")
        .split('/');
    world.project_code = Some(parts.next().expect("project code segment").to_string());
    assert_eq!(parts.next(), Some("intake"));
    let notation = parts
        .next()
        .expect("notation route segment")
        .split('?')
        .next()
        .expect("notation id segment");
    world.notation_id = Some(Uuid::parse_str(notation).expect("notation id is a UUID"));
}

#[then("the start response redirects to the client intake")]
async fn start_redirects(world: &mut StartDoorWorld) {
    assert_eq!(world.last_status, Some(303));
    assert!(
        world
            .last_location
            .as_deref()
            .is_some_and(|location| location.ends_with("?started=1")),
        "expected the start confirmation query"
    );
}

#[then(regex = r"^the intake asks question (\d+) of (\d+)$")]
async fn intake_asks_question(world: &mut StartDoorWorld, position: usize, total: usize) {
    match world.intake_step().await {
        ClientIntakeStep::NeedsAnswer {
            position: at,
            total: of,
            ..
        } => {
            assert_eq!((at, of), (position, total), "intake is not where expected");
        }
        ClientIntakeStep::Complete { .. } => panic!("the intake is already complete"),
    }
}

/// Answer the whole questionnaire over real HTTP, one table row per question.
///
/// Each row names the question it answers, so the walk cannot silently drift:
/// the step asserts the intake is actually on that question before posting,
/// and that the redirect carries no `?error=` flash. Without both, a rejected
/// answer still redirects (`303` back to the same question) and the scenario
/// would pass having recorded nothing.
#[when("the client answers every question the intake asks:")]
async fn answer_client_questions(world: &mut StartDoorWorld, step: &Step) {
    let table = step.table.as_ref().expect("questionnaire answer table");
    for row in table.rows.iter().skip(1) {
        let code = row.first().expect("a question code per row").as_str();
        let answer = row.get(1).expect("an answer per row").as_str();
        let ClientIntakeStep::NeedsAnswer { question, .. } = world.intake_step().await else {
            panic!("the intake finished before answering `{code}`");
        };
        assert_eq!(
            question.code, code,
            "the intake asks `{}`, not `{code}`",
            question.code
        );
        let response = world
            .journey()
            .client_post(
                world.client(),
                &world.intake_path(),
                &body_for(&question, answer),
            )
            .await;
        assert_eq!(
            response.status.as_u16(),
            303,
            "answering `{code}` returned {}",
            response.status
        );
        let location = response.location.clone().unwrap_or_default();
        assert!(
            !location.contains("error="),
            "answering `{code}` was refused: {location}"
        );
        world.last_status = Some(response.status.as_u16());
    }
}

/// The form body one answer posts, which depends on the question's type: a
/// `people_list` posts the widget's `p{row}_{part}` fields, everything else
/// posts the single `value` field.
fn body_for(question: &workflows::notation_session::QuestionDescriptor, answer: &str) -> String {
    if question.answer_type == "people_list" {
        format!("p0_name={}", features::form_encode(answer))
    } else {
        features::journey::answer_body(answer)
    }
}

#[then("the client's part of the intake is complete")]
async fn intake_is_complete(world: &mut StartDoorWorld) {
    match world.intake_step().await {
        ClientIntakeStep::Complete { .. } => {}
        ClientIntakeStep::NeedsAnswer {
            question, position, ..
        } => panic!(
            "the intake still asks `{}` at position {position}",
            question.code
        ),
    }
}

#[when("the firm advances the completed intake to lawyer review")]
async fn send_to_lawyer_review(world: &mut StartDoorWorld) {
    let worker = world.journey().worker();
    portal::retainer_walk::advance_to_lawyer_review(
        &world.journey().surreal,
        &worker,
        world.notation_id.expect("notation id"),
        None,
    )
    .await
    .expect("completed intake advances to lawyer review");
}

#[then(regex = r#"^the notation state is "([^"]+)"$"#)]
async fn notation_state(world: &mut StartDoorWorld, expected: String) {
    let notation = store::notations::find_by_id(
        &world.journey().surreal,
        world.notation_id.expect("notation id"),
    )
    .await
    .expect("query notation")
    .expect("notation exists");
    assert_eq!(notation.state, expected);
}

#[tokio::main]
async fn main() {
    StartDoorWorld::cucumber()
        .run_and_exit("tests/features/start_door.feature")
        .await;
}
