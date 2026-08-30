//! Cucumber runner for `features/closing_letter.feature`.
//!
//! Drives the admin walker (`/app/lawyer/notations/:id/step`) over an
//! `offboarding__letter` notation. The walker is generic over the bound
//! template's questionnaire, so this mirrors `retainer_intake.rs` with
//! the offboarding template's six-question walk — the firm-signed bookend
//! to the client-signed onboarding letter.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{gherkin::Step, given, then, when, World};
use features::{app_state, body_string, decode_character_references, form_encode, fs_storage};
use portal::{policy::PolicyClient, SessionStore};
use store::seed;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime, StateName};

const TEMPLATE_CODE: &str = "offboarding__letter";

#[derive(Default, World)]
#[world(init = Self::default)]
struct ClosingWorld {
    app: Option<axum::Router>,
    notation_id: Option<Uuid>,
    runtime: Option<Arc<InMemoryRuntime>>,
    last_status: Option<StatusCode>,
    last_body: String,
    final_status: Option<StatusCode>,
}

impl std::fmt::Debug for ClosingWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosingWorld")
            .field("notation_id", &self.notation_id)
            .field("last_status", &self.last_status)
            .field("final_status", &self.final_status)
            .finish_non_exhaustive()
    }
}

impl ClosingWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
    fn runtime(&self) -> &Arc<InMemoryRuntime> {
        self.runtime.as_ref().expect("runtime not built")
    }

    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation_id not built")
    }

    fn substitute(&self, uri: &str) -> String {
        uri.replace(":id", &self.notation_id().to_string())
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut ClosingWorld) {
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("closing").await;
    seed::seed_canonical(&surreal, &storage)
        .await
        .expect("seed canonical");
    let runtime = Arc::new(InMemoryRuntime::new());
    let state = app_state(
        runtime.clone(),
        storage,
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    let router = features::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    world.app = Some(router);
    world.runtime = Some(runtime);
}

#[given(regex = r#"^a closing notation for "([^"]+)" <([^>]+)> at BEGIN$"#)]
async fn seed_notation(world: &mut ClosingWorld, name: String, email: String) {
    let surreal = features::shared_surreal().await;
    let tmpl = store::templates::resolve(&features::shared_surreal().await, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed_canonical inserts offboarding__letter");
    let person =
        store::test_support::ensure_person(&surreal, &store::persons::NewPerson::new(name, email))
            .await;
    let proj = store::test_support::seed_project(&surreal, "closing matter").await;
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, person.id, proj.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    world.notation_id = Some(notation_id);
}

#[when(regex = r"^the lawyer visits (.+)$")]
async fn lawyer_visits(world: &mut ClosingWorld, path: String) {
    let uri = world.substitute(&path);
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

#[when("the lawyer submits the full questionnaire:")]
async fn lawyer_walks_all(world: &mut ClosingWorld, step: &Step) {
    let table = step.table.as_ref().expect("expected a data table");
    // First row is the header (`value`); skip it.
    let mut last_status = StatusCode::OK;
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        let body = format!("value={}", form_encode(value));
        let uri = world.substitute("/app/lawyer/notations/:id/step");
        let resp = world
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = resp.status();
    }
    world.final_status = Some(last_status);
}

#[then(regex = r"^the response status is (\d+)$")]
async fn assert_status(world: &mut ClosingWorld, code: u16) {
    assert_eq!(
        world.last_status.expect("no status captured").as_u16(),
        code,
        "body: {}",
        world.last_body
    );
}

#[then(regex = r"^the final response status is (\d+)$")]
async fn assert_final_status(world: &mut ClosingWorld, code: u16) {
    assert_eq!(
        world
            .final_status
            .expect("no final status captured")
            .as_u16(),
        code,
    );
}

#[then(regex = r#"^the page asks the "([^"]+)" question$"#)]
async fn assert_question(world: &mut ClosingWorld, code: String) {
    assert!(
        world.last_body.contains(&code),
        "expected page to mention {code}, got:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the page shows "([^"]+)"$"#)]
async fn assert_page_contains(world: &mut ClosingWorld, needle: String) {
    // The walker renders through Dioxus, which spells an apostrophe in a text
    // node as `&#39;`. The step is about the words lawyers read, so decode first.
    let body = decode_character_references(&world.last_body);
    assert!(
        body.contains(&needle),
        "expected page to contain {needle:?}, got:\n{}",
        world.last_body
    );
}

#[then(regex = r"^the questionnaire runtime has recorded (\d+) transitions?$")]
async fn assert_transitions(world: &mut ClosingWorld, expected: usize) {
    let events = StateMachineRuntime::events(
        world.runtime().as_ref(),
        MachineKind::Questionnaire,
        world.notation_id(),
    )
    .await;
    assert_eq!(events.len(), expected, "events: {events:?}");
}

#[then(regex = r#"^the last transition lands on "([^"]+)"$"#)]
async fn assert_last_state(world: &mut ClosingWorld, name: String) {
    let events = StateMachineRuntime::events(
        world.runtime().as_ref(),
        MachineKind::Questionnaire,
        world.notation_id(),
    )
    .await;
    let last = events.last().expect("at least one transition");
    let expected = if name == "END" {
        StateName::end()
    } else {
        StateName::from(name.as_str())
    };
    assert_eq!(last.to, expected, "events: {events:?}");
}

#[tokio::main]
async fn main() {
    ClosingWorld::cucumber()
        .run_and_exit("tests/features/closing_letter.feature")
        .await;
}
