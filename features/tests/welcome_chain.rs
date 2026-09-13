//! Cucumber runner for `features/welcome_chain.feature`.
//!
//! Drives the A2A surface (`/app/api/mcp/rpc`) end-to-end through the full
//! fully composed app — auth stack, route mounting, and all. A
//! scripted [`WelcomeChainRouter`] stands in for Gemini so the agentic
//! loop runs deterministically; everything it drives (the real
//! `show_person` + `send_welcome_email` tools, the DB, the welcome
//! email rendered through [`CapturingEmail`]) is production code. The
//! visible artifact is the captured welcome email — proof the two-step
//! chain actually sent, not just that the loop picked the right tools.
//!
//! Why a stub router and not real Gemini: a live model is
//! non-deterministic and costs money per call, so it has no place in
//! CI. The proof that Gemini *itself* picks lookup-then-send is the
//! out-of-band Vertex probe, kept with the router docs.

// Cucumber's step-attribute macros want `async fn` everywhere.
#![allow(clippy::unused_async)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state_with_email, body_string, fs_storage};
use portal::agent_router::{AgentRouter, RoutedCall, RouterError, Step, Turn};
use portal::email::CapturingEmail;
use portal::{policy::PolicyClient, SessionStore};
use serde_json::{json, Value};
use tower::ServiceExt;
use workflows::InMemoryRuntime;

/// Scripted stand-in for Gemini. Reads the conversation the handler
/// hands it and drives the exact chain the real model *should* run:
///
///   1. No tool has run yet → parse the address out of the user's
///      message and call `show_person` to resolve the person.
///   2. The lookup came back → pull the id out of its result (fed back
///      to us in the history) and call `send_welcome_email` with it.
///   3. The welcome was sent → finish with a plain-text confirmation.
struct WelcomeChainRouter;

#[async_trait::async_trait]
impl AgentRouter for WelcomeChainRouter {
    async fn next_step(&self, history: &[Turn], _skills: &[Value]) -> Result<Step, RouterError> {
        let last_result = history.iter().rev().find_map(|t| match t {
            Turn::Result { tool_name, content } => Some((tool_name.as_str(), content)),
            _ => None,
        });
        match last_result {
            None => {
                let user_text = history
                    .iter()
                    .find_map(|t| match t {
                        Turn::User(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let email = user_text
                    .split_whitespace()
                    .find(|word| word.contains('@'))
                    .unwrap_or_default();
                Ok(Step::Call(RoutedCall {
                    tool_name: "show_person".to_string(),
                    arguments: json!({ "email": email }),
                }))
            }
            Some(("show_person", content)) => {
                let person_id = content["structuredContent"]["persons"][0]["id"]
                    .as_str()
                    .expect("show_person must return a match for the seeded person");
                Ok(Step::Call(RoutedCall {
                    tool_name: "send_welcome_email".to_string(),
                    arguments: json!({ "person_id": person_id }),
                }))
            }
            Some(("send_welcome_email", _)) => {
                Ok(Step::Done("Sent the welcome email.".to_string()))
            }
            Some((other, _)) => panic!("unexpected tool in welcome chain: {other}"),
        }
    }
}

#[derive(Default, World)]
#[world(init = Self::default)]
struct ChainWorld {
    app: Option<axum::Router>,
    captured: Option<Arc<CapturingEmail>>,
    last_task: Option<Value>,
}

impl std::fmt::Debug for ChainWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainWorld").finish_non_exhaustive()
    }
}

impl ChainWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
    fn captured(&self) -> Vec<portal::email::OutboundEmail> {
        self.captured
            .as_ref()
            .expect("capturing email not wired")
            .captured()
    }
    fn task(&self) -> &Value {
        self.last_task.as_ref().expect("no A2A task captured")
    }
}

#[given("a CapturingEmail-backed Neon Law Navigator app whose Navigator MCP router runs the lookup-then-send chain")]
async fn build_app(world: &mut ChainWorld) {
    let storage = fs_storage("welcome-chain").await;
    let runtime = Arc::new(InMemoryRuntime::new());
    let email = Arc::new(CapturingEmail::new());
    let mut state = app_state_with_email(
        runtime,
        storage,
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
        email.clone(),
    )
    .await;
    // Production runs both timelines on one runtime instance; the test
    // helper splits them and leaves the questionnaire timeline bare.
    // `send_welcome_email` triggers on the questionnaire runtime, so
    // point it at the dispatching workflow runtime — otherwise the
    // welcome never reaches CapturingEmail.
    state.questionnaire_runtime = state.workflow_runtime.clone();
    state.a2a_router = Some(Arc::new(WelcomeChainRouter));
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
    world.captured = Some(email);
}

/// The firm-side operator driving Navigator MCP from Gemini Enterprise. The
/// confirmation gate only lets a lawyer/admin principal authorize a
/// client-facing send, so every request in this scenario is made as
/// this principal — injected the same way the prod auth middleware
/// populates it.
const LAWYER_EMAIL: &str = "lawyer@neonlaw.com";

#[given(regex = r#"^a persons row for "([^"]+)" with email "([^"]+)"$"#)]
async fn seed_person(world: &mut ChainWorld, name: String, email: String) {
    seed_person_with_role(world, name, email, store::persons::Role::Client).await;
}

