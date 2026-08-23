//! A2A (Agent2Agent) protocol surface for AIDA.
//!
//! Lives beside `/mcp` and reuses the exact same auth stack and tool
//! registry — A2A is a second protocol skin on the same body. The
//! point is to make Gemini Enterprise (and any future A2A-native
//! client) onboard from an agent-card URL rather than configuring a
//! custom MCP data store.
//!
//! Two endpoints, both under the private `/app/api` prefix:
//!
//! - `GET /app/api/aida.json` — the agent card: the agent's name,
//!   skills, transport, and `securitySchemes`. It needs a session like
//!   every other path under `/app/api`, which means A2A discovery is
//!   *not* self-service — a client cannot read the card to learn how to
//!   authenticate, because reading it already requires authenticating.
//!   That is a deliberate trade. Gemini Enterprise's connector takes its
//!   OAuth details from the registration form rather than from
//!   spec-driven discovery (see `docs/gemini-enterprise-mcp.md`), so the
//!   one client this serves is unaffected; onboarding a standards-based
//!   A2A client means handing over the OAuth details out of band.
//! - `POST /app/api/aida/rpc` — JSON-RPC 2.0. Same four-layer middleware
//!   stack as `/mcp` (`require_google_oauth` → `require_auth` →
//!   `require_policy` → `inject_principal`). Method scope is
//!   `message/send` only (synchronous completion). Two routing paths:
//!   if the client sends `metadata.skill`, dispatch directly; if not,
//!   delegate to the natural-language [`AgentRouter`] (Vertex AI
//!   Gemini Flash in prod, NullRouter in KIND).
//!
//! Future: task persistence (`tasks/get`, `tasks/cancel`), streaming,
//! push notifications. None of these is required for Gemini Enterprise
//! registration today.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent_router::{AgentRouter, RoutedCall, RouterError, Step, Turn};
use crate::audit_fields::{argument_count, argument_digest, person_id_field};
use crate::canonical_host::CanonicalHost;
use mcp::protocol::{codes, Request as RpcRequest, Response as RpcResponse};
use mcp::{tools, McpState, Principal};
use views::brand::FIRM_BRAND;

static PROVIDER_URL: std::sync::LazyLock<&'static str> =
    std::sync::LazyLock::new(|| match std::env::var("NAV_BASE_URL") {
        Ok(value) if !value.is_empty() => Box::leak(value.into_boxed_str()),
        _ => "https://www.example.com",
    });

fn provider_url() -> &'static str {
    if views::brand::base_url().is_empty() {
        *PROVIDER_URL
    } else {
        views::brand::base_url()
    }
}

// ---------------------------------------------------------------------------
// Agent card wire types — hand-rolled subset of the A2A 0.3 schema.
// ---------------------------------------------------------------------------

/// A2A protocol revision the card advertises. Bumping is a deliberate
/// breaking change: clients pin against this in their registration
/// flows. Keep aligned with the `a2a-protocol.org` spec.
pub const A2A_PROTOCOL_VERSION: &str = "0.3.0";

/// AIDA's semver-style version. Independent from
/// `CARGO_PKG_VERSION` because the agent's surface (skills, transport)
/// evolves on a different cadence than the `web` binary's build
/// version.
pub const AIDA_VERSION: &str = "0.1.0";

/// Default authority used when no canonical host is configured and the
/// request did not arrive with a `Host` header (e.g. a unit test that
/// hits the router without one). Real deployments override this with
/// `CANONICAL_HOST` (consumed by [`CanonicalHost`]), so the placeholder
/// here exists only so unit tests have something stable to compare
/// against — production never reaches this branch.
const FALLBACK_AUTHORITY: &str = "www.example.com";

/// Top-level agent card. Serialized at `GET /app/api/aida.json`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCard {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Absolute URL of the JSON-RPC endpoint. Built from the canonical
    /// host (production) or the request's `Host` header (KIND / tests).
    pub url: String,
    #[serde(rename = "preferredTransport")]
    pub preferred_transport: &'static str,
    pub version: &'static str,
    pub provider: Provider,
    pub capabilities: Capabilities,
    #[serde(rename = "defaultInputModes")]
    pub default_input_modes: Vec<&'static str>,
    #[serde(rename = "defaultOutputModes")]
    pub default_output_modes: Vec<&'static str>,
    pub skills: Vec<Skill>,
    #[serde(rename = "securitySchemes")]
    pub security_schemes: serde_json::Map<String, Value>,
    pub security: Vec<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Provider {
    pub organization: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub streaming: bool,
    #[serde(rename = "pushNotifications")]
    pub push_notifications: bool,
}

/// One declared capability of the agent. Mirrors one MCP tool.
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    /// A2A 0.3 requires at least one tag. v1 tags every skill as
    /// `"tool"`; refine into domain buckets (crm / notations /
    /// projects / governance) when the catalog grows.
    pub tags: Vec<&'static str>,
}

// ---------------------------------------------------------------------------
// Task / Message / Part / Artifact — the runtime payload types A2A
// returns from `message/send`. Synthesized per-request for v1 (no
// persistence); just enough to be spec-shaped.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    #[serde(rename = "contextId")]
    pub context_id: String,
    pub status: TaskStatus,
    pub artifacts: Vec<Artifact>,
    pub history: Vec<Message>,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    pub state: TaskState,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

/// Subset of A2A's TaskState enum. `Completed`/`Failed` are the two
/// terminal states v1 shipped; `InputRequired`/`Canceled` were added to
/// drive the confirmation gate. Per the A2A spec, `input-required` is a
/// non-terminal *interrupted* state — the task is alive and the client
/// continues it with a follow-up `message/send` carrying the same
/// `taskId` and `contextId`. `canceled` is terminal (the user declined).
/// The remaining variants (`submitted`, `working`, `auth-required`,
/// `rejected`) aren't needed for synchronous completion + confirmation.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Completed,
    Failed,
    InputRequired,
    Canceled,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub role: MessageRole,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Agent,
}

/// A2A discriminates parts by `kind`. v1 emits `text` (status / error
/// strings) and `data` (structured tool output). We accept `data` and
/// `text` on input.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Part {
    Text { text: String },
    Data { data: Value },
}

#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
    pub name: String,
    pub parts: Vec<Part>,
}

// ---------------------------------------------------------------------------
// Router builder + state.
// ---------------------------------------------------------------------------

/// State the A2A router carries. `Clone`-cheap.
#[derive(Clone)]
pub struct A2aState {
    pub mcp: McpState,
    pub canonical_host: CanonicalHost,
    /// Natural-language router used when an incoming `message/send`
    /// omits `metadata.skill`. `NullRouter` in dev / KIND;
    /// `GeminiRouter` in prod.
    pub router: Arc<dyn AgentRouter>,
    /// Side-effecting tool calls AIDA has proposed and is waiting on the
    /// user to confirm, keyed by `taskId`. See [`PendingConfirmations`].
    pub pending: PendingConfirmations,
}

/// How long a paused (`input-required`) task waits for the user's
/// confirmation before we forget it. We hold the resolved tool call
/// AIDA is about to run in memory keyed by `taskId`; pruning anything
/// older than this keeps an abandoned confirmation from pinning memory
/// or firing a stale side-effect days later. A2A lets an agent either
/// persist task state or reconstruct it — a confirmation that outlives
/// this window (or a pod restart) is simply re-driven from scratch when
/// the user asks again.
const PENDING_TTL: Duration = Duration::from_mins(15);

/// One paused side-effecting call plus exactly enough state to carry on
/// once the user approves. In-memory only.
struct PendingConfirmation {
    /// The conversation context this task belongs to. The resuming
    /// `message/send` must carry a matching `contextId` (spec: agents
    /// MUST reject mismatching `contextId`/`taskId`).
    context_id: String,
    /// The authenticated principal who started the task. Only the same
    /// principal may confirm it — you can't approve an action AIDA
    /// proposed to someone else.
    principal_email: String,
    /// The side-effecting call awaiting approval.
    pending_call: RoutedCall,
    /// What happens after the approver says yes.
    resume: Resume,
    /// When the pause was recorded, for [`PENDING_TTL`] pruning.
    created_at: Instant,
}

/// What a confirmed call continues into.
///
/// Both arms run the same approval boundary — matching `contextId`, the
/// same principal, a lawyer-tier approver, and the same `target: "audit"`
/// events. They differ only in what happens *after* the call succeeds,
/// because the two request shapes mean different things: a paused
/// agentic turn has more steps to take, and a named skill was the whole
/// request.
enum Resume {
    /// The natural-language loop paused mid-chain. Replay the seed and
    /// keep going for the remaining step budget — which may pause again
    /// on the next side-effecting call.
    Loop {
        /// The user's original request, replayed as the loop's seed.
        user_text: String,
        /// The reads / prior steps already run this turn.
        history: Vec<Turn>,
        /// The step index the loop had reached, so the remaining budget
        /// carries across the confirmation round-trip.
        steps_used: usize,
    },
    /// A `metadata.skill` dispatch paused before its single call. The
    /// client named the tool and the arguments itself, so there is no
    /// loop to resume and no model to consult: run the call and finish.
    DirectSkill,
}

impl Resume {
    /// The prior turns to quote in a confirmation prompt. A direct skill
    /// has none — the caller named the tool outright, so there is no
    /// preceding read whose result the prompt could resolve names from.
    fn history(&self) -> &[Turn] {
        match self {
            Self::Loop { history, .. } => history,
            Self::DirectSkill => &[],
        }
    }
}

/// Thread-safe map of `taskId` → [`PendingConfirmation`]. Cloned into
/// every request via [`A2aState`]; the inner `Arc<Mutex<…>>` is the one
/// shared store. The lock is only ever held for a `HashMap` insert /
/// remove — never across an `.await` — so a plain `std::sync::Mutex` is
/// correct and a contended async mutex would be overkill.
///
/// **Scope of durability (deliberate).** This store is *in-process and
/// best-effort*: it holds the live handle needed to resume a paused
/// task, nothing more. The durable, queryable record of *who authorized
/// what* is the `target: "audit"` log stream (→ Iceberg), not this map —
/// see [`audit_authorization`]. Consequence: a confirmation only
/// resolves if the follow-up `message/send` reaches the same process
/// within [`PENDING_TTL`]. On a multi-replica deploy without session
/// affinity (or across a pod restart), the resume can miss and AIDA
/// simply re-proposes from a fresh request. That is the accepted
/// trade-off for not standing up a shared task store; revisit if `web`
/// scales out and confirmation misses become common.
#[derive(Clone, Default)]
pub struct PendingConfirmations(Arc<Mutex<HashMap<String, PendingConfirmation>>>);

impl PendingConfirmations {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pause, pruning anything past [`PENDING_TTL`] first.
    fn insert(&self, task_id: String, pending: PendingConfirmation) {
        let mut map = self.0.lock().expect("pending-confirmations mutex poisoned");
        map.retain(|_, p| p.created_at.elapsed() < PENDING_TTL);
        map.insert(task_id, pending);
    }

    /// Remove and return the pending confirmation for `task_id`, if it
    /// exists and hasn't expired. Removing on read means a confirmation
    /// fires at most once.
    fn take(&self, task_id: &str) -> Option<PendingConfirmation> {
        let mut map = self.0.lock().expect("pending-confirmations mutex poisoned");
        map.retain(|_, p| p.created_at.elapsed() < PENDING_TTL);
        map.remove(task_id)
    }
}

/// Build the A2A sub-router. Caller is responsible for layering the
/// MCP-equivalent auth stack onto the `/app/api/aida/rpc` route, and the
/// session boundary onto the card route, before merging.
pub fn routes(state: A2aState) -> (Router, Router) {
    // Two routers because the two endpoints take different stacks, not
    // because one is anonymous: the card needs only a session, while the
    // RPC endpoint needs the full Google-OAuth-and-policy stack that
    // `/mcp` takes. `bootstrap` layers each accordingly.
    let card = Router::new()
        .route("/app/api/aida.json", get(card_handler))
        .with_state(state.clone());
    let rpc = Router::new()
        .route("/app/api/aida/rpc", post(rpc_handler))
        .with_state(state);
    (card, rpc)
}

// ---------------------------------------------------------------------------
// Agent card handler.
// ---------------------------------------------------------------------------

async fn card_handler(
    State(state): State<A2aState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let authority = resolve_authority(&state.canonical_host, &headers);
    let card = build_agent_card(&authority);
    let body = serde_json::to_vec(&card).expect("agent card is always serializable");
    let etag = compute_etag(&body);
    if let Some(if_none_match) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if if_none_match == etag {
            return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
        }
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::ETAG, etag),
            (
                header::CACHE_CONTROL,
                "public, max-age=300, must-revalidate".to_string(),
            ),
        ],
        body,
    )
        .into_response()
}

fn resolve_authority(canonical_host: &CanonicalHost, headers: &axum::http::HeaderMap) -> String {
    if let Some(host) = canonical_host.host() {
        return host.to_string();
    }
    if let Some(host) = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    {
        return host.to_string();
    }
    FALLBACK_AUTHORITY.to_string()
}

