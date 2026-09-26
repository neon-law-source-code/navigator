//! Start a Restate workflow by POSTing to the ingress.
//!
//! The one shared way the application *kicks off* a durable workflow
//! from outside Restate. Two callers converge here:
//!
//! - the `archives` crate's `trigger` binary (the nightly CronJob),
//!   which fires the `Archives` export workflow once per night, and
//! - `web`'s Cron schedules controls (`portal::cron_schedules`), which queue
//!   any scheduled workflow on demand for testing / recovery.
//!
//! Both need the same wire shape — `POST {ingress}/{Service}/{key}/{handler}`
//! with an optional `Authorization: Bearer …` header. Restate Cloud
//! authenticates every ingress call with the tenant bearer token
//! (`RESTATE_AUTH_TOKEN`); the in-cluster Restate Operator used in
//! KIND does not. Passing `auth_token = None` (or an empty string)
//! sends no header at all, so the same code path works in both
//! environments — the exact contract the [`crate::RestateRuntime`]
//! adapter already follows for the `notation` service.
//!
//! `one_way = true` targets Restate's `/send` variant: the call
//! returns as soon as the invocation is *accepted* (Restate then runs
//! it to completion on the worker, owning the retry schedule). Use it
//! when the caller must not block on the whole run — e.g. an HTTP
//! handler that would otherwise hold a request open for the duration
//! of a 26-table snapshot.

use serde::Serialize;
use thiserror::Error;

/// Failure starting a workflow invocation through the ingress.
#[derive(Debug, Error)]
pub enum TriggerError {
    /// The HTTP request never produced a response (DNS, connect, timeout).
    #[error("workflow trigger transport failure")]
    Transport,
    /// The ingress responded with a non-2xx status. A `401` here is
    /// the classic "bearer token missing or wrong" — the bug that
    /// silently stopped the nightly archives email.
    #[error("workflow trigger rejected with status {}", status.as_u16())]
    Rejected { status: reqwest::StatusCode },
}

/// POST to the Restate ingress to start one invocation of
/// `{service}/{key}/{handler}`.
///
/// - `ingress` — the Restate ingress base URL (Restate Cloud in prod,
///   the in-cluster `restate` Service in KIND). A trailing slash is
///   trimmed.
/// - `auth_token` — `Some(non-empty)` attaches `Authorization: Bearer
///   …`; `None` or `Some("")` sends no header (KIND / dev).
/// - `service` / `key` / `handler` — the Restate virtual-object
///   coordinates, e.g. `("Archives", "2026-06-05", "run")`.
/// - `body` — JSON request body for the handler (`&serde_json::json!({})`
///   for handlers that take an empty struct).
/// - `one_way` — `true` appends `/send` so the call returns on
///   acceptance instead of blocking until the workflow completes.
///
/// On success returns the ingress response body (for `/send` this is
/// the JSON `{"invocationId": "inv_…"}` the caller can log).
///
/// # Errors
///
/// [`TriggerError::Transport`] when the request can't be sent;
/// [`TriggerError::Rejected`] on any non-success status.
#[tracing::instrument(
    level = "info",
    name = "workflow.trigger",
    skip(ingress, auth_token, key, body),
    fields(
        service = service,
        handler = handler,
        operation = "workflow trigger",
        one_way
    )
)]
pub async fn start_workflow<B: Serialize + ?Sized>(
    ingress: &str,
    auth_token: Option<&str>,
    service: &str,
    key: &str,
    handler: &str,
    body: &B,
    one_way: bool,
) -> Result<String, TriggerError> {
    let suffix = if one_way { "/send" } else { "" };
    let url = format!(
        "{}/{}/{}/{}{}",
        ingress.trim_end_matches('/'),
        service,
        key,
        handler,
        suffix
    );

    // Bound the POST so a hung or unreachable ingress can never leave a
    // trigger pod running indefinitely. A `CronJob` with
    // `concurrencyPolicy: Forbid` treats a still-running job as a reason to
    // skip the next schedule, so an unbounded request turns one transient
    // ingress stall into a permanently wedged schedule (this is one half of
    // how the nightly Archives trigger silently stopped firing). 30s is far
    // longer than a healthy one-way `/send` (milliseconds) yet short enough
    // that the Job's `activeDeadlineSeconds` backstop and the next schedule
    // both still apply.
    let mut req = reqwest::Client::new()
        .post(&url)
        .json(body)
        .timeout(std::time::Duration::from_secs(30));
    // Empty token is treated as absent: a mounted-but-empty secret
    // must not produce `Authorization: Bearer ` (Restate Cloud rejects
    // that as malformed). Mirrors `RestateRuntime::with_auth_token`.
    if let Some(token) = auth_token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }

    // Inject the current span's W3C trace context (`traceparent`) so the
    // workflow handler can continue this trace across the Restate boundary
    // (extracted handler-side from `ctx.headers()`; see telemetry). Empty —
    // and a no-op — when OTLP is unconfigured or no sampled span is active, so
    // dev / KIND / OSS forks attach nothing. Only opaque trace context crosses
    // here, never a client field.
    for (name, value) in telemetry::current_trace_context_headers() {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            req = req.header(name, value);
        }
    }

    // Record the outcome as a metric (`navigator.workflow.trigger.fired`) and a
    // structured event on every path — safe fields and outcome counts only,
    // never identifiers or request content. This is the single instrumentation
    // point every trigger funnels through, so a service whose scheduled fire
    // silently stops shows up as a flat counter line and an absent "accepted" event.
    let Ok(resp) = req.send().await else {
        telemetry::record_trigger_fired(service, telemetry::outcome::TRANSPORT_ERROR);
        tracing::error!(
            service,
            handler,
            operation = "workflow trigger",
            outcome = telemetry::outcome::TRANSPORT_ERROR,
            "workflow trigger failed"
        );
        return Err(TriggerError::Transport);
    };
    let status = resp.status();
    let resp_body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        telemetry::record_trigger_fired(service, telemetry::outcome::REJECTED);
        tracing::error!(
            service,
            handler,
            operation = "workflow trigger",
            status = status.as_u16(),
            outcome = telemetry::outcome::REJECTED,
            "workflow trigger rejected by ingress"
        );
        return Err(TriggerError::Rejected { status });
    }
    telemetry::record_trigger_fired(service, telemetry::outcome::ACCEPTED);
    tracing::info!(
        service,
        handler,
        operation = "workflow trigger",
        status = status.as_u16(),
        outcome = telemetry::outcome::ACCEPTED,
        "workflow trigger accepted"
    );
    Ok(resp_body)
}

