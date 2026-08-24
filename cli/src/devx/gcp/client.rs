//! HTTP client used by every `setup` step.
//!
//! Holds a [`reqwest::Client`], a [`TokenProvider`] (real ADC in
//! prod, a static string in tests), and a per-service `base_url` map.
//! Tests stand up a `wiremock` server and override the relevant base
//! URL — no traffic ever leaves the host, no GCP credentials needed.
//!
//! ## Dry-run mode
//!
//! `gcloud` has no universal `--dry-run` flag, so we provide one
//! here. In [`Mode::DryRun`], `get` / `post_json` / `patch_json` / `shell_out` never
//! touch the network or the host shell: they log the request via
//! `tracing::info!`, append a [`RecordedCall`] to an in-memory log,
//! and return a synthetic 200 [`HttpResponse`] / zero-exit
//! [`ShellResult`]. That gives us "show me what `setup` would do"
//! semantics without depending on external tools.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;

/// GCP services we talk to. One entry per `*.googleapis.com` host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GcpService {
    Storage,
    ServiceUsage,
    Compute,
    Container,
    Iap,
    CloudResourceManager,
    ArtifactRegistry,
    Iam,
    SecretManager,
    CloudKms,
}

impl GcpService {
    /// Real production base URL (no trailing slash).
    #[must_use]
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::Storage => "https://storage.googleapis.com",
            Self::ServiceUsage => "https://serviceusage.googleapis.com",
            Self::Compute => "https://compute.googleapis.com",
            Self::Container => "https://container.googleapis.com",
            Self::Iap => "https://iap.googleapis.com",
            Self::CloudResourceManager => "https://cloudresourcemanager.googleapis.com",
            Self::ArtifactRegistry => "https://artifactregistry.googleapis.com",
            Self::Iam => "https://iam.googleapis.com",
            Self::SecretManager => "https://secretmanager.googleapis.com",
            Self::CloudKms => "https://cloudkms.googleapis.com",
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("auth error: {0}")]
    Auth(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("shell command failed: {command}: {message}")]
    Shell { command: String, message: String },
}

/// Supplies a bearer token. Implementations:
/// - [`StaticToken`] for tests / emulators
/// - `AdcToken` (in [`super::auth`]) for production
#[async_trait]
pub trait TokenProvider: Send + Sync {
    async fn token(&self) -> Result<String, ClientError>;
}

/// Always returns the same string. Tests only.
pub struct StaticToken(pub String);

#[async_trait]
impl TokenProvider for StaticToken {
    async fn token(&self) -> Result<String, ClientError> {
        Ok(self.0.clone())
    }
}

/// Execute against real GCP, or just record what would be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Execute,
    DryRun,
}

/// One captured request, written in [`Mode::DryRun`] (or alongside
/// execution if a future caller wants an audit log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub method: &'static str,
    pub url: String,
    pub body: Option<String>,
}

