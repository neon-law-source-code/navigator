//! JSON-RPC 2.0 envelope types for the MCP transport.
//!
//! MCP rides on plain JSON-RPC — there is no protocol surface here
//! beyond what the spec already defines. Keeping the envelope in its
//! own module means the dispatcher in [`crate::server`] never has to
//! think about serde details.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP protocol revision we advertise on `initialize`. Bumping this
/// is a deliberate, breaking change for clients — keep in sync with
/// what `LibreChat` ships against.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Inbound JSON-RPC request. `id` is optional because notifications
/// (per the JSON-RPC spec) omit it.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Outbound JSON-RPC response. Either `result` or `error` is set,
/// never both — that's a spec requirement, enforced by the
/// constructors below.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 reserved error codes. We deliberately do not invent
/// new codes here — tool-level failures ride on top of `result` with
/// `isError: true`, the way MCP recommends.
pub mod codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

impl Response {
    #[must_use]
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{codes, Request, Response};
    use serde_json::{json, Value};

    #[test]
    fn parses_request_with_id_method_params() {
        let raw = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "create_person", "arguments": {} }
        });
        let req: Request = serde_json::from_value(raw).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(7)));
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.params["name"], "create_person");
    }

    #[test]
    fn parses_notification_without_id() {
        let raw = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let req: Request = serde_json::from_value(raw).unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "notifications/initialized");
        assert_eq!(req.params, Value::Null);
    }

    #[test]
    fn ok_response_omits_error_field() {
        let resp = Response::ok(json!(1), json!({"x": 1}));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], json!(1));
        assert_eq!(v["result"], json!({"x": 1}));
        assert!(v.get("error").is_none());
    }

    #[test]
    fn err_response_omits_result_field() {
        let resp = Response::err(json!(1), codes::METHOD_NOT_FOUND, "no such method");
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"]["code"], codes::METHOD_NOT_FOUND);
        assert_eq!(v["error"]["message"], "no such method");
        assert!(v.get("result").is_none());
    }

    #[test]
    fn reserved_error_codes_match_jsonrpc_spec() {
        assert_eq!(codes::PARSE_ERROR, -32700);
        assert_eq!(codes::INVALID_REQUEST, -32600);
        assert_eq!(codes::METHOD_NOT_FOUND, -32601);
        assert_eq!(codes::INVALID_PARAMS, -32602);
        assert_eq!(codes::INTERNAL_ERROR, -32603);
    }
}
