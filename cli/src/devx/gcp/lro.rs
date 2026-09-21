//! Long-running operation polling.
//!
//! GCP control-plane writes (`services.batchEnable`,
//! `compute.networks.insert`,
//! `run.projects.locations.services.create`, …) return an
//! `Operation` resource that becomes `done: true` minutes later.
//! Every step in `setup` follows the same recipe: parse the
//! operation `name`, poll its status endpoint until done, and bail
//! if the operation reports an `error`.
//!
//! In [`Mode::DryRun`](super::client::Mode::DryRun), the synthetic
//! `{}` body that [`super::client::GcpClient`] returns is treated
//! as an already-complete operation — we skip the polling entirely.

use std::time::Duration;

use serde_json::Value;

use super::client::{GcpClient, GcpService, Mode};
use super::error::{SetupError, SetupResult};
use super::services;

/// How long to sleep between polls of an LRO. Tests override the
/// real timer via `with_poll_interval` on a `LroWaiter`; production
/// uses a few seconds.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Hard cap on total polling time. A GKE Autopilot cluster create routinely
/// takes 5–10 minutes; the others finish in seconds.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_mins(15);

/// Wait for the operation `op` (the JSON body returned from the
/// initial create call) to complete.
///
/// `status_path` is a format string with one `{name}` slot — e.g.
/// `"/v1/{name}"` for `ServiceUsage`, `"/compute/v1/{name}"` for
/// `Compute`. The operation's `name` field is interpolated into it.
pub async fn wait(
    client: &GcpClient,
    service: GcpService,
    op: &Value,
    status_path_template: &str,
) -> SetupResult<Value> {
    wait_with_interval(
        client,
        service,
        op,
        status_path_template,
        DEFAULT_POLL_INTERVAL,
    )
    .await
}

/// Same as [`wait`] but with a configurable poll interval (tests
/// pass `Duration::ZERO` to make polling effectively synchronous).
pub async fn wait_with_interval(
    client: &GcpClient,
    service: GcpService,
    op: &Value,
    status_path_template: &str,
    interval: Duration,
) -> SetupResult<Value> {
    // Dry-run short-circuits: the synthetic `{}` returned by
    // `record_and_synthesize` is "done" by definition.
    if client.mode() == Mode::DryRun {
        return Ok(op.clone());
    }
    if is_done(op) {
        return check_op_error(op);
    }
    let name = op
        .get("name")
        .and_then(Value::as_str)
        .ok_or(SetupError::Malformed("operation has no `name` field"))?
        .to_string();
    let path = status_path_template.replace("{name}", &name);
    eprintln!("gcp operation [{service:?}] {name}: waiting at {path}");

    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > DEFAULT_TIMEOUT {
            return Err(SetupError::OperationTimeout {
                name,
                timeout: DEFAULT_TIMEOUT,
            });
        }
        let resp = client.get(service, &path).await?;
        let status = resp.status_u16();
        let response_body = resp.into_text();
        if !(200..=299).contains(&status) {
            if service == GcpService::Compute
                && status == 403
                && services::activation_is_propagating(&response_body, "compute.googleapis.com")
            {
                eprintln!(
                    "gcp operation [{service:?}] {name}: compute.googleapis.com activation is \
                     still propagating; retrying poll"
                );
                if interval.is_zero() {
                    tokio::task::yield_now().await;
                } else {
                    tokio::time::sleep(interval).await;
                }
                continue;
            }
            return Err(SetupError::BadStatus {
                operation: format!("polling {name}"),
                status,
                body: response_body,
            });
        }
        let body: Value =
            serde_json::from_str(&response_body).map_err(|source| SetupError::Json {
                what: "operation poll response",
                source,
            })?;
        if is_done(&body) {
            let completed = check_op_error(&body)?;
            eprintln!("gcp operation [{service:?}] {name}: DONE");
            return Ok(completed);
        }
        if interval.is_zero() {
            // Yield to the runtime so wiremock matchers can see the
            // queued response in tests.
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(interval).await;
        }
    }
}