fn compute_etag(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(16).fold(String::new(), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!("\"{hex}\"")
}

/// Build the card from `mcp::tools::list_tools()` so the skills array
/// stays in lockstep with the actual MCP registry. Adding a tool only
/// requires updating one place (the MCP tools module); the A2A card
/// picks it up on the next request.
#[must_use]
pub fn build_agent_card(authority: &str) -> AgentCard {
    let scheme = if authority.starts_with("localhost")
        || authority.starts_with("127.0.0.1")
        || authority.starts_with("0.0.0.0")
    {
        "http"
    } else {
        "https"
    };
    let url = format!("{scheme}://{authority}/app/api/aida/rpc");
    let skills = tools::list_tools()
        .iter()
        .map(skill_from_descriptor)
        .collect();
    AgentCard {
        protocol_version: A2A_PROTOCOL_VERSION,
        name: "AIDA",
        description: "Neon Law Navigator's domain agent for legal-workflow automation: \
            people, entities, jurisdictions, notations, projects, and legal-council review. \
            Backed by the same MCP tool registry served at /mcp.",
        url,
        preferred_transport: "JSONRPC",
        version: AIDA_VERSION,
        provider: Provider {
            organization: FIRM_BRAND.site_name,
            url: provider_url(),
        },
        capabilities: Capabilities {
            streaming: false,
            push_notifications: false,
        },
        default_input_modes: vec!["application/json", "text/plain"],
        default_output_modes: vec!["application/json", "text/plain"],
        skills,
        security_schemes: google_oauth_scheme(),
        security: vec![{
            let mut m = serde_json::Map::new();
            m.insert("googleOAuth".into(), json!(["openid", "email"]));
            m
        }],
    }
}

fn skill_from_descriptor(descriptor: &Value) -> Skill {
    let mcp_name = descriptor["name"].as_str().unwrap_or_default();
    let id = strip_mcp_prefix(mcp_name).to_string();
    let description = descriptor["description"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Skill {
        name: humanize_tool_id(mcp_name),
        id,
        description,
        tags: vec!["tool"],
    }
}

/// Drop the `aida_` MCP namespace from a tool name. The prefix exists
/// because MCP clients (Claude.ai Connectors, LibreChat)
/// flatten tools from every connected server into one list — the
/// namespace prevents collisions. On A2A there is exactly one agent
/// per card, so the namespace is implicit and the prefix is noise.
fn strip_mcp_prefix(name: &str) -> &str {
    name.strip_prefix(tools::REQUIRED_PREFIX).unwrap_or(name)
}

/// Inverse of [`strip_mcp_prefix`]. Bridge prepends the namespace
/// before dispatching to `mcp::tools::call_tool`, which matches on the
/// fully-qualified MCP tool name. Idempotent: a client that sends the
/// prefixed form (`aida_create_person`) gets passed through unchanged.
fn to_mcp_tool_name(skill_id: &str) -> String {
    if skill_id.starts_with(tools::REQUIRED_PREFIX) {
        skill_id.to_string()
    } else {
        format!("{}{skill_id}", tools::REQUIRED_PREFIX)
    }
}

/// Turn `aida_create_person` into `"Create person"` for the
/// human-readable `name` field. A2A clients use `id` for routing
/// (the model only ever sees `id`); `name` is purely UI.
fn humanize_tool_id(id: &str) -> String {
    let trimmed = id.strip_prefix("aida_").unwrap_or(id);
    let mut out = String::with_capacity(trimmed.len());
    for (idx, word) in trimmed.split('_').enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.extend(chars);
        }
    }
    out
}

fn google_oauth_scheme() -> serde_json::Map<String, Value> {
    let mut schemes = serde_json::Map::new();
    schemes.insert(
        "googleOAuth".into(),
        json!({
            "type": "oauth2",
            "description": "Google OAuth 2.0 access token, validated via Google's tokeninfo endpoint. \
                            The same scheme that gates /mcp.",
            "flows": {
                "authorizationCode": {
                    "authorizationUrl": "https://accounts.google.com/o/oauth2/v2/auth",
                    "tokenUrl": "https://oauth2.googleapis.com/token",
                    "scopes": {
                        "openid": "OpenID Connect sign-in",
                        "email": "User's verified email",
                        "profile": "User's basic profile"
                    }
                }
            }
        }),
    );
    schemes
}

// ---------------------------------------------------------------------------
// JSON-RPC handler — v1 supports `message/send` only.
// ---------------------------------------------------------------------------

async fn rpc_handler(
    State(state): State<A2aState>,
    principal: Option<Extension<Principal>>,
    body: String,
) -> impl IntoResponse {
    let principal = principal.map(|Extension(p)| p);
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return rpc_reply(RpcResponse::err(
                Value::Null,
                codes::PARSE_ERROR,
                format!("parse error: {e}"),
            ));
        }
    };
    let request: RpcRequest = match serde_json::from_value(parsed) {
        Ok(r) => r,
        Err(e) => {
            return rpc_reply(RpcResponse::err(
                Value::Null,
                codes::INVALID_REQUEST,
                format!("invalid request: {e}"),
            ));
        }
    };
    if request.jsonrpc != "2.0" {
        let id = request.id.clone().unwrap_or(Value::Null);
        return rpc_reply(RpcResponse::err(
            id,
            codes::INVALID_REQUEST,
            "jsonrpc must be exactly \"2.0\"",
        ));
    }
    let id = request.id.clone().unwrap_or(Value::Null);
    let response = match request.method.as_str() {
        "message/send" => {
            handle_message_send(&state, principal.as_ref(), id, &request.params).await
        }
        // `tasks/get` and `tasks/cancel` are spec-required for stateful
        // agents — we explicitly do not persist tasks in v1 so any
        // referenced task ID is unknown by definition. METHOD_NOT_FOUND
        // is the honest answer until v2 lands.
        "tasks/get" | "tasks/cancel" | "message/stream" | "tasks/resubscribe" => RpcResponse::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!(
                "{} is not implemented in v1; only message/send is supported",
                request.method
            ),
        ),
        other => RpcResponse::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        ),
    };
    rpc_reply(response)
}

fn rpc_reply(response: RpcResponse) -> axum::response::Response {
    let body = serde_json::to_value(response).expect("RpcResponse is always serializable");
    (StatusCode::OK, Json(body)).into_response()
}

/// Bound on router↔tool round-trips for one inbound message. The
/// welcome-email chain (`show_person` → `send_welcome_email`) is two
/// calls; six leaves headroom for a lookup that needs a second attempt
/// without letting a confused model spin forever — and caps Vertex
/// spend per message. Hitting the cap returns a failed Task.
const MAX_ROUTER_STEPS: usize = 6;

async fn handle_message_send(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    params: &Value,
) -> RpcResponse {
    let Some(message) = params.get("message") else {
        return RpcResponse::err(id, codes::INVALID_PARAMS, "`params.message` is required");
    };
    let metadata = message.get("metadata").cloned().unwrap_or(Value::Null);
    let principal_email = principal.map_or(ANONYMOUS_PRINCIPAL, |p| p.email.as_str());
    let timestamp = chrono::Utc::now().to_rfc3339();

    // Path 0 — resume: this `message/send` carries a `taskId` we paused
    // on for confirmation. A2A's `input-required` round-trip: the client
    // replies in the same task with the user's "yes" / "no". Resolve it
    // before anything else, since a confirmation reply is just free text
    // (no `metadata.skill`) and would otherwise fall through to a fresh
    // routing pass.
    if let Some(task_id) = message
        .get("taskId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    {
        if let Some(pending) = state.pending.take(&task_id) {
            return resume_after_confirmation(
                state,
                principal,
                id,
                task_id,
                pending,
                message,
                principal_email,
                timestamp,
            )
            .await;
        }
    }

    let context_id = message
        .get("contextId")
        .and_then(Value::as_str)
        .map_or_else(new_uuid, ToString::to_string);
    let task_id = new_uuid();

    // Path 1 — the client named the skill: dispatch it directly, no
    // LLM. This is the `metadata.skill` backdoor every non-Gemini A2A
    // client uses, and stays a single deterministic tool call.
    if let Some(named_skill) = metadata.get("skill").and_then(Value::as_str) {
        let arguments = resolve_arguments(message, &metadata);
        tracing::info!(
            principal_kind = %principal_kind(principal_email),
            skill = %named_skill,
            task_id = %task_id,
            context_id = %context_id,
            routed_via_llm = false,
            "a2a: message/send accepted (direct skill)"
        );
        return dispatch_single(
            state,
            principal,
            id,
            task_id,
            context_id,
            timestamp,
            named_skill,
            &arguments,
        )
        .await;
    }

    // Path 2 — free-form text (Gemini Enterprise's shape): run the
    // natural-language agentic loop so multi-step requests (look a
    // person up, *then* email them) actually complete.
    let user_text = extract_user_text(message);
    if user_text.is_empty() {
        let text = "Empty message — include a text Part describing what you want AIDA to do, \
                    or set `metadata.skill` to dispatch a skill directly.";
        return RpcResponse::ok(
            id,
            serde_json::to_value(failed_task(task_id, context_id, timestamp, text))
                .expect("Task is always serializable"),
        );
    }
    run_router_loop(
        state,
        principal,
        id,
        task_id,
        context_id,
        timestamp,
        &user_text,
        principal_email,
    )
    .await
}

/// Execute exactly one named tool and wrap its result in a Task. The
/// `metadata.skill` direct-dispatch path — no router, no loop.
#[allow(clippy::too_many_arguments)]
async fn dispatch_single(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    task_id: String,
    context_id: String,
    timestamp: String,
    skill: &str,
    arguments: &Value,
) -> RpcResponse {
    // The `metadata.skill` path bypasses the router, so it is a
    // privileged programmatic entry point and carries its own two
    // controls. Both apply only to a *known* side-effecting skill: an
    // unknown name must fall through to `Unknown` rather than be
    // reported as an authorization failure.
    if tools::is_side_effecting(skill) && tools::is_known_tool(skill) {
        // Control 1 — tier. A side-effecting skill named directly must
        // be authorized by a lawyer/admin principal, the same tier the
        // router loop requires of the human who approves a paused
        // side-effect. Audited (→ OTLP → Iceberg) either way.
        let approver = match principal {
            Some(p) => approver_person(&state.mcp.surreal, &p.email).await,
            None => None,
        };
        let role = approver.as_ref().map(|p| p.role);
        let authorized = role.is_some_and(store::persons::Role::is_lawyer_tier);
        let confirmation_required = tools::requires_confirmation(skill);
        tracing::info!(
            target: "audit",
            event = "a2a.direct_skill.side_effect",
            skill = %skill,
            principal_kind = principal_kind(principal.map_or(ANONYMOUS_PRINCIPAL, |p| p.email.as_str())),
            person_id = %person_id_field(approver.as_ref()),
            authorized,
            confirmed = false,
            confirmation_required,
            "a2a: side-effecting skill dispatched directly via metadata.skill",
        );
        if !authorized {
            return RpcResponse::ok(
                id,
                serde_json::to_value(failed_task(
                    task_id,
                    context_id,
                    timestamp,
                    "This skill changes data and can be dispatched directly only by a lawyer \
                     principal. Sign in as lawyer, or send it as free-form text so AIDA can \
                     confirm the action with you first.",
                ))
                .expect("Task is always serializable"),
            );
        }

        // Control 2 — supervision. Lawyer tier answers *who is asking*;
        // it does not answer *has a human agreed to this particular
        // act*. For a skill that emails a client or moves a binding
        // artifact, those are different questions, and a model naming
        // the skill is the agent proposing — never the human approving.
        // So pause exactly as the router loop does, and let the same
        // `input-required` round trip resolve it. Park the resolved call
        // under this `taskId`; Path 0 in `handle_message_send` picks it
        // back up.
        if confirmation_required {
            let pending_call = RoutedCall {
                tool_name: skill.to_string(),
                arguments: arguments.clone(),
            };
            let prompt = confirmation_prompt(&pending_call, &[]);
            tracing::info!(
                target: "audit",
                event = "agent_action_authorization",
                decision = "proposed",
                principal_kind = %principal_kind(
                    principal.map_or(ANONYMOUS_PRINCIPAL, |p| p.email.as_str())
                ),
                person_id = %person_id_field(approver.as_ref()),
                tool = %skill,
                arguments_sha256 = %argument_digest(&pending_call.arguments),
                argument_count = argument_count(&pending_call.arguments),
                task_id = %task_id,
                routed_via_llm = false,
                "a2a: named skill needs confirmation → pausing (input-required)"
            );
            let response = input_required_response(
                id,
                task_id.clone(),
                context_id.clone(),
                timestamp,
                &prompt,
            );
            state.pending.insert(
                task_id,
                PendingConfirmation {
                    context_id,
                    principal_email: principal.map_or_else(String::new, |p| p.email.clone()),
                    pending_call,
                    resume: Resume::DirectSkill,
                    created_at: Instant::now(),
                },
            );
            return response;
        }
    }
    let mcp_tool_name = to_mcp_tool_name(skill);
    single_call_task(
        state,
        principal,
        id,
        task_id,
        context_id,
        timestamp,
        skill,
        &mcp_tool_name,
        arguments,
    )
    .await
}

/// Run exactly one tool and wrap the outcome in a terminal Task.
///
/// Shared by the two endings a named skill can have, so they cannot
/// drift: the unconfirmed dispatch in [`dispatch_single`], and the
/// approved second call in [`resume_after_confirmation`]. A client that
/// had to confirm an action gets back the same Task shape it would have
/// received had the action needed no confirmation at all.
#[allow(clippy::too_many_arguments)]
async fn single_call_task(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    task_id: String,
    context_id: String,
    timestamp: String,
    skill: &str,
    mcp_tool_name: &str,
    arguments: &Value,
) -> RpcResponse {
    match tools::call_tool(&state.mcp, principal, mcp_tool_name, arguments).await {
        Ok(result) => RpcResponse::ok(
            id,
            serde_json::to_value(Task {
                id: task_id,
                context_id,
                status: TaskStatus {
                    state: TaskState::Completed,
                    timestamp,
                    message: None,
                },
                artifacts: vec![Artifact {
                    artifact_id: new_uuid(),
                    name: skill.to_string(),
                    parts: tool_result_to_parts(skill, &result),
                }],
                history: vec![],
                kind: "task",
            })
            .expect("Task is always serializable"),
        ),
        Err(err) => RpcResponse::ok(id, {
            tracing::warn!(skill = %skill, error = err.kind(), "a2a: direct skill dispatch failed");
            serde_json::to_value(failed_task(
                task_id,
                context_id,
                timestamp,
                &tool_error_text(&err),
            ))
            .expect("Task is always serializable")
        }),
    }
}

