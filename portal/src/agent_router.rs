//! Natural-language → skill router for the A2A handler.
//!
//! A2A's design treats agents as opaque message handlers: the client
//! sends a freeform user message via `message/send` and the agent
//! decides which of its declared skills to invoke. Without a
//! routing layer, Navigator MCP can only serve clients that pre-fill
//! `metadata.skill` themselves — which Gemini Enterprise does not.
//!
//! This module owns the routing layer. The [`AgentRouter`] trait
//! takes the user's text plus the MCP tool descriptors and returns a
//! [`RoutedCall`] naming a specific tool and its arguments. The A2A
//! handler dispatches the returned call through the same
//! `mcp::tools::call_tool` that direct `metadata.skill` calls use, so
//! the routing decision is the *only* new code path.
//!
//! Two implementations ship:
//!
//! - [`GeminiRouter`] — production. Calls Vertex AI Gemini Flash's
//!   `generateContent` with the tool descriptors converted to
//!   `functionDeclarations`. Auth via Workload Identity: the pod's
//!   K8s ServiceAccount is bound to a GCP service account carrying
//!   `roles/aiplatform.user` in the configured `NAVIGATOR_GCP_PROJECT_ID`.
//!   No new API key — the pod fetches a short-lived access token
//!   from the GKE metadata server on each request.
//! - [`NullRouter`] — dev / KIND / tests. Always returns
//!   [`RouterError::NotConfigured`]; the A2A handler converts that
//!   into a helpful Task pointing at the `metadata.skill` backdoor.
//!
//! Adding another provider later (Claude direct, Vertex AI Anthropic,
//! local LLM) means writing a new `impl AgentRouter` and choosing it
//! from `lib.rs`. The A2A handler doesn't know which one it's using.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

/// What the router picked. Tool name uses the unprefixed A2A skill id
/// (`create_person`), not the MCP-namespaced one — the A2A handler
/// re-prepends the prefix via `to_mcp_tool_name` before dispatching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedCall {
    pub tool_name: String,
    pub arguments: Value,
}

/// One entry in the router's running view of the conversation. The
/// A2A handler owns this history and grows it a step at a time; the
/// router only reads it to decide the next move. Kept
/// provider-neutral — [`GeminiRouter`] maps these onto Vertex
/// `contents`; a future provider maps them onto its own transcript
/// shape.
#[derive(Debug, Clone)]
pub enum Turn {
    /// The human's free-form request. Always the first entry.
    User(String),
    /// A tool Navigator MCP chose and the handler then executed this round.
    /// `tool_name` is the unprefixed skill id (`show_person`).
    Call { tool_name: String, arguments: Value },
    /// The result that call returned, fed back to the model so it can
    /// decide the next step (or finish). `content` is the tool's MCP
    /// result payload, or an `{ "error": … }` object when the call
    /// failed — feeding failures back lets the model self-correct
    /// (e.g. a `NotFound` nudges it to look the person up differently)
    /// rather than the loop dead-ending on the first miss.
    Result { tool_name: String, content: Value },
}

/// What the router decided to do next, given the history so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Execute this call, append its result to the history, ask again.
    Call(RoutedCall),
    /// No more tools — Navigator MCP's final word. May be empty when the model
    /// stops without commentary.
    Done(String),
}

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// No router configured (KIND / local dev). Surfaced to A2A
    /// callers as a Task with a helpful text part — *not* a JSON-RPC
    /// error envelope.
    #[error("agent router not configured (set NAVIGATOR_GCP_PROJECT_ID + NAVIGATOR_GCP_LOCATION to enable Vertex AI routing)")]
    NotConfigured,
    /// Router answered but couldn't pick any skill for the input.
    #[error("router could not pick a skill for: {0}")]
    NoMatch(String),
    /// Transport / HTTP layer failure (metadata server, Vertex AI).
    #[error("router transport: {0}")]
    Transport(String),
    /// Router returned a malformed response we couldn't parse.
    #[error("router returned invalid response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait AgentRouter: Send + Sync {
    /// Decide the next step given the conversation so far. The first
    /// call passes `[Turn::User(text)]`; the handler then appends the
    /// chosen call and its result and calls again, until the router
    /// answers [`Step::Done`]. `skills` is the same list of MCP
    /// descriptors `mcp::tools::list_tools()` returns — each has
    /// `name`, `description`, `inputSchema`.
    ///
    /// This is the agentic loop's single LLM hop. Keeping it one step
    /// (rather than owning the whole loop) leaves tool execution — and
    /// thus the DB — entirely on the handler side, so the router stays
    /// a pure brain with no `McpState` dependency.
    async fn next_step(&self, history: &[Turn], skills: &[Value]) -> Result<Step, RouterError>;
}

