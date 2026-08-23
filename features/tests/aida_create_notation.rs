//! Cucumber runner for `features/aida_create_notation.feature`.
//!
//! Drives the conversational notation walk over the A2A surface
//! (`POST /app/api/aida/rpc`) through the fully composed app: auth
//! stack, route mounting, and all. The client names the
//! `create_notation` / `answer_notation` skills directly, which is the
//! `metadata.skill` path every non-Gemini A2A client uses, so no model
//! stands between the scenario and the real tools.
//!
//! Both skills are `mcp::tools::requires_confirmation` acts, so every
//! call pauses in `input-required` and the firm lawyer authorizes it
//! before the write runs. Below that gate everything is production
//! code: the real tools, the real questionnaire runtime, a real store.
//!
//! The walk used to run over `/mcp`, and cannot any more: MCP has no
//! state to pause in, so that endpoint withholds both skills and
//! refuses one named anyway. The last scenario pins that refusal on
//! the same app, so the supervised path and the withheld one are
//! proved against one another rather than in separate suites.

#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::{policy::PolicyClient, SessionStore};
use serde_json::{json, Value};
use store::seed;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime, StateName};

/// The firm-side principal driving AIDA. Only a lawyer/admin may
/// authorize a supervised act, and `create_notation` additionally
/// checks that this lawyer is scoped to the matter, so this identity
/// is both the caller and the approver in every scenario.
const LAWYER_EMAIL: &str = "lawyer@neonlaw.com";

#[derive(Default, World)]
#[world(init = Self::default)]
struct NotationWorld {
    app: Option<axum::Router>,
    runtime: Option<Arc<InMemoryRuntime>>,
    notation_id: Option<Uuid>,
    /// The matter the lawyer opens the notation on. AIDA never creates
    /// one: `create_notation` names an existing Project.
    project_id: Option<Uuid>,
    /// JSON-RPC `id` counter so each call gets a fresh request id.
    next_rpc_id: u64,
    /// Most recent A2A Task (paused, completed, or failed).
    last_task: Option<Value>,
    /// Most recent `/mcp` `result` payload, for the withheld scenario.
    last_mcp_result: Option<Value>,
}

impl std::fmt::Debug for NotationWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotationWorld")
            .field("notation_id", &self.notation_id)
            .field("last_task", &self.last_task)
            .finish_non_exhaustive()
    }
}

impl NotationWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
    fn runtime(&self) -> &Arc<InMemoryRuntime> {
        self.runtime.as_ref().expect("runtime not built")
    }
    fn task(&self) -> &Value {
        self.last_task.as_ref().expect("no A2A task captured")
    }
    /// The structured tool output a completed Task carries: the `data`
    /// Part of its single artifact, which is the tool's
    /// `structuredContent` verbatim.
    fn task_data(&self) -> &Value {
        &self.task()["artifacts"][0]["parts"][1]["data"]
    }

    fn fresh_rpc_id(&mut self) -> u64 {
        self.next_rpc_id += 1;
        self.next_rpc_id
    }

    /// POST one JSON-RPC message to the A2A endpoint as the firm
    /// lawyer and capture the Task. Injecting the [`portal::Principal`]
    /// mirrors the prod auth middleware: the tier gate and the
    /// confirmation gate both resolve the caller against `persons`, so
    /// an anonymous request could neither dispatch nor approve.
    async fn post_as_lawyer(&mut self, body: Value) {
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
        req.extensions_mut()
            .insert(portal::Principal::new(LAWYER_EMAIL));
        let resp = self.app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "A2A HTTP status");
        let raw = body_string(resp).await;
        let envelope: Value = serde_json::from_str(&raw).expect("A2A response is JSON");
        assert!(
            envelope.get("error").is_none(),
            "expected `result`, got JSON-RPC `error`: {envelope}",
        );
        self.last_task = Some(envelope["result"].clone());
        // A completed `create_notation` carries the id every later
        // answer addresses. Captured here so the walk never has to
        // thread it through a step.
        if let Some(id) = self.task_data()["notation_id"].as_str() {
            self.notation_id = Some(Uuid::parse_str(id).expect("notation_id is a UUID"));
        }
    }

    /// Name a skill directly, through the `metadata.skill` entry point, with
    /// the arguments alongside it.
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
        self.post_as_lawyer(body).await;
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut NotationWorld) {
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("aida-create-notation").await;
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

#[given(regex = r#"^a lawyer persons row for "([^"]+)" with email "([^"]+)"$"#)]
async fn seed_lawyer(_world: &mut NotationWorld, name: String, email: String) {
    store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Lawyer),
    )
    .await;
}

