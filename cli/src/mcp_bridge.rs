//! `navigator site mcp` — a stdio MCP server that speaks A2A upstream.
//!
//! Claude speaks MCP: local stdio servers and remote HTTP connectors. It
//! has no A2A client, so pointing it at `/app/api/aida/rpc` does nothing
//! on its own. This is the adapter: MCP over stdio facing Claude, A2A
//! `message/send` facing a running deployment.
//!
//! ```text
//! Claude Desktop / Claude Code
//!   │  MCP JSON-RPC over stdio (newline-delimited)
//!   ▼
//! navigator site mcp
//!   │  POST /app/api/aida/rpc, metadata.skill, Bearer <site login token>
//!   ▼
//! web → portal::a2a::dispatch_single → mcp::tools::call_tool
//! ```
//!
//! **Why A2A upstream and not `/mcp` upstream**, when this speaks MCP on
//! the near side: A2A is where the supervision gate lives, because A2A
//! has an `input-required` Task state and MCP has no equivalent.
//! Dispatching through A2A means a call from Claude meets the same
//! lawyer-tier check and the same `target: "audit"` events a call from
//! Gemini Enterprise meets. Sending to `/mcp` instead would skip all of
//! it.
//!
//! **Claude picks the tool, not Gemini.** A2A's free-form path runs its
//! own agentic loop with Vertex AI choosing the tools; bridging that
//! would put two models in series and let the weaker one choose the
//! actions. This uses `metadata.skill` direct dispatch, so the tool and
//! its arguments are Claude's decision and the host's only job is to
//! authorize and execute.
//!
//! ## The catalog is deliberately narrower than the host's
//!
//! Only tools that need no human approval are advertised — see
//! [`advertised_catalog`]. MCP has no way to pause a call and ask a
//! person, and a "handshake" where the model makes both calls is the
//! model approving its own action, not supervision. So rather than
//! simulate a gate this transport cannot carry, the tools that require
//! one are absent from `tools/list`, and naming one anyway returns a
//! result saying where to perform it. What remains — every read, plus
//! the CRM writers — is enough to open a matter with its entities and
//! people.
//!
//! ## Version skew
//!
//! The advertised descriptors are compiled into this binary, so a CLI
//! older than the deployment advertises the catalog it shipped with. The
//! failure is graceful in both directions: a tool the host has but this
//! binary does not is simply unavailable, and one this binary advertises
//! that the host has dropped comes back as an `Unknown` tool error
//! naming it. Neither can execute the wrong thing.

use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use mcp::protocol::{codes, Request, Response, PROTOCOL_VERSION};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// The A2A JSON-RPC endpoint every call is dispatched to.
const A2A_RPC_PATH: &str = "/app/api/aida/rpc";

/// What this server calls itself on `initialize`. Distinct from the
/// in-cluster `navigator-mcp` that `mcp::server` reports, so a client
/// with both configured can tell which transport answered.
const SERVER_NAME: &str = "navigator-site-mcp";

/// The upstream A2A surface, behind a trait so the stdio dispatch can be
/// tested without a deployment. The real implementation is
/// [`HttpUpstream`]; tests supply a fake.
#[async_trait::async_trait]
pub trait Upstream: Send + Sync {
    /// Dispatch one named skill and return the A2A `Task` it produced.
    async fn send_skill(&self, skill: &str, arguments: &Value) -> Result<Value>;
}

/// The real upstream: an authenticated `POST` to a running deployment.
///
/// The credential is resolved per call rather than once at startup, and
/// both halves of that matter. A server that refused to start without a
/// login would appear in the client as a dead entry with no message,
/// because a stdio server that exits has no way to say why — whereas one
/// that starts, lists its tools, and explains itself on first use puts
/// the reason where the user is looking. And because
/// `remote::resolve` re-reads `~/.navigator.json` each time, a
/// `navigator site login` part-way through a session is picked up on the
/// next call with no restart, which is exactly what an eight-hour token
/// needs.
pub struct HttpUpstream {
    client: reqwest::Client,
    host: Option<String>,
}

impl HttpUpstream {
    #[must_use]
    pub fn new(host: Option<&str>) -> Self {
        Self {
            client: client(),
            host: host.map(ToOwned::to_owned),
        }
    }
}

