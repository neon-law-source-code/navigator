//! Axum-mounted MCP server. The router exposes a single `/mcp` POST
//! endpoint that speaks JSON-RPC 2.0 over HTTP — that's the
//! Streamable HTTP transport in the MCP spec, which `LibreChat`
//! understands out of the box.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde_json::{json, Value};
use tracing::Instrument;
use workflows::StateMachineRuntime;

use crate::principal::Principal;
use crate::protocol::{codes, Request, Response, PROTOCOL_VERSION};
use crate::tools::{self, ToolError};

/// State the MCP router carries. Holds clones of the shared `Db`
/// (so inserts the model triggers land in the same database the
/// public website reads from) and the questionnaire runtime (so
/// the conversational notation tools drive the same state machine
/// the admin HTML form does).
#[derive(Clone)]
pub struct McpState {
    /// The store. `persons` lives here, so every tool that resolves a
    /// person — and the lawyer-tier gate on the side-effecting tools —
    /// reads it.
    pub surreal: store::surreal::SurrealDb,
    pub questionnaire_runtime: Arc<dyn StateMachineRuntime>,
    /// Object storage. `portal::bootstrap` always populates it; the
    /// `Option` is for test fixtures that exercise only the read-only
    /// tool set. The notation tools read template bodies from storage,
    /// so they refuse when it is absent.
    pub storage: Option<Arc<dyn cloud::StorageService>>,
    /// The outbound mailer, same `Arc` the JSON API routes hold — the
    /// `LoggingEmail`-wrapped service whose `send` writes the
    /// `sent_emails` audit row. `aida_send_welcome_email` dispatches
    /// through `workflows::email::welcome::send_welcome` with it, which
    /// is how the agent door records what the API door records. `web`
    /// always populates it; the `Option` is for test fixtures that
    /// exercise only the tools that never send mail, and the tool
    /// refuses rather than sending unaudited when it is absent.
    pub email: Option<Arc<dyn workflows::EmailService>>,
}

impl McpState {
    #[must_use]
    pub fn new(
        surreal: store::surreal::SurrealDb,
        questionnaire_runtime: Arc<dyn StateMachineRuntime>,
    ) -> Self {
        Self {
            surreal,
            questionnaire_runtime,
            storage: None,
            email: None,
        }
    }
}

/// Build an `axum::Router` that hosts the MCP endpoint. The caller
/// can either serve this directly (standalone `mcp` binary) or merge
/// it into the web router (embedded mode).
pub fn build_router(state: McpState) -> Router {
    Router::new()
        .route("/mcp", post(rpc_handler))
        .with_state(state)
}

async fn rpc_handler(
    State(state): State<McpState>,
    principal: Option<Extension<Principal>>,
    body: String,
) -> impl IntoResponse {
    let principal = principal.map(|Extension(p)| p);
    // Parse JSON first so a malformed body produces a JSON-RPC parse
    // error rather than an axum-level 400 with no envelope.
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::OK,
                Json(serialize(Response::err(
                    Value::Null,
                    codes::PARSE_ERROR,
                    format!("parse error: {e}"),
                ))),
            );
        }
    };

    let request: Request = match serde_json::from_value(parsed) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::OK,
                Json(serialize(Response::err(
                    Value::Null,
                    codes::INVALID_REQUEST,
                    format!("invalid request: {e}"),
                ))),
            );
        }
    };

    if request.jsonrpc != "2.0" {
        let id = request.id.clone().unwrap_or(Value::Null);
        return (
            StatusCode::OK,
            Json(serialize(Response::err(
                id,
                codes::INVALID_REQUEST,
                "jsonrpc must be exactly \"2.0\"",
            ))),
        );
    }

    let response = dispatch(&state, principal.as_ref(), &request).await;
    (StatusCode::OK, Json(serialize(response)))
}

fn serialize(resp: Response) -> Value {
    // `Response` serializes infallibly — it's plain owned data.
    serde_json::to_value(resp).expect("Response is always serializable")
}

