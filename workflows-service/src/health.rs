//! Always-on, unsigned liveness/readiness probe for the worker (ENG-551).
//!
//! The Restate SDK's endpoint on `:9080` requires a valid Restate Cloud
//! signature on every request once `RESTATE_IDENTITY_KEY` is set — kubelet
//! and the GCE LB can never produce one — and Envoy answers
//! `GET /restate/health` with a static `200` instead of proxying to the
//! worker (ENG-509, and rightly so: do not change that). Neither path ever
//! exercises the worker's own request-handling code, which is exactly how
//! ENG-550's `CryptoProvider` panic shipped invisibly. This listener is a
//! second, separate Axum server (distinct from both the Restate endpoint and
//! the GitHub webhook receiver) whose one route calls
//! [`crate::request_identity::crypto_provider_is_healthy`], round-tripping a
//! throwaway signature through the same verification path real traffic
//! uses, so a future regression there fails the probe instead of shipping
//! silently again.

use std::net::SocketAddr;

use anyhow::Context;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;

use crate::request_identity::crypto_provider_is_healthy;

/// Distinct from both the Restate endpoint's `:9080` and the GitHub webhook
/// receiver's `:9082`, so kubelet can reach this unconditionally on every
/// deployment (the webhook receiver binds only on the automation home).
const DEFAULT_HEALTH_LISTEN: &str = "0.0.0.0:9083";

pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> StatusCode {
    if crypto_provider_is_healthy() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// Resolve the probe's listen address from `WORKFLOWS_SERVICE_HEALTH_LISTEN`,
/// defaulting to [`DEFAULT_HEALTH_LISTEN`].
///
/// # Errors
///
/// Returns an error when the configured value is not a valid socket address.
pub fn health_listen_addr(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<SocketAddr> {
    get("WORKFLOWS_SERVICE_HEALTH_LISTEN")
        .unwrap_or_else(|| DEFAULT_HEALTH_LISTEN.to_owned())
        .parse()
        .context("parse WORKFLOWS_SERVICE_HEALTH_LISTEN")
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::{health_listen_addr, router};

    #[test]
    fn listen_addr_defaults_when_unset() {
        let addr = health_listen_addr(|_| None).expect("default parses");
        assert_eq!(addr.to_string(), "0.0.0.0:9083");
    }

    #[test]
    fn listen_addr_honours_the_override() {
        let addr = health_listen_addr(|key| {
            (key == "WORKFLOWS_SERVICE_HEALTH_LISTEN").then(|| "127.0.0.1:7001".to_owned())
        })
        .expect("override parses");
        assert_eq!(addr.to_string(), "127.0.0.1:7001");
    }

    #[test]
    fn listen_addr_rejects_a_malformed_value() {
        assert!(health_listen_addr(|_| Some("not-an-address".to_owned())).is_err());
    }

    #[tokio::test]
    async fn healthz_responds_ok_without_any_restate_signature() {
        crate::request_identity::install_crypto_provider();
        let request = Request::builder()
            .method("GET")
            .uri("/healthz")
            .body(Body::empty())
            .expect("unsigned health request builds");
        let response = router().oneshot(request).await.expect("router responds");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