// ---------------------------------------------------------------------------
// NullRouter — used when the env hasn't been configured (KIND / tests
// that don't exercise the router path).
// ---------------------------------------------------------------------------

/// No-op router. Always returns `NotConfigured`.
#[derive(Debug, Default)]
pub struct NullRouter;

#[async_trait]
impl AgentRouter for NullRouter {
    async fn next_step(&self, _history: &[Turn], _skills: &[Value]) -> Result<Step, RouterError> {
        Err(RouterError::NotConfigured)
    }
}

// ---------------------------------------------------------------------------
// GeminiRouter — Vertex AI Gemini Flash via Workload Identity.
// ---------------------------------------------------------------------------

/// Default GKE metadata-server URL for fetching a Workload Identity
/// access token. The Metadata server lives on a link-local address
/// inside every GKE pod; the `Metadata-Flavor: Google` header is
/// mandatory. Overridable via `GOOGLE_METADATA_URL` so tests can point
/// at a wiremock server.
pub const DEFAULT_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

/// Default Vertex AI region for the router. Matches the prod region
/// (`us-west4`) so we stay in the same locality as the rest of the
/// stack. Overridable via `NAVIGATOR_GCP_LOCATION`.
pub const DEFAULT_VERTEX_LOCATION: &str = "us-west4";

/// Default Vertex AI Gemini model. Flash is cheap, fast, and good at
/// the kind of "map this sentence to one of these tool descriptors"
/// task we hand it. `gemini-2.5-flash` is the unpinned alias that
/// Vertex resolves to the current 2.5-series Flash build. Keep it
/// unpinned: a suffixed reference like `gemini-2.0-flash-001` 404s in
/// every region. Overridable via `NAVIGATOR_ROUTER_MODEL`.
pub const DEFAULT_VERTEX_MODEL: &str = "gemini-2.5-flash";

/// Vertex AI–backed natural-language router.
pub struct GeminiRouter {
    project_id: String,
    location: String,
    model: String,
    /// Pluggable so tests can inject a static token instead of hitting
    /// the metadata server.
    token_source: Box<dyn TokenSource>,
    /// Vertex base URL. In production this is computed from
    /// `location` (e.g. `https://us-west4-aiplatform.googleapis.com`).
    /// Tests override via `for_test` to point at a wiremock server.
    vertex_base_url: String,
    http: reqwest::Client,
}

impl GeminiRouter {
    /// Production constructor. Returns `None` when
    /// `NAVIGATOR_GCP_PROJECT_ID` is unset — caller should fall back
    /// to `NullRouter` in that case.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("NAVIGATOR_GCP_PROJECT_ID").ok()?;
        if project_id.is_empty() {
            return None;
        }
        let location = std::env::var("NAVIGATOR_GCP_LOCATION")
            .unwrap_or_else(|_| DEFAULT_VERTEX_LOCATION.to_string());
        let model = std::env::var("NAVIGATOR_ROUTER_MODEL")
            .unwrap_or_else(|_| DEFAULT_VERTEX_MODEL.to_string());
        let metadata_url = std::env::var("GOOGLE_METADATA_URL")
            .unwrap_or_else(|_| DEFAULT_METADATA_TOKEN_URL.to_string());
        let vertex_base_url = format!("https://{location}-aiplatform.googleapis.com");
        Some(Self {
            project_id,
            location,
            model,
            token_source: Box::new(MetadataTokenSource::new(metadata_url)),
            vertex_base_url,
            http: reqwest::Client::new(),
        })
    }

    /// Test constructor — bypasses the metadata server with a static
    /// token and points Vertex at the given (wiremock) base URL.
    #[must_use]
    pub fn for_test(
        project_id: impl Into<String>,
        location: impl Into<String>,
        model: impl Into<String>,
        static_token: impl Into<String>,
        vertex_base_url: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
            model: model.into(),
            token_source: Box::new(StaticTokenSource::new(static_token.into())),
            vertex_base_url: vertex_base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
            self.vertex_base_url, self.project_id, self.location, self.model
        )
    }
}

