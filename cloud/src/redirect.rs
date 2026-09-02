//! HTTP redirect service deployed to Cloud Run for standalone host redirects.
//!
//! The service returns a 301 (`MOVED_PERMANENTLY`) for domain migrations.
//!
//! The dispatch table lives in [`redirect_target`] — a pure function over the
//! `Host` and request path so it's trivially unit-testable. The axum wrapper in
//! [`router`] turns `None` into 404.
//!
//! The registered hosts are `neonlaw.org` and `www.neonlaw.org`, both of which
//! redirect to the corresponding path on `www.neonlaw.com`.

use axum::http::{header::HeaderValue, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;

pub fn router() -> Router {
    Router::new().fallback(any(handler))
}

// `axum_extra::extract::Host` is deprecated (axum#3442 — it trusts
// `X-Forwarded-Host` / `Forwarded`, a spoofing footgun); read the
// `Host` header directly. This edge redirector sits behind GKE
// ingress, so the request-line `Host` is the authority we match on.
async fn handler(headers: HeaderMap, uri: Uri) -> Result<Response, StatusCode> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::NOT_FOUND)?;
    let path_and_query = uri.path_and_query().map_or("/", |value| value.as_str());
    let target = redirect_target(host, path_and_query).ok_or(StatusCode::NOT_FOUND)?;
    let location = HeaderValue::from_str(&target).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response())
}

/// Compute the redirect destination for a request, or `None` if the host is
/// one we don't own a rule for (handler turns that into 404).
#[must_use]
pub fn redirect_target(host: &str, path_and_query: &str) -> Option<String> {
    match host {
        "neonlaw.org" | "www.neonlaw.org" => {
            Some(format!("https://www.neonlaw.com{path_and_query}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn unknown_host_returns_none() {
        assert!(redirect_target("example.com", "/").is_none());
        // The apex + www of neonlaw.com are intentionally NOT handled
        // here — the apex→www redirect is a DNSimple `URL` record, and
        // www is served by the stack that owns the marketing site.
        assert!(redirect_target("neonlaw.com", "/").is_none());
        assert!(redirect_target("www.neonlaw.com", "/").is_none());
    }

    #[test]
    fn org_hosts_preserve_the_path_and_query() {
        for host in ["neonlaw.org", "www.neonlaw.org"] {
            assert_eq!(
                redirect_target(host, "/attorneys?source=org"),
                Some("https://www.neonlaw.com/attorneys?source=org".into())
            );
        }
    }

    #[tokio::test]
    async fn router_redirects_org_hosts_with_a_301() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/legal-aid?source=org")
                    .header("host", "neonlaw.org")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            response.headers().get(axum::http::header::LOCATION),
            Some(
                &"https://www.neonlaw.com/legal-aid?source=org"
                    .parse()
                    .unwrap()
            )
        );
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