async fn dispatch(state: &McpState, principal: Option<&Principal>, req: &Request) -> Response {
    let id = req.id.clone().unwrap_or(Value::Null);
    match req.method.as_str() {
        "initialize" => Response::ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "navigator-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        // The advertised catalog, NOT the whole one: the confirmation-gated
        // tools are withheld from every model client. See
        // `tools::advertised_catalog` for why the two differ.
        "tools/list" => Response::ok(id, json!({ "tools": tools::advertised_catalog() })),
        "tools/call" => handle_tools_call(state, principal, id, &req.params).await,
        "ping" => Response::ok(id, json!({})),
        // MCP notifications expect no reply, but HTTP needs a body —
        // we return an empty success so the client gets a clean 200.
        m if m.starts_with("notifications/") => Response::ok(id, json!({})),
        other => Response::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        ),
    }
}

async fn handle_tools_call(
    state: &McpState,
    principal: Option<&Principal>,
    id: Value,
    params: &Value,
) -> Response {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Response::err(id, codes::INVALID_PARAMS, "`name` is required");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // A gated tool is absent from `tools/list`, but a client holding a stale
    // catalog — or one naming a tool it never listed — can still ask for it.
    // Refuse before dispatch, so the supervised act does not run, and say
    // where to perform it instead. An *unknown* name falls through to
    // `call_tool`, which answers it as `Unknown` rather than as a refusal.
    if tools::is_known_tool(name) && !tools::is_advertised(name) {
        telemetry::record_mcp_tool_called(name, telemetry::mcp_outcome::ERROR);
        return Response::ok(
            id,
            json!({
                "isError": true,
                "content": [{ "type": "text", "text": tools::withheld_message(name) }]
            }),
        );
    }

    // The one instrumented chokepoint for every `/mcp` tool call — the direct
    // counterpart to the A2A audit span, so the shared tool catalog is observable
    // on both protocol surfaces. Identifiers and counts only: the span and the
    // metric carry the tool NAME and the OUTCOME enum, never `arguments` (which
    // can hold a client name, email, or answer body) and never the result body.
    let result = tools::call_tool(state, principal, name, &arguments)
        .instrument(tracing::info_span!("mcp.tool.call", tool = name))
        .await;
    telemetry::record_mcp_tool_called(name, tool_call_outcome(&result));

    match result {
        Ok(value) => Response::ok(id, value),
        Err(e) => {
            tracing::warn!(tool = name, "mcp tool call failed");
            Response::ok(id, tool_error_payload(&e))
        }
    }
}

/// Map a tool-call result to its telemetry `outcome` label. Pure, so the
/// classification is unit-tested without a live meter; the label is an enum
/// string ([`telemetry::mcp_outcome`]), never the error text or the result body.
fn tool_call_outcome(result: &Result<Value, ToolError>) -> &'static str {
    if result.is_ok() {
        telemetry::mcp_outcome::OK
    } else {
        telemetry::mcp_outcome::ERROR
    }
}

