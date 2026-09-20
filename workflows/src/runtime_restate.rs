//! Restate-backed [`crate::StateMachineRuntime`].
//!
//! Talks to a running Restate broker over HTTP. In local dev the
//! broker runs in-cluster (`cargo run -p cli -- dev up` brings up the
//! Restate Operator + `RestateCluster`). In production the same
//! code talks to **Restate Cloud** — set `RESTATE_BROKER_URL` to
//! the Restate Cloud ingress and `RESTATE_AUTH_TOKEN` to the
//! tenant's bearer token (sourced from Secret Manager in GKE).
//!
//! When `RESTATE_AUTH_TOKEN` is unset the runtime sends requests
//! unauthenticated — the in-cluster Restate Operator does not
//! require auth, so KIND keeps working with zero config.
//!
//! Wire shape (matches the Restate ingress contract — see the
//! `workflows-service` worker for the receiving end):
//!
//! ```text
//! POST {broker}/notation/{id}/{kind}_start           {"spec_yaml": "..."}
//! POST {broker}/notation/{id}/{kind}_signal          {"condition": "...", "value": "..."}
//! POST {broker}/notation/{id}/{kind}_current_state   (no body, no Content-Type)
//! POST {broker}/notation/{id}/{kind}_events          (no body, no Content-Type)
//! ```
//!
//! `value` is omitted from the signal body when no payload is
//! threaded — workflow signals always omit it; questionnaire
//! signals carry the respondent's answer when the walker has one
//! and omit it on the trailing transition to END.
//!
//! The `*_current_state` and `*_events` reads declare
//! `input_description: "none"` on the worker side. Restate rejects
//! those calls with a 400 if a request body or `Content-Type` is
//! present — even an empty JSON object. Hence the dedicated
//! [`RestateRuntime::post_empty`] helper.
//!
//! `{kind}` is the lowercase
//! [`crate::spec::MachineKind`] token: `workflow` or
//! `questionnaire`. One Restate service hosts both timelines per
//! Notation, keyed by the same `notation_id`, so their signals
//! interleave on a single logical journal.
//!
//! The `notation` service handlers live in the
//! `workflows-service` worker binary that registers with Restate
//! (using `restate-sdk`). This adapter is the *caller* side — the
//! application reaches Restate through this trait; Restate
//! reaches the workflow handlers through its own ingress.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::runtime::{SignalContext, StateMachineRuntime, WorkflowEvent, WorkflowRuntimeError};
use crate::spec::{MachineKind, StateName, WorkflowSpec};

/// Default service name registered with Restate.
pub const DEFAULT_SERVICE: &str = "notation";

/// Default Restate ingress URL — what the in-cluster `restate`
/// Service exposes on port 8080 in the `navigator` namespace.
pub const DEFAULT_BROKER_URL: &str = "http://localhost:8080";

/// HTTP client targeting a Restate broker.
#[derive(Clone)]
pub struct RestateRuntime {
    client: reqwest::Client,
    broker_url: String,
    service: String,
    auth_token: Option<String>,
}