#[given(regex = r#"^a seeded person "([^"]+)" with email "([^"]+)"$"#)]
async fn seed_person(_world: &mut NotationWorld, name: String, email: String) {
    store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
    )
    .await;
}

#[given(regex = r#"^an open matter whose client is "([^"]+)"$"#)]
async fn open_matter(world: &mut NotationWorld, email: String) {
    let surreal = features::shared_surreal().await;
    let client = store::persons::find_by_email_ci(&surreal, &email)
        .await
        .unwrap()
        .expect("client person seeded in a prior step");
    // A matter always opens against a pre-existing entity
    // (`projects.entity_id` is NOT NULL).
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("mcp-matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        client.id,
        store::projects::DriSide::Client,
    )
    .await
    .unwrap();
    // The firm side of the same matter. A Lawyer is project-scoped like
    // everyone else (`create_notation` refuses an actor with no
    // participation row), so the matter is not usable by the seeded
    // lawyer until this row exists.
    let lawyer = store::persons::find_by_email_ci(&surreal, LAWYER_EMAIL)
        .await
        .unwrap()
        .expect("firm lawyer seeded in a prior step");
    store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer")
        .await
        .unwrap();
    world.project_id = Some(project.id);
}

#[when(regex = r#"^the LLM names the create_notation skill for "([^"]+)" on that matter$"#)]
async fn name_create_notation(world: &mut NotationWorld, template_code: String) {
    let project_id = world.project_id.expect("no matter opened");
    world
        .name_skill(
            "create_notation",
            json!({
                "template_code": template_code,
                "project_id": project_id,
            }),
        )
        .await;
}

#[when(regex = r#"^the LLM names the answer_notation skill with code "([^"]+)" value "([^"]+)"$"#)]
async fn name_answer_notation(world: &mut NotationWorld, code: String, value: String) {
    let id = world.notation_id.expect("no notation_id captured");
    world
        .name_skill(
            "answer_notation",
            json!({
                "notation_id": id,
                "question_code": code,
                "value": value,
            }),
        )
        .await;
}

#[then(regex = r#"^AIDA pauses for authorization to "([^"]+)"$"#)]
async fn assert_paused(world: &mut NotationWorld, action: String) {
    let task = world.task();
    assert_eq!(
        task["status"]["state"], "input-required",
        "a supervised act must pause for authorization: {task}"
    );
    assert!(
        task["artifacts"].as_array().is_none_or(Vec::is_empty),
        "nothing may run before the approval: {task}"
    );
    let prompt = task["status"]["message"]["parts"][0]["text"]
        .as_str()
        .expect("authorization prompt text");
    assert!(
        prompt.contains("Authorize this action?"),
        "prompt must ask for authorization, got: {prompt}"
    );
    assert!(
        prompt.contains(action.as_str()),
        "prompt must name the action {action:?}, got: {prompt}"
    );
}

#[when("the firm authorizes the pending action")]
async fn authorize(world: &mut NotationWorld) {
    let task_id = world.task()["id"]
        .as_str()
        .expect("paused task carries an id")
        .to_string();
    let context_id = world.task()["contextId"]
        .as_str()
        .expect("paused task carries a contextId")
        .to_string();
    let rpc_id = world.fresh_rpc_id();
    // The gate reads only the structured yes/no selection its
    // `input-required` hint asks for, and there is no free-text input, so
    // the choice rides in a `data` Part.
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
    world.post_as_lawyer(body).await;
}

