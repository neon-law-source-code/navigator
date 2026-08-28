//! Cucumber runner for `features/workshop_navigator_walkthrough.feature`.
//!
//! Grounds the workshop README's prose ("Using the Neon Law Navigator to
//! Rapidly Solve Legal Outcomes") in real Neon Law Navigator behavior. Every
//! scenario maps directly onto a Bloom-tagged learning objective in
//! the README — if a scenario breaks, the page is stale.
//!
//! The attorney is the actor in every `When` step; Neon Law Navigator is the
//! instrument. Scorpio's trust claim (from the engineer council
//! review) is asserted at the bottom: the notation's `state` is
//! `draft` until the attorney explicitly advances the workflow.
//!
//! Binding a notation is a `mcp::tools::requires_confirmation` act, so
//! the walk runs over the supervised A2A surface (`POST
//! /app/api/aida/rpc`): the attorney names the `create_notation` skill,
//! the task pauses in `input-required`, and the same attorney
//! authorizes it before anything is written. `/mcp` withholds the skill
//! outright, which is exactly the workshop's point — the instrument
//! never binds a matter on its own.

#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::{policy::PolicyClient, SessionStore};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;
use workflows::InMemoryRuntime;

/// Stable code for the workshop's retainer template. Used by the
/// `create_notation` skill to look up the template row inserted
/// in the Background.
const RETAINER_TEMPLATE_CODE: &str = "onboarding__letter";

#[derive(Default, World)]
#[world(init = Self::default)]
struct WorkshopWorld {
    app: Option<axum::Router>,
    storage: Option<Arc<dyn cloud::StorageService>>,
    /// The stock local attorney persona whose firm-side participation scopes
    /// the seeded litigation matter.
    attorney_email: Option<String>,
    project_id: Option<Uuid>,
    notation_id: Option<Uuid>,
    /// JSON-RPC `id` counter so each call gets a fresh request id.
    next_rpc_id: u64,
    /// Most recent A2A Task (paused, completed, or failed).
    last_task: Option<Value>,
}

impl std::fmt::Debug for WorkshopWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkshopWorld")
            .field("attorney_email", &self.attorney_email)
            .field("project_id", &self.project_id)
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl WorkshopWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
    fn storage(&self) -> &Arc<dyn cloud::StorageService> {
        self.storage.as_ref().expect("storage not built")
    }
    fn fresh_rpc_id(&mut self) -> u64 {
        self.next_rpc_id += 1;
        self.next_rpc_id
    }

    /// The structured tool output a completed A2A Task carries: the
    /// `data` Part of its single artifact, which is the tool's
    /// `structuredContent` verbatim.
    fn task_data(&self) -> &Value {
        &self.task()["artifacts"][0]["parts"][1]["data"]
    }

    fn task(&self) -> &Value {
        self.last_task.as_ref().expect("no A2A task captured")
    }

    /// POST one JSON-RPC message to the A2A endpoint as the firm
    /// attorney and capture the Task. Injecting the
    /// [`portal::Principal`] mirrors the prod auth middleware: the tier
    /// gate and the confirmation gate both resolve the caller against
    /// `persons`, so an anonymous request could neither dispatch nor
    /// approve.
    async fn post_as_attorney(&mut self, body: Value) {
        let email = self
            .attorney_email
            .clone()
            .expect("the workshop seed registers the attorney persona");
        let mut req = Request::builder()
            .method("POST")
            .uri("/app/api/aida/rpc")
            .header(
                "authorization",
                portal::test_support::lawyer_bearer_header(),
            )
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        req.extensions_mut().insert(portal::Principal::new(email));
        let resp = self.app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "A2A HTTP status");
        let raw = body_string(resp).await;
        let envelope: Value = serde_json::from_str(&raw).expect("A2A response is JSON");
        assert!(
            envelope.get("error").is_none(),
            "expected `result`, got JSON-RPC `error`: {envelope}",
        );
        self.last_task = Some(envelope["result"].clone());
    }

    /// Name a skill directly, through the `metadata.skill` entry point,
    /// with its arguments alongside.
    async fn name_skill(&mut self, skill: &str, arguments: Value) {
        let rpc_id = self.fresh_rpc_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "method": "message/send",
            "params": { "message": {
                "messageId": format!("m-{rpc_id}"),
                "role": "user",
                "kind": "message",
                "parts": [],
                "metadata": { "skill": skill, "arguments": arguments }
            }}
        });
        self.post_as_attorney(body).await;
    }

    /// Answer the pending `input-required` authorization with a yes, so
    /// the supervised act runs under the attorney's approval.
    async fn authorize_pending(&mut self) {
        let task_id = self.task()["id"]
            .as_str()
            .expect("paused task carries an id")
            .to_string();
        let context_id = self.task()["contextId"]
            .as_str()
            .expect("paused task carries a contextId")
            .to_string();
        let rpc_id = self.fresh_rpc_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "method": "message/send",
            "params": { "message": {
                "messageId": format!("m-{rpc_id}"),
                "role": "user",
                "kind": "message",
                "taskId": task_id,
                "contextId": context_id,
                "parts": [{ "kind": "data", "data": { "confirmation": "yes" } }]
            }}
        });
        self.post_as_attorney(body).await;
    }
}