/// The agentic loop entry point: seed the history with the user's
/// request and drive it from step 0. See [`drive_loop`] for the body —
/// which is shared with the post-confirmation resume path so a chain
/// that pauses, gets approved, and continues runs the exact same code.
#[allow(clippy::too_many_arguments)]
async fn run_router_loop(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    task_id: String,
    context_id: String,
    timestamp: String,
    user_text: &str,
    principal_email: &str,
) -> RpcResponse {
    let history = vec![Turn::User(user_text.to_string())];
    drive_loop(
        state,
        principal,
        id,
        task_id,
        context_id,
        timestamp,
        user_text,
        principal_email,
        history,
        0,
        None,
    )
    .await
}

/// The agentic loop body: ask the router for the next step, execute the
/// tool it picks, feed the result back, and repeat until the router
/// answers [`Step::Done`] or we hit [`MAX_ROUTER_STEPS`]. This is what
/// lets a free-form "send a welcome email to libra@…" resolve the
/// person first and then send.
///
/// The confirmation gate lives here: when the router picks a
/// **side-effecting** tool ([`tools::is_side_effecting`]), the loop does
/// NOT execute it. It stashes the resolved call plus the loop state into
/// [`A2aState::pending`] keyed by `task_id` and returns the task in the
/// `input-required` state with a prompt. The client then continues the
/// task with another `message/send` (same `taskId`/`contextId`), which
/// [`resume_after_confirmation`] routes back into this same loop at
/// `start_step + 1`. Read-only tools run inline, unconfirmed, so a
/// lookup→act chain only ever stops the user once — at the act. The
/// rule in three words: **reads run; writes wait.**
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn drive_loop(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    task_id: String,
    context_id: String,
    timestamp: String,
    user_text: &str,
    principal_email: &str,
    mut history: Vec<Turn>,
    start_step: usize,
    // The last *successful* action — its result becomes the Task's
    // artifact (the durable side-effect AIDA produced for the user).
    // Carried across a confirmation round-trip so an earlier approved
    // call still shows up as the artifact.
    mut last_action: Option<(String, Value)>,
) -> RpcResponse {
    let catalog = tools::list_tools();

    for step_n in start_step..MAX_ROUTER_STEPS {
        let step = match state.router.next_step(&history, &catalog).await {
            Ok(step) => step,
            Err(err) => {
                tracing::warn!(
                    principal_kind = %principal_kind(principal_email),
                    task_id = %task_id,
                    step = step_n,
                    error = %err,
                    "a2a: router step failed; returning enhanced-error Task"
                );
                let text = router_failure_message(&err, user_text);
                return RpcResponse::ok(
                    id,
                    serde_json::to_value(failed_task(task_id, context_id, timestamp, &text))
                        .expect("Task is always serializable"),
                );
            }
        };
        match step {
            Step::Done(done_text) => {
                return RpcResponse::ok(
                    id,
                    serde_json::to_value(loop_completed_task(
                        task_id,
                        context_id,
                        timestamp,
                        last_action,
                        &done_text,
                    ))
                    .expect("Task is always serializable"),
                );
            }
            Step::Call(RoutedCall {
                tool_name,
                arguments,
            }) => {
                // Confirmation gate: a side-effecting call pauses the
                // loop and asks the user before anything is written or
                // sent. Read-only lookups fall through and run inline.
                if tools::is_side_effecting(&tool_name) {
                    let pending_call = RoutedCall {
                        tool_name: tool_name.clone(),
                        arguments,
                    };
                    let prompt = confirmation_prompt(&pending_call, &history);
                    let proposer = approver_person(&state.mcp.surreal, principal_email).await;
                    tracing::info!(
                        target: "audit",
                        event = "agent_action_authorization",
                        decision = "proposed",
                        principal_kind = %principal_kind(principal_email),
                        person_id = %person_id_field(proposer.as_ref()),
                        tool = %tool_name,
                        arguments_sha256 = %argument_digest(&pending_call.arguments),
                        argument_count = argument_count(&pending_call.arguments),
                        task_id = %task_id,
                        step = step_n,
                        "a2a: side-effecting call → pausing for confirmation (input-required)"
                    );
                    let response = input_required_response(
                        id,
                        task_id.clone(),
                        context_id.clone(),
                        timestamp,
                        &prompt,
                    );
                    state.pending.insert(
                        task_id,
                        PendingConfirmation {
                            context_id,
                            principal_email: principal_email.to_string(),
                            pending_call,
                            resume: Resume::Loop {
                                user_text: user_text.to_string(),
                                history,
                                steps_used: step_n,
                            },
                            created_at: Instant::now(),
                        },
                    );
                    return response;
                }

                let mcp_tool_name = to_mcp_tool_name(&tool_name);
                tracing::info!(
                    principal_kind = %principal_kind(principal_email),
                    skill = %tool_name,
                    mcp_tool = %mcp_tool_name,
                    task_id = %task_id,
                    step = step_n,
                    routed_via_llm = true,
                    "a2a: router step → read-only tool call"
                );
                match tools::call_tool(&state.mcp, principal, &mcp_tool_name, &arguments).await {
                    Ok(result) => {
                        history.push(Turn::Call {
                            tool_name: tool_name.clone(),
                            arguments,
                        });
                        history.push(Turn::Result {
                            tool_name: tool_name.clone(),
                            content: result.clone(),
                        });
                        last_action = Some((tool_name, result));
                    }
                    Err(err) => {
                        // Feed the failure back as the tool result so
                        // the model can adapt (different lookup, fixed
                        // argument) instead of the chain dead-ending.
                        // Bounded by MAX_ROUTER_STEPS; we keep any
                        // earlier success in `last_action`. Sanitized so a
                        // raw DB error never reaches the LLM (and thence
                        // the client); the detail is logged.
                        tracing::warn!(skill = %tool_name, error = err.kind(), "a2a: router tool call failed");
                        history.push(Turn::Call {
                            tool_name: tool_name.clone(),
                            arguments,
                        });
                        history.push(Turn::Result {
                            tool_name,
                            content: json!({ "error": tool_error_text(&err) }),
                        });
                    }
                }
            }
        }
    }

    tracing::warn!(
        principal_kind = %principal_kind(principal_email),
        task_id = %task_id,
        "a2a: router loop hit MAX_ROUTER_STEPS without finishing"
    );
    let text = format!(
        "AIDA couldn't complete {user_text:?} within {MAX_ROUTER_STEPS} steps. \
         Try rephrasing, or send `metadata.skill` to dispatch one skill directly."
    );
    RpcResponse::ok(
        id,
        serde_json::to_value(failed_task(task_id, context_id, timestamp, &text))
            .expect("Task is always serializable"),
    )
}

/// Continue a task that paused on `input-required` once the user's
/// confirmation reply arrives. Enforces the A2A continuation contract
/// and the trust boundary, then either runs the held call and resumes
/// the loop (approved), cancels (declined), or re-prompts (ambiguous).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn resume_after_confirmation(
    state: &A2aState,
    principal: Option<&Principal>,
    id: Value,
    task_id: String,
    pending: PendingConfirmation,
    message: &Value,
    principal_email: &str,
    timestamp: String,
) -> RpcResponse {
    // Spec: agents MUST reject a message whose `contextId` doesn't match
    // the task it continues. When the client omits `contextId`, infer it
    // from the task (the stored one). Re-stash so a corrected retry can
    // still confirm.
    if let Some(cid) = message.get("contextId").and_then(Value::as_str) {
        if cid != pending.context_id {
            let restash_task_id = task_id.clone();
            let pending_ctx = pending.context_id.clone();
            state.pending.insert(restash_task_id, pending);
            tracing::warn!(
                principal_kind = %principal_kind(principal_email),
                task_id = %task_id,
                "a2a: confirmation rejected — contextId mismatch"
            );
            return RpcResponse::err(
                id,
                codes::INVALID_PARAMS,
                format!(
                    "contextId {cid:?} does not match task {task_id:?} (expected {pending_ctx:?})"
                ),
            );
        }
    }

    // Authorization boundary — two checks, both rendered as a
    // user-facing failed Task (an authz *outcome*, distinct from the
    // contextId case above which is a protocol error). Neither is
    // re-stashed: we drop the pending call rather than let the wrong
    // caller fish for it.
    //
    // (1) Identity: only the principal who *started* the task may
    //     confirm it.
    if principal_email != pending.principal_email {
        // Resolve the caller's row *here*, inside the failure branch, rather
        // than before the comparison: the approving path below does its own
        // lookup, and a denial is rare enough that every successful
        // confirmation should not pay for one. This is the decision a reader
        // of the trail most wants an actor on — somebody tried to approve a
        // pause that was not theirs — so it is the one place a missing
        // `person_id` would cost the most.
        let attempted_by = approver_person(&state.mcp.surreal, principal_email).await;
        audit_authorization(
            "denied_identity",
            principal_email,
            attempted_by.as_ref().map(|p| p.role),
            attempted_by.as_ref(),
            &task_id,
            &pending,
        );
        let text = "This task is awaiting confirmation from a different user; \
                    it can't be confirmed from this account.";
        return RpcResponse::ok(
            id,
            serde_json::to_value(failed_task(task_id, pending.context_id, timestamp, text))
                .expect("Task is always serializable"),
        );
    }

    // (2) Role: a client-facing side-effect is a supervised act — only
    //     a firm-side principal (lawyer or admin) may authorize it. This
    //     is the UPL / Model-Rule-5.3-supervision line the legal council
    //     drew: an agent may *propose*, but a licensed human authorizes.
    let approver = approver_person(&state.mcp.surreal, principal_email).await;
    let approver_role = approver.as_ref().map(|p| p.role);
    if !approver_role.is_some_and(store::persons::Role::is_lawyer_tier) {
        audit_authorization(
            "denied_unauthorized",
            principal_email,
            approver_role,
            approver.as_ref(),
            &task_id,
            &pending,
        );
        let text = "Only firm lawyers can authorize a client-facing action. \
                    This account isn't permitted to confirm it.";
        return RpcResponse::ok(
            id,
            serde_json::to_value(failed_task(task_id, pending.context_id, timestamp, text))
                .expect("Task is always serializable"),
        );
    }

    match extract_confirmation(message) {
        Confirmation::Affirm => {
            audit_authorization(
                "authorized",
                principal_email,
                approver_role,
                approver.as_ref(),
                &task_id,
                &pending,
            );
            let RoutedCall {
                tool_name,
                arguments,
            } = pending.pending_call;
            let mcp_tool_name = to_mcp_tool_name(&tool_name);

            // A direct skill was the entire request, so the approved
            // call is the answer. Run it and return the same Task shape
            // an unconfirmed `metadata.skill` dispatch returns — the
            // bridge's second call sees `completed`, not a loop it never
            // asked for.
            let Resume::Loop {
                user_text,
                mut history,
                steps_used,
            } = pending.resume
            else {
                return single_call_task(
                    state,
                    principal,
                    id,
                    task_id,
                    pending.context_id,
                    timestamp,
                    &tool_name,
                    &mcp_tool_name,
                    &arguments,
                )
                .await;
            };

            let mut last_action: Option<(String, Value)> = None;
            match tools::call_tool(&state.mcp, principal, &mcp_tool_name, &arguments).await {
                Ok(result) => {
                    history.push(Turn::Call {
                        tool_name: tool_name.clone(),
                        arguments,
                    });
                    history.push(Turn::Result {
                        tool_name: tool_name.clone(),
                        content: result.clone(),
                    });
                    last_action = Some((tool_name, result));
                }
                Err(err) => {
                    tracing::warn!(skill = %tool_name, error = err.kind(), "a2a: resumed tool call failed");
                    history.push(Turn::Call {
                        tool_name: tool_name.clone(),
                        arguments,
                    });
                    history.push(Turn::Result {
                        tool_name,
                        content: json!({ "error": tool_error_text(&err) }),
                    });
                }
            }
            // Resume the loop for any remaining steps — which may pause
            // again on the next side-effecting call.
            drive_loop(
                state,
                principal,
                id,
                task_id,
                pending.context_id,
                timestamp,
                &user_text,
                principal_email,
                history,
                steps_used + 1,
                last_action,
            )
            .await
        }
        Confirmation::Deny => {
            audit_authorization(
                "declined",
                principal_email,
                approver_role,
                approver.as_ref(),
                &task_id,
                &pending,
            );
            let text = format!(
                "Okay — I won't {}. Cancelled, nothing was changed.",
                humanize_tool_id(&pending.pending_call.tool_name).to_lowercase()
            );
            RpcResponse::ok(
                id,
                serde_json::to_value(canceled_task(task_id, pending.context_id, timestamp, &text))
                    .expect("Task is always serializable"),
            )
        }
        Confirmation::Ambiguous => {
            let prompt = format!(
                "I didn't catch that as a yes or no.\n\n{}",
                confirmation_prompt(&pending.pending_call, pending.resume.history())
            );
            let restash_task_id = task_id.clone();
            let pending_ctx = pending.context_id.clone();
            // Keep waiting: re-stash so the next reply still resolves
            // (refreshing the TTL is intentional — the user is engaged).
            let response = input_required_response(id, task_id, pending_ctx, timestamp, &prompt);
            state.pending.insert(
                restash_task_id,
                PendingConfirmation {
                    created_at: Instant::now(),
                    ..pending
                },
            );
            response
        }
    }
}