#[given(regex = r#"^a lawyer persons row for "([^"]+)" with email "([^"]+)"$"#)]
async fn seed_lawyer_person(world: &mut ChainWorld, name: String, email: String) {
    seed_person_with_role(world, name, email, store::persons::Role::Lawyer).await;
}

async fn seed_person_with_role(
    _world: &mut ChainWorld,
    name: String,
    email: String,
    role: store::persons::Role,
) {
    store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson::with_role(name, email, role),
    )
    .await;
}

/// POST a `message/send` to the A2A RPC surface as the firm lawyer
/// principal, asserting the HTTP + JSON-RPC envelope and stashing the
/// returned Task. Injecting the [`portal::Principal`] mirrors the prod
/// auth middleware: the confirmation role gate resolves the approver
/// against `persons`, so an anonymous request could never authorize.
async fn post_as_lawyer(world: &mut ChainWorld, body: Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/app/api/mcp/rpc")
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
    assert_eq!(resp.status(), StatusCode::OK, "A2A HTTP status");
    let raw = body_string(resp).await;
    let envelope: Value = serde_json::from_str(&raw).expect("A2A response is JSON");
    assert!(
        envelope.get("error").is_none(),
        "expected `result`, got JSON-RPC `error`: {envelope}"
    );
    world.last_task = Some(envelope["result"].clone());
}

#[when(regex = r#"^Navigator MCP receives the A2A message "([^"]+)"$"#)]
async fn send_message(world: &mut ChainWorld, text: String) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "message/send",
        "params": { "message": {
            "messageId": "m-1",
            "role": "user",
            "kind": "message",
            "parts": [{ "kind": "text", "text": text }]
        }}
    });
    post_as_lawyer(world, body).await;
}

#[then(
    regex = r#"^Navigator MCP pauses for authorization to send the welcome email to "([^"]+)"$"#
)]
async fn assert_pauses(world: &mut ChainWorld, person: String) {
    let task = world.task();
    assert_eq!(
        task["status"]["state"], "input-required",
        "expected the side-effecting send to pause for authorization: {task}"
    );
    // Nothing has run yet — the lookup is a read, not an artifact.
    assert!(
        task["artifacts"].as_array().is_none_or(Vec::is_empty),
        "no side-effect should run before authorization: {task}"
    );
    let prompt = task["status"]["message"]["parts"][0]["text"]
        .as_str()
        .expect("authorization prompt text");
    assert!(
        prompt.contains("Authorize this action?"),
        "prompt must ask for authorization, got: {prompt}"
    );
    assert!(
        prompt.contains("Send Welcome Email"),
        "prompt must name the action, got: {prompt}"
    );
    assert!(
        prompt.contains(&person),
        "prompt must name the person {person:?}, got: {prompt}"
    );
}

#[then("no email has been captured yet")]
async fn assert_nothing_sent_yet(world: &mut ChainWorld) {
    let captured = world.captured();
    assert!(
        captured.is_empty(),
        "no email should be sent before authorization: {captured:?}"
    );
}

#[when(regex = r#"^the firm authorizes the pending action with "([^"]+)"$"#)]
async fn authorize(world: &mut ChainWorld, reply: String) {
    let task_id = world.task()["id"]
        .as_str()
        .expect("paused task carries an id")
        .to_string();
    let context_id = world.task()["contextId"]
        .as_str()
        .expect("paused task carries a contextId")
        .to_string();
    // The confirmation gate reads only the structured yes/no selection
    // the `input-required` hint asks for — there is no free-text input —
    // so the firm authorizes by sending the choice as a `data` Part, not
    // a typed text reply.
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "message/send",
        "params": { "message": {
            "messageId": "m-2",
            "role": "user",
            "kind": "message",
            "taskId": task_id,
            "contextId": context_id,
            "parts": [{ "kind": "data", "data": { "confirmation": reply } }]
        }}
    });
    post_as_lawyer(world, body).await;
}

#[then("the A2A task completes with the welcome send as its artifact")]
async fn assert_completed(world: &mut ChainWorld) {
    let task = world.task();
    assert_eq!(task["status"]["state"], "completed", "task: {task}");
    // The artifact is the terminal action (the send), not the
    // intermediate lookup.
    assert_eq!(task["artifacts"][0]["name"], "send_welcome_email");
}

#[then(regex = r"^exactly (\d+) captured emails? exists?$")]
async fn assert_count(world: &mut ChainWorld, n: usize) {
    let captured = world.captured();
    assert_eq!(captured.len(), n, "captured: {captured:?}");
}

#[then(regex = r#"^the captured email is addressed to "([^"]+)"$"#)]
async fn assert_to(world: &mut ChainWorld, expected: String) {
    let captured = world.captured();
    assert_eq!(captured.first().expect("one captured email").to, expected);
}

#[then(regex = r#"^the captured email subject is "([^"]+)"$"#)]
async fn assert_subject(world: &mut ChainWorld, expected: String) {
    let captured = world.captured();
    assert_eq!(
        captured.first().expect("one captured email").subject,
        expected
    );
}

#[tokio::main]
async fn main() {
    ChainWorld::cucumber()
        .run_and_exit("tests/features/welcome_chain.feature")
        .await;
}