#[given("a fresh dev Navigator app with the sample-matter workshop seed")]
async fn build_app_with_sample_seed(world: &mut WorkshopWorld) {
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("workshop-navigator-walkthrough").await;
    store::seed::seed_environment(
        &surreal,
        &storage,
        store::DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .expect("seed the sample-matter fixture");
    let litigation = store::projects::find_by_code(&surreal, "sample-litigation")
        .await
        .expect("query litigation matter")
        .expect("dev seed opens the litigation matter");
    let lawyer = store::persons::find_by_email_ci(&surreal, "lawyer@neonlaw.com")
        .await
        .expect("query local lawyer persona")
        .expect("dev seed registers the local lawyer persona");

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = app_state(
        runtime,
        storage.clone(),
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    let router = features::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    world.app = Some(router);
    world.storage = Some(storage);
    world.project_id = Some(litigation.id);
    world.attorney_email = Some(lawyer.email);
}

#[then(regex = r#"^the schema defines a "([^"]+)" table$"#)]
async fn schema_defines_table(_world: &mut WorkshopWorld, table: String) {
    let tables = store::schema::introspect(&features::shared_surreal().await)
        .await
        .expect("introspect the schema");
    assert!(
        tables.contains_key(&table),
        "expected table {table:?} to be defined (every Neon Law Navigator noun must be a real \
         schema entity); the schema defines: {:?}",
        tables.keys().collect::<Vec<_>>(),
    );
}

#[then(regex = r#"^a project named "([^"]+)" exists in the database$"#)]
async fn project_exists_named(world: &mut WorkshopWorld, name: String) {
    let id = world.project_id.expect("no project id captured");
    let row = store::projects::find_by_id(&features::shared_surreal().await, id)
        .await
        .expect("project lookup")
        .expect("project row");
    assert_eq!(row.name, name, "project name");
}

#[then(regex = r#"^the project status is "([^"]+)"$"#)]
async fn project_status_is(world: &mut WorkshopWorld, expected: String) {
    let id = world.project_id.expect("no project id captured");
    let row = store::projects::find_by_id(&features::shared_surreal().await, id)
        .await
        .expect("project lookup")
        .expect("project row");
    assert_eq!(row.status, expected, "project status");
}

#[when("the attorney binds the retainer template as a notation")]
async fn attorney_binds_notation(world: &mut WorkshopWorld) {
    // The notation hangs on the seeded matter; its respondent is the
    // matter's client DRI (the seeded client account), not the lawyer presenter.
    let project_id = world
        .project_id
        .expect("the attorney opens the Project before binding a notation");
    world
        .name_skill(
            "create_notation",
            json!({
                "template_code": RETAINER_TEMPLATE_CODE,
                "project_id": project_id,
            }),
        )
        .await;
    // Binding a notation is a supervised act, so nothing is written
    // until the attorney answers the pause. That is the workshop's
    // point: the instrument never acts on its own.
    assert_eq!(
        world.task()["status"]["state"],
        "input-required",
        "create_notation must pause for the attorney: {}",
        world.task(),
    );
    world.authorize_pending().await;
    assert_eq!(
        world.task()["status"]["state"],
        "completed",
        "the authorized binding should have run: {}",
        world.task(),
    );
    let id_str = world.task_data()["notation_id"]
        .as_str()
        .expect("artifact data carries the notation_id");
    world.notation_id = Some(Uuid::parse_str(id_str).expect("notation id is a UUID"));
}

#[then("a notation row exists linking the retainer template to the client")]
async fn notation_links_template_to_attorney(world: &mut WorkshopWorld) {
    let id = world.notation_id.expect("no notation id captured");
    let row = store::notations::find_by_id(&features::shared_surreal().await, id)
        .await
        .expect("notation lookup")
        .expect("notation row");
    let person_row = store::persons::find_by_id(&features::shared_surreal().await, row.person_id)
        .await
        .expect("person lookup")
        .expect("person row");
    assert_eq!(
        person_row.email, "client@neonlaw.com",
        "notation respondent"
    );
    let template_row =
        store::templates::find_by_id(&features::shared_surreal().await, row.template_id)
            .await
            .expect("template lookup")
            .expect("template row");
    assert_eq!(
        template_row.code, RETAINER_TEMPLATE_CODE,
        "notation template code",
    );
}

#[then(regex = r#"^the retainer template body carries the "([^"]+)" placeholder$"#)]
async fn retainer_template_body_carries_placeholder(world: &mut WorkshopWorld, needle: String) {
    let surreal = features::shared_surreal().await;
    // Through the notation's pinned `template_id`, the way the sibling step
    // The notation pins the exact catalog version, so the body under test is
    // the one this notation actually bound.
    let id = world.notation_id.expect("no notation id captured");
    let notation_row = store::notations::find_by_id(&surreal, id)
        .await
        .expect("notation lookup")
        .expect("notation row");
    let row = store::templates::find_by_id(&surreal, notation_row.template_id)
        .await
        .expect("template lookup")
        .expect("retainer template row");
    assert_eq!(
        row.code, RETAINER_TEMPLATE_CODE,
        "the bound retainer template"
    );
    let body = store::templates::body(&surreal, world.storage(), &row)
        .await
        .expect("retainer body in storage");
    assert!(
        body.contains(&needle),
        "retainer template body must contain {needle:?}; got body: {body:?}",
    );
}

#[then(regex = r#"^the notation state is "([^"]+)"$"#)]
async fn notation_state_is(world: &mut WorkshopWorld, expected: String) {
    let id = world.notation_id.expect("no notation id captured");
    let row = store::notations::find_by_id(&features::shared_surreal().await, id)
        .await
        .expect("notation lookup")
        .expect("notation row");
    assert_eq!(row.state, expected, "notation state");
}

#[then(regex = r#"^the notation state is not "([^"]+)"$"#)]
async fn notation_state_is_not(world: &mut WorkshopWorld, forbidden: String) {
    let id = world.notation_id.expect("no notation id captured");
    let row = store::notations::find_by_id(&features::shared_surreal().await, id)
        .await
        .expect("notation lookup")
        .expect("notation row");
    assert_ne!(
        row.state, forbidden,
        "Scorpio's load-bearing trust claim: the retainer must not be {forbidden:?} until the attorney advances the workflow"
    );
}

#[tokio::main]
async fn main() {
    WorkshopWorld::cucumber()
        // Every scenario seeds the same SurrealDB portfolio, so running them
        // concurrently can bind a shared Project row to another scenario's
        // Entity record.
        .max_concurrent_scenarios(1)
        .run_and_exit("tests/features/workshop_navigator_walkthrough.feature")
        .await;
}