#[async_trait]
impl AgentRouter for GeminiRouter {
    async fn next_step(&self, history: &[Turn], skills: &[Value]) -> Result<Step, RouterError> {
        let token = self.token_source.token().await?;
        let function_declarations = skills
            .iter()
            .map(skill_to_function_declaration)
            .collect::<Vec<_>>();
        let contents = history.iter().map(turn_to_content).collect::<Vec<_>>();
        let body = json!({
            "contents": contents,
            "tools": [{ "functionDeclarations": function_declarations }],
            // AUTO (not ANY): the model must be free to answer in plain
            // text instead of a function call, otherwise the loop can
            // never terminate. ANY would force one more call forever —
            // and it's what made "send welcome email to <addr>" pick
            // show_person and stop, since send_welcome_email had no
            // person_id to fill in a single forced shot.
            "toolConfig": {
                "functionCallingConfig": { "mode": "AUTO" }
            },
            "systemInstruction": {
                "parts": [{
                    "text": SYSTEM_INSTRUCTION
                }]
            }
        });
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| RouterError::Transport(format!("vertex POST: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| RouterError::Transport(format!("vertex body: {e}")))?;
        if !status.is_success() {
            return Err(RouterError::Transport(format!(
                "vertex http {status}: {body}",
                body = String::from_utf8_lossy(&bytes)
            )));
        }
        let parsed: VertexResponse = serde_json::from_slice(&bytes).map_err(|e| {
            RouterError::InvalidResponse(format!(
                "vertex response parse: {e}; body={body}",
                body = String::from_utf8_lossy(&bytes)
            ))
        })?;
        let (call, text) = parsed.into_call_or_text();
        Ok(match call {
            Some(call) => Step::Call(RoutedCall {
                tool_name: call.name,
                arguments: call.args,
            }),
            None => Step::Done(text),
        })
    }
}

/// Map one [`Turn`] onto a Vertex `contents[]` entry. The wire shape is
/// fixed by the Gemini API: a model `functionCall` is echoed back under
/// `role: "model"`, and its result returns under `role: "user"` (the
/// REST API only has `user`/`model` roles — there is no `function`
/// role) as a `functionResponse` part whose `response` MUST be a JSON
/// object.
fn turn_to_content(turn: &Turn) -> Value {
    match turn {
        Turn::User(text) => json!({
            "role": "user",
            "parts": [{ "text": text }]
        }),
        Turn::Call {
            tool_name,
            arguments,
        } => json!({
            "role": "model",
            "parts": [{ "functionCall": { "name": tool_name, "args": arguments } }]
        }),
        Turn::Result { tool_name, content } => json!({
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": tool_name,
                    "response": function_response_object(content),
                }
            }]
        }),
    }
}

/// Vertex requires `functionResponse.response` to be a JSON object. MCP
/// tool results usually already are, but a tool that returns a bare
/// array or scalar would be rejected — wrap those under `result`.
fn function_response_object(content: &Value) -> Value {
    if content.is_object() {
        content.clone()
    } else {
        json!({ "result": content })
    }
}

