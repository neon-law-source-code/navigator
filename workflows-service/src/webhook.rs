//! GitHub webhook receiver hosted by the worker.
//!
//! `web` does not mount the receiver: `www.<domain>` remains behind the tailnet perimeter,
//! and GitHub — an external sender that cannot join it — can only reach the public
//! `workflows.<domain>` host. The receiver therefore runs here, on its own Axum
//! listener beside the Restate endpoint, and Envoy routes `/webhooks/github/*`
//! to it while every other path stays on the Restate leg.
//!
//! It is present only on the GitHub automation-home deployment: `receiver_from_env`
//! returns `None` everywhere else, exactly as the old `web` mount was absent
//! whenever `ReceiverConfig::from_env` failed. The receiver key requirement in
//! `store::deployment` (project-scoped to the automation home) is what makes
//! `ops ship` refuse an automation-home deployment that omits the credentials,
//! so a silent runtime absence cannot ship.

use std::net::SocketAddr;

use anyhow::Context;
use axum::Router;
use github_webhooks::ReceiverConfig;

/// The receiver's own listener. Distinct from the Restate endpoint's port so
/// Envoy can route `/webhooks/github/*` here and everything else to Restate.
const DEFAULT_WEBHOOK_LISTEN: &str = "0.0.0.0:9082";

/// Build the receiver router from the environment, or `None` when this
/// deployment is not the automation home (`ReceiverConfig::from_env` returns
/// `NotAutomationHome`) or the receiver credentials are unset.
#[must_use]
pub fn receiver_from_env() -> Option<Router> {
    ReceiverConfig::from_env()
        .ok()
        .map(|config| github_webhooks::webhook_routes(config.app_state()))
}

/// Resolve the receiver's listen address from `WORKFLOWS_WEBHOOK_LISTEN`,
/// defaulting to [`DEFAULT_WEBHOOK_LISTEN`].
///
/// # Errors
///
/// Returns an error when the configured value is not a valid socket address.
pub fn webhook_listen_addr(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<SocketAddr> {
    get("WORKFLOWS_WEBHOOK_LISTEN")
        .unwrap_or_else(|| DEFAULT_WEBHOOK_LISTEN.to_owned())
        .parse()
        .context("parse WORKFLOWS_WEBHOOK_LISTEN")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use github_webhooks::sig::sign_hmac_sha256_hex;
    use github_webhooks::{AppState, Route, RouterSettings, SubmissionError, WorkflowSubmitter};
    use tower::ServiceExt;

    use super::webhook_listen_addr;

    #[test]
    fn listen_addr_defaults_when_unset() {
        let addr = webhook_listen_addr(|_| None).expect("default parses");
        assert_eq!(addr.to_string(), "0.0.0.0:9082");
    }

    #[test]
    fn listen_addr_honours_the_override() {
        let addr = webhook_listen_addr(|key| {
            (key == "WORKFLOWS_WEBHOOK_LISTEN").then(|| "127.0.0.1:7000".to_owned())
        })
        .expect("override parses");
        assert_eq!(addr.to_string(), "127.0.0.1:7000");
    }

    #[test]
    fn listen_addr_rejects_a_malformed_value() {
        assert!(webhook_listen_addr(|_| Some("not-an-address".to_owned())).is_err());
    }

    /// The signature check never reaches Restate, so the mount is proven with a
    /// submitter that records nothing.
    struct NoopSubmitter;

    #[async_trait::async_trait]
    impl WorkflowSubmitter for NoopSubmitter {
        async fn submit(&self, _route: &Route) -> Result<(), SubmissionError> {
            Ok(())
        }
    }

    fn receiver(secret: &str) -> Router {
        let state = AppState::new(
            secret,
            RouterSettings::new(
                "owner/canonical".to_owned(),
                "owner".to_owned(),
                "app[bot]".to_owned(),
            ),
            Arc::new(NoopSubmitter),
        );
        github_webhooks::webhook_routes(state)
    }

    /// The worker serves the receiver and verifies the raw request body: a
    /// correctly signed request passes auth, and the identical bytes with a
    /// wrong signature are rejected. This guards the mount against any future
    /// middleware that would re-encode the body and break `X-Hub-Signature-256`.
    #[tokio::test]
    async fn a_signed_request_passes_and_a_tampered_signature_fails() {
        const SECRET: &str = "shhh";
        let body = br#"{"zen":"Keep it simple."}"#.to_vec();

        let signature = sign_hmac_sha256_hex(SECRET.as_bytes(), &body);
        let signed = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/github/{SECRET}"))
            .header("x-github-event", "ping")
            .header("x-github-delivery", "delivery-1")
            .header("x-hub-signature-256", signature)
            .body(Body::from(body.clone()))
            .expect("build signed request");
        let accepted = receiver(SECRET)
            .oneshot(signed)
            .await
            .expect("receiver responds");
        assert_ne!(
            accepted.status(),
            StatusCode::UNAUTHORIZED,
            "a correctly signed request must clear signature verification"
        );

        let tampered = Request::builder()
            .method("POST")
            .uri(format!("/webhooks/github/{SECRET}"))
            .header("x-github-event", "ping")
            .header("x-github-delivery", "delivery-2")
            .header("x-hub-signature-256", "sha256=deadbeef")
            .body(Body::from(body))
            .expect("build tampered request");
        let rejected = receiver(SECRET)
            .oneshot(tampered)
            .await
            .expect("receiver responds");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }
}