/// The HTTP client every dispatch uses. It does **not** follow redirects,
/// and that is the whole point of having a constructor for it.
///
/// An unaccepted credential does not come back as an error status. The
/// policy layer answers `303` toward a sign-in page, and a client that
/// follows the hop fetches that page and hands back HTML — so the failure
/// surfaces as "A2A response was not valid JSON" and reads like a
/// protocol bug rather than the auth problem it is. Refusing the hop lets
/// the `303` reach [`dispatch`], which knows what it means.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        // Only a TLS/resolver initialization failure can fail this, and
        // the default builder is what `Client::new` itself unwraps.
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[async_trait::async_trait]
impl Upstream for HttpUpstream {
    async fn send_skill(&self, skill: &str, arguments: &Value) -> Result<Value> {
        let (base, token) = crate::remote::resolve(self.host.as_deref())?;
        dispatch(&self.client, &base, &token, skill, arguments).await
    }
}

/// POST one named skill to a deployment's A2A endpoint and return the
/// `Task` it produced.
///
/// Split out of [`HttpUpstream::send_skill`] so that finding a credential
/// and speaking the protocol are separable — the caller resolves the
/// first, this owns the second, and a test can exercise every branch here
/// against a local server without a credential file or a mutated
/// environment.
async fn dispatch(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    skill: &str,
    arguments: &Value,
) -> Result<Value> {
    // The A2A envelope for a direct dispatch: no text parts, the skill
    // and its arguments in `metadata`.
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "message/send",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "role": "user",
                "kind": "message",
                "parts": [],
                "metadata": { "skill": skill, "arguments": arguments }
            }
        }
    });
    let url = format!("{base}{A2A_RPC_PATH}");
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;

    let status = resp.status();
    let text = resp.text().await.context("read A2A response body")?;
    if !status.is_success() {
        // 401 here is nearly always the one-hour CLI token having aged
        // out mid-session. Say so, rather than handing Claude a bare
        // status code it will paraphrase as a mystery.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "the stored token for {base} is no longer accepted — run \
                 `navigator site login --host {base}`, then try again (no restart needed)"
            ));
        }
        // A `303` is the policy layer redirecting an unauthenticated
        // caller to a login page, which a JSON-RPC client cannot follow.
        if status == reqwest::StatusCode::SEE_OTHER {
            return Err(anyhow!(
                "{base} redirected the call to a sign-in page, so the stored credential was \
                 not accepted — run `navigator site login --host {base}` and try again"
            ));
        }
        return Err(anyhow!("A2A returned {status}: {text}"));
    }
    let envelope: Value = serde_json::from_str(&text).context("A2A response was not valid JSON")?;
    if let Some(err) = envelope.get("error") {
        return Err(anyhow!("A2A error: {err}"));
    }
    envelope
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("A2A response carried neither `result` nor `error`"))
}

/// The tools this bridge advertises: every catalog entry that runs
/// without a human approving it — the reads, plus the CRM writers
/// exempted by `mcp::tools::requires_confirmation`.
///
/// The predicate itself lives in [`mcp::tools::advertised_catalog`],
/// shared with the `/mcp` endpoint so the two transports cannot drift:
/// a newly-gated tool is withheld from both without a second edit.
///
/// Confirmation is the only reason a tool is withheld. It used to not be:
/// `aida_list_projects` was also held back because the read carried no
/// principal and returned every matter in the deployment. Since ENG-216
/// it answers through the caller's own lens — a firm or client
/// participant gets the matters they are on, an owner or admin gets the
/// oversight directory — so there is nothing left for this transport to
/// withhold, and a read that discloses only what its caller may see is
/// one a model client may be handed.
#[must_use]
pub fn advertised_catalog() -> Vec<Value> {
    mcp::tools::advertised_catalog()
}

/// An MCP `tools/call` result. MCP carries tool *failures* in the result
/// with `isError: true`, reserving JSON-RPC errors for protocol faults —
/// so a refusal or a tool error still returns `Ok` at this layer.
fn tool_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    })
}