#[cfg(test)]
mod tests {
    use super::{start_workflow, TriggerError};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;
    use wiremock::matchers::{body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn posts_to_service_key_handler_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Archives/2026-06-05/run"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("{\"invocationId\":\"inv_1\"}"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let body = start_workflow(
            &server.uri(),
            None,
            "Archives",
            "2026-06-05",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap();
        assert!(body.contains("inv_1"));
    }

    #[tokio::test]
    async fn one_way_targets_the_send_variant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Archives/manual-7/run/send"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        start_workflow(
            &server.uri(),
            None,
            "Archives",
            "manual-7",
            "run",
            &json!({}),
            true,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn attaches_bearer_when_token_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .and(header("authorization", "Bearer s3cret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        start_workflow(
            &server.uri(),
            Some("s3cret"),
            "Archives",
            "d",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sends_no_authorization_header_when_token_absent() {
        let server = MockServer::start().await;
        // Any request carrying an Authorization header must NOT match.
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        start_workflow(
            &server.uri(),
            None,
            "Archives",
            "d",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn empty_token_is_treated_as_absent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        start_workflow(
            &server.uri(),
            Some(""),
            "Archives",
            "d",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn passes_the_json_body_through() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Archives/d/run"))
            .and(body_partial_json(json!({"run_date": "2026-06-05"})))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        start_workflow(
            &server.uri(),
            None,
            "Archives",
            "d",
            "run",
            &json!({"run_date": "2026-06-05"}),
            false,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rejected_error_and_trace_are_status_only() {
        let server = MockServer::start().await;
        let receipt = "receipt-sentinel-906";
        let invocation = "invocation-sentinel-906";
        let request_url = format!("{}/EmailSummary/{receipt}/run/send", server.uri());
        Mock::given(method("POST"))
            .and(path(format!("/EmailSummary/{receipt}/run/send")))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(format!("rejected {receipt} {invocation} {request_url}")),
            )
            .mount(&server)
            .await;

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        let err = start_workflow(
            &server.uri(),
            None,
            "EmailSummary",
            receipt,
            "run",
            &json!({}),
            true,
        )
        .await
        .unwrap_err();
        drop(guard);
        let message = err.to_string();
        let trace = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");

        match err {
            TriggerError::Rejected { status } => {
                assert_eq!(status.as_u16(), 401);
            }
            other @ TriggerError::Transport => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(message, "workflow trigger rejected with status 401");
        for rendered in [&message, &trace] {
            assert!(!rendered.contains(receipt), "unsafe receipt in {rendered}");
            assert!(
                !rendered.contains(invocation),
                "unsafe invocation in {rendered}"
            );
            assert!(!rendered.contains(&request_url), "unsafe URL in {rendered}");
        }
        assert!(trace.contains("EmailSummary"));
        assert!(trace.contains("run"));
        assert!(trace.contains("rejected"));
        assert!(trace.contains("401"));
    }

    #[tokio::test]
    async fn transport_error_and_trace_are_status_only() {
        let ingress = "http://[::1";
        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        let err = start_workflow(
            ingress,
            None,
            "Archives",
            "receipt-sentinel-transport-906",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap_err();
        drop(guard);
        let message = err.to_string();
        let trace = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");

        assert!(matches!(err, TriggerError::Transport));
        assert_eq!(message, "workflow trigger transport failure");
        assert!(!message.contains(ingress));
        assert!(!trace.contains(ingress));
        assert!(!trace.contains("receipt-sentinel-transport-906"));
        assert!(trace.contains("Archives"));
        assert!(trace.contains("run"));
        assert!(trace.contains("transport_error"));
    }

    #[tokio::test]
    async fn unreachable_ingress_becomes_transport_error() {
        let err = start_workflow(
            "http://192.0.2.1:1",
            None,
            "Archives",
            "d",
            "run",
            &json!({}),
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TriggerError::Transport));
    }
}
