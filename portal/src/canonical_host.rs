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
//!
//! Locally there is no DNS standing in for a second brand's real hostname, so
//! [`CanonicalHost`] also carries a *local port map*: a `BrandKey` a
//! developer reached by binding one of its own local ports (see
//! [`views::brand::BrandKey::local_port_env_var`]) resolves straight from the
//! `Host:` header's port, ahead of the ordinary hostname match. A browser or
//! `curl` sends the port it actually connected to in `Host:` whenever that
//! port is not the scheme default, so `localhost:<port>` alone is enough —
//! nothing here reads which socket accepted the connection.

use std::collections::BTreeMap;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use views::brand::{registered_brand_key, BrandKey};

#[derive(Clone)]
pub struct CanonicalHost {
    canonical: Option<String>,
    local_ports: BTreeMap<u16, BrandKey>,
}

impl CanonicalHost {
    /// Build from `CANONICAL_HOST` and every registered key's local-port env
    /// var. Empty / unset `CANONICAL_HOST` disables redirect enforcement;
    /// an unset local-port var means that key has no local override.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            std::env::var("CANONICAL_HOST")
                .ok()
                .filter(|s| !s.is_empty()),
        )
        .with_local_ports(local_brand_ports_from_lookup(|k| std::env::var(k).ok()))
    }

    #[must_use]
    pub fn new(host: Option<String>) -> Self {
        Self {
            canonical: host.filter(|s| !s.is_empty()),
            local_ports: BTreeMap::new(),
        }
    }

    /// Attach the local port → brand overrides this deployment resolved.
    /// Chainable so [`Self::from_env`] composes cleanly and a test can build
    /// one directly from a literal map.
    #[must_use]
    pub fn with_local_ports(mut self, local_ports: BTreeMap<u16, BrandKey>) -> Self {
        self.local_ports = local_ports;
        self
    }

    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.canonical.is_some()
    }

    /// The configured canonical hostname, if any. Public so other
    /// modules (e.g. the A2A agent card) can build absolute URLs that
    /// match the host the middleware redirects to.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.canonical.as_deref()
    }

    /// Every local port this deployment binds beyond the primary
    /// `PORT`, and the brand each one forces — what
    /// [`portal::hosting::run`](crate::hosting::run) binds additional
    /// listeners for.
    #[must_use]
    pub fn local_ports(&self) -> impl Iterator<Item = u16> + '_ {
        self.local_ports.keys().copied()
    }

    fn canonical(&self) -> Option<&str> {
        self.canonical.as_deref()
    }

    /// Resolve a brand straight from the `Host:` header's port, when that
    /// port is one of this deployment's local overrides. `None` when the
    /// header carries no port, or a port no override claims.
    fn resolve_local_port(&self, host_header: &str) -> Option<BrandKey> {
        let (_, port) = host_header.rsplit_once(':')?;
        let port: u16 = port.parse().ok()?;
        self.local_ports.get(&port).copied()
    }
}

/// Build the local port → brand map from any `key -> Option<value>` lookup.
/// The testable seam behind [`CanonicalHost::from_env`]: every registered
/// key without a set env var (including `Neon`, which has none) is simply
/// absent from the map.
fn local_brand_ports_from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> BTreeMap<u16, BrandKey> {
    BrandKey::ALL
        .iter()
        .copied()
        .filter_map(|key| {
            let var = key.local_port_env_var()?;
            let port: u16 = get(var)?.parse().ok()?;
            Some((port, key))
        })
        .collect()
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
    let raw_host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    // The local port map takes the full authority — a bound local port
    // forces its brand regardless of hostname, since `localhost` (or any
    // other host a developer's resolver happens to answer for) claims no
    // entry in the compiled registry. Every other match strips the port:
    // the registry and `CANONICAL_HOST` both name real hostnames, which
    // never carry a port of their own.
    let resolved = raw_host.and_then(|host| {
        cfg.resolve_local_port(host).or_else(|| {
            let stripped = strip_port(host);
            registered_brand_key(stripped)
                .or_else(|| (cfg.canonical() == Some(stripped)).then_some(BrandKey::default()))
        })
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
    use super::{local_brand_ports_from_lookup, strip_port, CanonicalHost};
    use views::brand::BrandKey;

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

    /// A registered key whose local-port var is set contributes its
    /// port; `Neon`, which has no such var, never appears in the map
    /// however the lookup answers.
    #[test]
    fn local_brand_ports_from_lookup_reads_every_registered_var() {
        let map = local_brand_ports_from_lookup(|k| {
            (k == "NAVIGATOR_LOCAL_DELETE_YOUR_DATA_PORT").then(|| "20630".to_string())
        });
        assert_eq!(map.get(&20_630), Some(&BrandKey::DeleteYourData));
        assert_eq!(map.len(), 1, "Neon must not appear: {map:?}");
    }

    /// An unset, empty, or non-numeric value leaves that key out of the
    /// map entirely rather than panicking or defaulting to port 0.
    #[test]
    fn local_brand_ports_from_lookup_ignores_unset_or_unparsable_values() {
        assert!(local_brand_ports_from_lookup(|_| None).is_empty());
        assert!(local_brand_ports_from_lookup(|_| Some(String::new())).is_empty());
        assert!(local_brand_ports_from_lookup(|_| Some("not-a-port".into())).is_empty());
    }

    /// The full authority (with port) resolves through the local port map
    /// even though the bare host resolves through neither the registry nor
    /// `CANONICAL_HOST`; a port absent from the map falls through instead
    /// of matching by accident.
    #[test]
    fn resolve_local_port_matches_only_a_registered_port() {
        let cfg = CanonicalHost::new(None)
            .with_local_ports([(20_630, BrandKey::DeleteYourData)].into_iter().collect());
        assert_eq!(
            cfg.resolve_local_port("localhost:20630"),
            Some(BrandKey::DeleteYourData)
        );
        assert_eq!(cfg.resolve_local_port("localhost:20600"), None);
        assert_eq!(cfg.resolve_local_port("localhost"), None);
    }

    /// [`CanonicalHost::local_ports`] is what `hosting::run` binds beyond
    /// the primary port — every key in the map, and nothing else.
    #[test]
    fn local_ports_lists_every_configured_local_port() {
        let cfg = CanonicalHost::new(None)
            .with_local_ports([(20_630, BrandKey::DeleteYourData)].into_iter().collect());
        assert_eq!(cfg.local_ports().collect::<Vec<_>>(), vec![20_630]);
        assert!(CanonicalHost::new(None).local_ports().next().is_none());
    }
}
