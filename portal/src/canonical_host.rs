//! Brand-host resolution and canonical-host enforcement middleware.
//!
//! Every request's `Host:` header resolves to a [`views::brand::BrandKey`]
//! through the compiled registry
//! ([`views::brand::registered_brand_key`]). A host the registry names
//! passes through carrying its own resolved brand, whatever `CANONICAL_HOST`
//! says. The deployment's own configured host — `CANONICAL_HOST`, whatever
//! literal value that deployment names — also passes through as the default
//! brand even when it is not itself a registry entry, which is what keeps an
//! arbitrary test host or a not-yet-registered deployment host working.
//! Every other host is permanently redirected to the same path on the
//! configured host, except `/health`: kubelet and load-balancer probes
//! address a backend rather than its public hostname. When `CANONICAL_HOST`
//! is unset (the default), enforcement is a pass-through and every host still
//! resolves to its registered brand (or the default brand for an
//! unregistered one) — useful for local development and integration tests.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use views::brand::{registered_brand_key, BrandKey};

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

/// Axum middleware. Resolves the request's brand from its `Host:` header and
/// stashes it as a [`BrandKey`] request extension for `scope_branding` to
/// read; redirects an unregistered, non-canonical host when enforcement is
/// configured.
pub async fn resolve_brand_and_enforce_host(
    State(cfg): State<CanonicalHost>,
    mut req: Request,
    next: Next,
) -> Response {
    // Health probes reach a pod or backend IP and therefore cannot promise
    // the public Host header. Redirecting them would mark every backend
    // unhealthy as soon as canonical-host enforcement is enabled.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let actual_host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(strip_port);
    let resolved = actual_host.and_then(|host| {
        registered_brand_key(host)
            .or_else(|| (cfg.canonical() == Some(host)).then_some(BrandKey::default()))
    });
    match (resolved, cfg.canonical()) {
        (Some(key), _) => {
            req.extensions_mut().insert(key);
            next.run(req).await
        }
        (None, None) => {
            req.extensions_mut().insert(BrandKey::default());
            next.run(req).await
        }
        (None, Some(canonical)) => {
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