/// System instruction handed to Gemini Flash. Under
/// `functionCallingConfig: AUTO` the model chooses, each turn, between
/// calling a function and answering in text — so this prompt has to do
/// two jobs: push it to *act* via functions (not chat), and teach it
/// the lookup-then-act pattern so an action tool that needs an id it
/// wasn't handed gets that id from a lookup tool first instead of being
/// abandoned for the lookup. The loop terminates when the model stops
/// calling functions and replies with a plain-text confirmation.
const SYSTEM_INSTRUCTION: &str =
    "You are Navigator MCP's tool router. Use the declared functions to fully carry out \
     the user's request, then stop. Call functions to act — do not ask the user \
     for information you can obtain with a function. When a function needs an \
     identifier you were not given (for example a person_id), first call a \
     lookup function such as show_person to obtain it, then call the function \
     that needs it. Never create a record to satisfy a request to find, \
     message, email, or otherwise act on someone — look them up first; a \
     request that merely mentions an email address is not a request to create \
     a person. Fill arguments only from the conversation; never invent a \
     function or an argument value. When the request has been fully carried \
     out, reply with a short plain-text confirmation and no further function \
     call.";

/// Convert an MCP tool descriptor into Vertex AI's
/// `functionDeclarations[]` shape. The schemas are 1:1 — JSON Schema
/// on both sides — but Vertex rejects `additionalProperties` so we
/// strip it during translation. The tool name passes through unchanged:
/// the MCP tool name, the A2A skill id and the Gemini function name are
/// one string.
fn skill_to_function_declaration(descriptor: &Value) -> Value {
    let name = descriptor["name"].as_str().unwrap_or_default();
    let description = descriptor["description"].as_str().unwrap_or_default();
    let mut parameters = descriptor["inputSchema"].clone();
    sanitize_schema(&mut parameters);
    json!({
        "name": name,
        "description": description,
        "parameters": parameters,
    })
}

/// Strip Vertex-incompatible JSON Schema keywords in place. Vertex's
/// `parameters` field accepts a strict subset of JSON Schema; the
/// keyword that bites us in practice is `additionalProperties`, which
/// MCP descriptors use to lock down inputs. Strip it (Vertex defaults
/// to the same behavior anyway).
fn sanitize_schema(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("additionalProperties");
        // Recurse into properties + items so nested schemas are clean too.
        if let Some(props) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
            for (_, v) in props.iter_mut() {
                sanitize_schema(v);
            }
        }
        if let Some(items) = obj.get_mut("items") {
            sanitize_schema(items);
        }
    }
}

// ---------------------------------------------------------------------------
// Token sources.
// ---------------------------------------------------------------------------

#[async_trait]
trait TokenSource: Send + Sync {
    async fn token(&self) -> Result<String, RouterError>;
}

/// Production token source — fetches a fresh access token from the
/// GKE metadata server on each call. The metadata server is on
/// localhost (link-local 169.254.169.254 via the
/// `metadata.google.internal` DNS name) so latency is negligible. No
/// caching in v1; if the per-request overhead becomes hot we can add
/// a `Mutex<(String, Instant)>` with the response's `expires_in`.
struct MetadataTokenSource {
    url: String,
    http: reqwest::Client,
}

impl MetadataTokenSource {
    fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl TokenSource for MetadataTokenSource {
    async fn token(&self) -> Result<String, RouterError> {
        let resp = self
            .http
            .get(&self.url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| RouterError::Transport(format!("metadata GET: {e}")))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| RouterError::Transport(format!("metadata body: {e}")))?;
        if !status.is_success() {
            return Err(RouterError::Transport(format!(
                "metadata http {status}: {body}",
                body = String::from_utf8_lossy(&bytes)
            )));
        }
        let parsed: MetadataTokenResponse = serde_json::from_slice(&bytes).map_err(|e| {
            RouterError::InvalidResponse(format!(
                "metadata response parse: {e}; body={body}",
                body = String::from_utf8_lossy(&bytes)
            ))
        })?;
        Ok(parsed.access_token)
    }
}

#[derive(Debug, Deserialize)]
struct MetadataTokenResponse {
    access_token: String,
    #[serde(default)]
    #[allow(dead_code)]
    expires_in: u32,
    #[serde(default)]
    #[allow(dead_code)]
    token_type: String,
}