#[then(regex = r#"^the task completes with status "([^"]+)"$"#)]
async fn assert_completed_status(world: &mut NotationWorld, expected: String) {
    let task = world.task();
    assert_eq!(
        task["status"]["state"], "completed",
        "the authorized act should have run: {task}"
    );
    let actual = world.task_data()["status"]
        .as_str()
        .expect("artifact data carries the tool status");
    assert_eq!(actual, expected.as_str(), "{}", world.task());
}

#[then(regex = r#"^the next question is "([^"]+)"$"#)]
async fn assert_next_question(world: &mut NotationWorld, expected: String) {
    let actual = world.task_data()["next_question"]["code"]
        .as_str()
        .expect("artifact data carries the next question");
    assert_eq!(actual, expected.as_str(), "{}", world.task());
}

#[then("the notation has reached the questionnaire END state")]
async fn assert_end(world: &mut NotationWorld) {
    let id = world.notation_id.expect("no notation_id captured");
    let current = StateMachineRuntime::current_state(
        world.runtime().as_ref(),
        MachineKind::Questionnaire,
        id,
    )
    .await
    .expect("runtime should know this notation");
    assert_eq!(current, StateName::end(), "runtime state");
    // And the notation row exists under the seeded person.
    let surreal = features::shared_surreal().await;
    let row = store::notations::find_by_id(&surreal, id)
        .await
        .unwrap()
        .expect("notation row");
    assert_eq!(row.template_id.to_string().len(), 36);
}

#[then(regex = r#"^the task fails mentioning "([^"]+)"$"#)]
async fn assert_task_failed(world: &mut NotationWorld, needle: String) {
    let task = world.task();
    assert_eq!(
        task["status"]["state"], "failed",
        "expected a failed task, got {task}"
    );
    let text = task["status"]["message"]["parts"][0]["text"]
        .as_str()
        .expect("failed task carries a message");
    assert!(
        text.contains(needle.as_str()),
        "error `{text}` does not mention `{needle}`",
    );
}

#[when(regex = r#"^the LLM calls aida_create_notation for "([^"]+)" on that matter over /mcp$"#)]
async fn call_over_mcp(world: &mut NotationWorld, template_code: String) {
    let project_id = world.project_id.expect("no matter opened");
    let rpc_id = world.fresh_rpc_id();
    let body = json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "method": "tools/call",
        "params": {
            "name": "aida_create_notation",
            "arguments": {
                "template_code": template_code,
                "project_id": project_id,
            }
        }
    });
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(
            "authorization",
            portal::test_support::lawyer_bearer_header(),
        )
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    req.extensions_mut()
        .insert(portal::Principal::new(LAWYER_EMAIL));
    let resp = world.app().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "MCP HTTP status");
    let raw = body_string(resp).await;
    let envelope: Value = serde_json::from_str(&raw).expect("MCP response is JSON");
    assert!(
        envelope.get("error").is_none(),
        "expected `result`, got JSON-RPC `error`: {envelope}",
    );
    world.last_mcp_result = Some(envelope["result"].clone());
}

#[then("the MCP result refuses the act and routes the caller to the Navigator app")]
async fn assert_mcp_refusal(world: &mut NotationWorld) {
    let result = world
        .last_mcp_result
        .as_ref()
        .expect("no MCP result captured");
    assert_eq!(result["isError"], true, "expected a refusal, got {result}");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("refusal carries text");
    assert!(text.contains("aida_create_notation"), "got `{text}`");
    assert!(text.contains("approval"), "got `{text}`");
    assert!(text.contains("Navigator app"), "got `{text}`");
}

#[then("no notation exists on that matter")]
async fn assert_no_notation(world: &mut NotationWorld) {
    let project_id = world.project_id.expect("no matter opened");
    let surreal = features::shared_surreal().await;
    assert!(
        !store::notations::exists_for_project(&surreal, project_id)
            .await
            .unwrap(),
        "the refusal must precede dispatch: no Notation may have been created"
    );
}

#[tokio::main]
async fn main() {
    NotationWorld::run("tests/features/aida_create_notation.feature").await;
}