/// Render an A2A `Task` as an MCP tool result.
///
/// `completed` carries the artifacts; `failed` carries the reason in
/// `status.message`. `input-required` should be unreachable, because a
/// tool that pauses is never advertised — but a host newer than this
/// binary could gate something this catalog still lists, so it is
/// handled rather than assumed away.
fn task_to_tool_result(task: &Value) -> Value {
    let state = task
        .pointer("/status/state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match state {
        "completed" => {
            let text = artifact_text(task);
            if text.is_empty() {
                tool_result("Done.", false)
            } else {
                tool_result(text, false)
            }
        }
        "input-required" => tool_result(
            "This action needs a lawyer's explicit approval, which this connection cannot \
             collect. Perform it in the Navigator app, where the approval is recorded.",
            true,
        ),
        "canceled" => tool_result(
            format!("Canceled. {}", status_text(task))
                .trim_end()
                .to_string(),
            true,
        ),
        _ => {
            let reason = status_text(task);
            if reason.is_empty() {
                tool_result(
                    format!("The action did not complete (state: {state})."),
                    true,
                )
            } else {
                tool_result(reason, true)
            }
        }
    }
}

/// Every `text` part across the task's artifacts, plus any `data` part
/// rendered as JSON so structured tool output survives the trip.
fn artifact_text(task: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    if let Some(artifacts) = task.get("artifacts").and_then(Value::as_array) {
        for artifact in artifacts {
            if let Some(parts) = artifact.get("parts").and_then(Value::as_array) {
                for part in parts {
                    match part.get("kind").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                out.push(t.to_string());
                            }
                        }
                        Some("data") => {
                            if let Some(d) = part.get("data") {
                                out.push(
                                    serde_json::to_string_pretty(d)
                                        .unwrap_or_else(|_| d.to_string()),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out.join("\n")
}

/// The task's status message as plain text — where a `failed` task keeps
/// its reason.
fn status_text(task: &Value) -> String {
    task.pointer("/status/message/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Handle one parsed MCP request. Returns `None` for a notification,
/// which per JSON-RPC gets no reply at all.
pub async fn handle(request: &Request, upstream: &dyn Upstream) -> Option<Response> {
    // Notifications carry no `id` and must not be answered. MCP sends
    // `notifications/initialized` right after the handshake; replying to
    // it makes strict clients drop the connection. Taking the id through
    // `?` is the same statement as the guard: no id, no reply.
    let id = request.id.clone()?;

    match request.method.as_str() {
        "initialize" => Some(Response::ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            }),
        )),
        "ping" => Some(Response::ok(id, json!({}))),
        "tools/list" => Some(Response::ok(id, json!({ "tools": advertised_catalog() }))),
        "tools/call" => Some(handle_tools_call(id, &request.params, upstream).await),
        other => Some(Response::err(
            id,
            codes::METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        )),
    }
}

async fn handle_tools_call(id: Value, params: &Value, upstream: &dyn Upstream) -> Response {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Response::err(id, codes::INVALID_PARAMS, "`params.name` is required");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if !mcp::tools::is_advertised(name) {
        // Distinguish "gated, so deliberately absent" from "no such
        // tool". The first is a routing answer the user can act on; the
        // second is a mistake.
        let text = if mcp::tools::is_known_tool(name) {
            mcp::tools::withheld_message(name)
        } else {
            format!("`{name}` is not a tool this connection offers.")
        };
        return Response::ok(id, tool_result(text, true));
    }

    match upstream.send_skill(name, &arguments).await {
        Ok(task) => Response::ok(id, task_to_tool_result(&task)),
        // A transport failure is the bridge's problem, not the tool's, so
        // it still rides back as a tool error — Claude can relay it, and
        // the session survives to try again after a re-login.
        Err(e) => Response::ok(id, tool_result(format!("{e:#}"), true)),
    }
}

/// The stdio serve loop: newline-delimited JSON-RPC in, the same out.
///
/// Generic over the streams so a test can drive it with in-memory
/// buffers. Nothing but protocol frames may reach `writer` — an MCP
/// client parses every stdout line, so all diagnostics go to stderr.
pub async fn serve<R, W>(reader: R, mut writer: W, upstream: &dyn Upstream) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await.context("read stdin")? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(&request, upstream).await,
            Err(e) => Some(Response::err(
                Value::Null,
                codes::PARSE_ERROR,
                format!("parse error: {e}"),
            )),
        };
        if let Some(response) = response {
            let mut bytes = serde_json::to_vec(&response).context("serialize response")?;
            bytes.push(b'\n');
            writer.write_all(&bytes).await.context("write stdout")?;
            writer.flush().await.context("flush stdout")?;
        }
    }
    Ok(())
}

/// `navigator site mcp` — resolve the stored credential, then serve MCP
/// on stdio until the client closes it.
pub async fn run(host: Option<&str>) -> ExitCode {
    let upstream = HttpUpstream::new(host);
    // stderr, never stdout: the client is parsing stdout as protocol.
    // Report a missing login here as a warning rather than an exit — the
    // catalog still lists, and the first call explains itself.
    match crate::remote::resolve(host) {
        Ok((base, _)) => eprintln!("navigator site mcp: {base}"),
        Err(e) => eprintln!("navigator site mcp: not ready — {e:#}"),
    }
    eprintln!(
        "navigator site mcp: serving {} tool(s) over stdio",
        advertised_catalog().len()
    );
    match serve(tokio::io::stdin(), tokio::io::stdout(), &upstream).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("navigator site mcp: {e:#}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what was dispatched and returns a scripted Task.
    struct FakeUpstream {
        task: Value,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl FakeUpstream {
        fn returning(task: Value) -> Self {
            Self {
                task,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Upstream for FakeUpstream {
        async fn send_skill(&self, skill: &str, arguments: &Value) -> Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((skill.to_string(), arguments.clone()));
            Ok(self.task.clone())
        }
    }

    /// An upstream that always fails, for the transport-error path.
    struct BrokenUpstream;

    #[async_trait::async_trait]
    impl Upstream for BrokenUpstream {
        async fn send_skill(&self, _skill: &str, _arguments: &Value) -> Result<Value> {
            Err(anyhow!(
                "the stored token for https://x is no longer accepted"
            ))
        }
    }

    fn completed_task(text: &str) -> Value {
        json!({
            "id": "t-1",
            "contextId": "c-1",
            "kind": "task",
            "status": { "state": "completed", "timestamp": "2026-08-21T00:00:00Z" },
            "artifacts": [{
                "artifactId": "a-1",
                "name": "create_person",
                "parts": [{ "kind": "text", "text": text }]
            }],
            "history": []
        })
    }

    fn request(method: &str, params: &Value) -> Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.clone()
        }))
        .expect("well-formed request")
    }

    #[tokio::test]
    async fn initialize_reports_tools_capability_and_the_protocol_version() {
        let up = FakeUpstream::returning(completed_task("ok"));
        let resp = handle(&request("initialize", &json!({})), &up)
            .await
            .expect("initialize is a request, not a notification");
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert!(
            result["capabilities"]["tools"].is_object(),
            "a tools-only server must advertise the tools capability: {result}"
        );
    }

    #[tokio::test]
    async fn a_notification_is_never_answered() {
        // MCP sends `notifications/initialized` with no id right after
        // the handshake. Answering it breaks strict clients.
        let up = FakeUpstream::returning(completed_task("ok"));
        let notification: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap();
        assert!(handle(&notification, &up).await.is_none());
    }

    #[test]
    fn the_advertised_catalog_excludes_every_gated_tool() {
        let names: Vec<String> = advertised_catalog()
            .iter()
            .filter_map(|d| d["name"].as_str().map(ToString::to_string))
            .collect();

        // The onboarding chain this bridge exists to drive.
        for wanted in [
            "aida_create_person",
            "aida_create_project",
            "aida_link_person_project",
            "aida_bulk_import",
            "aida_list_entities",
            "aida_show_person",
        ] {
            assert!(
                names.iter().any(|n| n == wanted),
                "{wanted} must be advertised; got {names:?}"
            );
        }
        // Advertised since ENG-216 scoped it: the read answers through
        // the caller's own lens, so it discloses nothing this connection
        // could not already see. Re-withholding it would be a regression.
        assert!(
            names.iter().any(|n| n == "aida_list_projects"),
            "aida_list_projects is participation-scoped, so it must be advertised; got \
             {names:?}"
        );
        // The tools MCP cannot supervise must not appear at all.
        for gated in [
            "aida_send_welcome_email",
            "aida_create_notation",
            "aida_answer_notation",
        ] {
            assert!(
                !names.iter().any(|n| n == gated),
                "{gated} needs an approval this transport cannot collect, so it must not \
                 be advertised; got {names:?}"
            );
        }
        assert!(
            names.iter().all(|n| !mcp::tools::requires_confirmation(n)),
            "nothing advertised may require confirmation: {names:?}"
        );
    }

    #[tokio::test]
    async fn tools_list_returns_the_filtered_catalog_with_schemas() {
        let up = FakeUpstream::returning(completed_task("ok"));
        let resp = handle(&request("tools/list", &json!({})), &up)
            .await
            .unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), advertised_catalog().len());
        // Claude needs a schema per tool or it cannot form a call.
        for tool in &tools {
            assert!(
                tool["inputSchema"].is_object(),
                "every advertised tool needs an inputSchema: {tool}"
            );
        }
    }

    #[tokio::test]
    async fn calling_an_advertised_tool_dispatches_the_named_skill_upstream() {
        let up = FakeUpstream::returning(completed_task("Created Ada Counsel."));
        let resp = handle(
            &request(
                "tools/call",
                &json!({
                    "name": "aida_create_person",
                    "arguments": { "name": "Ada Counsel", "email": "ada@example.com" }
                }),
            ),
            &up,
        )
        .await
        .unwrap();

        let calls = up.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one upstream dispatch");
        assert_eq!(calls[0].0, "aida_create_person");
        assert_eq!(calls[0].1["email"], "ada@example.com");

        let result = resp.result.unwrap();
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], "Created Ada Counsel.");
    }

    #[tokio::test]
    async fn calling_a_gated_tool_never_reaches_the_host() {
        // The refusal is local. A gated tool must not be dispatched at
        // all — otherwise the host pauses and the pause has nowhere to go.
        let up = FakeUpstream::returning(completed_task("should not happen"));
        let resp = handle(
            &request(
                "tools/call",
                &json!({ "name": "aida_send_welcome_email", "arguments": { "person_id": "x" } }),
            ),
            &up,
        )
        .await
        .unwrap();

        assert!(
            up.calls.lock().unwrap().is_empty(),
            "a gated tool must not be dispatched upstream"
        );
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("approval") && text.contains("Navigator app"),
            "the refusal must say why and where to go instead, got: {text}"
        );
    }

    #[tokio::test]
    async fn the_matter_list_is_dispatched_upstream_now_that_it_is_scoped() {
        // This read used to be withheld here, because it returned every
        // matter in the deployment. It is now answered through the
        // caller's own lens by the host, so the bridge's job is to pass
        // it through rather than to stand in for a filter it cannot
        // apply.
        let up = FakeUpstream::returning(completed_task("the matters you are on"));
        let resp = handle(
            &request("tools/call", &json!({ "name": "aida_list_projects" })),
            &up,
        )
        .await
        .unwrap();
        assert_eq!(
            up.calls.lock().unwrap().len(),
            1,
            "a scoped read belongs upstream, where the identity is"
        );
        let result = resp.result.unwrap();
        assert_ne!(result["isError"], true, "got: {result}");
    }

    #[tokio::test]
    async fn calling_an_unknown_tool_says_so_without_dispatching() {
        let up = FakeUpstream::returning(completed_task("nope"));
        let resp = handle(
            &request("tools/call", &json!({ "name": "aida_not_a_tool" })),
            &up,
        )
        .await
        .unwrap();
        assert!(up.calls.lock().unwrap().is_empty());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("not a tool this connection offers"),
            "an unknown name is a mistake, not a routing answer: {text}"
        );
    }

    #[tokio::test]
    async fn a_failed_task_carries_its_reason_back_as_a_tool_error() {
        let failed = json!({
            "id": "t-2",
            "contextId": "c-2",
            "kind": "task",
            "status": {
                "state": "failed",
                "timestamp": "2026-08-21T00:00:00Z",
                "message": {
                    "kind": "message",
                    "messageId": "m-1",
                    "role": "agent",
                    "parts": [{ "kind": "text", "text": "invalid arguments: missing field `name`" }]
                }
            },
            "artifacts": [],
            "history": []
        });
        let up = FakeUpstream::returning(failed);
        let resp = handle(
            &request("tools/call", &json!({ "name": "aida_create_person" })),
            &up,
        )
        .await
        .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "invalid arguments: missing field `name`"
        );
    }

    #[tokio::test]
    async fn an_input_required_task_explains_itself_rather_than_hanging() {
        // Unreachable through the filter, but a host newer than this
        // binary could gate a tool this catalog still lists.
        let paused = json!({
            "id": "t-3",
            "contextId": "c-3",
            "kind": "task",
            "status": { "state": "input-required", "timestamp": "2026-08-21T00:00:00Z" },
            "artifacts": [],
            "history": []
        });
        let up = FakeUpstream::returning(paused);
        let resp = handle(
            &request("tools/call", &json!({ "name": "aida_create_person" })),
            &up,
        )
        .await
        .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Navigator app"));
    }

    #[tokio::test]
    async fn an_expired_token_becomes_a_readable_tool_error() {
        let resp = handle(
            &request("tools/call", &json!({ "name": "aida_create_person" })),
            &BrokenUpstream,
        )
        .await
        .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("no longer accepted"),
            "an aged-out token must reach the user as words"
        );
    }

    #[tokio::test]
    async fn an_unknown_method_is_a_jsonrpc_error() {
        let up = FakeUpstream::returning(completed_task("ok"));
        let resp = handle(&request("resources/list", &json!({})), &up)
            .await
            .unwrap();
        assert_eq!(resp.error.unwrap().code, codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn the_serve_loop_frames_one_response_per_line_and_skips_notifications() {
        let up = FakeUpstream::returning(completed_task("ok"));
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
        );
        let mut out: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut out, &up).await.unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "two requests, one notification → two frames: {text}"
        );
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["id"], 1);
        assert_eq!(second["id"], 2);
        assert!(second["result"]["tools"].is_array());
    }

    #[tokio::test]
    async fn a_malformed_line_is_a_parse_error_and_the_loop_continues() {
        let up = FakeUpstream::returning(completed_task("ok"));
        let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
        let mut out: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut out, &up).await.unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "the loop must survive a bad frame: {text}");
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["error"]["code"], codes::PARSE_ERROR);
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["id"], 7);
    }

    mod http {
        use super::*;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        async fn post(server: &MockServer) -> anyhow::Result<Value> {
            dispatch(
                &client(),
                &server.uri(),
                "the-stored-token",
                "aida_create_person",
                &json!({ "name": "Ada", "email": "ada@example.com" }),
            )
            .await
        }

        #[tokio::test]
        async fn the_request_carries_the_bearer_and_the_named_skill() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/app/api/aida/rpc"))
                .and(header("authorization", "Bearer the-stored-token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "id": "t", "contextId": "c", "kind": "task",
                                "status": { "state": "completed", "timestamp": "now" },
                                "artifacts": [], "history": [] }
                })))
                .expect(1)
                .mount(&server)
                .await;

            let task = post(&server).await.expect("a 200 with a result is a Task");
            assert_eq!(task["status"]["state"], "completed");
        }

        #[tokio::test]
        async fn an_unauthorized_answer_names_the_re_login() {
            // The one failure a user will actually hit: a token that aged
            // out part-way through a session.
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(401))
                .mount(&server)
                .await;

            let err = post(&server).await.expect_err("401 must not be a Task");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("navigator site login") && msg.contains("no restart needed"),
                "the message must say how to recover, got: {msg}"
            );
        }

        #[tokio::test]
        async fn a_redirect_to_sign_in_is_explained_rather_than_followed() {
            // What an unaccepted credential actually produces: the policy
            // layer redirects to a login page, which a JSON-RPC client
            // cannot follow. Left as a bare status this reads as a
            // protocol fault rather than an auth one.
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(303).insert_header("location", "/auth/login"))
                .mount(&server)
                .await;

            let err = post(&server).await.expect_err("303 must not be a Task");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("sign-in page") && msg.contains("navigator site login"),
                "a redirect should be reported as an auth problem, got: {msg}"
            );
        }

        #[tokio::test]
        async fn a_non_json_body_is_reported_as_such() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
                .mount(&server)
                .await;

            let err = post(&server).await.expect_err("HTML is not a Task");
            assert!(format!("{err:#}").contains("not valid JSON"));
        }

        #[tokio::test]
        async fn a_jsonrpc_error_envelope_surfaces_the_error() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": { "code": -32601, "message": "method not found" }
                })))
                .mount(&server)
                .await;

            let err = post(&server)
                .await
                .expect_err("an error envelope is not a Task");
            assert!(format!("{err:#}").contains("method not found"));
        }

        #[tokio::test]
        async fn a_result_less_envelope_is_an_error_not_a_silent_empty_task() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({ "jsonrpc": "2.0", "id": 1 })),
                )
                .mount(&server)
                .await;

            let err = post(&server).await.expect_err("neither result nor error");
            assert!(format!("{err:#}").contains("neither"));
        }

        #[tokio::test]
        async fn another_failing_status_carries_its_body_for_diagnosis() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
                .mount(&server)
                .await;

            let err = post(&server).await.expect_err("500 must not be a Task");
            let msg = format!("{err:#}");
            assert!(msg.contains("500") && msg.contains("boom"), "got: {msg}");
        }
    }
}