impl RestateRuntime {
    /// Build with explicit broker URL + service name, no auth. Most
    /// callers use [`RestateRuntime::from_env`].
    #[must_use]
    pub fn new(broker_url: impl Into<String>, service: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            broker_url: broker_url.into(),
            service: service.into(),
            auth_token: None,
        }
    }

    /// Builder-style auth-token setter. Pass `Some("…")` to inject
    /// an `Authorization: Bearer …` header on every outbound
    /// request — required by Restate Cloud. Pass `None` (or skip the
    /// call) for unauthenticated brokers like the in-cluster
    /// Restate Operator.
    #[must_use]
    pub fn with_auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token.filter(|t| !t.is_empty());
        self
    }

    /// Build from `RESTATE_BROKER_URL` (default
    /// `http://localhost:8080`), `RESTATE_SERVICE` (default
    /// `notation`), and `RESTATE_AUTH_TOKEN` (optional — present
    /// only when targeting Restate Cloud).
    #[must_use]
    pub fn from_env() -> Self {
        let url =
            std::env::var("RESTATE_BROKER_URL").unwrap_or_else(|_| DEFAULT_BROKER_URL.to_string());
        let service =
            std::env::var("RESTATE_SERVICE").unwrap_or_else(|_| DEFAULT_SERVICE.to_string());
        let token = std::env::var("RESTATE_AUTH_TOKEN").ok();
        Self::new(url, service).with_auth_token(token)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut req = match &self.auth_token {
            Some(token) => req.bearer_auth(token),
            None => req,
        };
        // Every notation operation shares the caller's trace, including reads
        // whose Restate contract forbids a body and Content-Type.
        for (name, value) in telemetry::current_trace_context_headers() {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(&value),
            ) {
                req = req.header(name, value);
            }
        }
        req
    }

    fn handler_url(&self, kind: MachineKind, notation_id: Uuid, handler: &str) -> String {
        format!(
            "{}/{}/{}/{}_{}",
            self.broker_url.trim_end_matches('/'),
            self.service,
            notation_id,
            kind.as_str(),
            handler
        )
    }

    async fn post_json<I: Serialize, O: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &I,
    ) -> Result<O, WorkflowRuntimeError> {
        let resp = self
            .authed(self.client.post(url))
            .json(body)
            .send()
            .await
            .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(WorkflowRuntimeError::Transport(format!(
                "{} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        resp.json::<O>()
            .await
            .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))
    }

    /// POST with no body and no Content-Type. Restate handlers that
    /// declare `input_description: "none"` (the shared
    /// `*_current_state` and `*_events` readers) reject the call when
    /// a body or Content-Type is present, even an empty JSON object.
    async fn post_empty<O: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
    ) -> Result<O, WorkflowRuntimeError> {
        let resp = self
            .authed(self.client.post(url))
            .send()
            .await
            .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(WorkflowRuntimeError::Transport(format!(
                "{} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        resp.json::<O>()
            .await
            .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))
    }

    async fn post_json_no_response<I: Serialize>(
        &self,
        url: &str,
        body: &I,
    ) -> Result<(), WorkflowRuntimeError> {
        let resp = self
            .authed(self.client.post(url))
            .json(body)
            .send()
            .await
            .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(WorkflowRuntimeError::Transport(format!(
                "{} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }

    async fn start_with_ephemeral(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        spec: &WorkflowSpec,
        ephemeral: bool,
    ) -> Result<(), WorkflowRuntimeError> {
        let url = self.handler_url(kind, notation_id, "start");
        let body = StartBody {
            spec_yaml: serde_yaml::to_string(spec)
                .map_err(|e| WorkflowRuntimeError::Transport(e.to_string()))?,
            ephemeral,
        };
        self.post_json_no_response(&url, &body).await
    }

    async fn signal_with_ephemeral(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        condition: &str,
        payload: Option<&str>,
        ephemeral: bool,
        context: Option<SignalContext>,
    ) -> Result<StateName, WorkflowRuntimeError> {
        let url = self.handler_url(kind, notation_id, "signal");
        let resp: SignalResponse = self
            .post_json(
                &url,
                &SignalBody {
                    condition,
                    value: payload,
                    acting_person_id: context.map(|c| c.acting_person_id),
                    ephemeral,
                },
            )
            .await?;
        Ok(StateName(resp.next_state))
    }
}

#[derive(Serialize)]
struct StartBody {
    spec_yaml: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ephemeral: bool,
}

#[derive(Serialize)]
struct SignalBody<'a> {
    condition: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acting_person_id: Option<Uuid>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ephemeral: bool,
}

#[derive(Deserialize)]
struct SignalResponse {
    next_state: String,
}

#[derive(Deserialize)]
struct CurrentStateResponse {
    state: Option<String>,
}

#[derive(Deserialize)]
struct EventsResponse {
    events: Vec<WorkflowEvent>,
}

#[async_trait]
impl StateMachineRuntime for RestateRuntime {
    async fn start(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        spec: &WorkflowSpec,
    ) -> Result<(), WorkflowRuntimeError> {
        self.start_with_ephemeral(kind, notation_id, spec, false)
            .await
    }

    async fn start_ephemeral(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        spec: &WorkflowSpec,
    ) -> Result<(), WorkflowRuntimeError> {
        self.start_with_ephemeral(kind, notation_id, spec, true)
            .await
    }

    async fn signal(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        condition: &str,
        payload: Option<&str>,
    ) -> Result<StateName, WorkflowRuntimeError> {
        self.signal_with_ephemeral(kind, notation_id, condition, payload, false, None)
            .await
    }

    async fn signal_with_context(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        condition: &str,
        payload: Option<&str>,
        context: SignalContext,
    ) -> Result<StateName, WorkflowRuntimeError> {
        self.signal_with_ephemeral(kind, notation_id, condition, payload, false, Some(context))
            .await
    }

    async fn signal_ephemeral(
        &self,
        kind: MachineKind,
        notation_id: Uuid,
        condition: &str,
        payload: Option<&str>,
    ) -> Result<StateName, WorkflowRuntimeError> {
        self.signal_with_ephemeral(kind, notation_id, condition, payload, true, None)
            .await
    }

    async fn current_state(&self, kind: MachineKind, notation_id: Uuid) -> Option<StateName> {
        let url = self.handler_url(kind, notation_id, "current_state");
        let resp: Result<CurrentStateResponse, _> = self.post_empty(&url).await;
        resp.ok().and_then(|r| r.state.map(StateName))
    }

    async fn events(&self, kind: MachineKind, notation_id: Uuid) -> Vec<WorkflowEvent> {
        let url = self.handler_url(kind, notation_id, "events");
        let resp: Result<EventsResponse, _> = self.post_empty(&url).await;
        resp.map(|r| r.events).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{RestateRuntime, DEFAULT_BROKER_URL, DEFAULT_SERVICE};
    use crate::runtime::{SignalContext, StateMachineRuntime, WorkflowRuntimeError};
    use crate::spec::{MachineKind, QuestionnaireSpec, StateName, WorkflowSpec};
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{body_bytes, body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Stable test notation id (lower 128 bits = 7).
    const N7: Uuid = Uuid::from_u128(7);
    const N7_PATH: &str = "00000000-0000-0000-0000-000000000007";
    const N42: Uuid = Uuid::from_u128(42);
    const PERSON: Uuid = Uuid::from_u128(99);
    const N42_PATH: &str = "00000000-0000-0000-0000-00000000002a";

    const SPEC: &str = "
BEGIN:
  created: lawyer_review__for_grantor
lawyer_review__for_grantor:
  approve: END
  reject: END
END: {}
";

    const QUESTIONNAIRE: &str = "
BEGIN:
  _: client_name
client_name:
  _: END
END: {}
";

    fn spec() -> WorkflowSpec {
        WorkflowSpec::from_yaml(SPEC).unwrap()
    }

    fn questionnaire() -> QuestionnaireSpec {
        QuestionnaireSpec::from_yaml(QUESTIONNAIRE).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runtime_propagates_current_trace_on_writes_and_bodyless_reads() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing::Instrument as _;
        use tracing_subscriber::prelude::*;

        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer().with_tracer(provider.tracer("runtime-propagation-test")),
        );
        let _guard = tracing::subscriber::set_default(subscriber);
        let broker = MockServer::start().await;
        let runtime = RestateRuntime::new(broker.uri(), "notation")
            .with_auth_token(Some("synthetic-token".into()));
        let span = tracing::info_span!("http.request");
        telemetry::set_span_parent(
            &span,
            Some("00-0102030405060708090a0b0c0d0e0f10-1112131415161718-01"),
            None,
        );
        async {
            let expected = telemetry::current_trace_context_headers()
                .into_iter()
                .find(|(name, _)| name == "traceparent")
                .expect("active trace")
                .1;
            for handler in [
                "workflow_start",
                "workflow_signal",
                "workflow_current_state",
            ] {
                Mock::given(method("POST"))
                    .and(path(format!("/notation/{N7_PATH}/{handler}")))
                    .and(header("traceparent", expected.as_str()))
                    .and(header("authorization", "Bearer synthetic-token"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "next_state": "END", "state": "END"
                    })))
                    .expect(1)
                    .mount(&broker)
                    .await;
            }
            StateMachineRuntime::start(&runtime, MachineKind::Workflow, N7, &spec())
                .await
                .expect("start response");
            StateMachineRuntime::signal(&runtime, MachineKind::Workflow, N7, "approve", None)
                .await
                .expect("signal response");
            StateMachineRuntime::current_state(&runtime, MachineKind::Workflow, N7)
                .await
                .expect("state response");
            let requests = broker.received_requests().await.expect("captured requests");
            assert_eq!(requests.len(), 3);
            assert!(requests[2].body.is_empty());
            assert!(!requests[2].headers.contains_key("content-type"));
            assert!(requests.iter().all(|r| !r.headers.contains_key("baggage")));
        }
        .instrument(span)
        .await;
    }

    #[test]
    fn default_constants_match_compose_setup() {
        assert_eq!(DEFAULT_BROKER_URL, "http://localhost:8080");
        assert_eq!(DEFAULT_SERVICE, "notation");
    }

    #[test]
    fn handler_url_includes_kind_prefix_so_questionnaire_and_workflow_dont_collide() {
        let rt = RestateRuntime::new("http://example.com:8080/", "notation");
        assert_eq!(
            rt.handler_url(MachineKind::Workflow, N42, "signal"),
            format!("http://example.com:8080/notation/{N42_PATH}/workflow_signal")
        );
        assert_eq!(
            rt.handler_url(MachineKind::Questionnaire, N42, "signal"),
            format!("http://example.com:8080/notation/{N42_PATH}/questionnaire_signal")
        );
    }

    #[tokio::test]
    async fn workflow_start_posts_spec_yaml_to_workflow_start_handler() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        StateMachineRuntime::start(&rt, MachineKind::Workflow, N7, &spec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn questionnaire_start_posts_spec_yaml_to_questionnaire_start_handler() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/questionnaire_start")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        StateMachineRuntime::start(&rt, MachineKind::Questionnaire, N7, questionnaire().inner())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn signal_returns_next_state_from_broker_response() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_signal")))
            .and(body_partial_json(json!({"condition": "created"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"next_state": "lawyer_review__for_grantor"})),
            )
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let next = StateMachineRuntime::signal(&rt, MachineKind::Workflow, N7, "created", None)
            .await
            .unwrap();
        assert_eq!(next.as_str(), "lawyer_review__for_grantor");
    }

    #[tokio::test]
    async fn questionnaire_signal_targets_the_questionnaire_handler() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/questionnaire_signal")))
            .and(body_partial_json(json!({"condition": "_"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"next_state": "client_name"})),
            )
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let next = StateMachineRuntime::signal(&rt, MachineKind::Questionnaire, N7, "_", None)
            .await
            .unwrap();
        assert_eq!(next.as_str(), "client_name");
    }

    #[tokio::test]
    async fn questionnaire_signal_threads_answer_value_into_post_body() {
        // The walker forwards the respondent's answer through the
        // signal payload; the worker's `questionnaire_signal` reads
        // it as `value` and stamps it into the journaled event's
        // `payload` column. Pin the wire shape so a future refactor
        // can't silently drop the value again.
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/questionnaire_signal")))
            .and(body_partial_json(
                json!({"condition": "_", "value": "Libra"}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"next_state": "client_name"})),
            )
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let next =
            StateMachineRuntime::signal(&rt, MachineKind::Questionnaire, N7, "_", Some("Libra"))
                .await
                .unwrap();
        assert_eq!(next.as_str(), "client_name");
    }

    #[tokio::test]
    async fn signal_with_context_threads_acting_person_into_post_body() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_signal")))
            .and(body_partial_json(json!({
                "condition": "approved",
                "acting_person_id": PERSON
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"next_state": "END"})))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let next = StateMachineRuntime::signal_with_context(
            &rt,
            MachineKind::Workflow,
            N7,
            "approved",
            None,
            SignalContext {
                acting_person_id: PERSON,
            },
        )
        .await
        .unwrap();
        assert_eq!(next.as_str(), "END");
    }

    #[tokio::test]
    async fn signal_surfaces_broker_error_as_transport_error() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_signal")))
            .respond_with(ResponseTemplate::new(500).set_body_string("broker fell over"))
            .mount(&broker)
            .await;
        let rt = RestateRuntime::new(broker.uri(), "notation");
        let err = StateMachineRuntime::signal(&rt, MachineKind::Workflow, N7, "anything", None)
            .await
            .unwrap_err();
        assert!(matches!(err, WorkflowRuntimeError::Transport(_)));
    }

    #[tokio::test]
    async fn current_state_decodes_response_or_returns_none() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_current_state")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"state": "lawyer_review__for_grantor"})),
            )
            .mount(&broker)
            .await;
        let rt = RestateRuntime::new(broker.uri(), "notation");
        let state = StateMachineRuntime::current_state(&rt, MachineKind::Workflow, N7)
            .await
            .unwrap();
        assert_eq!(state.as_str(), "lawyer_review__for_grantor");
    }

    #[tokio::test]
    async fn current_state_returns_none_on_broker_outage() {
        // No mock registered → broker 404s → we treat that as
        // "no workflow with that id".
        let broker = MockServer::start().await;
        let rt = RestateRuntime::new(broker.uri(), "notation");
        assert_eq!(
            StateMachineRuntime::current_state(&rt, MachineKind::Workflow, N7).await,
            None
        );
    }

    #[tokio::test]
    async fn events_decodes_event_log_or_returns_empty() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_events")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "events": [
                    {
                        "notation_id": N7_PATH,
                        "from": "BEGIN",
                        "to": "lawyer_review__for_grantor",
                        "condition": "created"
                    }
                ]
            })))
            .mount(&broker)
            .await;
        let rt = RestateRuntime::new(broker.uri(), "notation");
        let evs = StateMachineRuntime::events(&rt, MachineKind::Workflow, N7).await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].notation_id, N7);
        assert_eq!(evs[0].condition, "created");
        assert_eq!(evs[0].from, StateName::begin());
    }

    #[tokio::test]
    async fn auth_token_attaches_bearer_header_to_outbound_requests() {
        // Restate Cloud authenticates requests with a bearer token.
        // The in-cluster Operator does not. Same code path — the
        // optional token flips the header on.
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .and(header("authorization", "Bearer s3cret-token"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation")
            .with_auth_token(Some("s3cret-token".to_string()));
        StateMachineRuntime::start(&rt, MachineKind::Workflow, N7, &spec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn no_auth_token_means_no_authorization_header_on_the_wire() {
        // KIND keeps working without any auth config — the absence
        // of `RESTATE_AUTH_TOKEN` must not produce a header at all
        // (an empty Authorization header would itself be a bug).
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&broker)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        StateMachineRuntime::start(&rt, MachineKind::Workflow, N7, &spec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn start_ephemeral_sets_ephemeral_true_on_the_wire() {
        // The worker reads `ephemeral` and skips the
        // `notation_events` journal append. The flag must travel
        // on the start AND the signal body — both are pinned here.
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .and(body_partial_json(json!({"ephemeral": true})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        rt.start_ephemeral(MachineKind::Workflow, N7, &spec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn signal_ephemeral_sets_ephemeral_true_on_the_wire() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_signal")))
            .and(body_partial_json(
                json!({"condition": "signup_recorded", "ephemeral": true}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"next_state": "email_send__welcome"})),
            )
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let next = rt
            .signal_ephemeral(MachineKind::Workflow, N7, "signup_recorded", None)
            .await
            .unwrap();
        assert_eq!(next.as_str(), "email_send__welcome");
    }

    #[tokio::test]
    async fn empty_auth_token_is_treated_as_absent() {
        // A common deploy bug: the Secret Manager mount exists but
        // the file is empty. Treat that as no-auth rather than
        // sending `Authorization: Bearer ` (which Restate Cloud
        // would reject as malformed).
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&broker)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_start")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation").with_auth_token(Some(String::new()));
        StateMachineRuntime::start(&rt, MachineKind::Workflow, N7, &spec())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn current_state_sends_no_body_and_no_content_type() {
        // The worker's `*_current_state` handler declares
        // `input_description: "none"`. Restate's ingress rejects the
        // call with a 400 if a body or `Content-Type` is present —
        // an empty JSON object is still a body. Pin the wire shape
        // so a future refactor can't silently reintroduce
        // `.json(&json!({}))`.
        let broker = MockServer::start().await;

        // Negative: any request that carries a Content-Type must
        // NOT match this handler.
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_current_state")))
            .and(header_exists("content-type"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": null})))
            .expect(0)
            .mount(&broker)
            .await;

        // Positive: empty body, no Content-Type.
        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_current_state")))
            .and(body_bytes(Vec::<u8>::new()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "BEGIN"})))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let state = StateMachineRuntime::current_state(&rt, MachineKind::Workflow, N7)
            .await
            .expect("broker returned a state");
        assert_eq!(state.as_str(), "BEGIN");
    }

    #[tokio::test]
    async fn events_sends_no_body_and_no_content_type() {
        // Same wire constraint as current_state — the
        // `*_events` reader is also `input_description: "none"`.
        let broker = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_events")))
            .and(header_exists("content-type"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"events": []})))
            .expect(0)
            .mount(&broker)
            .await;

        Mock::given(method("POST"))
            .and(path(format!("/notation/{N7_PATH}/workflow_events")))
            .and(body_bytes(Vec::<u8>::new()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"events": []})))
            .expect(1)
            .mount(&broker)
            .await;

        let rt = RestateRuntime::new(broker.uri(), "notation");
        let evs = StateMachineRuntime::events(&rt, MachineKind::Workflow, N7).await;
        assert!(evs.is_empty());
    }

    #[tokio::test]
    async fn state_machine_runtime_methods_use_the_kind_arg_directly() {
        let broker = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/notation/{N7_PATH}/questionnaire_current_state"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"state": "client_email"})),
            )
            .mount(&broker)
            .await;
        let rt = RestateRuntime::new(broker.uri(), "notation");
        let s = StateMachineRuntime::current_state(&rt, MachineKind::Questionnaire, N7)
            .await
            .unwrap();
        assert_eq!(s.as_str(), "client_email");
    }
}
