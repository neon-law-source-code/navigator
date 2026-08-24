//! Embedded Rego authorization decisions.
//!
//! Navigator compiles its one Rego policy once during server construction and
//! evaluates it in-process for each request. A policy that cannot compile is a
//! boot failure; an evaluation that cannot produce `true` is a deny.
//!
//! The `input.session.role` field — which the default policy
//! checks against the system-wide tier (`owner`, `admin`, `lawyer`, `clerk`, `client`)
//! — is always sourced from the `persons.role` column on the local
//! database, never from the IdP token. See
//! [`docs/access-model.md`](../../../docs/access-model.md) and
//! [`docs/oidc.md`](../../../docs/oidc.md).

use std::sync::Arc;

use regorus::{Engine, Value};
use thiserror::Error;

const ENTRYPOINT: &str = "data.navigator.authz.allow";
const POLICY_SOURCE: &str = include_str!("../policy/navigator.rego");

/// What [`PolicyClient::evaluate`] returns. `allow=false` is a deny; the raw
/// Rego result stays available for structured audit logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    pub allow: bool,
    pub raw: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("Rego policy {stage} failed: {message}")]
    Rego {
        stage: &'static str,
        message: String,
    },
    #[error("Regorus returned a result that was not JSON: {0}")]
    Result(String),
}

/// The compiled evaluator is cheap to clone and performs no I/O. Passthrough
/// exists solely for test fixtures that intentionally do not exercise authz.
#[derive(Debug, Clone)]
pub struct PolicyClient {
    inner: ClientInner,
}

#[derive(Debug, Clone)]
enum ClientInner {
    Enforced(Arc<regorus::CompiledPolicy>),
    Passthrough,
}

impl PolicyClient {
    /// Compile one Rego source at construction. Syntax and entrypoint failures
    /// are boot failures rather than a policy bypass.
    pub fn new(policy: &str) -> Result<Self, PolicyError> {
        let mut engine = Engine::new();
        engine
            .add_policy("navigator.rego".to_owned(), policy.to_owned())
            .map_err(|error| PolicyError::Rego {
                stage: "parse",
                message: error.to_string(),
            })?;
        let compiled = engine
            .compile_with_entrypoint(&ENTRYPOINT.into())
            .map_err(|error| PolicyError::Rego {
                stage: "compile",
                message: error.to_string(),
            })?;
        Ok(Self {
            inner: ClientInner::Enforced(Arc::new(compiled)),
        })
    }

    /// Build a passthrough client — `evaluate` always returns
    /// `allow=true` without touching the network. Use only for test fixtures
    /// that intentionally do not exercise authorization.
    #[must_use]
    pub fn passthrough() -> Self {
        Self {
            inner: ClientInner::Passthrough,
        }
    }

    /// `true` when this client applies the embedded policy.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        matches!(self.inner, ClientInner::Enforced(_))
    }

    /// Build the production client from Navigator's checked-in policy.
    pub fn embedded() -> Result<Self, PolicyError> {
        Self::new(policy_source())
    }

    /// Evaluate the policy against one request-local input document.
    pub fn evaluate(&self, input: &serde_json::Value) -> Result<PolicyDecision, PolicyError> {
        let compiled = match &self.inner {
            ClientInner::Enforced(compiled) => compiled,
            ClientInner::Passthrough => {
                return Ok(PolicyDecision {
                    allow: true,
                    raw: serde_json::Value::Bool(true),
                });
            }
        };
        let regorus_input =
            Value::from_json_str(&input.to_string()).map_err(|error| PolicyError::Rego {
                stage: "input conversion",
                message: error.to_string(),
            })?;
        let result =
            compiled
                .eval_with_input(regorus_input)
                .map_err(|error| PolicyError::Rego {
                    stage: "evaluation",
                    message: error.to_string(),
                })?;
        let raw: serde_json::Value = serde_json::from_str(
            &result
                .to_json_str()
                .map_err(|error| PolicyError::Result(error.to_string()))?,
        )
        .map_err(|error| PolicyError::Result(error.to_string()))?;
        Ok(PolicyDecision {
            allow: raw.as_bool().unwrap_or(false),
            raw,
        })
    }
}