/// MCP convention: tool-level failures are NOT JSON-RPC errors. They
/// ride on `result` with `isError: true` so the model can read the
/// failure text and recover.
fn tool_error_payload(err: &ToolError) -> Value {
    json!({
        "isError": true,
        "content": [{
            "type": "text",
            "text": err.to_string()
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::{build_router, tool_call_outcome, McpState};
    use crate::tools::ToolError;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;
    use workflows::InMemoryRuntime;

    async fn db() -> store::surreal::SurrealDb {
        store::test_support::mem_surreal().await
    }

    fn state(surreal: &store::surreal::SurrealDb) -> McpState {
        McpState::new(surreal.clone(), Arc::new(InMemoryRuntime::new()))
    }

    async fn call(router: axum::Router, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version_and_server_info() {
        let router = build_router(state(&db().await));
        let (status, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(body["result"]["serverInfo"]["name"], "navigator-mcp");
        assert!(body["result"]["capabilities"]["tools"].is_object());
    }

    /// `tools/list` serves the *advertised* catalog, so the three
    /// confirmation-gated tools are absent and the other twelve present.
    ///
    /// This assertion used to run the other way — it pinned
    /// `aida_create_notation` and `aida_answer_notation` as present,
    /// which is the defect ENG-315 fixed. Asserting the absent set is the
    /// point: a test that only checks what IS listed stays green when a
    /// supervised act is served by mistake.
    #[tokio::test]
    async fn tools_list_withholds_the_confirmation_gated_tools() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
        let tools = body["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

        for gated in [
            "aida_create_notation",
            "aida_answer_notation",
            "aida_send_welcome_email",
        ] {
            assert!(
                !names.contains(&gated),
                "`{gated}` is a supervised act and must not be advertised on /mcp; got {names:?}"
            );
        }

        for offered in [
            "aida_create_person",
            "aida_show_person",
            "aida_list_jurisdictions",
            "aida_list_entities",
            "aida_validate_notation",
            "aida_list_deadlines",
            "aida_create_project",
            "aida_list_projects",
            "aida_link_person_project",
            "aida_list_tools",
            "aida_bulk_import",
            "aida_spawn_legal_council",
        ] {
            assert!(
                names.contains(&offered),
                "`{offered}` missing; got {names:?}"
            );
        }

        assert_eq!(names.len(), 12, "got {names:?}");
        for name in &names {
            assert!(name.starts_with("aida_"), "got `{name}`");
        }
    }

    /// The withheld set is exactly `requires_confirmation`, computed
    /// rather than restated — so a newly-gated tool is covered by this
    /// test the day it is classified, with no edit here.
    #[tokio::test]
    async fn tools_list_is_exactly_the_unconfirmed_catalog() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
        let names: Vec<String> = body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect();

        let expected: Vec<String> = crate::tools::list_tools()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .filter(|n| !crate::tools::requires_confirmation(n))
            .map(String::from)
            .collect();
        assert_eq!(names, expected);
        assert!(
            crate::tools::list_tools().len() > expected.len(),
            "the served catalog must be strictly narrower than the whole one"
        );
    }

    /// Naming a gated tool anyway is refused *before* dispatch, with a
    /// message routing the caller to where the approval is recorded.
    #[tokio::test]
    async fn tools_call_refuses_a_gated_tool_without_dispatching() {
        let surreal = db().await;
        let router = build_router(state(&surreal));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "aida_create_notation",
                    "arguments": { "template": "anything" }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["isError"], true);
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("aida_create_notation"), "got `{text}`");
        assert!(text.contains("Navigator app"), "got `{text}`");
        assert!(text.contains("approval"), "got `{text}`");
    }

    /// The refusal is not the unknown-tool answer. An unrecognized name
    /// still falls through to `call_tool` and comes back as unknown, so
    /// "gated" and "no such tool" stay distinguishable to a caller.
    #[tokio::test]
    async fn tools_call_still_reports_an_unknown_tool_as_unknown() {
        let surreal = db().await;
        let router = build_router(state(&surreal));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": { "name": "aida_not_a_tool", "arguments": {} }
            }),
        )
        .await;
        assert_eq!(body["result"]["isError"], true);
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("Navigator app"),
            "unknown must not read as a gated refusal; got `{text}`"
        );
    }

    #[tokio::test]
    async fn tools_call_aida_create_person_inserts_a_row() {
        let surreal = db().await;
        let router = build_router(state(&surreal));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "aida_create_person",
                    "arguments": { "name": "Libra", "email": "libra@example.com" }
                }
            }),
        )
        .await;
        assert_ne!(body["result"]["isError"].as_bool(), Some(true));
        assert_eq!(body["result"]["structuredContent"]["name"], "Libra");

        let rows = store::persons::list_directory(&surreal, "", "", &[])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].email, "libra@example.com");
    }

    #[tokio::test]
    async fn tools_call_aida_show_person_returns_an_existing_row() {
        let surreal = db().await;
        let router = build_router(state(&surreal));
        // Seed via the create tool so the test exercises both surfaces.
        let (_, _) = call(
            router.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "aida_create_person",
                    "arguments": { "name": "Libra", "email": "libra@example.com" }
                }
            }),
        )
        .await;
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "aida_show_person",
                    "arguments": { "email": "libra@example.com" }
                }
            }),
        )
        .await;
        assert_ne!(body["result"]["isError"].as_bool(), Some(true));
        assert_eq!(body["result"]["structuredContent"]["count"], 1);
        assert_eq!(
            body["result"]["structuredContent"]["persons"][0]["name"],
            "Libra"
        );
        assert_eq!(
            body["result"]["structuredContent"]["persons"][0]["email"],
            "libra@example.com"
        );
    }

    #[tokio::test]
    async fn tools_call_aida_list_jurisdictions_returns_seeded_rows() {
        let surreal = db().await;
        store::jurisdictions::create(
            &surreal,
            &store::jurisdictions::NewJurisdiction::new("Nevada", "NV", "state"),
        )
        .await
        .unwrap();

        let router = build_router(state(&surreal));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "aida_list_jurisdictions",
                    "arguments": {}
                }
            }),
        )
        .await;
        assert_ne!(body["result"]["isError"].as_bool(), Some(true));
        assert_eq!(body["result"]["structuredContent"]["count"], 1);
        assert_eq!(
            body["result"]["structuredContent"]["jurisdictions"][0]["code"],
            "NV"
        );
    }

    #[tokio::test]
    async fn tools_call_with_unknown_tool_returns_iserror_result() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "does_not_exist", "arguments": {} }
            }),
        )
        .await;
        // MCP convention: tool failure is a result with isError=true,
        // not a JSON-RPC error envelope.
        assert!(body.get("error").is_none());
        assert_eq!(body["result"]["isError"], true);
        assert!(body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("does_not_exist"));
    }

    #[tokio::test]
    async fn tools_call_with_missing_name_param_is_invalid_params() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {}
            }),
        )
        .await;
        assert_eq!(body["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn tools_call_with_invalid_arguments_is_iserror_result() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "aida_create_person",
                    "arguments": { "name": "", "email": "libra@example.com" }
                }
            }),
        )
        .await;
        assert_eq!(body["result"]["isError"], true);
        assert!(body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid arguments"));
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found_error() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "2.0", "id": 7, "method": "bogus" }),
        )
        .await;
        assert_eq!(body["error"]["code"], -32601);
        assert!(body["error"]["message"].as_str().unwrap().contains("bogus"));
    }

    #[tokio::test]
    async fn jsonrpc_version_must_be_exactly_2_0() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "1.0", "id": 8, "method": "ping" }),
        )
        .await;
        assert_eq!(body["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn malformed_json_body_returns_parse_error() {
        let router = build_router(state(&db().await));
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from("{ not json"))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn ping_method_returns_empty_result() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }),
        )
        .await;
        assert_eq!(body["result"], json!({}));
    }

    #[tokio::test]
    async fn notifications_initialized_returns_ok_without_id() {
        let router = build_router(state(&db().await));
        let (_, body) = call(
            router,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        // No `id` in the request → null in the response per JSON-RPC.
        assert_eq!(body["id"], Value::Null);
        assert!(body["result"].is_object());
    }

    #[test]
    fn tool_call_outcome_maps_ok_and_error_to_enum_labels() {
        let ok: Result<Value, ToolError> = Ok(json!({"done": true}));
        assert_eq!(tool_call_outcome(&ok), "ok");

        let err: Result<Value, ToolError> = Err(ToolError::Unknown("nope".into()));
        assert_eq!(tool_call_outcome(&err), "error");
    }
}
