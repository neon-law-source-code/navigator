//! Natural-language → skill router for the A2A handler.
//!
//! A2A's design treats agents as opaque message handlers: the client
//! sends a freeform user message via `message/send` and the agent
//! decides which of its declared skills to invoke. This module owns
//! the seam where that decision would be made: the [`AgentRouter`]
//! trait takes the user's text plus the MCP tool descriptors and
//! returns a [`RoutedCall`] naming a specific tool and its arguments.
//! The A2A handler dispatches the returned call through the same
//! `mcp::tools::call_tool` that direct `metadata.skill` calls use, so
//! a routing decision is the *only* new code path a provider adds.
//!
//! **No provider ships today.** [`NullRouter`] is the only
//! implementation: it always returns [`RouterError::NotConfigured`],
//! and the A2A handler turns that into a Task pointing the caller at
//! `metadata.skill`. Navigator retired its Vertex AI Gemini router
//! along with the Gemini Enterprise registration — the CLI and a
//! general-purpose MCP client cover that ground, and both name the
//! tool themselves rather than asking Navigator to guess it from
//! prose.
//!
//! The trait stays because the seam is the invariant: a provider
//! (Claude direct, a local model, a rules engine) is a new
//! `impl AgentRouter` chosen from `lib.rs`, and it reuses this tool
//! catalog rather than forking one. The A2A handler never knows which
//! implementation it holds.

use async_trait::async_trait;
use serde_json::Value;

/// What the router picked. The tool name is the MCP tool name, which
/// is also the A2A skill id — one bare string, dispatched as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedCall {
    pub tool_name: String,
    pub arguments: Value,
}

/// One entry in the router's running view of the conversation. The
/// A2A handler owns this history and grows it a step at a time; the
/// router only reads it to decide the next move. Kept
/// provider-neutral: an implementation maps these onto whatever
/// transcript shape its own model expects.
#[derive(Debug, Clone)]
pub enum Turn {
    /// The human's free-form request. Always the first entry.
    User(String),
    /// A tool Navigator MCP chose and the handler then executed this round.
    /// `tool_name` is the MCP tool name (`show_person`).
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
