//! Cucumber runner for `features/retainer_intake.feature`.
//!
//! Drives the admin retainer walker (`/lawyer/retainers/...` +
//! `/lawyer/notations/:id/step`) against an in-memory runtime,
//! mirroring `web/tests/retainer_walk_handler.rs` but expressed in
//! Gherkin.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{gherkin::Step, given, then, when, World};
use features::{app_state, body_string, form_encode, fs_storage};
use portal::{policy::PolicyClient, SessionStore};
use store::seed;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime, StateName};

const TEMPLATE_CODE: &str = "onboarding__letter";

#[derive(Default, World)]
#[world(init = Self::default)]
struct RetainerWorld {
    app: Option<axum::Router>,
    notation_id: Option<Uuid>,
    runtime: Option<Arc<InMemoryRuntime>>,
    last_status: Option<StatusCode>,
    last_body: String,
    last_location: Option<String>,
    final_status: Option<StatusCode>,
}

impl std::fmt::Debug for RetainerWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainerWorld")
            .field("notation_id", &self.notation_id)
            .field("last_status", &self.last_status)
            .field("last_location", &self.last_location)
            .field("final_status", &self.final_status)
            .finish_non_exhaustive()
    }
}

impl RetainerWorld {
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
async fn build_app(world: &mut RetainerWorld) {
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("retainer").await;
    seed::seed_canonical(&surreal, &storage)
        .await
        .expect("seed canonical");
    let runtime = Arc::new(InMemoryRuntime::new());
    let state = app_state(
        runtime.clone(),
        storage.clone(),
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    let router = features::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    world.app = Some(router);
    world.runtime = Some(runtime);
}

#[given(regex = r#"^a retainer notation for "([^"]+)" <([^>]+)> at BEGIN$"#)]
async fn seed_notation(world: &mut RetainerWorld, name: String, email: String) {
    let surreal = features::shared_surreal().await;
    let tmpl = store::templates::resolve(&features::shared_surreal().await, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed_canonical inserts onboarding__letter");
    let person =
        store::test_support::ensure_person(&surreal, &store::persons::NewPerson::new(name, email))
            .await;
    let proj = store::test_support::seed_project(&surreal, "retainer matter").await;
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
async fn lawyer_visits(world: &mut RetainerWorld, path: String) {
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
    world.last_location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    world.last_body = body_string(resp).await;
}

#[when(regex = r#"^the lawyer submits "([^"]*)" to (.+)$"#)]
async fn lawyer_submits(world: &mut RetainerWorld, value: String, path: String) {
    let uri = world.substitute(&path);
    let body = format!("value={}", form_encode(&value));
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
    world.last_status = Some(resp.status());
    world.last_location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    world.last_body = body_string(resp).await;
}

#[when("the lawyer submits the full questionnaire:")]
async fn lawyer_walks_all(world: &mut RetainerWorld, step: &Step) {
    let table = step.table.as_ref().expect("expected a data table");
    // First row is the header (`value`); skip it.
    let mut last_status = StatusCode::OK;
    for row in table.rows.iter().skip(1) {
        let value = row.first().expect("each row carries one cell").as_str();
        let body = format!("value={}", form_encode(value));
        let uri = world.substitute("/lawyer/notations/:id/step");
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
async fn assert_status(world: &mut RetainerWorld, code: u16) {
    assert_eq!(
        world.last_status.expect("no status captured").as_u16(),
        code,
        "body: {}",
        world.last_body
    );
}

#[then(regex = r"^the final response status is (\d+)$")]
async fn assert_final_status(world: &mut RetainerWorld, code: u16) {
    assert_eq!(
        world
            .final_status
            .expect("no final status captured")
            .as_u16(),
        code,
    );
}

#[then(regex = r#"^the page asks the "([^"]+)" question$"#)]
async fn assert_question(world: &mut RetainerWorld, code: String) {
    assert!(
        world.last_body.contains(&code),
        "expected page to mention {code}, got:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the page shows "([^"]+)"$"#)]
async fn assert_page_contains(world: &mut RetainerWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "expected page to contain {needle:?}, got:\n{}",
        world.last_body
    );
}

#[then(regex = r"^the response redirects back to (.+)$")]
async fn assert_redirect(world: &mut RetainerWorld, target: String) {
    let expected = world.substitute(&target);
    assert_eq!(world.last_location.as_deref(), Some(expected.as_str()));
}

#[then(regex = r"^the questionnaire runtime has recorded (\d+) transitions?$")]
async fn assert_transitions(world: &mut RetainerWorld, expected: usize) {
    let events = StateMachineRuntime::events(
        world.runtime().as_ref(),
        MachineKind::Questionnaire,
        world.notation_id(),
    )
    .await;
    assert_eq!(events.len(), expected, "events: {events:?}");
}

#[then(regex = r#"^the last transition lands on "([^"]+)"$"#)]
async fn assert_last_state(world: &mut RetainerWorld, name: String) {
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

#[then(regex = r#"^an answer row exists with value "([^"]+)"$"#)]
async fn assert_answer_row(world: &mut RetainerWorld, value: String) {
    // Scoped to *this scenario's* notation, not the whole table. The cucumber
    // suite shares one embedded engine across concurrently-running scenarios,
    // so a table-wide count would see every other scenario's "Libra" too. The
    // value is the JSONB primitive envelope `{"value": …}`.
    let rows: Vec<_> =
        store::answers::for_notation(&features::shared_surreal().await, world.notation_id())
            .await
            .unwrap()
            .into_iter()
            .filter(|a| store::answers::display_value(&a.value) == value)
            .collect();
    assert_eq!(rows.len(), 1, "expected one answer row for {value:?}");
}

#[then("a GET to /lawyer/notations/:id/step now redirects to /app/lawyer")]
async fn assert_post_end_redirect(world: &mut RetainerWorld) {
    let uri = world.substitute("/lawyer/notations/:id/step");
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
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/app/lawyer"),
    );
}

#[tokio::main]
async fn main() {
    RetainerWorld::cucumber()
        .run_and_exit("tests/features/retainer_intake.feature")
        .await;
}