/// Build the terminal Task when the router answers `Done`. With at
/// least one successful tool call, the last call's result is the
/// artifact (the side-effect the user cares about) and the model's
/// closing line rides along as the status message. With no tool call —
/// the model just answered in text — that answer becomes the artifact
/// so the client still has something to render.
fn loop_completed_task(
    task_id: String,
    context_id: String,
    timestamp: String,
    last_action: Option<(String, Value)>,
    done_text: &str,
) -> Task {
    let (artifacts, message) = if let Some((skill, result)) = last_action {
        let parts = tool_result_to_parts(&skill, &result);
        let message = (!done_text.is_empty()).then(|| Message {
            message_id: new_uuid(),
            role: MessageRole::Agent,
            parts: vec![Part::Text {
                text: done_text.to_string(),
            }],
            metadata: None,
            kind: "message",
        });
        (
            vec![Artifact {
                artifact_id: new_uuid(),
                name: skill,
                parts,
            }],
            message,
        )
    } else {
        let text = if done_text.is_empty() {
            "AIDA had nothing to do for that request.".to_string()
        } else {
            done_text.to_string()
        };
        (
            vec![Artifact {
                artifact_id: new_uuid(),
                name: "message".to_string(),
                parts: vec![Part::Text { text }],
            }],
            None,
        )
    };
    Task {
        id: task_id,
        context_id,
        status: TaskStatus {
            state: TaskState::Completed,
            timestamp,
            message,
        },
        artifacts,
        history: vec![],
        kind: "task",
    }
}