const fn policy_source() -> &'static str {
    POLICY_SOURCE
}

/// Axum middleware that requires an embedded Rego `allow=true` for the request.
///
/// Reads the session cookie (if any), builds an `input` JSON
/// containing `path`, `method`, and `session`, and evaluates it in process.
/// On `allow=true` the next handler runs; on `allow=false` or any
/// evaluation error the request is rejected with `403 Forbidden`.
/// Errors are logged but never leaked to the client.
///
/// Designed to live underneath an existing session/auth middleware
/// so callers can rely on the session being populated before this
/// middleware fires. When no session cookie is present, the input
/// `session` field is `null` and the policy decides whether
/// unauthenticated access is allowed.
pub async fn require_policy(
    axum::extract::State((sessions, client)): axum::extract::State<(
        crate::session::SessionStore,
        PolicyClient,
    )>,
    cookies: tower_cookies::Cookies,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    // Primary: an upstream bearer-session middleware may already have
    // decoded the full signed session, including the DB person id.
    let mut session = req
        .extensions()
        .get::<crate::session::SessionData>()
        .cloned();
    // Browser SSO sets the same session shape in a cookie.
    if session.is_none() {
        session = cookies
            .get(crate::session::SESSION_COOKIE_NAME)
            .and_then(|c| sessions.decode(c.value()));
    }
    // Fallback: a bearer-token / Google-OAuth middleware upstream
    // has already authenticated the caller and inserted AuthClaims.
    // Synthesize a session-shaped value so the Rego rule that checks
    // `input.session.role` works for both flows uniformly.
    if session.is_none() {
        if let Some(claims) = req.extensions().get::<crate::auth::AuthClaims>() {
            session = Some(crate::session::SessionData {
                sub: claims.sub.clone(),
                email: Some(claims.sub.clone()),
                person_id: None,
                exp: claims.exp.max(0),
                role: claims.role,
                csrf_token: String::new(),
                source: crate::session::SessionSource::Browser,
                provider: None,
                impersonation: None,
            });
        }
    }
    let swagger_ui_request = req.headers().contains_key("x-navigator-swagger-ui");
    let path = req.uri().path().to_string();
    let path_segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    if req.method() == axum::http::Method::POST
        && path == "/app/impersonation/stop"
        && session
            .as_ref()
            .is_some_and(|session| session.impersonation.is_some())
    {
        let mut req = req;
        if let Some(session) = session {
            req.extensions_mut().insert(session);
        }
        return Ok(next.run(req).await);
    }
    let input = serde_json::json!({
        "path": path_segments,
        "method": req.method().as_str(),
        "session": session,
    });
    match client.evaluate(&input) {
        Ok(decision) if decision.allow => {
            let mut req = req;
            if let Some(session) = session {
                req.extensions_mut().insert(session);
            }
            Ok(next.run(req).await)
        }
        Ok(_) => Ok(deny_response(&path, session.is_some(), swagger_ui_request)),
        Err(e) => {
            tracing::warn!(error = %e, "policy evaluation failed; failing closed");
            Ok(deny_response(&path, session.is_some(), swagger_ui_request))
        }
    }
}

/// Render the denial. Anonymous visitors get a 303 to the OIDC
/// start endpoint so the experience is "click protected link →
/// land at the IdP", not "click protected link → blank 403 page".
/// Authenticated visitors who lack the role stay at 403 — the IdP
/// flow won't help them; they need a role grant in the DB. For
/// browser surfaces that 403 carries the styled HTML page; for
/// `/app/api/*` and `/mcp` it stays a tiny JSON body so JSON-RPC clients
/// see a parseable error.
fn deny_response(
    path: &str,
    has_session: bool,
    swagger_ui_request: bool,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if has_session {
        tracing::info!(path, "policy denied request (authenticated; 403)");
        if crate::wants_json(path) {
            (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "error": "forbidden" })),
            )
                .into_response()
        } else {
            (
                axum::http::StatusCode::FORBIDDEN,
                webapp::error_pages::forbidden(webapp::error_pages::Viewer::SignedIn),
            )
                .into_response()
        }
    } else if swagger_ui_request && crate::wants_json(path) {
        swagger_ui_unauthenticated(path)
    } else {
        let target = format!("/auth/login?return_to={}", percent_encode_path(path));
        tracing::info!(path, target = %target, "policy denied request (anonymous; redirecting to /auth/login)");
        axum::response::Redirect::to(&target).into_response()
    }
}

