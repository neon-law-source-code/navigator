//! Canonical-host enforcement middleware.
//!
//! When `CANONICAL_HOST` is set, every request whose `Host:` header
//! is not the canonical value is permanently redirected to the same path on
//! the canonical host, except `/health`: kubelet and load-balancer probes
//! address a backend rather than its public hostname. When the env var is
//! unset (the default), the middleware is a pass-through — useful for local
//! development and integration tests.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

#[derive(Clone)]
pub struct CanonicalHost(Option<String>);

impl CanonicalHost {
    /// Build from `CANONICAL_HOST`. Empty / unset disables enforcement.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            std::env::var("CANONICAL_HOST")
                .ok()
                .filter(|s| !s.is_empty()),
        )
    }

    #[must_use]
    pub fn new(host: Option<String>) -> Self {
        Self(host.filter(|s| !s.is_empty()))
    }

    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.0.is_some()
    }

    /// The configured canonical hostname, if any. Public so other
    /// modules (e.g. the A2A agent card) can build absolute URLs that
    /// match the host the middleware redirects to.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.0.as_deref()
    }

    fn canonical(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// Axum middleware. Redirects non-canonical hosts; passes everything
/// else through unchanged.
pub async fn enforce_canonical_host(
    State(cfg): State<CanonicalHost>,
    req: Request,
    next: Next,
) -> Response {
    // Health probes reach a pod or backend IP and therefore cannot promise
    // the public Host header. Redirecting them would mark every backend
    // unhealthy as soon as canonical-host enforcement is enabled.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let Some(canonical) = cfg.canonical() else {
        return next.run(req).await;
    };
    let actual_host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(strip_port);
    match actual_host {
        Some(h) if h == canonical => next.run(req).await,
        _ => {
            let path_and_query = req
                .uri()
                .path_and_query()
                .map_or_else(|| "/".to_string(), ToString::to_string);
            let target = format!("https://{canonical}{path_and_query}");
            // Build a permanent redirect so caches learn.
            match Uri::try_from(&target) {
                Ok(_) => Redirect::permanent(&target).into_response(),
                Err(_) => (StatusCode::BAD_REQUEST, "invalid host redirect").into_response(),
            }
        }
    }
}

fn strip_port(host_header: &str) -> &str {
    host_header.split(':').next().unwrap_or(host_header)
}

#[cfg(test)]
mod tests {
    use super::{strip_port, CanonicalHost};

    #[test]
    fn from_env_disabled_when_var_unset_or_empty() {
        assert!(!CanonicalHost::new(None).is_enforced());
        assert!(!CanonicalHost::new(Some(String::new())).is_enforced());
    }

    #[test]
    fn enabled_when_set() {
        assert!(CanonicalHost::new(Some("example.org".into())).is_enforced());
    }

    #[test]
    fn strip_port_removes_port_when_present() {
        assert_eq!(strip_port("example.org"), "example.org");
        assert_eq!(strip_port("example.org:443"), "example.org");
        assert_eq!(strip_port("localhost:3001"), "localhost");
    }
}