/// Response body returned by [`GcpClient`]. Wraps just enough of
/// `reqwest::Response` for our callers (`status` + the body bytes)
/// so dry-run can synthesize one without going through reqwest.
pub struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    #[must_use]
    pub fn status_u16(&self) -> u16 {
        self.status
    }

    /// Consume the response and return its body as a UTF-8 string.
    /// Lossy on invalid bytes — matches what callers want for error
    /// messages.
    #[must_use]
    pub fn into_text(self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Thin wrapper around `reqwest::Client` with per-service base URL
/// overrides, a pluggable token source, and a dry-run mode.
pub struct GcpClient {
    http: reqwest::Client,
    token: Arc<dyn TokenProvider>,
    base_urls: HashMap<GcpService, String>,
    mode: Mode,
    recorded: Arc<Mutex<Vec<RecordedCall>>>,
}

impl GcpClient {
    /// Production constructor: real URLs, real HTTP, ADC tokens.
    #[must_use]
    pub fn new(token: Arc<dyn TokenProvider>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
            base_urls: HashMap::new(),
            mode: Mode::Execute,
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Override a service's base URL. Tests redirect traffic to a
    /// `wiremock` server with this; prod never overrides.
    #[cfg(test)]
    #[must_use]
    pub fn with_base_url(mut self, service: GcpService, url: impl Into<String>) -> Self {
        self.base_urls.insert(service, url.into());
        self
    }

    /// Switch the client into dry-run mode. No HTTP traffic will
    /// leave the process; calls are logged + recorded instead.
    #[must_use]
    pub fn with_dry_run(mut self) -> Self {
        self.mode = Mode::DryRun;
        self
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Snapshot of every call captured so far (dry-run only writes
    /// here; in `Execute` mode the log stays empty).
    #[must_use]
    pub fn recorded_calls(&self) -> Vec<RecordedCall> {
        self.recorded
            .lock()
            .expect("recorded lock poisoned")
            .clone()
    }

    /// Resolve the base URL for `service`, falling back to the real
    /// production endpoint.
    #[must_use]
    pub fn base_url(&self, service: GcpService) -> &str {
        self.base_urls
            .get(&service)
            .map_or_else(|| service.default_base_url(), String::as_str)
    }

    /// Build an absolute URL for `service` + `path`. `path` should
    /// start with `/`.
    #[must_use]
    pub fn url(&self, service: GcpService, path: &str) -> String {
        format!("{}{}", self.base_url(service), path)
    }

    /// Issue a GET. In `DryRun`, logs+records and returns a 200 with
    /// an empty JSON body.
    pub async fn get(&self, service: GcpService, path: &str) -> Result<HttpResponse, ClientError> {
        let url = self.url(service, path);
        if self.mode == Mode::DryRun {
            return Ok(self.record_and_synthesize("GET", &url, None));
        }
        let token = self.token.token().await?;
        let resp = self.http.get(&url).bearer_auth(token).send().await?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await?.to_vec();
        Ok(HttpResponse { status, body })
    }

    /// Issue a POST with a JSON body. In `DryRun`, logs+records the
    /// serialized body and returns a 200 with `{}`.
    pub async fn post_json(
        &self,
        service: GcpService,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<HttpResponse, ClientError> {
        let url = self.url(service, path);
        if self.mode == Mode::DryRun {
            let serialized =
                serde_json::to_string(body).unwrap_or_else(|_| "<unserializable>".into());
            return Ok(self.record_and_synthesize("POST", &url, Some(serialized)));
        }
        let token = self.token.token().await?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let resp_body = resp.bytes().await?.to_vec();
        Ok(HttpResponse {
            status,
            body: resp_body,
        })
    }

    /// Issue a PUT with a JSON body. In [`Mode::DryRun`], logs+records the
    /// serialized body and returns a 200 with `{}`.
    pub async fn put_json(
        &self,
        service: GcpService,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<HttpResponse, ClientError> {
        let url = self.url(service, path);
        if self.mode == Mode::DryRun {
            let serialized =
                serde_json::to_string(body).unwrap_or_else(|_| "<unserializable>".into());
            return Ok(self.record_and_synthesize("PUT", &url, Some(serialized)));
        }
        let token = self.token.token().await?;
        let resp = self
            .http
            .put(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let resp_body = resp.bytes().await?.to_vec();
        Ok(HttpResponse {
            status,
            body: resp_body,
        })
    }

    /// Issue a PATCH with a JSON body. In [`Mode::DryRun`], logs+records the
    /// serialized body and returns a 200 with `{}`.
    pub async fn patch_json(
        &self,
        service: GcpService,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<HttpResponse, ClientError> {
        let url = self.url(service, path);
        if self.mode == Mode::DryRun {
            let serialized =
                serde_json::to_string(body).unwrap_or_else(|_| "<unserializable>".into());
            return Ok(self.record_and_synthesize("PATCH", &url, Some(serialized)));
        }
        let token = self.token.token().await?;
        let resp = self
            .http
            .patch(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let resp_body = resp.bytes().await?.to_vec();
        Ok(HttpResponse {
            status,
            body: resp_body,
        })
    }

    fn record_and_synthesize(
        &self,
        method: &'static str,
        url: &str,
        body: Option<String>,
    ) -> HttpResponse {
        tracing::info!(
            target: "devx::gcp::dry_run",
            method = method,
            url = url,
            body = body.as_deref().unwrap_or(""),
            "[dry-run] would call GCP",
        );
        self.recorded
            .lock()
            .expect("recorded lock poisoned")
            .push(RecordedCall {
                method,
                url: url.to_string(),
                body,
            });
        HttpResponse {
            status: 200,
            body: b"{}".to_vec(),
        }
    }

    /// Run an external command. In `DryRun`, logs+records the
    /// rendered command line under the synthetic `"SHELL"` method
    /// and returns a successful [`ShellResult`] with empty
    /// stdout/stderr.
    ///
    /// The `program` is invoked with the given `args` directly (no
    /// shell interpolation); callers should not embed user input
    /// inside an `arg`'s string.
    pub async fn shell_out(
        &self,
        program: &str,
        args: &[&str],
    ) -> Result<ShellResult, ClientError> {
        self.shell_out_with_stdin(program, args, None).await
    }

    /// Variant of [`Self::shell_out`] that pipes `stdin` to the child
    /// process. Used for `kubectl apply -f -` and similar. In
    /// `DryRun`, the stdin payload is recorded as the call's `body`
    /// so the audit log shows what would have been piped in.
    pub async fn shell_out_with_stdin(
        &self,
        program: &str,
        args: &[&str],
        stdin: Option<&str>,
    ) -> Result<ShellResult, ClientError> {
        let command_line = render_command_line(program, args);
        if self.mode == Mode::DryRun {
            tracing::info!(
                target: "devx::gcp::dry_run",
                command = %command_line,
                stdin = stdin.unwrap_or(""),
                "[dry-run] would shell out",
            );
            self.recorded
                .lock()
                .expect("recorded lock poisoned")
                .push(RecordedCall {
                    method: "SHELL",
                    url: command_line.clone(),
                    body: stdin.map(str::to_string),
                });
            return Ok(ShellResult {
                exit: 0,
                command_line,
                stderr: String::new(),
            });
        }
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        if stdin.is_some() {
            cmd.stdin(std::process::Stdio::piped());
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| ClientError::Shell {
            command: command_line.clone(),
            message: e.to_string(),
        })?;
        if let Some(payload) = stdin {
            use tokio::io::AsyncWriteExt;
            if let Some(mut input) = child.stdin.take() {
                input
                    .write_all(payload.as_bytes())
                    .await
                    .map_err(|e| ClientError::Shell {
                        command: command_line.clone(),
                        message: format!("write stdin: {e}"),
                    })?;
            }
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| ClientError::Shell {
                command: command_line.clone(),
                message: e.to_string(),
            })?;
        Ok(ShellResult {
            exit: output.status.code().unwrap_or(-1),
            command_line,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Outcome of [`GcpClient::shell_out`].
#[derive(Debug, Clone)]
pub struct ShellResult {
    pub exit: i32,
    pub command_line: String,
    pub stderr: String,
}

impl ShellResult {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit == 0
    }

    /// True if the process exited non-zero AND its stderr looks like
    /// gcloud's `"already exists"` / `"ALREADY_EXISTS"` shape. Lets
    /// the GKE ensure_* functions treat re-runs as success without a
    /// pre-check describe call.
    #[must_use]
    pub fn is_already_exists(&self) -> bool {
        if self.exit == 0 {
            return false;
        }
        let lower = self.stderr.to_ascii_lowercase();
        lower.contains("already exists") || lower.contains("alreadyexists")
    }
}

/// Render `program + args` as a single shell-style line for logging
/// and recording. Quotes any arg containing whitespace; not a real
/// shell-escape — just readable enough for an audit log.
fn render_command_line(program: &str, args: &[&str]) -> String {
    let mut out = program.to_string();
    for a in args {
        out.push(' ');
        if a.chars().any(char::is_whitespace) {
            out.push('"');
            out.push_str(a);
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn base_url_falls_back_to_real_endpoint_when_not_overridden() {
        let client = GcpClient::new(Arc::new(StaticToken("t".into())));
        assert_eq!(
            client.base_url(GcpService::Storage),
            "https://storage.googleapis.com"
        );
        assert_eq!(
            client.base_url(GcpService::Container),
            "https://container.googleapis.com"
        );
    }

    #[tokio::test]
    async fn with_base_url_redirects_traffic_for_that_service() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/foo"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("test-token".into())))
            .with_base_url(GcpService::Storage, server.uri());
        let resp = client
            .get(GcpService::Storage, "/storage/v1/b/foo")
            .await
            .unwrap();
        assert_eq!(resp.status_u16(), 200);
    }

    #[tokio::test]
    async fn post_json_sends_bearer_token_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .and(header("authorization", "Bearer t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        let resp = client
            .post_json(
                GcpService::Storage,
                "/storage/v1/b",
                &serde_json::json!({"name": "x"}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status_u16(), 200);
    }

    #[tokio::test]
    async fn patch_json_sends_bearer_token_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/proj-assets"))
            .and(header("authorization", "Bearer t"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        let resp = client
            .patch_json(
                GcpService::Storage,
                "/storage/v1/b/proj-assets",
                &serde_json::json!({"cors": []}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status_u16(), 200);
    }

    #[tokio::test]
    async fn put_json_sends_bearer_token_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/storage/v1/b/p-assets/iam"))
            .and(header("authorization", "Bearer t"))
            .and(body_partial_json(
                serde_json::json!({"bindings": [{"role": "roles/storage.objectViewer"}]}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        let resp = client
            .put_json(
                GcpService::Storage,
                "/storage/v1/b/p-assets/iam",
                &serde_json::json!({"bindings": [{"role": "roles/storage.objectViewer"}]}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status_u16(), 200);
    }

    #[tokio::test]
    async fn dry_run_records_calls_without_hitting_the_network() {
        // Point the client at an unreachable address so any real
        // network call would fail loudly. Dry-run must not touch
        // it.
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, "http://127.0.0.1:1")
            .with_dry_run();
        let resp = client
            .post_json(
                GcpService::Storage,
                "/storage/v1/b?project=p",
                &serde_json::json!({"name": "p-assets"}),
            )
            .await
            .unwrap();
        assert_eq!(resp.status_u16(), 200);
        let calls = client.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert!(calls[0].url.ends_with("/storage/v1/b?project=p"));
        assert!(
            calls[0].body.as_deref().unwrap().contains("p-assets"),
            "body should contain bucket name, got {:?}",
            calls[0].body
        );
    }

    #[tokio::test]
    async fn execute_mode_does_not_populate_recorded_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        client
            .post_json(GcpService::Storage, "/storage/v1/b", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(client.recorded_calls().is_empty());
    }
}