fn is_done(op: &Value) -> bool {
    op.get("done").and_then(Value::as_bool) == Some(true)
        || op.get("status").and_then(Value::as_str) == Some("DONE")
}

fn check_op_error(op: &Value) -> SetupResult<Value> {
    if let Some(err) = op.get("error") {
        return Err(SetupError::OperationFailed(err.to_string()));
    }
    Ok(op.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpClient, GcpService, StaticToken};
    use super::wait_with_interval;

    #[tokio::test]
    async fn returns_immediately_when_op_is_already_done() {
        let server = MockServer::start().await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, server.uri());
        let op = json!({"name": "operations/x", "done": true});
        // No mocks registered — if we polled, wiremock would 404.
        let out = wait_with_interval(
            &client,
            GcpService::ServiceUsage,
            &op,
            "/v1/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out["done"], json!(true));
    }

    #[tokio::test]
    async fn returns_immediately_when_rest_operation_status_is_done() {
        let server = MockServer::start().await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, server.uri());
        let op = json!({"name": "operation-123", "status": "DONE"});
        // Compute operations use `status: DONE`, not the
        // google.longrunning.Operation `done` boolean.
        let out = wait_with_interval(
            &client,
            GcpService::Compute,
            &op,
            "/compute/v1/projects/p/global/operations/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out["status"], json!("DONE"));
    }

    #[tokio::test]
    async fn polls_status_endpoint_until_done() {
        let server = MockServer::start().await;
        // First poll: not done. Second: done.
        Mock::given(method("GET"))
            .and(path("/v1/operations/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operations/abc",
                "done": false
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/operations/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operations/abc",
                "done": true
            })))
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, server.uri());
        let op = json!({"name": "operations/abc"});
        let out = wait_with_interval(
            &client,
            GcpService::ServiceUsage,
            &op,
            "/v1/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out["done"], json!(true));
    }

    #[tokio::test]
    async fn retries_compute_poll_while_api_activation_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/compute/v1/projects/p/global/operations/operation-123",
            ))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": {
                    "status": "PERMISSION_DENIED",
                    "details": [{
                        "reason": "SERVICE_DISABLED",
                        "metadata": {
                            "service": "compute.googleapis.com"
                        }
                    }]
                }
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/compute/v1/projects/p/global/operations/operation-123",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-123",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, server.uri());
        let op = json!({"name": "operation-123", "status": "RUNNING"});
        let out = wait_with_interval(
            &client,
            GcpService::Compute,
            &op,
            "/compute/v1/projects/p/global/operations/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out["status"], json!("DONE"));
    }

    #[tokio::test]
    async fn does_not_retry_unrelated_compute_poll_permission_denial() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/compute/v1/projects/p/global/operations/operation-123",
            ))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": {
                    "status": "PERMISSION_DENIED",
                    "message": "caller cannot read operations"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, server.uri());
        let op = json!({"name": "operation-123", "status": "RUNNING"});
        let err = wait_with_interval(
            &client,
            GcpService::Compute,
            &op,
            "/compute/v1/projects/p/global/operations/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err}").contains("caller cannot read operations"),
            "got {err}"
        );
    }

    #[tokio::test]
    async fn bails_when_op_completes_with_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/operations/bad"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operations/bad",
                "done": true,
                "error": { "code": 7, "message": "permission denied" }
            })))
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, server.uri());
        let op = json!({"name": "operations/bad"});
        let err = wait_with_interval(
            &client,
            GcpService::ServiceUsage,
            &op,
            "/v1/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(format!("{err}").contains("permission denied"), "got {err}");
    }

    #[tokio::test]
    async fn dry_run_skips_polling_entirely() {
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, "http://127.0.0.1:1")
            .with_dry_run();
        let op = json!({"name": "operations/x"});
        // No `done` field, dry-run still returns immediately.
        let out = wait_with_interval(
            &client,
            GcpService::ServiceUsage,
            &op,
            "/v1/{name}",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out["name"], json!("operations/x"));
    }
}