/// The refusal Swagger UI's "Try it out" gets when the reader has no session.
///
/// A bare `401` would surface in the explorer as an opaque failure, so the body
/// names the fix and points at `/app/api` — the page the reader is standing on
/// — rather than at the API path they happened to invoke.
///
/// Both anonymous gates emit this: [`crate::auth::require_session`], which is
/// the layer an anonymous "Try it out" now reaches first, and [`deny_response`]
/// for an authenticated-but-unresolvable caller the policy turns away. Sharing the
/// constructor is what keeps the two from drifting into different payloads for
/// the same user-visible situation.
pub(crate) fn swagger_ui_unauthenticated(path: &str) -> axum::response::Response {
    use axum::response::IntoResponse;

    let login = format!("/auth/login?return_to={}", percent_encode_path("/app/api"));
    tracing::info!(path, login = %login, "refused an anonymous Swagger UI request (401)");
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::WWW_AUTHENTICATE,
            "NavigatorSession realm=\"Neon Law Navigator API\"",
        )],
        axum::Json(serde_json::json!({
            "error": "unauthenticated",
            "message": "Sign in before using Swagger UI's Try it out.",
            "login": login,
        })),
    )
        .into_response()
}

/// Percent-encode a path so it survives being a `?return_to=` query
/// value. Only the small set of characters that materially break a
/// query string (`?`, `&`, `#`, `%`, `+`, space) is encoded — `/`
/// stays raw so the resulting URL is readable in the Location header.
///
/// Shared with [`crate::auth::require_session`] so the session boundary and
/// the policy deny produce byte-identical login redirects.
pub(crate) fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'?' => out.push_str("%3F"),
            b'&' => out.push_str("%26"),
            b'#' => out.push_str("%23"),
            b'%' => out.push_str("%25"),
            b'+' => out.push_str("%2B"),
            b' ' => out.push_str("%20"),
            _ => out.push(b as char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{PolicyClient, PolicyError};
    use serde_json::json;

    const POLICY: &str = r"
        package navigator.authz
        import rego.v1
        default allow := false
        allow if input.permitted
    ";

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn embedded_policy_allows_and_denies_without_network_io() {
        assert_send_sync::<PolicyClient>();
        let client = PolicyClient::new(POLICY).expect("policy compiles");
        let decision = client.evaluate(&json!({ "permitted": true })).unwrap();
        assert!(decision.allow);
        let decision = client.evaluate(&json!({ "permitted": false })).unwrap();
        assert!(!decision.allow);
    }

    #[test]
    fn undefined_or_non_boolean_results_deny() {
        let undefined =
            PolicyClient::new("package navigator.authz\nimport rego.v1\nallow if input.missing")
                .expect("policy compiles");
        let decision = undefined.evaluate(&json!({})).unwrap();
        assert!(!decision.allow);
        let non_boolean = PolicyClient::new(
            "package navigator.authz\nimport rego.v1\nallow := {\"not\": \"a boolean\"}",
        )
        .expect("policy compiles");
        let decision = non_boolean.evaluate(&json!({})).unwrap();
        assert!(!decision.allow);
    }

    #[test]
    fn evaluation_error_is_returned_for_the_middleware_to_deny() {
        let client = PolicyClient::new(
            "package navigator.authz\nimport rego.v1\ndefault allow := false\nallow := 1 / 0",
        )
        .expect("policy compiles");
        assert!(matches!(
            client.evaluate(&json!({})),
            Err(PolicyError::Rego {
                stage: "evaluation",
                ..
            })
        ));
    }

    #[test]
    fn malformed_policy_fails_construction_and_passthrough_stays_explicit() {
        assert!(matches!(
            PolicyClient::new("this is not Rego"),
            Err(PolicyError::Rego { .. })
        ));
        assert!(!PolicyClient::passthrough().is_enforced());
    }
}