/// Test-only token source — returns a baked-in string. Production
/// never instantiates this.
struct StaticTokenSource(String);

impl StaticTokenSource {
    fn new(token: String) -> Self {
        Self(token)
    }
}

#[async_trait]
impl TokenSource for StaticTokenSource {
    async fn token(&self) -> Result<String, RouterError> {
        Ok(self.0.clone())
    }
}

// ---------------------------------------------------------------------------
// Vertex response parsing.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VertexResponse {
    #[serde(default)]
    candidates: Vec<VertexCandidate>,
}

#[derive(Debug, Deserialize)]
struct VertexCandidate {
    content: VertexContent,
}

#[derive(Debug, Deserialize)]
struct VertexContent {
    #[serde(default)]
    parts: Vec<VertexPart>,
}

#[derive(Debug, Deserialize)]
struct VertexPart {
    #[serde(rename = "functionCall", default)]
    function_call: Option<VertexFunctionCall>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VertexFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

impl VertexResponse {
    /// Reduce a response to the agentic-loop decision: the first
    /// `functionCall` if the model wants to act, otherwise the
    /// concatenated text it answered with. A `functionCall` always
    /// wins over text on the same turn (Gemini occasionally emits a
    /// brief "let me look that up" preamble alongside the call); a
    /// response with neither yields `(None, "")`, which the handler
    /// treats as an empty [`Step::Done`].
    fn into_call_or_text(self) -> (Option<VertexFunctionCall>, String) {
        let mut call = None;
        let mut text = String::new();
        for candidate in self.candidates {
            for part in candidate.content.parts {
                match part.function_call {
                    Some(fc) if call.is_none() => call = Some(fc),
                    _ => {
                        if let Some(t) = part.text {
                            text.push_str(&t);
                        }
                    }
                }
            }
        }
        (call, text)
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn skills_fixture() -> Vec<Value> {
        vec![
            json!({
                "name": "create_person",
                "description": "Create a new person record.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "email": { "type": "string" }
                    },
                    "required": ["name", "email"],
                    "additionalProperties": false
                }
            }),
            json!({
                "name": "list_jurisdictions",
                "description": "List all jurisdictions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            }),
        ]
    }

    fn user(text: &str) -> Vec<Turn> {
        vec![Turn::User(text.to_string())]
    }

    fn expect_call(step: Step) -> RoutedCall {
        match step {
            Step::Call(call) => call,
            Step::Done(text) => panic!("expected Step::Call, got Done({text:?})"),
        }
    }

    #[tokio::test]
    async fn null_router_always_returns_not_configured() {
        let router = NullRouter;
        let err = router
            .next_step(&user("anything"), &skills_fixture())
            .await
            .unwrap_err();
        assert!(matches!(err, RouterError::NotConfigured));
    }

    #[tokio::test]
    async fn gemini_router_maps_user_text_to_function_call() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/v1/projects/p/locations/us-west4/publishers/google/models/{DEFAULT_VERTEX_MODEL}:generateContent"
            )))
            .and(header("authorization", "Bearer test-token"))
            // The body must include the function declarations (sanity:
            // the prefix is stripped so Gemini sees `create_person`)
            // and the loop must run in AUTO mode so it can terminate.
            .and(body_partial_json(json!({
                "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
                "tools": [{
                    "functionDeclarations": [{ "name": "create_person" }, { "name": "list_jurisdictions" }]
                }]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "create_person",
                                "args": { "name": "Libra", "email": "libra@example.com" }
                            }
                        }]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiRouter::for_test(
            "p",
            "us-west4",
            DEFAULT_VERTEX_MODEL,
            "test-token",
            server.uri(),
        );
        let routed = expect_call(
            r.next_step(
                &user("create a person named Libra with email libra@example.com"),
                &skills_fixture(),
            )
            .await
            .unwrap(),
        );
        assert_eq!(routed.tool_name, "create_person");
        assert_eq!(routed.arguments["name"], "Libra");
        assert_eq!(routed.arguments["email"], "libra@example.com");
    }

    #[tokio::test]
    async fn gemini_router_returns_done_when_model_answers_in_text() {
        // AUTO mode: a text-only response (no functionCall) is the
        // loop's terminator, surfaced as Step::Done — not an error.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [{ "text": "Done — the welcome email is on its way." }]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiRouter::for_test("p", "us-west4", DEFAULT_VERTEX_MODEL, "t", server.uri());
        let step = r
            .next_step(&user("send the welcome email"), &skills_fixture())
            .await
            .unwrap();
        match step {
            Step::Done(text) => assert!(text.contains("welcome email")),
            Step::Call(c) => panic!("expected Done, got Call({c:?})"),
        }
    }

    #[tokio::test]
    async fn gemini_router_prefers_function_call_over_preamble_text() {
        // Gemini sometimes emits a chatty preamble part *and* a
        // functionCall in the same turn. The call must win so the
        // loop keeps acting.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [
                            { "text": "Sure, let me look that up." },
                            { "functionCall": { "name": "list_jurisdictions", "args": {} } }
                        ]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiRouter::for_test("p", "us-west4", DEFAULT_VERTEX_MODEL, "t", server.uri());
        let call = expect_call(
            r.next_step(&user("list jurisdictions"), &skills_fixture())
                .await
                .unwrap(),
        );
        assert_eq!(call.tool_name, "list_jurisdictions");
    }

    #[tokio::test]
    async fn gemini_router_serializes_history_into_contents() {
        // The second hop of a chain: the handler has already run
        // show_person and fed the result back. Assert the request
        // carries the model `functionCall` (role model) and the tool
        // result (role user, functionResponse) — the exact wire shape
        // multi-turn function calling requires — and that the router
        // then picks the action tool.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "contents": [
                    { "role": "user", "parts": [{ "text": "welcome libra" }] },
                    { "role": "model", "parts": [{ "functionCall": { "name": "show_person" } }] },
                    { "role": "user", "parts": [{
                        "functionResponse": { "name": "show_person", "response": { "id": "p-1" } }
                    }] }
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": { "name": "create_person", "args": { "name": "Libra" } }
                        }]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiRouter::for_test("p", "us-west4", DEFAULT_VERTEX_MODEL, "t", server.uri());
        let history = vec![
            Turn::User("welcome libra".to_string()),
            Turn::Call {
                tool_name: "show_person".to_string(),
                arguments: json!({ "name": "libra" }),
            },
            Turn::Result {
                tool_name: "show_person".to_string(),
                content: json!({ "id": "p-1" }),
            },
        ];
        let call = expect_call(r.next_step(&history, &skills_fixture()).await.unwrap());
        assert_eq!(call.tool_name, "create_person");
    }

    #[tokio::test]
    async fn gemini_router_returns_transport_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .expect(1)
            .mount(&server)
            .await;
        let r = GeminiRouter::for_test("p", "us-west4", DEFAULT_VERTEX_MODEL, "t", server.uri());
        let err = r
            .next_step(&user("anything"), &skills_fixture())
            .await
            .unwrap_err();
        assert!(matches!(err, RouterError::Transport(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn real_gemini_chains_lookup_then_welcome() {
        // Live, paid, non-deterministic Vertex probe. Opt in explicitly
        // so it never costs money or flakes in the default suite or CI —
        // it self-skips (a green no-op) unless `NAVIGATOR_RUN_LIVE_VERTEX`
        // is set. With the flag set (and GCP creds present) it runs for
        // real: `NAVIGATOR_RUN_LIVE_VERTEX=1 cargo test -p portal --lib real_gemini`.
        if std::env::var("NAVIGATOR_RUN_LIVE_VERTEX").is_err() {
            eprintln!(
                "skipping live Vertex probe; set NAVIGATOR_RUN_LIVE_VERTEX=1 \
                 (needs GCP creds, costs money, non-deterministic)"
            );
            return;
        }
        let router = GeminiRouter::from_env()
            .expect("set NAVIGATOR_GCP_PROJECT_ID to run the live Vertex probe");
        let catalog = mcp::tools::list_tools();

        // Step 1: a free-form welcome request must resolve the person
        // first — never stop at the lookup, never invent an address.
        let mut history = vec![Turn::User(
            "send a welcome email to nick@neonlaw.com".to_string(),
        )];
        let call1 = expect_call(router.next_step(&history, &catalog).await.unwrap());
        assert_eq!(
            call1.tool_name, "show_person",
            "real Gemini should look the person up first"
        );

        // Feed back a synthetic lookup result. The handler would run the
        // real tool here; the router is DB-free, so a stand-in row is
        // enough to exercise the model's chaining decision.
        let person_id = "11111111-1111-1111-1111-111111111111";
        history.push(Turn::Call {
            tool_name: call1.tool_name,
            arguments: call1.arguments,
        });
        history.push(Turn::Result {
            tool_name: "show_person".to_string(),
            content: json!({
                "content": [{ "type": "text", "text": "Found 1 person." }],
                "structuredContent": {
                    "count": 1,
                    "persons": [{
                        "id": person_id,
                        "name": "Nick",
                        "email": "nick@neonlaw.com",
                        "role": "client",
                        "oidc_subject": null
                    }]
                }
            }),
        });

        // Step 2: with the id in hand, it must send the welcome.
        let call2 = expect_call(router.next_step(&history, &catalog).await.unwrap());
        assert_eq!(
            call2.tool_name, "send_welcome_email",
            "real Gemini should send once it has the id"
        );
        assert_eq!(call2.arguments["person_id"], person_id);
    }

    #[test]
    fn turn_to_content_maps_each_role_and_part_shape() {
        assert_eq!(
            turn_to_content(&Turn::User("hi".to_string())),
            json!({ "role": "user", "parts": [{ "text": "hi" }] })
        );
        assert_eq!(
            turn_to_content(&Turn::Call {
                tool_name: "show_person".to_string(),
                arguments: json!({ "email": "a@b.com" }),
            }),
            json!({
                "role": "model",
                "parts": [{ "functionCall": { "name": "show_person", "args": { "email": "a@b.com" } } }]
            })
        );
        assert_eq!(
            turn_to_content(&Turn::Result {
                tool_name: "show_person".to_string(),
                content: json!({ "id": "p-1" }),
            }),
            json!({
                "role": "user",
                "parts": [{
                    "functionResponse": { "name": "show_person", "response": { "id": "p-1" } }
                }]
            })
        );
    }

    #[test]
    fn function_response_object_wraps_non_objects() {
        // Vertex rejects a `response` that isn't a JSON object.
        assert_eq!(
            function_response_object(&json!([1, 2, 3])),
            json!({ "result": [1, 2, 3] })
        );
        assert_eq!(
            function_response_object(&json!("ok")),
            json!({ "result": "ok" })
        );
        // Objects pass through untouched.
        assert_eq!(
            function_response_object(&json!({ "id": "p-1" })),
            json!({ "id": "p-1" })
        );
    }

    #[test]
    fn sanitize_schema_removes_additional_properties_recursively() {
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "nested": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": { "x": { "type": "string" } }
                },
                "items_array": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false
                    }
                }
            }
        });
        sanitize_schema(&mut schema);
        assert!(schema.get("additionalProperties").is_none());
        assert!(schema["properties"]["nested"]
            .get("additionalProperties")
            .is_none());
        assert!(schema["properties"]["items_array"]["items"]
            .get("additionalProperties")
            .is_none());
    }

    #[test]
    fn skill_to_function_declaration_strips_prefix_and_sanitizes() {
        let decl = skill_to_function_declaration(&json!({
            "name": "create_person",
            "description": "Create a person.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }
        }));
        assert_eq!(decl["name"], "create_person");
        assert_eq!(decl["description"], "Create a person.");
        assert!(decl["parameters"].get("additionalProperties").is_none());
        assert_eq!(decl["parameters"]["properties"]["name"]["type"], "string");
    }
}