/// Concatenate every text Part on the inbound message. A2A clients
/// MAY send multiple text parts (e.g. a user prompt + a system note);
/// joining preserves both for the router.
fn extract_user_text(message: &Value) -> String {
    let Some(parts) = message.get("parts").and_then(Value::as_array) else {
        return String::new();
    };
    parts
        .iter()
        .filter_map(|p| {
            if p.get("kind").and_then(Value::as_str) == Some("text") {
                p.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Build a failed Task whose status carries a single Agent text Part.
/// Used by the router-failure path: a *failed* (not error) Task means
/// Gemini Enterprise renders the message to the user instead of
/// surfacing a JSON-RPC error envelope.
/// Client-facing text for a tool error. The variants whose message is
/// about the caller's own input (unknown tool, bad arguments, not-found,
/// forbidden) are safe and *helpful* to surface verbatim. The ones that
/// wrap an internal failure — a raw `String` (which can carry the
/// SQL and column names) or an internal string — collapse to a generic
/// line; the detail goes to `tracing` only, never to the A2A client.
fn tool_error_text(err: &tools::ToolError) -> String {
    use tools::ToolError::{Conflict, Database, Internal, InvalidArguments, NotFound, Unknown};
    match err {
        Unknown(_) | InvalidArguments(_) | NotFound(_) | tools::ToolError::Forbidden(_) => {
            err.to_string()
        }
        Conflict(_) => "conflict: the write would violate a uniqueness constraint".to_string(),
        Database(_) => "a database error occurred".to_string(),
        Internal(_) => "an internal error occurred".to_string(),
    }
}

fn failed_task(task_id: String, context_id: String, timestamp: String, text: &str) -> Task {
    Task {
        id: task_id,
        context_id,
        status: TaskStatus {
            state: TaskState::Failed,
            timestamp,
            message: Some(agent_message(text)),
        },
        artifacts: vec![],
        history: vec![],
        kind: "task",
    }
}

/// Build an Agent-role text message — the `TaskStatus.message` an A2A
/// client renders to the user. Shared by the failed / input-required /
/// canceled task builders.
fn agent_message(text: &str) -> Message {
    Message {
        message_id: new_uuid(),
        role: MessageRole::Agent,
        parts: vec![Part::Text {
            text: text.to_string(),
        }],
        metadata: None,
        kind: "message",
    }
}

/// Build a task in the non-terminal `input-required` state. The prompt
/// rides in `status.message`; the task stays alive and the client
/// continues it with another `message/send` carrying this `taskId` and
/// `contextId`.
///
/// The confirmation gate only ever needs a yes/no, so the message
/// carries a structured [`confirmation_choice_part`] *alongside* the
/// human-readable text. The data Part declares the expected input as a
/// constrained `enum` (the universal JSON-Schema signal for "render a
/// choice, not a free-text box") so a capable A2A client surfaces a
/// one-tap **Yes** / **No** instead of asking the approver to type the
/// word. The text Part stays for clients that don't render the hint, and
/// [`classify_confirmation`] still parses a typed reply either way.
fn input_required_response(
    id: Value,
    task_id: String,
    context_id: String,
    timestamp: String,
    prompt: &str,
) -> RpcResponse {
    let message = Message {
        message_id: new_uuid(),
        role: MessageRole::Agent,
        parts: vec![
            Part::Text {
                text: prompt.to_string(),
            },
            confirmation_choice_part(),
        ],
        metadata: None,
        kind: "message",
    };
    RpcResponse::ok(
        id,
        serde_json::to_value(Task {
            id: task_id,
            context_id,
            status: TaskStatus {
                state: TaskState::InputRequired,
                timestamp,
                message: Some(message),
            },
            artifacts: vec![],
            history: vec![],
            kind: "task",
        })
        .expect("Task is always serializable"),
    )
}

/// Structured input hint for the confirmation gate: a `data` Part whose
/// payload is a labeled JSON-Schema enum of the only two answers the gate
/// accepts. `enum` (with the `oneOf`/`const`/`title` labels that
/// schema-driven form renderers honor) is what tells the client to draw a
/// yes/no control rather than a free-text input. Constructed once per
/// pause; the values match [`classify_confirmation`]'s affirm/deny words.
fn confirmation_choice_part() -> Part {
    Part::Data {
        data: json!({
            "expectedInput": {
                "title": "Authorize this action?",
                "type": "string",
                "enum": ["yes", "no"],
                "oneOf": [
                    { "const": "yes", "title": "Yes, authorize" },
                    { "const": "no", "title": "No, cancel" }
                ]
            }
        }),
    }
}

/// Build a terminal `canceled` task — the user declined the
/// confirmation, so nothing ran.
fn canceled_task(task_id: String, context_id: String, timestamp: String, text: &str) -> Task {
    Task {
        id: task_id,
        context_id,
        status: TaskStatus {
            state: TaskState::Canceled,
            timestamp,
            message: Some(agent_message(text)),
        },
        artifacts: vec![],
        history: vec![],
        kind: "task",
    }
}

/// The prompt shown when AIDA pauses before a side-effecting call.
///
/// Per the legal council, the approval is an *authorization* act by a
/// licensed human, so the copy: (1) names the action in plain language,
/// (2) resolves opaque ids to the human they refer to (a lawyer can't
/// authorize sending to a UUID — see [`resolve_person_refs`]), (3) warns
/// that it runs a real, client-facing action now, and (4) asks for an
/// explicit yes/no. Falls back to the raw arguments only when nothing
/// in the conversation resolves them to a name.
fn confirmation_prompt(call: &RoutedCall, history: &[Turn]) -> String {
    let action = humanize_tool_id(&call.tool_name);
    let people = resolve_person_refs(history);
    let detail = describe_arguments(&call.arguments, &people);
    format!(
        "**Authorize this action?**\n\n\
         AIDA wants to **{action}**{detail}.\n\n\
         This performs a real, client-facing action now and may not be reversible.\n\n\
         Choose **yes** to authorize, or **no** to cancel."
    )
}

/// Build an `id → \"Name (email)\"` map from the `show_person` results
/// already in the conversation, so the confirmation prompt can name the
/// human a UUID argument refers to. Reads the `structuredContent.persons`
/// shape `aida_show_person` returns; ignores anything else.
fn resolve_person_refs(history: &[Turn]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for turn in history {
        let Turn::Result { content, .. } = turn else {
            continue;
        };
        let Some(persons) = content
            .get("structuredContent")
            .and_then(|s| s.get("persons"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for person in persons {
            let (Some(person_id), Some(name)) = (
                person.get("id").and_then(Value::as_str),
                person.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let label = match person.get("email").and_then(Value::as_str) {
                Some(email) => format!("{name} ({email})"),
                None => name.to_string(),
            };
            map.insert(person_id.to_string(), label);
        }
    }
    map
}

/// Render a tool call's arguments for the confirmation prompt. If any
/// argument value resolves to a known person, lead with that human
/// ("for Nick (nick@…)"); otherwise fall back to the raw JSON so the
/// approver still sees exactly what will run.
fn describe_arguments(arguments: &Value, people: &HashMap<String, String>) -> String {
    let resolved: Vec<&String> = arguments
        .as_object()
        .into_iter()
        .flat_map(|obj| obj.values())
        .filter_map(Value::as_str)
        .filter_map(|v| people.get(v))
        .collect();
    if let Some(first) = resolved.first() {
        return format!(" for {first}");
    }
    let args = serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string());
    if args == "{}" {
        String::new()
    } else {
        format!(" with {args}")
    }
}

/// Emit one structured audit event for an agent-action authorization
/// decision. These flow into the log pipeline (and onward to Iceberg)
/// as the durable, analyzable record of every side-effect AIDA proposed
/// and how a human ruled on it — the supervision trail the legal council
/// requires (ABA Model Rules 5.1/5.3). `target: "audit"` lets the
/// pipeline select these out by target. NOTE: this log — not a database
/// row — is the record of authority; the in-memory pending store is only
/// the live, best-effort handle for resuming the paused task.
fn audit_authorization(
    decision: &str,
    principal_email: &str,
    approver_role: Option<store::persons::Role>,
    approver: Option<&store::persons::Person>,
    task_id: &str,
    pending: &PendingConfirmation,
) {
    tracing::info!(
        target: "audit",
        event = "agent_action_authorization",
        decision = decision,
        principal_kind = %principal_kind(principal_email),
        approver_role = approver_role.map_or("none", |r| r.as_str()),
        person_id = %person_id_field(approver),
        // The proposer is a second principal, so it gets the same treatment as
        // the approver above: whether it authenticated, never its address.
        proposer_kind = %principal_kind(&pending.principal_email),
        tool = %pending.pending_call.tool_name,
        arguments_sha256 = %argument_digest(&pending.pending_call.arguments),
        argument_count = argument_count(&pending.pending_call.arguments),
        task_id = %task_id,
        context_id = %pending.context_id,
        "a2a: agent action authorization decision"
    );
}

/// Whether a request arrived authenticated, as a telemetry label.
///
/// The address itself must never reach a log field: telemetry leaves the
/// firm's trust boundary and an email is client-identifying content, per the
/// standing order in `telemetry/src/lib.rs`. What these log lines actually
/// used the address *for* is the anonymous-versus-authenticated distinction,
/// which this preserves.
///
/// This answers only *whether* a caller authenticated. The identity travels
/// beside it as `person_id`, resolved through [`approver_person`] — see
/// [`person_id_field`] for why that spelling is the one that survives export.
fn principal_kind(email: &str) -> &'static str {
    if email == ANONYMOUS_PRINCIPAL {
        "anonymous"
    } else {
        "authenticated"
    }
}

/// Stand-in used where no principal authenticated. Not an address, so it is
/// safe in a log field — but it is compared, so it has one spelling.
const ANONYMOUS_PRINCIPAL: &str = "<anonymous>";

/// The approver's `persons` row, not just their tier.
///
/// Every caller needs both halves of the same row: `role` for the
/// authorization gate, and `id` for the audit record that has to name who
/// decided. One lookup serves both, so resolving the person is never more
/// expensive than resolving the tier alone.
async fn approver_person(
    surreal: &store::surreal::SurrealDb,
    email: &str,
) -> Option<store::persons::Person> {
    store::persons::find_by_email_ci(surreal, email)
        .await
        .ok()
        .flatten()
}

/// How the user answered a confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirmation {
    Affirm,
    Deny,
    Ambiguous,
}

/// Classify a free-text confirmation reply. Deliberately conservative:
/// anything that isn't a clear yes or no is [`Confirmation::Ambiguous`],
/// which re-prompts rather than guessing — we never run a side-effect on
/// an unclear answer. Keyword-based for v1; the councils may swap in a
/// model-judged classifier for natural phrasings.
/// Read the approver's yes/no decision from a confirmation reply.
///
/// The gate advertises the answer as a constrained `enum`
/// (see [`confirmation_choice_part`]), so a schema-aware client renders a
/// one-tap Yes/No and the approver types nothing. The decision is read
/// from the structured selection first; if the client doesn't wrap the
/// choice in a `data` Part but echoes the chosen token back as plain text
/// (Gemini Enterprise's shape), that token is accepted too — so the gate
/// behaves identically regardless of envelope, with no external client
/// behavior left to verify.
///
/// **There is no free-text command surface.** Only the *exact* offered
/// tokens (`yes` / `no`) authorize or decline; a free-form sentence — the
/// old "type a prompt" path — matches neither and is
/// [`Confirmation::Ambiguous`], so it re-prompts rather than running a
/// side-effect off prose. "Remove the text input for the yes/no gate"
/// lands here: the action needs only a `yes`, and only a `yes` is read.
fn extract_confirmation(message: &Value) -> Confirmation {
    let selection =
        extract_structured_choice(message).unwrap_or_else(|| extract_user_text(message));
    match selection.trim().to_lowercase().as_str() {
        "yes" => Confirmation::Affirm,
        "no" => Confirmation::Deny,
        _ => Confirmation::Ambiguous,
    }
}

/// Pull the yes/no selection out of a `data` Part on the reply — the only
/// input the gate reads, so the confirmation needs no typed text at all.
/// Accepts the two shapes a client renders the [`confirmation_choice_part`]
/// enum back as: a bare string value (`"data": "yes"`) or a `confirmation`
/// field (`"data": {"confirmation": "yes"}`). Returns `None` when no
/// `data` Part carries a choice, so the caller falls back to reading an
/// exact `yes`/`no` token echoed as plain text.
fn extract_structured_choice(message: &Value) -> Option<String> {
    let parts = message.get("parts").and_then(Value::as_array)?;
    parts.iter().find_map(|part| {
        if part.get("kind").and_then(Value::as_str) != Some("data") {
            return None;
        }
        let data = part.get("data")?;
        if let Some(s) = data.as_str() {
            return Some(s.to_string());
        }
        data.get("confirmation")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

/// Human-readable explanation of why the router didn't dispatch.
/// Lists the available skills so Gemini Enterprise's UI shows the
/// user *what to try* — the enhanced-error path from the council's
/// "Leo + Capricorn" agreement. Uses unprefixed skill ids since
/// that's the public A2A vocabulary.
fn router_failure_message(err: &RouterError, user_text: &str) -> String {
    let skill_list = tools::list_tools()
        .iter()
        .filter_map(|t| t["name"].as_str().map(|n| to_a2a_skill_id(n).to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let reason = match err {
        RouterError::NotConfigured => {
            "AIDA's natural-language router isn't configured in this environment."
        }
        RouterError::NoMatch(_) => "AIDA couldn't pick a skill that matches your request.",
        RouterError::Transport(_) => "AIDA's router is temporarily unavailable.",
        RouterError::InvalidResponse(_) => "AIDA's router returned an unexpected response.",
    };
    // The raw `err` can carry upstream Vertex transport/body text — log it,
    // never surface it to the A2A client.
    tracing::warn!(error = %err, "a2a: router failure");
    format!(
        "{reason}\n\n\
         You asked: {user_text:?}\n\n\
         Available skills: {skill_list}.\n\n\
         Tip: A2A clients can also send `metadata.skill` directly to bypass the router \
         — for example, `{{\"skill\":\"create_person\",\"arguments\":{{\"name\":\"…\",\"email\":\"…\"}}}}`."
    )
}

/// Inverse of `to_mcp_tool_name` — strip the MCP namespace so the
/// returned id matches what A2A clients see on the agent card.
fn to_a2a_skill_id(mcp_name: &str) -> &str {
    mcp_name
        .strip_prefix(tools::REQUIRED_PREFIX)
        .unwrap_or(mcp_name)
}

/// Convert an MCP tool result into A2A artifact parts.
///
/// MCP tools return `{ content: [{type: "text", text: "..."}],
/// structuredContent: { ... } }`. A2A artifacts accept multiple
/// parts of different kinds. We emit:
///
/// 1. A `text` Part with the MCP `content[0].text` so Gemini
///    Enterprise (and any chat-UI A2A client) renders a
///    human-readable success line to the user.
/// 2. A `data` Part with `structuredContent` (or the raw result
///    when no `structuredContent` is present) so programmatic
///    A2A clients can still parse the structured output.
///
/// If the tool didn't supply a text summary, fall back to a
/// generic "AIDA ran <skill>." line — the user still gets *some*
/// confirmation instead of an empty-looking response.
fn tool_result_to_parts(skill: &str, result: &Value) -> Vec<Part> {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("text").and_then(Value::as_str))
        .map_or_else(|| format!("AIDA ran {skill}."), ToString::to_string);
    let data = result
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| result.clone());
    vec![Part::Text { text }, Part::Data { data }]
}

/// Find the tool arguments. v1 contract: arguments live in
/// `metadata.arguments` (object) OR in the first `data` Part. The
/// former is what `aida_spawn_legal_council`-style explicit RPC calls use;
/// the latter is what an A2A client that doesn't know about our
/// metadata convention will send. Empty object if neither is present.
fn resolve_arguments(message: &Value, metadata: &Value) -> Value {
    if let Some(args) = metadata.get("arguments").cloned() {
        return args;
    }
    if let Some(parts) = message.get("parts").and_then(Value::as_array) {
        for part in parts {
            if part.get("kind").and_then(Value::as_str) == Some("data") {
                if let Some(data) = part.get("data").cloned() {
                    return data;
                }
            }
        }
    }
    json!({})
}

fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// Public helper: caller in `portal::bootstrap` uses this to keep the
// auth-layer composition out of the lib.rs body.
// ---------------------------------------------------------------------------

/// Convenience that returns the public card router plus the
/// rpc router wrapped in the same four-layer auth stack `/mcp` uses.
/// Caller still merges both into the top-level router.
pub fn build_routers(
    state: A2aState,
    google_oauth: crate::google_oauth::GoogleOauthConfig,
    auth_config: crate::auth::AuthConfig,
    sessions: crate::session::SessionStore,
    policy_client: crate::policy::PolicyClient,
) -> (Router, Router) {
    // Capture the person store before `state` is moved so the OAuth
    // validator can resolve the verified email to its real `persons.role`.
    let surreal = state.mcp.surreal.clone();
    let (card, rpc) = routes(state);
    let rpc = rpc
        .route_layer(axum::middleware::from_fn_with_state(
            google_oauth.clone(),
            crate::mcp_principal::inject_principal,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            (sessions.clone(), policy_client),
            crate::policy::require_policy,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_config,
            crate::auth::require_auth,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            google_oauth.with_db(surreal),
            crate::google_oauth::require_google_oauth,
        ))
        // Outermost of the auth chain, so it runs FIRST and every layer
        // below sees a resolved session: the `navigator` CLI's own bearer.
        //
        // Without this the CLI cannot reach A2A at all. Its credential is
        // the HMAC-signed `SessionData` blob `cli_auth` mints — not a JWT
        // and not a Google access token — so `require_auth` found nothing
        // to validate, the policy layer evaluated an anonymous session
        // against a rule requiring `is_lawyer`, and the request was
        // redirected to a login page a JSON-RPC client cannot follow.
        //
        // Resolving it here is not a widening: `SessionStore::decode`
        // accepts only a blob this deployment signed, carrying the role
        // and expiry it minted at an authenticated OIDC login. It is the
        // same mechanism every other `navigator site` command already
        // relies on through `session_boundary`.
        .route_layer(axum::middleware::from_fn_with_state(
            sessions,
            crate::auth::inject_bearer_session,
        ));
    (card, rpc)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;
    use workflows::InMemoryRuntime;

    async fn db() -> store::surreal::SurrealDb {
        store::test_support::mem_surreal().await
    }

    /// An `McpState` shaped like the one `web` builds: the mailer is
    /// injected, because `aida_send_welcome_email` reaches it through the
    /// shared command rather than the Restate trigger (ENG-317), and a
    /// fixture without one would exercise the refusal path instead of the
    /// send.
    fn mcp_state(surreal: store::surreal::SurrealDb) -> McpState {
        let mut st = McpState::new(surreal.clone(), Arc::new(InMemoryRuntime::new()));
        st.email = Some(Arc::new(crate::email::LoggingEmail::new(
            Arc::new(workflows::email::CapturingEmail::new()),
            surreal,
            "support@example.com",
        )));
        st
    }

    fn state_with(surreal: store::surreal::SurrealDb) -> A2aState {
        A2aState {
            mcp: mcp_state(surreal),
            canonical_host: CanonicalHost::new(Some("www.example.com".into())),
            router: Arc::new(crate::agent_router::NullRouter),
            pending: PendingConfirmations::new(),
        }
    }

    fn state_with_router(
        surreal: store::surreal::SurrealDb,
        router: Arc<dyn crate::agent_router::AgentRouter>,
    ) -> A2aState {
        A2aState {
            mcp: mcp_state(surreal),
            canonical_host: CanonicalHost::new(Some("www.example.com".into())),
            router,
            pending: PendingConfirmations::new(),
        }
    }

    async fn get_card(router: Router, host: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method("GET").uri("/app/api/aida.json");
        if let Some(h) = host {
            builder = builder.header("host", h);
        }
        let resp = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    async fn post_rpc(router: Router, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri("/app/api/aida/rpc")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    /// Like [`post_rpc`] but injects an authenticated [`Principal`] into
    /// the request extensions — the same shape the auth middleware
    /// produces in prod. Confirmation tests need this because the role
    /// gate looks the principal up in `persons`.
    async fn post_rpc_as(router: Router, body: Value, email: &str) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri("/app/api/aida/rpc")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        req.extensions_mut()
            .insert(Principal::new(email.to_string()));
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    /// Seed a `persons` row with the given role so the confirmation role
    /// gate has something to resolve the approver against.
    async fn seed_person(
        surreal: &store::surreal::SurrealDb,
        name: &str,
        email: &str,
        role: store::persons::Role,
    ) {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(name, email, role),
        )
        .await
        .expect("seed person");
    }

    #[tokio::test]
    async fn card_skills_are_mcp_tools_with_aida_prefix_stripped() {
        // Every MCP tool must appear as a skill on the card, with the
        // `aida_` namespace dropped. A2A clients see clean IDs (AIDA
        // herself is the namespace); MCP clients still see the
        // prefixed names because their tool lists are flat across
        // multiple servers. Drift between the two surfaces would mean
        // a tool the model can call via /mcp but not via A2A.
        let (card, _) = routes(state_with(db().await));
        let (status, body) = get_card(card, None).await;
        assert_eq!(status, StatusCode::OK);
        let skill_ids: Vec<&str> = body["skills"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["id"].as_str())
            .collect();
        let expected: Vec<String> = tools::list_tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(|n| strip_mcp_prefix(n).to_string()))
            .collect();
        for want in &expected {
            assert!(
                skill_ids.contains(&want.as_str()),
                "skill `{want}` missing from card; card has {skill_ids:?}"
            );
            assert!(
                !want.starts_with(tools::REQUIRED_PREFIX),
                "expected skill ids to strip `{}`, got `{want}`",
                tools::REQUIRED_PREFIX
            );
        }
        // These are production tools, not placeholders: the card must
        // never advertise a `mock_`/`demo_`/`test_` prefixed skill. A
        // client (Gemini Enterprise) derives its tool names from these
        // ids, so a stray placeholder prefix would surface to end users.
        for id in &skill_ids {
            for placeholder in ["mock_", "demo_", "test_", "stub_"] {
                assert!(
                    !id.starts_with(placeholder),
                    "skill id `{id}` carries placeholder prefix `{placeholder}`; \
                     AIDA's A2A skills are production tools and must ship clean names"
                );
            }
        }
        assert_eq!(skill_ids.len(), expected.len());
    }

    #[tokio::test]
    async fn card_advertises_jsonrpc_transport_and_googleoauth_security() {
        let (card, _) = routes(state_with(db().await));
        let (_, body) = get_card(card, None).await;
        assert_eq!(body["protocolVersion"], A2A_PROTOCOL_VERSION);
        assert_eq!(body["name"], "AIDA");
        assert_eq!(body["preferredTransport"], "JSONRPC");
        assert_eq!(body["capabilities"]["streaming"], false);
        assert_eq!(body["capabilities"]["pushNotifications"], false);
        assert_eq!(body["securitySchemes"]["googleOAuth"]["type"], "oauth2");
        // `security` must reference the declared scheme so clients
        // know to send a bearer.
        assert!(body["security"][0]["googleOAuth"].is_array());
    }

    #[tokio::test]
    async fn card_url_uses_canonical_host_over_request_host() {
        // Spoofed Host header must be ignored when canonical_host is
        // configured — Gemini Enterprise will dial whatever URL the
        // card advertises, so the card MUST point at the real
        // hostname, not whatever an attacker put in the Host header.
        let (card, _) = routes(state_with(db().await));
        let (_, body) = get_card(card, Some("evil.example.com")).await;
        let url = body["url"].as_str().unwrap();
        assert_eq!(url, "https://www.example.com/app/api/aida/rpc");
    }

    #[tokio::test]
    async fn card_falls_back_to_http_for_localhost() {
        let engines = db().await;
        let state = A2aState {
            mcp: McpState::new(engines, Arc::new(InMemoryRuntime::new())),
            canonical_host: CanonicalHost::new(None),
            router: Arc::new(crate::agent_router::NullRouter),
            pending: PendingConfirmations::new(),
        };
        let (card, _) = routes(state);
        let (_, body) = get_card(card, Some("localhost:8080")).await;
        let url = body["url"].as_str().unwrap();
        assert!(url.starts_with("http://"), "got {url}");
    }

    #[tokio::test]
    async fn rpc_message_send_dispatches_validate_notation() {
        let (_, rpc) = routes(state_with(db().await));
        let (status, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-1",
                        "role": "user",
                        "kind": "message",
                        "parts": [],
                        "metadata": {
                            "skill": "validate_notation",
                            "arguments": {
                                "contents": "# H\n",
                                "markdown_only": true
                            }
                        }
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "completed");
        assert_eq!(task["kind"], "task");
        let artifact = &task["artifacts"][0];
        // Artifact name echoes what the client sent — the unprefixed
        // skill id, not the internal MCP tool name.
        assert_eq!(artifact["name"], "validate_notation");
        // Text part first (renders in chat UIs), data part second
        // (programmatic access).
        assert_eq!(artifact["parts"][0]["kind"], "text");
        assert!(
            artifact["parts"][0]["text"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "expected non-empty text part, got: {}",
            artifact["parts"][0]
        );
        assert_eq!(artifact["parts"][1]["kind"], "data");
        assert_eq!(artifact["parts"][1]["data"]["clean"], true);
    }

    /// Body for a direct `metadata.skill` dispatch of `create_person`.
    fn direct_create_person(id: i64) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": "m-cp",
                    "role": "user",
                    "kind": "message",
                    "parts": [],
                    "metadata": {
                        "skill": "create_person",
                        "arguments": { "name": "New Person", "email": "new@example.com" }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn direct_skill_side_effect_is_refused_without_a_lawyer_principal() {
        // The metadata.skill backdoor must not run a write for an
        // anonymous caller — that would skip the router's confirmation
        // gate entirely.
        let (_, rpc) = routes(state_with(db().await));
        let (status, body) = post_rpc(rpc, direct_create_person(1)).await;
        assert_eq!(status, StatusCode::OK);
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "failed");
        let text = task["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            text.contains("lawyer"),
            "expected a lawyer-authorization message, got: {text}"
        );
    }

    #[tokio::test]
    async fn direct_skill_side_effect_runs_for_a_lawyer_principal() {
        let engines = db().await;
        seed_person(
            &engines,
            "Aida Lawyer",
            "lawyer@example.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with(engines));
        let (status, body) = post_rpc_as(rpc, direct_create_person(2), "lawyer@example.com").await;
        assert_eq!(status, StatusCode::OK);
        let task = &body["result"];
        assert_eq!(
            task["status"]["state"], "completed",
            "lawyer direct dispatch should run; got: {body}"
        );
    }

    #[tokio::test]
    async fn rpc_message_send_with_arguments_in_data_part() {
        // A2A client that doesn't know about our metadata.arguments
        // convention should still work by stuffing args into a data
        // Part. Skill name still has to be in metadata.
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-2",
                        "role": "user",
                        "kind": "message",
                        "parts": [
                            {
                                "kind": "data",
                                "data": { "contents": "# H\n", "markdown_only": true }
                            }
                        ],
                        "metadata": { "skill": "validate_notation" }
                    }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["status"]["state"], "completed");
    }

    #[tokio::test]
    async fn rpc_message_send_accepts_prefixed_skill_for_backwards_compat() {
        // A client that hard-coded the MCP-namespaced skill id should
        // still work — the bridge is idempotent on the prefix so
        // mid-migration callers don't have to flip in lockstep with
        // the card update.
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-7",
                        "role": "user",
                        "kind": "message",
                        "parts": [],
                        "metadata": {
                            "skill": "aida_validate_notation",
                            "arguments": { "contents": "# H\n", "markdown_only": true }
                        }
                    }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["status"]["state"], "completed");
    }

    #[tokio::test]
    async fn rpc_message_send_unknown_skill_becomes_failed_task() {
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-4",
                        "role": "user",
                        "kind": "message",
                        "parts": [],
                        "metadata": { "skill": "does_not_exist" }
                    }
                }
            }),
        )
        .await;
        // Tool-level failures come back as a failed Task, not a
        // JSON-RPC error envelope.
        assert!(body.get("error").is_none(), "got error: {body}");
        assert_eq!(body["result"]["status"]["state"], "failed");
        let text = body["result"]["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        // The MCP dispatcher reports the fully-qualified tool name
        // (after the bridge prepended `aida_`).
        assert!(
            text.contains("aida_does_not_exist"),
            "expected error to name the resolved MCP tool, got: {text}"
        );
    }

    #[tokio::test]
    async fn rpc_tasks_get_is_method_not_found_in_v1() {
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tasks/get",
                "params": { "id": "some-uuid" }
            }),
        )
        .await;
        assert_eq!(body["error"]["code"], codes::METHOD_NOT_FOUND);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tasks/get"));
    }

    #[tokio::test]
    async fn rpc_message_stream_is_method_not_found_in_v1() {
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "message/stream",
                "params": {}
            }),
        )
        .await;
        assert_eq!(body["error"]["code"], codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn rpc_malformed_json_returns_parse_error() {
        let state = state_with(db().await);
        let (_, rpc) = routes(state);
        let req = Request::builder()
            .method("POST")
            .uri("/app/api/aida/rpc")
            .header("content-type", "application/json")
            .body(Body::from("{ not json"))
            .unwrap();
        let resp = rpc.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], codes::PARSE_ERROR);
    }

    #[tokio::test]
    async fn card_sets_etag_and_responds_304_on_match() {
        let (card, _) = routes(state_with(db().await));
        let resp = card
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/app/api/aida.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag = resp
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string)
            .expect("agent card response must carry an ETag");

        let resp2 = card
            .oneshot(
                Request::builder()
                    .uri("/app/api/aida.json")
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn humanize_tool_id_drops_prefix_and_title_cases() {
        assert_eq!(humanize_tool_id("aida_create_person"), "Create Person");
        assert_eq!(
            humanize_tool_id("aida_list_jurisdictions"),
            "List Jurisdictions"
        );
        assert_eq!(
            humanize_tool_id("aida_spawn_legal_council"),
            "Spawn Legal Council"
        );
    }

    #[test]
    fn etag_is_stable_for_same_body() {
        let body = br#"{"name":"AIDA"}"#;
        assert_eq!(compute_etag(body), compute_etag(body));
    }

    /// Stub router that emits one configured call, then finishes once
    /// its result is in the history — drives the loop through a single
    /// tool without a live LLM. Replaces what would otherwise need
    /// wiremock + Vertex AI plumbing in the A2A tests.
    struct StubRouter {
        tool_name: String,
        arguments: Value,
    }

    #[async_trait::async_trait]
    impl crate::agent_router::AgentRouter for StubRouter {
        async fn next_step(
            &self,
            history: &[crate::agent_router::Turn],
            _skills: &[Value],
        ) -> Result<crate::agent_router::Step, crate::agent_router::RouterError> {
            let already_ran = history
                .iter()
                .any(|t| matches!(t, crate::agent_router::Turn::Result { .. }));
            if already_ran {
                Ok(crate::agent_router::Step::Done(String::new()))
            } else {
                Ok(crate::agent_router::Step::Call(
                    crate::agent_router::RoutedCall {
                        tool_name: self.tool_name.clone(),
                        arguments: self.arguments.clone(),
                    },
                ))
            }
        }
    }

    /// Scripted two-step router emulating what Gemini *should* do for
    /// "send a welcome email to <addr>": first look the person up by
    /// email, then — reading the id straight out of the lookup result
    /// fed back in the history — send the welcome, then finish. Lets
    /// the whole agentic loop be exercised end-to-end deterministically,
    /// with the real `show_person` + `send_welcome_email` tools and a
    /// real DB, but no live LLM.
    struct WelcomeChainRouter {
        email: String,
    }

    #[async_trait::async_trait]
    impl crate::agent_router::AgentRouter for WelcomeChainRouter {
        async fn next_step(
            &self,
            history: &[crate::agent_router::Turn],
            _skills: &[Value],
        ) -> Result<crate::agent_router::Step, crate::agent_router::RouterError> {
            use crate::agent_router::{RoutedCall, Step, Turn};
            // Find the most recent tool result, if any.
            let last_result = history.iter().rev().find_map(|t| match t {
                Turn::Result { tool_name, content } => Some((tool_name.as_str(), content)),
                _ => None,
            });
            match last_result {
                // No tool has run yet → look the person up by email.
                None => Ok(Step::Call(RoutedCall {
                    tool_name: "show_person".to_string(),
                    arguments: json!({ "email": self.email }),
                })),
                // The lookup came back → pull the id out and send.
                Some(("show_person", content)) => {
                    let person_id = content["structuredContent"]["persons"][0]["id"]
                        .as_str()
                        .expect("show_person result must carry a match id");
                    Ok(Step::Call(RoutedCall {
                        tool_name: "send_welcome_email".to_string(),
                        arguments: json!({ "person_id": person_id }),
                    }))
                }
                // The welcome was sent → done.
                Some(("send_welcome_email", _)) => {
                    Ok(Step::Done("Sent the welcome email.".to_string()))
                }
                Some((other, _)) => panic!("unexpected tool in chain: {other}"),
            }
        }
    }

    #[tokio::test]
    async fn rpc_welcome_email_pauses_for_confirmation_then_sends_on_yes() {
        // The headline flow with the confirmation gate: free-form "send
        // a welcome email to <addr>" resolves the person inline (a
        // read), then PAUSES before the side-effecting send — returning
        // a non-terminal `input-required` task. A second message/send
        // carrying the same taskId/contextId and "yes" runs the send and
        // completes, with the send as the artifact.
        let engines = db().await;
        let email = "nick@neonlaw.com";
        store::persons::create(&engines, &store::persons::NewPerson::new("Nick", email))
            .await
            .expect("seed person for welcome chain");
        // The lawyer authorizing the send must be a firm lawyer.
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;

        let router = Arc::new(WelcomeChainRouter {
            email: email.to_string(),
        });
        let (_, rpc) = routes(state_with_router(engines, router));

        // Round 1 — request. show_person runs inline; send_welcome_email
        // is side-effecting so the loop pauses for confirmation.
        let (status, body) = post_rpc_as(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 20,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-20",
                        "role": "user",
                        "kind": "message",
                        "parts": [
                            { "kind": "text", "text": "send a welcome email to nick@neonlaw.com" }
                        ]
                    }
                }
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "input-required", "task: {task}");
        // Nothing sent yet — no artifact.
        assert!(
            task["artifacts"].as_array().unwrap().is_empty(),
            "no side-effect should run before confirmation: {task}"
        );
        // The prompt names the specific action awaiting approval.
        let prompt = task["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            prompt.contains("Send Welcome Email"),
            "prompt must name the action, got: {prompt}"
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let context_id = task["contextId"].as_str().unwrap().to_string();

        // Round 2 — user confirms in the same task.
        let (_, body2) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 21,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-21",
                        "role": "user",
                        "kind": "message",
                        "taskId": task_id,
                        "contextId": context_id,
                        "parts": [{ "kind": "data", "data": { "confirmation": "yes" } }]
                    }
                }
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task2 = &body2["result"];
        assert_eq!(task2["status"]["state"], "completed", "task: {task2}");
        // The artifact is the terminal action (the send), not the lookup.
        assert_eq!(task2["artifacts"][0]["name"], "send_welcome_email");
        assert_eq!(
            task2["status"]["message"]["parts"][0]["text"],
            "Sent the welcome email."
        );
    }

    #[tokio::test]
    async fn rpc_confirmation_offers_structured_yes_no_choice() {
        // The input-required pause carries a structured `data` Part that
        // declares the expected input as a constrained yes/no enum, so a
        // capable A2A client renders a one-tap choice instead of a
        // free-text box. The human-readable text Part rides alongside (at
        // index 0) for clients that don't honor the hint.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra@example.com" }),
        });
        let engines = db().await;
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with_router(engines, stub));

        let (_, body) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 40, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-40", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "input-required", "task: {task}");
        let parts = task["status"]["message"]["parts"].as_array().unwrap();
        // Part 0 stays the human-readable prompt.
        assert_eq!(parts[0]["kind"], "text");
        // Part 1 is the structured choice hint a client renders as buttons.
        let expected = &parts[1]["data"]["expectedInput"];
        assert_eq!(parts[1]["kind"], "data");
        assert_eq!(expected["type"], "string");
        assert_eq!(expected["enum"], json!(["yes", "no"]));
        assert_eq!(expected["oneOf"][0]["const"], "yes");
        assert_eq!(expected["oneOf"][1]["const"], "no");
    }

    #[tokio::test]
    async fn rpc_confirmation_resolves_from_structured_choice_without_any_text() {
        // The no-free-text path end to end: the approver replies with a
        // structured `data` Part carrying the selected enum value and
        // sends NO text Part at all. The gate still authorizes and the
        // held side-effect runs — proving the confirmation never requires
        // a typed answer.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra@example.com" }),
        });
        let engines = db().await;
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with_router(engines, stub));

        // Round 1 — request pauses for confirmation.
        let (_, body) = post_rpc_as(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0", "id": 50, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-50", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "input-required", "task: {task}");
        let task_id = task["id"].as_str().unwrap().to_string();
        let context_id = task["contextId"].as_str().unwrap().to_string();

        // Round 2 — approve with a structured choice only: a single `data`
        // Part, zero text Parts.
        let (_, body2) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 51, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-51", "role": "user", "kind": "message",
                    "taskId": task_id, "contextId": context_id,
                    "parts": [{ "kind": "data", "data": { "confirmation": "yes" } }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task2 = &body2["result"];
        assert_eq!(task2["status"]["state"], "completed", "task: {task2}");
        assert_eq!(task2["artifacts"][0]["name"], "create_person");
    }

    #[tokio::test]
    async fn rpc_side_effecting_call_declined_cancels_and_runs_nothing() {
        // A "no" reply to the confirmation cancels the task: terminal
        // `canceled` state, no artifact, and the held tool never runs.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra@example.com" }),
        });
        let engines = db().await;
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with_router(engines, stub));

        let (_, body) = post_rpc_as(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0", "id": 30, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-30", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task = &body["result"];
        assert_eq!(task["status"]["state"], "input-required");
        let task_id = task["id"].as_str().unwrap().to_string();
        let context_id = task["contextId"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 31, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-31", "role": "user", "kind": "message",
                    "taskId": task_id, "contextId": context_id,
                    "parts": [{ "kind": "data", "data": { "confirmation": "no" } }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task2 = &body2["result"];
        assert_eq!(task2["status"]["state"], "canceled", "task: {task2}");
        assert!(task2["artifacts"].as_array().unwrap().is_empty());
        let text = task2["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(text.contains("cancel"), "got: {text}");
    }

    #[tokio::test]
    async fn rpc_confirmation_ambiguous_reply_reprompts() {
        // An unclear reply re-prompts (stays input-required, same task)
        // rather than guessing — we never run a side-effect on a vague
        // answer.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra2@example.com" }),
        });
        let engines = db().await;
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with_router(engines, stub));

        let (_, body) = post_rpc_as(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0", "id": 40, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-40", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task_id = body["result"]["id"].as_str().unwrap().to_string();
        let context_id = body["result"]["contextId"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 41, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-41", "role": "user", "kind": "message",
                    "taskId": task_id.clone(), "contextId": context_id,
                    "parts": [{ "kind": "text", "text": "hmm what does that do" }]
                }}
            }),
            "lawyer@neonlaw.com",
        )
        .await;
        let task2 = &body2["result"];
        assert_eq!(task2["status"]["state"], "input-required", "task: {task2}");
        // Same task — still the one awaiting confirmation.
        assert_eq!(task2["id"].as_str().unwrap(), task_id);
        let text = task2["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("yes") && text.contains("no"), "got: {text}");
    }

    #[tokio::test]
    async fn rpc_confirmation_with_mismatched_context_id_is_rejected() {
        // Spec: agents MUST reject a continuation whose contextId
        // doesn't match the task. We return a JSON-RPC error and keep
        // the pending confirmation alive for a corrected retry.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra3@example.com" }),
        });
        let (_, rpc) = routes(state_with_router(db().await, stub));

        let (_, body) = post_rpc(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0", "id": 50, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-50", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
        )
        .await;
        let task_id = body["result"]["id"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 51, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-51", "role": "user", "kind": "message",
                    "taskId": task_id, "contextId": "some-other-context",
                    "parts": [{ "kind": "text", "text": "yes" }]
                }}
            }),
        )
        .await;
        assert_eq!(body2["error"]["code"], codes::INVALID_PARAMS);
        assert!(body2["result"].is_null());
    }

    #[tokio::test]
    async fn rpc_confirmation_by_non_lawyer_is_refused() {
        // The UPL / supervision line: only a firm-side principal (lawyer
        // or admin) may authorize a client-facing side-effect. A client
        // who proposes and then tries to confirm is refused — the held
        // call never runs.
        let stub = Arc::new(StubRouter {
            tool_name: "create_person".to_string(),
            arguments: json!({ "name": "Libra", "email": "libra4@example.com" }),
        });
        let engines = db().await;
        seed_person(
            &engines,
            "A Client",
            "client@example.com",
            store::persons::Role::Client,
        )
        .await;
        let (_, rpc) = routes(state_with_router(engines, stub));

        let (_, body) = post_rpc_as(
            rpc.clone(),
            json!({
                "jsonrpc": "2.0", "id": 60, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-60", "role": "user", "kind": "message",
                    "parts": [{ "kind": "text", "text": "add Libra" }]
                }}
            }),
            "client@example.com",
        )
        .await;
        let task_id = body["result"]["id"].as_str().unwrap().to_string();
        let context_id = body["result"]["contextId"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc_as(
            rpc,
            json!({
                "jsonrpc": "2.0", "id": 61, "method": "message/send",
                "params": { "message": {
                    "messageId": "m-61", "role": "user", "kind": "message",
                    "taskId": task_id, "contextId": context_id,
                    "parts": [{ "kind": "text", "text": "yes" }]
                }}
            }),
            "client@example.com",
        )
        .await;
        let task2 = &body2["result"];
        assert_eq!(task2["status"]["state"], "failed", "task: {task2}");
        let text = task2["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("firm lawyer"),
            "must explain the lawyer-only authorization rule, got: {text}"
        );
    }

    #[test]
    fn extract_confirmation_accepts_only_the_exact_yes_no_token() {
        // Structured object shape: { "data": { "confirmation": "yes" } }.
        let obj_choice = json!({
            "parts": [{ "kind": "data", "data": { "confirmation": "yes" } }]
        });
        assert_eq!(extract_confirmation(&obj_choice), Confirmation::Affirm);

        // Structured bare-string shape: { "data": "no" }.
        let bare_choice = json!({
            "parts": [{ "kind": "data", "data": "no" }]
        });
        assert_eq!(extract_confirmation(&bare_choice), Confirmation::Deny);

        // The exact token echoed as plain text (a client that doesn't wrap
        // the choice in a data Part) is accepted too, so the gate works
        // regardless of envelope. Case/whitespace-insensitive.
        let text_token = json!({
            "parts": [{ "kind": "text", "text": " YES " }]
        });
        assert_eq!(extract_confirmation(&text_token), Confirmation::Affirm);

        // But there is NO free-form command surface: a sentence — even an
        // obviously-affirmative one — is not the token and re-prompts.
        for prose in ["go ahead", "ok do it", "yes please send that", ""] {
            let reply = json!({ "parts": [{ "kind": "text", "text": prose }] });
            assert_eq!(
                extract_confirmation(&reply),
                Confirmation::Ambiguous,
                "free-form prose must not authorize: {prose:?}"
            );
        }

        // The structured choice wins over a conflicting text Part.
        let choice_with_text = json!({
            "parts": [
                { "kind": "text", "text": "no" },
                { "kind": "data", "data": { "confirmation": "yes" } }
            ]
        });
        assert_eq!(
            extract_confirmation(&choice_with_text),
            Confirmation::Affirm
        );
    }

    #[tokio::test]
    async fn rpc_message_send_loop_surfaces_text_only_answer() {
        // A router that finishes immediately (no tool call) yields a
        // completed Task carrying the model's text as the artifact, so
        // a purely conversational reply still renders.
        struct DoneRouter;
        #[async_trait::async_trait]
        impl crate::agent_router::AgentRouter for DoneRouter {
            async fn next_step(
                &self,
                _history: &[crate::agent_router::Turn],
                _skills: &[Value],
            ) -> Result<crate::agent_router::Step, crate::agent_router::RouterError> {
                Ok(crate::agent_router::Step::Done(
                    "AIDA can create people, projects, and notations.".to_string(),
                ))
            }
        }
        let (_, rpc) = routes(state_with_router(db().await, Arc::new(DoneRouter)));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 21,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-21",
                        "role": "user",
                        "kind": "message",
                        "parts": [{ "kind": "text", "text": "what can you do?" }]
                    }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["status"]["state"], "completed");
        assert_eq!(body["result"]["artifacts"][0]["name"], "message");
        assert!(body["result"]["artifacts"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("create people"));
    }

    #[tokio::test]
    async fn rpc_message_send_without_skill_falls_through_to_router() {
        // Gemini Enterprise's actual wire shape: free-form text Part,
        // no metadata.skill. The router (mocked) decides which tool.
        let stub = Arc::new(StubRouter {
            tool_name: "validate_notation".to_string(),
            arguments: json!({ "contents": "# H\n", "markdown_only": true }),
        });
        let (_, rpc) = routes(state_with_router(db().await, stub));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-10",
                        "role": "user",
                        "kind": "message",
                        "parts": [
                            { "kind": "text", "text": "validate this markdown for me" }
                        ]
                    }
                }
            }),
        )
        .await;
        assert!(body.get("error").is_none(), "got error: {body}");
        assert_eq!(body["result"]["status"]["state"], "completed");
        assert_eq!(body["result"]["artifacts"][0]["name"], "validate_notation");
    }

    #[tokio::test]
    async fn rpc_message_send_with_null_router_returns_enhanced_error_task() {
        // No metadata.skill, no real router → completed-with-failed
        // Task that lists the catalog so Gemini Enterprise renders
        // it to the user. NOT a JSON-RPC error envelope.
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-11",
                        "role": "user",
                        "kind": "message",
                        "parts": [
                            { "kind": "text", "text": "create a person named Libra" }
                        ]
                    }
                }
            }),
        )
        .await;
        assert!(body.get("error").is_none(), "got JSON-RPC error: {body}");
        assert_eq!(body["result"]["status"]["state"], "failed");
        let text = body["result"]["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("create_person") && text.contains("validate_notation"),
            "enhanced error must list skill catalog, got: {text}"
        );
        assert!(
            text.contains("metadata.skill"),
            "enhanced error must mention the backdoor, got: {text}"
        );
        assert!(
            text.contains("create a person named Libra"),
            "enhanced error must echo the user text, got: {text}"
        );
    }

    #[tokio::test]
    async fn rpc_message_send_empty_text_returns_failed_task() {
        let (_, rpc) = routes(state_with(db().await));
        let (_, body) = post_rpc(
            rpc,
            json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "message/send",
                "params": {
                    "message": {
                        "messageId": "m-12",
                        "role": "user",
                        "kind": "message",
                        "parts": []
                    }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["status"]["state"], "failed");
        let text = body["result"]["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("Empty message"), "got: {text}");
    }

    #[test]
    fn tool_error_text_hides_internal_db_detail_but_keeps_helpful_messages() {
        // An engine error can carry query text / field names — it must
        // never reach the A2A client.
        let db = tools::ToolError::Database("SELECT secret_column FROM persons".into());
        let text = super::tool_error_text(&db);
        assert!(!text.contains("secret_column"), "leaked DB detail: {text}");
        assert!(text.contains("database error"), "got: {text}");

        // Caller-facing errors stay verbatim — they help the model fix
        // its own input.
        let forbidden = tools::ToolError::Forbidden("alice is not lawyer or admin".into());
        assert!(super::tool_error_text(&forbidden).contains("not lawyer"));
        let bad_args = tools::ToolError::InvalidArguments("missing field `email`".into());
        assert!(super::tool_error_text(&bad_args).contains("email"));
    }

    #[test]
    fn tool_result_to_parts_emits_text_then_data() {
        let result = json!({
            "content": [{ "type": "text", "text": "Created person Libra." }],
            "structuredContent": { "id": "abc", "name": "Libra" }
        });
        let parts = tool_result_to_parts("create_person", &result);
        assert_eq!(parts.len(), 2);
        let Part::Text { text } = &parts[0] else {
            panic!("expected text part first, got {:?}", parts[0])
        };
        assert_eq!(text, "Created person Libra.");
        let Part::Data { data } = &parts[1] else {
            panic!("expected data part second, got {:?}", parts[1])
        };
        assert_eq!(data["id"], "abc");
        assert_eq!(data["name"], "Libra");
    }

    #[test]
    fn tool_result_to_parts_falls_back_when_tool_lacks_content() {
        let result = json!({ "raw": "no envelope" });
        let parts = tool_result_to_parts("spawn_legal_council", &result);
        let Part::Text { text } = &parts[0] else {
            panic!("expected fallback text, got {:?}", parts[0])
        };
        assert_eq!(text, "AIDA ran spawn_legal_council.");
        // No structuredContent → emit the whole result as data.
        let Part::Data { data } = &parts[1] else {
            panic!("expected data part, got {:?}", parts[1])
        };
        assert_eq!(data["raw"], "no envelope");
    }

    #[test]
    fn extract_user_text_joins_every_text_part() {
        let msg = json!({
            "parts": [
                { "kind": "text", "text": "first" },
                { "kind": "data", "data": { "ignored": true } },
                { "kind": "text", "text": "second" }
            ]
        });
        assert_eq!(extract_user_text(&msg), "first\nsecond");
    }

    #[test]
    fn strip_mcp_prefix_drops_aida_namespace() {
        assert_eq!(strip_mcp_prefix("aida_create_person"), "create_person");
        assert_eq!(strip_mcp_prefix("create_person"), "create_person");
        assert_eq!(strip_mcp_prefix("aida_"), "");
    }

    #[test]
    fn to_mcp_tool_name_is_idempotent_on_prefix() {
        assert_eq!(to_mcp_tool_name("create_person"), "aida_create_person");
        assert_eq!(to_mcp_tool_name("aida_create_person"), "aida_create_person");
    }

    /// A `metadata.skill` request for a named skill, with no confirmation
    /// token. `arguments` ride in `metadata` like every other direct call.
    fn direct_skill(id: i64, skill: &str, arguments: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": format!("m-{id}"),
                    "role": "user",
                    "kind": "message",
                    "parts": [],
                    "metadata": { "skill": skill, "arguments": arguments }
                }
            }
        })
    }

    /// The confirmation reply: same task, same context, one "yes"/"no".
    fn confirm_reply(id: i64, task_id: &str, context_id: &str, answer: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "message/send",
            "params": {
                "message": {
                    "messageId": format!("m-{id}"),
                    "role": "user",
                    "kind": "message",
                    "taskId": task_id,
                    "contextId": context_id,
                    "parts": [{ "kind": "text", "text": answer }]
                }
            }
        })
    }

    /// Seed a firm lawyer plus a client to act on. Returns the store and
    /// the client's `person_id`, which is how `send_welcome_email`
    /// identifies a recipient — never a free-text address.
    async fn welcome_fixture() -> (store::surreal::SurrealDb, uuid::Uuid) {
        let engines = db().await;
        let person = store::persons::create(
            &engines,
            &store::persons::NewPerson::new("Nick", "nick@neonlaw.com"),
        )
        .await
        .expect("seed the person to welcome");
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        (engines, person.id)
    }

    #[tokio::test]
    async fn direct_skill_requiring_confirmation_pauses_instead_of_running() {
        // The gate this whole path was missing: a lawyer naming
        // `send_welcome_email` outright must still be asked. Lawyer tier
        // says who is calling; it does not say a human agreed to mail
        // this client.
        let (engines, person_id) = welcome_fixture().await;
        let (_, rpc) = routes(state_with(engines));
        let (status, body) = post_rpc_as(
            rpc,
            direct_skill(30, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.get("error").is_none(), "unexpected error: {body}");
        let task = &body["result"];
        assert_eq!(
            task["status"]["state"], "input-required",
            "a named client-facing skill must pause; got: {task}"
        );
        assert!(
            task["artifacts"].as_array().unwrap().is_empty(),
            "nothing may run before the approval: {task}"
        );
        let prompt = task["status"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            prompt.contains("Send Welcome Email"),
            "the prompt must name the action awaiting approval, got: {prompt}"
        );
    }

    #[tokio::test]
    async fn direct_skill_confirmation_completes_without_entering_the_router_loop() {
        // The two-call handshake the stdio bridge translates. The state
        // here carries `NullRouter`, so if the approved call fell through
        // to `drive_loop` the task would come back `failed` — a passing
        // `completed` is the proof that a named skill resumes as one
        // call and stops.
        let (engines, person_id) = welcome_fixture().await;
        let (_, rpc) = routes(state_with(engines));
        let (_, body) = post_rpc_as(
            rpc.clone(),
            direct_skill(31, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;
        let task_id = body["result"]["id"].as_str().unwrap().to_string();
        let context_id = body["result"]["contextId"].as_str().unwrap().to_string();

        let (status, body2) = post_rpc_as(
            rpc,
            confirm_reply(32, &task_id, &context_id, "yes"),
            "lawyer@neonlaw.com",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let task = &body2["result"];
        assert_eq!(
            task["status"]["state"], "completed",
            "the approved call should run and finish; got: {task}"
        );
        assert_eq!(
            task["id"], task_id,
            "the confirmation continues the same task"
        );
        let artifacts = task["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 1, "one call, one artifact: {task}");
        assert_eq!(artifacts[0]["name"], "send_welcome_email");
    }

    #[tokio::test]
    async fn direct_skill_confirmation_declined_runs_nothing() {
        let (engines, person_id) = welcome_fixture().await;
        let (_, rpc) = routes(state_with(engines));
        let (_, body) = post_rpc_as(
            rpc.clone(),
            direct_skill(33, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;
        let task_id = body["result"]["id"].as_str().unwrap().to_string();
        let context_id = body["result"]["contextId"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc_as(
            rpc,
            confirm_reply(34, &task_id, &context_id, "no"),
            "lawyer@neonlaw.com",
        )
        .await;

        let task = &body2["result"];
        assert_eq!(
            task["status"]["state"], "canceled",
            "a declined named skill cancels; got: {task}"
        );
        assert!(
            task["artifacts"].as_array().unwrap().is_empty(),
            "a decline must run nothing: {task}"
        );
    }

    #[tokio::test]
    async fn direct_skill_confirmation_is_refused_from_another_account() {
        // The identity half of the approval boundary reaches the direct
        // path too: the pause belongs to the principal who opened it.
        let (engines, person_id) = welcome_fixture().await;
        seed_person(
            &engines,
            "Other Lawyer",
            "other@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with(engines));
        let (_, body) = post_rpc_as(
            rpc.clone(),
            direct_skill(35, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;
        assert_eq!(
            body["result"]["status"]["state"], "input-required",
            "the pause must exist before we test who may resolve it; got: {body}"
        );
        let task_id = body["result"]["id"].as_str().unwrap().to_string();
        let context_id = body["result"]["contextId"].as_str().unwrap().to_string();

        let (_, body2) = post_rpc_as(
            rpc,
            confirm_reply(36, &task_id, &context_id, "yes"),
            "other@neonlaw.com",
        )
        .await;

        let task = &body2["result"];
        assert_eq!(
            task["status"]["state"], "failed",
            "another lawyer must not be able to approve this pause; got: {task}"
        );
        assert!(
            task["artifacts"].as_array().unwrap().is_empty(),
            "a refused approval must run nothing: {task}"
        );
    }

    /// A `tracing` writer that keeps every line in memory.
    #[derive(Clone)]
    struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
        type Writer = LogBuf;
        fn make_writer(&'a self) -> LogBuf {
            self.clone()
        }
    }

    /// Install a JSON subscriber writing into `buf` as this thread's default.
    ///
    /// `#[tokio::test]` is a current-thread runtime, so the audit events fire
    /// on this thread and the thread-local default captures them.
    /// `ensure_callsite_interest` is required, not optional: without a global
    /// dispatcher a callsite can cache as `Interest::never()` and the event is
    /// dropped before the thread-local subscriber is ever consulted.
    fn capture_into(
        buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ) -> tracing::subscriber::DefaultGuard {
        crate::test_tracing::ensure_callsite_interest();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::INFO)
            .with_writer(LogBuf(buf.clone()))
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    /// The captured line whose `event` field is `event`.
    fn audit_line(logged: &str, event: &str) -> String {
        logged
            .lines()
            .find(|l| l.contains(&format!("\"event\":\"{event}\"")))
            .unwrap_or_else(|| panic!("expected an {event} record in: {logged}"))
            .to_string()
    }

    /// The four `target: "audit"` records in this module are the *entire*
    /// trail for an agent-action authorization — `store/src` has no table
    /// behind them. That makes two properties of the record behavioural
    /// rather than cosmetic: it must name who decided, and it must not name
    /// them by address.
    ///
    /// Both are asserted here because every other test on this path exercises
    /// it and checks nothing about the fields, which is exactly how a raw
    /// address survived on this line while the suite stayed green.
    #[tokio::test]
    async fn the_proposed_audit_record_names_the_person_and_carries_no_address() {
        let (engines, person_id) = welcome_fixture().await;
        let lawyer = store::persons::find_by_email_ci(&engines, "lawyer@neonlaw.com")
            .await
            .expect("query the seeded lawyer")
            .expect("welcome_fixture seeds lawyer@neonlaw.com");
        let (_, rpc) = routes(state_with(engines));

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let _guard = capture_into(&buf);
            let (_, body) = post_rpc_as(
                rpc,
                direct_skill(40, "send_welcome_email", &json!({ "person_id": person_id })),
                "lawyer@neonlaw.com",
            )
            .await;
            assert_eq!(
                body["result"]["status"]["state"], "input-required",
                "the pause is what emits the record under test; got: {body}"
            );
        }

        let logged = String::from_utf8(buf.lock().unwrap().clone()).expect("utf-8 log");
        let line = audit_line(&logged, "agent_action_authorization");

        assert!(
            line.contains(&format!("\"person_id\":\"{}\"", lawyer.id)),
            "the proposing lawyer must be queryable by id: {line}"
        );
        assert!(
            line.contains("\"principal_kind\":\"authenticated\""),
            "the record must still say whether the caller authenticated: {line}"
        );
        assert!(
            line.contains("\"decision\":\"proposed\""),
            "the record must name the decision it is recording: {line}"
        );
        // The load-bearing half, and the mutation guard: `principal =
        // …p.email…` stood on this line and no test failed. An address must
        // now turn this red.
        assert!(
            !line.contains('@'),
            "no email address may reach the audit stream: {line}"
        );
    }

    /// The wrong-caller denial is the decision a reader of the trail most
    /// wants an actor on, and it was the one left without one: its
    /// `audit_authorization` call passed `None` for both the tier and the row.
    #[tokio::test]
    async fn the_denied_identity_record_names_the_person_who_tried() {
        let (engines, person_id) = welcome_fixture().await;
        seed_person(
            &engines,
            "Other Lawyer",
            "other@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let other = store::persons::find_by_email_ci(&engines, "other@neonlaw.com")
            .await
            .expect("query the second lawyer")
            .expect("seeded just above");
        let (_, rpc) = routes(state_with(engines));

        let (_, body) = post_rpc_as(
            rpc.clone(),
            direct_skill(41, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;
        assert_eq!(
            body["result"]["status"]["state"], "input-required",
            "the pause must exist before another account may be refused it; got: {body}"
        );
        let task_id = body["result"]["id"].as_str().expect("task id").to_string();
        let context_id = body["result"]["contextId"]
            .as_str()
            .expect("context id")
            .to_string();

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let _guard = capture_into(&buf);
            let (_, refused) = post_rpc_as(
                rpc,
                confirm_reply(42, &task_id, &context_id, "yes"),
                "other@neonlaw.com",
            )
            .await;
            assert_eq!(
                refused["result"]["status"]["state"], "failed",
                "the wrong account must be refused; got: {refused}"
            );
        }

        let logged = String::from_utf8(buf.lock().unwrap().clone()).expect("utf-8 log");
        let line = audit_line(&logged, "agent_action_authorization");

        assert!(
            line.contains("\"decision\":\"denied_identity\""),
            "the refusal must be recorded as denied_identity: {line}"
        );
        assert!(
            line.contains(&format!("\"person_id\":\"{}\"", other.id)),
            "the person who tried to approve must be named: {line}"
        );
        assert!(
            !line.contains('@'),
            "no email address may reach the audit stream: {line}"
        );
    }

    /// The `fields` object of a captured JSON line, as a sorted key list.
    /// `tracing_subscriber`'s JSON layer nests every event field under
    /// `fields`, so this is the record's field set as a reader of the stream
    /// sees it.
    fn field_names(line: &str) -> Vec<String> {
        let parsed: Value = serde_json::from_str(line).expect("a captured line is JSON");
        let mut names: Vec<String> = parsed["fields"]
            .as_object()
            .expect("the JSON layer nests event fields under `fields`")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The `decision = "proposed"` record emitted when a lawyer names a
    /// confirmation-gated skill directly.
    ///
    /// Asserted as an *exact* set rather than a presence check, because the
    /// defect being closed was a field that should not have been there. A
    /// presence check cannot fail on an extra field; an exact set can, so
    /// re-adding `arguments` — or any other unreviewed field — turns this red
    /// instead of passing quietly the way the `direct_skill_side_effect_*`
    /// tests did while a whole tool-call payload rode this line.
    #[tokio::test]
    async fn the_proposed_record_carries_a_digest_and_a_count_and_no_payload() {
        let (engines, person_id) = welcome_fixture().await;
        let (_, rpc) = routes(state_with(engines));

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let _guard = capture_into(&buf);
            let (_, body) = post_rpc_as(
                rpc,
                direct_skill(50, "send_welcome_email", &json!({ "person_id": person_id })),
                "lawyer@neonlaw.com",
            )
            .await;
            assert_eq!(
                body["result"]["status"]["state"], "input-required",
                "the pause is what emits the record under test; got: {body}"
            );
        }
        let logged = String::from_utf8(buf.lock().unwrap().clone()).expect("utf-8 log");
        let line = audit_line(&logged, "agent_action_authorization");

        assert_eq!(
            field_names(&line),
            vec![
                "argument_count",
                "arguments_sha256",
                "decision",
                "event",
                "message",
                "person_id",
                "principal_kind",
                "routed_via_llm",
                "task_id",
                "tool",
            ],
            "the proposed record's field set is the contract; got: {line}"
        );

        let parsed: Value = serde_json::from_str(&line).expect("JSON line");
        let fields = &parsed["fields"];
        assert_eq!(
            fields["arguments_sha256"],
            Value::from(argument_digest(&json!({ "person_id": person_id }))),
            "the digest must be of the payload actually proposed: {line}"
        );
        assert_eq!(
            fields["argument_count"], 1,
            "one top-level argument was proposed: {line}"
        );
        // The load-bearing assertion, and the reason this test exists: the
        // *value* the payload carried must not be reconstructible from the
        // record. The argument names a different person from the approving
        // lawyer, so its id appearing here could only have come from the
        // payload.
        assert!(
            !line.contains(&person_id.to_string()),
            "no tool-call argument value may reach the audit stream: {line}"
        );
        assert!(
            !line.contains('@'),
            "no email address may reach the audit stream: {line}"
        );
    }

    /// The same contract for the record `audit_authorization` emits — the one
    /// serving all four terminal decisions (`denied_identity`,
    /// `denied_unauthorized`, `authorized`, `declined`). Exercised through
    /// `authorized`, the decision whose evidentiary weight is highest: it is
    /// the record that says a licensed human agreed to a client-facing act.
    #[tokio::test]
    async fn the_authorized_record_carries_a_digest_and_a_count_and_no_payload() {
        let (engines, person_id) = welcome_fixture().await;
        let (_, rpc) = routes(state_with(engines));

        let (_, body) = post_rpc_as(
            rpc.clone(),
            direct_skill(51, "send_welcome_email", &json!({ "person_id": person_id })),
            "lawyer@neonlaw.com",
        )
        .await;
        let task_id = body["result"]["id"].as_str().expect("task id").to_string();
        let context_id = body["result"]["contextId"]
            .as_str()
            .expect("context id")
            .to_string();

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let _guard = capture_into(&buf);
            let (_, approved) = post_rpc_as(
                rpc,
                confirm_reply(52, &task_id, &context_id, "yes"),
                "lawyer@neonlaw.com",
            )
            .await;
            assert_eq!(
                approved["result"]["status"]["state"], "completed",
                "the approval is what emits the record under test; got: {approved}"
            );
        }
        let logged = String::from_utf8(buf.lock().unwrap().clone()).expect("utf-8 log");
        let line = audit_line(&logged, "agent_action_authorization");

        assert_eq!(
            field_names(&line),
            vec![
                "approver_role",
                "argument_count",
                "arguments_sha256",
                "context_id",
                "decision",
                "event",
                "message",
                "person_id",
                "principal_kind",
                "proposer_kind",
                "task_id",
                "tool",
            ],
            "the authorization record's field set is the contract; got: {line}"
        );

        let parsed: Value = serde_json::from_str(&line).expect("JSON line");
        assert_eq!(
            parsed["fields"]["decision"], "authorized",
            "this test asserts the approval record specifically: {line}"
        );
        // The trail must still tie the proposal to its authorization. Both
        // halves of that: the id it was paused under, and a digest saying the
        // approved call is the *same* call that was proposed.
        assert_eq!(
            parsed["fields"]["task_id"], task_id,
            "the approval must be tied to the proposal it resolves: {line}"
        );
        assert_eq!(
            parsed["fields"]["arguments_sha256"],
            Value::from(argument_digest(&json!({ "person_id": person_id }))),
            "the digest must match the payload that was proposed: {line}"
        );
        assert_eq!(
            parsed["fields"]["tool"], "send_welcome_email",
            "the trail must say which tool was authorized: {line}"
        );
        assert!(
            !line.contains(&person_id.to_string()),
            "no tool-call argument value may reach the audit stream: {line}"
        );
        assert!(
            !line.contains('@'),
            "no email address may reach the audit stream: {line}"
        );
    }

    #[tokio::test]
    async fn direct_skill_exempt_crm_write_runs_without_a_pause() {
        // The other half of the decision: a bulk contact load must not
        // cost a round trip per row, so the CRM writers stay unpaused on
        // the lawyer-tier check alone. Asserts the row actually landed,
        // not merely that the task said `completed`.
        let engines = db().await;
        seed_person(
            &engines,
            "Firm Lawyer",
            "lawyer@neonlaw.com",
            store::persons::Role::Lawyer,
        )
        .await;
        let (_, rpc) = routes(state_with(engines.clone()));
        let (_, body) = post_rpc_as(
            rpc,
            direct_skill(
                37,
                "create_person",
                &json!({ "name": "Straight Through", "email": "straight@example.com" }),
            ),
            "lawyer@neonlaw.com",
        )
        .await;

        assert_eq!(
            body["result"]["status"]["state"], "completed",
            "an exempt CRM write must not pause; got: {body}"
        );
        let landed = store::persons::find_by_email_ci(&engines, "straight@example.com")
            .await
            .expect("person lookup should succeed");
        assert!(landed.is_some(), "the row should have been written");
    }
}
