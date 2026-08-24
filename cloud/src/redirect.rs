//! HTTP redirect service deployed to Cloud Run for standalone host redirects.
//!
//! Status code is 308 (`PERMANENT_REDIRECT`) to mirror the
//! workspace convention spelled out in
//! `k8s/overlays/gke/ingress/frontend-config.yaml` — clients
//! re-issue with the original method, which matters for any POST
//! traffic that ever lands on one of these hosts.
//!
//! The dispatch table lives in [`redirect_target`] — a pure
//! function over the `Host` so it's trivially unit-testable. The
//! axum wrapper in [`router`] turns `None` into 404.
//!
//! No host is currently registered here. This shell previously carried a
//! `chat.neonlaw.com` → Gemini Enterprise landing-page arm, retired because
//! the native Gemini app for macOS covers that access directly against the
//! same Workspace identity, with no bookmark redirect required. The next
//! arm this service is expected to carry is `neonlaw.org` → `www.neonlaw.com`
//! (path-preserving), tracked separately.

use axum::http::{HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::routing::any;
use axum::Router;

pub fn router() -> Router {
    Router::new().fallback(any(handler))
}

// `axum_extra::extract::Host` is deprecated (axum#3442 — it trusts
// `X-Forwarded-Host` / `Forwarded`, a spoofing footgun); read the
// `Host` header directly. This edge redirector sits behind GKE
// ingress, so the request-line `Host` is the authority we match on.
async fn handler(headers: HeaderMap) -> Result<Redirect, StatusCode> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::NOT_FOUND)?;
    redirect_target(host)
        .map(|t| Redirect::permanent(&t))
        .ok_or(StatusCode::NOT_FOUND)
}

/// Compute the redirect destination for a request, or `None` if
/// the host is one we don't own a rule for (handler turns that
/// into 404).
///
/// No arm is registered today — see the module docs. The signature and
/// doc comment stay so the next arm (`neonlaw.org`, tracked separately)
/// has a home to land in.
#[must_use]
pub fn redirect_target(_host: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn unknown_host_returns_none() {
        assert!(redirect_target("example.com").is_none());
        // The apex + www of neonlaw.com are intentionally NOT handled
        // here — the apex→www redirect is a DNSimple `URL` record, and
        // www is served by the stack that owns the marketing site.
        assert!(redirect_target("neonlaw.com").is_none());
        assert!(redirect_target("www.neonlaw.com").is_none());
    }

    #[tokio::test]
    async fn router_returns_404_for_unknown_host() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
