#![allow(clippy::doc_markdown)]
//! The one observability seam for every Neon Law Navigator binary.
//!
//! [`init`] wires `tracing` once and returns a [`TelemetryGuard`] whose drop
//! flushes any pending OpenTelemetry export. Every `main` calls it with its
//! service name; nothing else hand-rolls a subscriber.
//!
//! Two modes, chosen by whether `OTEL_EXPORTER_OTLP_ENDPOINT` is set:
//!
//! - **Unset (dev / CI / OSS fork)** — a human-readable `fmt` layer to stdout
//!   and nothing else. Zero OTel cost, no network.
//! - **Set with the complete OpenObserve contract** — stdout switches to
//!   **structured JSON** and the process exports **traces, metrics, and logs**
//!   directly over OTLP/gRPC to OpenObserve. `OTEL_EXPORTER_OTLP_ENDPOINT`
//!   selects the endpoint; `NAVIGATOR_OPENOBSERVE_USERNAME`,
//!   `NAVIGATOR_OPENOBSERVE_PASSWORD`, `NAVIGATOR_OPENOBSERVE_ORGANIZATION`,
//!   and `NAVIGATOR_OPENOBSERVE_STREAM` authenticate and route every signal.
//!   A partial contract falls back safely to stdout instead of attempting an
//!   unauthenticated export. The stdout JSON layer stays on in this mode too,
//!   so an export outage never means a lost local log line.
//!
//! **The one rule for anyone adding a span, metric, or log field (legal- and
//! engineering-council standing order): identifiers and counts, never
//! content.** A `notation_id`, a `service` name, an `outcome`, a duration, a
//! status code — yes. A client name, an answer body, an email address, a
//! document body — never. The [`SanitizingSubscriber`] is the source-side
//! backstop: it rejects unsafe log records before stdout or direct OpenObserve
//! OTLP export. Telemetry leaves the firm's trust boundary; client content does
//! not.

use base64::Engine as _;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing::subscriber::Interest;
use tracing::{span, Event, Id, Metadata, Subscriber};
use tracing_core::span::Current;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

/// The instrumentation scope name for durable-execution metrics.
const TRIGGER_METER: &str = "navigator.workflow.trigger";

/// Counter: how many times a workflow trigger POSTed to the Restate ingress,
/// dimensioned by `service` and `outcome`. A flat line for a `service` that
/// should fire on a schedule is the signal that a trigger has silently stopped
/// — the exact failure that hid for days before this existed.
pub const TRIGGER_FIRED: &str = "navigator.workflow.trigger.fired";

/// Outcome label values for [`TRIGGER_FIRED`]. Status only — never content.
pub mod outcome {
    /// The ingress accepted the invocation (2xx).
    pub const ACCEPTED: &str = "accepted";
    /// The ingress answered but rejected it (e.g. 401 stale token, 404 service
    /// not registered).
    pub const REJECTED: &str = "rejected";
    /// The POST never got an answer (DNS, connect, or the 30s timeout).
    pub const TRANSPORT_ERROR: &str = "transport_error";
}

/// Flush-on-drop guard for the OTLP providers. Hold it for the lifetime of
/// `main`; dropping it (or calling [`TelemetryGuard::shutdown`]) exports any
/// batched spans/metrics/logs before the process exits.
#[must_use = "dropping the guard immediately flushes and tears down telemetry"]
pub struct TelemetryGuard {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logger: Option<SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Explicitly flush and tear down. Equivalent to dropping the guard; offered
    /// so a `main` can shut telemetry down ahead of other cleanup and read as
    /// intentional.
    pub fn shutdown(self) {}
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(p) = self.tracer.take() {
            let _ = p.shutdown();
        }
        if let Some(m) = self.meter.take() {
            let _ = m.shutdown();
        }
        if let Some(l) = self.logger.take() {
            let _ = l.shutdown();
        }
    }
}

/// The three OTLP providers built for the export (prod) path, sharing one
/// [`Resource`]. Kept as a struct so [`init`] and the unit tests construct them
/// the same way — the tests exercise this without touching the process-global
/// subscriber, which can only be installed once.
struct ExportProviders {
    tracer: SdkTracerProvider,
    meter: SdkMeterProvider,
    logger: SdkLoggerProvider,
}

/// The source-side content boundary for every log sink.
///
/// Direct OpenObserve receives the same events that the process writes to
/// stdout, so a backend-side allow-list cannot protect either copy. This
/// subscriber rejects an event before delegating it to the configured layers
/// when one of its values is clearly content-bearing. Rejection is deliberate:
/// retaining a partial event would make an opaque id look like evidence about a
/// value that was not retained, and `tracing` does not provide a way to mutate
/// an event in place for all downstream layers.
struct SanitizingSubscriber<S> {
    inner: S,
}

impl<S> SanitizingSubscriber<S> {
    fn new(inner: S) -> Self {
        Self { inner }
    }
}

#[derive(Default)]
struct SafetyVisitor {
    unsafe_value: bool,
}

impl SafetyVisitor {
    fn inspect(&mut self, field: &tracing::field::Field, value: &str) {
        let name = field.name();
        let body_like = matches!(
            name,
            "answer"
                | "body"
                | "content"
                | "document"
                | "file_name"
                | "filename"
                | "payload"
                | "path"
                | "query"
                | "raw"
                | "response"
                | "sql"
                | "text"
                | "url"
                | "value"
        );
        let identity_like = matches!(
            name,
            "email"
                | "email_address"
                | "ein"
                | "government_id"
                | "phone"
                | "phone_number"
                | "ssn"
                | "tax_id"
        );

        self.unsafe_value = self.unsafe_value
            || identity_like
            || (body_like && !value.is_empty())
            || contains_sensitive_text(value)
            || (name == "message" && looks_like_document_body(value));
    }
}

impl tracing::field::Visit for SafetyVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.inspect(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.inspect(field, value);
    }
}

/// Detect content classes that must not reach a direct telemetry sink.
///
/// This is intentionally conservative for string values. Typed counters and
/// bounded enum fields are not converted to text by the visitor, so approved
/// numeric/id fields remain available to the operator while untrusted strings
/// are rejected when they resemble client content.
fn contains_sensitive_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let has_email_shape = lower.split_whitespace().any(|word| {
        let candidate = word.trim_matches(|c: char| "()[]{}<>,;:\"'".contains(c));
        let Some((local, domain)) = candidate.split_once('@') else {
            return false;
        };
        !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
    });
    if has_email_shape {
        return true;
    }

    let digits = value.chars().filter(char::is_ascii_digit).count();
    let phone_shape = (10..=15).contains(&digits)
        && value.chars().any(|c| c.is_ascii_digit())
        && (value.contains('+')
            || value.contains('-')
            || value.contains('(')
            || value.contains(')')
            || value.split_whitespace().count() > 1);
    let government_id_shape = value
        .split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .filter(|candidate| !candidate.is_empty())
        .any(|candidate| {
            let groups: Vec<_> = candidate.split('-').collect();
            matches!(groups.as_slice(), [a, b, c] if a.len() == 3
                && b.len() == 2
                && c.len() == 4
                && groups.iter().all(|group| group.chars().all(|c| c.is_ascii_digit())))
                || matches!(groups.as_slice(), [a, b] if a.len() == 2
                    && b.len() == 7
                    && groups.iter().all(|group| group.chars().all(|c| c.is_ascii_digit())))
                || ((9..=10).contains(&digits) && candidate.chars().all(|c| c.is_ascii_digit()))
        });

    phone_shape || government_id_shape
}

fn looks_like_document_body(value: &str) -> bool {
    value.len() > 160
        || value.contains('\n')
        || [
            "confidential",
            "agreement",
            "attorney-client",
            "client",
            "contract",
            "document",
            "defendant",
            "exhibit",
            "plaintiff",
            "the party shall",
        ]
        .iter()
        .any(|marker| value.to_ascii_lowercase().contains(marker))
}

impl<S: Subscriber> Subscriber for SanitizingSubscriber<S> {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn new_span(&self, span: &span::Attributes<'_>) -> Id {
        self.inner.new_span(span)
    }

    fn record(&self, span: &Id, values: &span::Record<'_>) {
        self.inner.record(span, values);
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.inner.record_follows_from(span, follows);
    }

    fn event(&self, event: &Event<'_>) {
        let mut visitor = SafetyVisitor::default();
        event.record(&mut visitor);
        if !visitor.unsafe_value {
            self.inner.event(event);
        }
    }

    fn enter(&self, span: &Id) {
        self.inner.enter(span);
    }

    fn exit(&self, span: &Id) {
        self.inner.exit(span);
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.inner.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.inner.try_close(id)
    }

    fn current_span(&self) -> Current {
        self.inner.current_span()
    }

    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.inner.register_callsite(metadata)
    }

    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        self.inner.max_level_hint()
    }
}

/// The complete direct-OTLP contract OpenObserve requires. Keeping the
/// credential fields here, instead of at each exporter call site, guarantees
/// traces, metrics, and logs authenticate and land in the same organization
/// and stream.
#[derive(Debug, Eq, PartialEq)]
struct OpenObserveExportConfig {
    endpoint: String,
    authorization: String,
    organization: String,
    stream: String,
}

impl OpenObserveExportConfig {
    fn metadata(&self) -> opentelemetry_otlp::tonic_types::metadata::MetadataMap {
        let mut metadata = opentelemetry_otlp::tonic_types::metadata::MetadataMap::new();
        metadata.insert(
            "authorization",
            self.authorization
                .parse()
                .expect("Basic authorization is valid gRPC metadata"),
        );
        metadata.insert(
            "organization",
            self.organization
                .parse()
                .expect("OpenObserve organization is valid gRPC metadata"),
        );
        metadata.insert(
            "stream-name",
            self.stream
                .parse()
                .expect("OpenObserve stream is valid gRPC metadata"),
        );
        metadata
    }
}

/// Normalize the raw `OTEL_EXPORTER_OTLP_ENDPOINT` value: an unset, empty, or
/// whitespace-only endpoint means "do not export" and yields `None`. Factored
/// out so the dev/prod branch decision is unit-testable without mutating
/// process env.
fn normalize_endpoint(raw: Option<String>) -> Option<String> {
    raw.filter(|v| !v.trim().is_empty())
}

fn required_config_value(name: &str, raw: Option<String>, missing: &mut Vec<String>) -> String {
    raw.filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            missing.push(name.to_string());
            String::new()
        })
}

/// Build the direct OpenObserve export contract. An unset endpoint preserves
/// stdout-only development. Once an endpoint is set, every credential and
/// routing value is mandatory: partial configuration must never cause an
/// unauthenticated export attempt.
fn openobserve_export_config(
    endpoint: Option<String>,
    username: Option<String>,
    password: Option<String>,
    organization: Option<String>,
    stream: Option<String>,
) -> Result<Option<OpenObserveExportConfig>, String> {
    let Some(endpoint) = normalize_endpoint(endpoint) else {
        return Ok(None);
    };

    let mut missing = Vec::new();
    let username = required_config_value("NAVIGATOR_OPENOBSERVE_USERNAME", username, &mut missing);
    let password = required_config_value("NAVIGATOR_OPENOBSERVE_PASSWORD", password, &mut missing);
    let organization = required_config_value(
        "NAVIGATOR_OPENOBSERVE_ORGANIZATION",
        organization,
        &mut missing,
    );
    let stream = required_config_value("NAVIGATOR_OPENOBSERVE_STREAM", stream, &mut missing);
    if !missing.is_empty() {
        return Err(format!(
            "OTEL_EXPORTER_OTLP_ENDPOINT is set but OpenObserve export is disabled because {} {} missing",
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" }
        ));
    }

    let authorization = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    );
    Ok(Some(OpenObserveExportConfig {
        endpoint,
        authorization,
        organization,
        stream,
    }))
}

/// Build the trace / metric / log OTLP providers for `endpoint`, all sharing a
/// single [`Resource`] (DRY: one resource, three providers — never three
/// resources that can drift). Building an exporter does **not** open a
/// connection — tonic connects lazily on first export — so this is safe to call
/// offline (and the unit tests do exactly that).
fn build_export_providers(
    config: &OpenObserveExportConfig,
    service_name: &str,
    release: Option<&str>,
) -> ExportProviders {
    // Tag every signal with the deployed release (`YY.M.D`) under the OTel
    // `service.version` convention, so a span/metric/log in OpenObserve says
    // which release emitted it. This is the headless
    // counterpart to `web`'s `GET /version`: the worker and the trigger
    // CronJobs have no HTTP surface, but they self-report their release here.
    let mut builder = Resource::builder().with_service_name(service_name.to_string());
    if let Some(release) = release {
        builder = builder.with_attribute(KeyValue::new("service.version", release.to_string()));
    }
    let resource = builder.build();

    // Traces — one batch span exporter.
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .with_metadata(config.metadata())
        .build()
        .expect("build OTLP span exporter");
    let tracer = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();

    // Metrics — periodic OTLP push.
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .with_metadata(config.metadata())
        .build()
        .expect("build OTLP metric exporter");
    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build();
    let meter = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource.clone())
        .build();

    // Logs — batch OTLP push, bridged from `tracing` (see [`init`]). The same
    // resource binds all three signals to one `service.name`.
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .with_metadata(config.metadata())
        .build()
        .expect("build OTLP log exporter");
    let logger = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    ExportProviders {
        tracer,
        meter,
        logger,
    }
}

/// Initialize the global `tracing` subscriber and, when configured, OTLP
/// export. Call exactly once per process, early in `main`.
pub fn init(default_service_name: &str) -> TelemetryGuard {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let export_config = openobserve_export_config(
        std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
        std::env::var("NAVIGATOR_OPENOBSERVE_USERNAME").ok(),
        std::env::var("NAVIGATOR_OPENOBSERVE_PASSWORD").ok(),
        std::env::var("NAVIGATOR_OPENOBSERVE_ORGANIZATION").ok(),
        std::env::var("NAVIGATOR_OPENOBSERVE_STREAM").ok(),
    );

    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_service_name.to_string());

    // The deployed release (`YY.M.D`), baked into every image as
    // `NAVIGATOR_RELEASE_TAG`. `None` on a local build (unset, or the honest
    // `unknown`), so dev telemetry carries no bogus version.
    let release = std::env::var("NAVIGATOR_RELEASE_TAG")
        .ok()
        .filter(|s| !s.trim().is_empty() && s != "unknown");

    // JSON to stdout when exporting so a deployment's log viewer parses each
    // field; human-readable otherwise. Boxed so both arms share one type.
    let fmt_layer = if export_config.as_ref().is_ok_and(Option::is_some) {
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer().boxed()
    };

    let config = match export_config {
        Ok(Some(config)) => config,
        Ok(None) => {
            tracing::subscriber::set_global_default(SanitizingSubscriber::new(
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(fmt_layer),
            ))
            .expect("install telemetry subscriber");
            tracing::info!(
                service = %service_name,
                release = %release.as_deref().unwrap_or("unknown"),
                "telemetry initialized (stdout only)"
            );
            return TelemetryGuard {
                tracer: None,
                meter: None,
                logger: None,
            };
        }
        Err(error) => {
            tracing::subscriber::set_global_default(SanitizingSubscriber::new(
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(fmt_layer),
            ))
            .expect("install telemetry subscriber");
            tracing::warn!(%error, "telemetry initialized (stdout only)");
            return TelemetryGuard {
                tracer: None,
                meter: None,
                logger: None,
            };
        }
    };

    let ExportProviders {
        tracer,
        meter,
        logger,
    } = build_export_providers(&config, &service_name, release.as_deref());

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let otel_trace_layer =
        tracing_opentelemetry::layer().with_tracer(tracer.tracer(service_name.clone()));

    // Register the meter provider globally so `record_trigger_fired` (and any
    // future instrument) reaches it.
    opentelemetry::global::set_meter_provider(meter.clone());

    // Bridge `tracing` log records to the OTLP logger. This is the third layer
    // alongside the stdout fmt layer — logs **dual-emit** (stdout JSON *and*
    // OTLP), with the sanitizing subscriber in front of both direct sinks.
    let otel_log_layer = OpenTelemetryTracingBridge::new(&logger);

    tracing::subscriber::set_global_default(SanitizingSubscriber::new(
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(otel_trace_layer)
            .with(otel_log_layer),
    ))
    .expect("install telemetry subscriber");

    tracing::info!(
        service = %service_name,
        release = %release.as_deref().unwrap_or("unknown"),
        "telemetry initialized (stdout + OTLP)"
    );

    TelemetryGuard {
        tracer: Some(tracer),
        meter: Some(meter),
        logger: Some(logger),
    }
}

/// Record one workflow-trigger fire. Safe to call unconditionally: when OTLP is
/// not configured the global meter is a no-op, so this costs nothing in dev.
/// `service` is the Restate service name (e.g. `Archives`); `outcome` is one of
/// the [`outcome`] constants. Identifiers and counts only — never content.
pub fn record_trigger_fired(service: &str, outcome: &str) {
    let counter = opentelemetry::global::meter(TRIGGER_METER)
        .u64_counter(TRIGGER_FIRED)
        .build();
    counter.add(
        1,
        &[
            KeyValue::new("service", service.to_string()),
            KeyValue::new("outcome", outcome.to_string()),
        ],
    );
}

/// The instrumentation scope name for the `/mcp` tool-call metric.
const MCP_METER: &str = "navigator.mcp";

/// Counter: how many times a tool was invoked over the `/mcp` JSON-RPC surface,
/// dimensioned by `tool` and `outcome`. The A2A surface already audits its tool
/// calls; this is the matching signal for the *direct* `/mcp` callers (Claude.ai
/// Connectors, Claude Code, LibreChat) so neither protocol surface that
/// shares the one tool catalog is blind in prod.
pub const MCP_TOOL_CALLED: &str = "navigator.mcp.tool.called";

/// Outcome label values for [`MCP_TOOL_CALLED`]. Status only — never the
/// arguments a client passed nor the tool's result body.
pub mod mcp_outcome {
    /// The tool ran and returned a result.
    pub const OK: &str = "ok";
    /// The tool returned a `ToolError` (rendered to the caller as an `isError`
    /// result per MCP convention).
    pub const ERROR: &str = "error";
}

/// Record one `/mcp` tool invocation. Safe to call unconditionally: when OTLP is
/// not configured the global meter is a no-op, so this costs nothing in dev.
/// `tool` is the namespaced tool name (e.g. `aida_create_person`); `outcome` is
/// one of the [`mcp_outcome`] constants. Identifiers and counts only — the tool
/// name and the outcome enum, never the arguments or the result.
pub fn record_mcp_tool_called(tool: &str, outcome: &str) {
    let counter = opentelemetry::global::meter(MCP_METER)
        .u64_counter(MCP_TOOL_CALLED)
        .build();
    counter.add(
        1,
        &[
            KeyValue::new("tool", tool.to_string()),
            KeyValue::new("outcome", outcome.to_string()),
        ],
    );
}

/// The instrumentation scope name for public website visit metrics.
const WEB_VISIT_METER: &str = "navigator.web.visit";

/// Counter: how many public website page views reached `web`, dimensioned only
/// by bounded aggregate labels. No IP address, user-agent, raw query string,
/// full URL, referrer URL, session id, or person id is ever attached.
pub const WEB_VISIT_COUNT: &str = "navigator.web.visit.count";

/// Record one public website visit. Safe to call unconditionally: when OTLP is
/// not configured the global meter is a no-op, so this costs nothing in dev.
/// `route` is the matched route pattern, `country` is a trusted edge-supplied
/// region/country code or `ZZ`, `source` is a bounded UTM/ref/referrer source
/// bucket, `locale` is a bounded route-derived language bucket, and
/// `status_class` is a coarse HTTP status family.
pub fn record_web_visit(
    route: &str,
    country: &str,
    source: &str,
    locale: &str,
    status_class: &str,
) {
    let counter = opentelemetry::global::meter(WEB_VISIT_METER)
        .u64_counter(WEB_VISIT_COUNT)
        .build();
    counter.add(
        1,
        &[
            KeyValue::new("route", route.to_string()),
            KeyValue::new("country", country.to_string()),
            KeyValue::new("source", source.to_string()),
            KeyValue::new("locale", locale.to_string()),
            KeyValue::new("status_class", status_class.to_string()),
        ],
    );
}

// ---------------------------------------------------------------------------
// Cross-service trace propagation (W3C `traceparent`).
//
// The one place the inject/extract pair lives, so every boundary crossing
// speaks the same wire format: `workflows::trigger` injects on the outbound
// POST to the Restate ingress; the `Archives` / `Notation` handlers extract
// from `ctx.headers()` and parent their spans on the result, so a trace begun
// in `web` continues through the durable workflow. The helpers take a plain
// `opentelemetry::Context` and `&str` header values — never reqwest's
// `HeaderMap<HeaderValue>` nor the Restate SDK's `HeaderMap<String>` — so both
// sides reuse them without type coupling.
//
// LEGAL (#2): only trace context crosses here — `traceparent` is
// `version-traceid-spanid-flags`, all opaque. Never put a client field in
// baggage or a propagated header.
// ---------------------------------------------------------------------------

/// Collects the propagator's injected headers into name/value pairs for a
/// caller to attach to its outbound request.
struct HeaderCollector(Vec<(String, String)>);

impl Injector for HeaderCollector {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}

/// Extracts trace context from a fixed `traceparent` / `tracestate` pair — the
/// only two headers `TraceContextPropagator` reads.
struct PairExtractor<'a> {
    traceparent: Option<&'a str>,
    tracestate: Option<&'a str>,
}

impl Extractor for PairExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            "traceparent" => self.traceparent,
            "tracestate" => self.tracestate,
            _ => None,
        }
    }

    fn keys(&self) -> Vec<&str> {
        ["traceparent", "tracestate"]
            .into_iter()
            .filter(|k| self.get(k).is_some())
            .collect()
    }
}

/// Inject the W3C trace context of `cx` into HTTP header name/value pairs (the
/// outbound side of cross-service tracing). Returns the propagation headers —
/// typically `traceparent`, plus `tracestate` when present — for the caller to
/// attach to its request. Empty when no sampled span is active or OTLP is
/// unconfigured (the global propagator is then a no-op), so tracing degrades
/// gracefully: the caller simply attaches nothing.
#[must_use]
pub fn trace_context_headers(cx: &opentelemetry::Context) -> Vec<(String, String)> {
    let mut collector = HeaderCollector(Vec::new());
    opentelemetry::global::get_text_map_propagator(|p| p.inject_context(cx, &mut collector));
    collector.0
}

/// Inject the *current* tracing span's trace context — the common call site
/// (the caller is inside an instrumented span). Convenience wrapper over
/// [`trace_context_headers`].
#[must_use]
pub fn current_trace_context_headers() -> Vec<(String, String)> {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    trace_context_headers(&tracing::Span::current().context())
}

/// Rebuild the parent [`opentelemetry::Context`] from the W3C trace headers a
/// handler received (the receiving side). Pass the incoming `traceparent` and
/// `tracestate` header values. Attach the result to a span with
/// `tracing_opentelemetry::OpenTelemetrySpanExt::set_parent` so the handler's
/// spans join the caller's trace. Returns an empty context (a fresh root) when
/// no `traceparent` is present.
#[must_use]
pub fn parent_context_from(
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) -> opentelemetry::Context {
    let extractor = PairExtractor {
        traceparent,
        tracestate,
    };
    opentelemetry::global::get_text_map_propagator(|p| p.extract(&extractor))
}

/// Parent `span` on the trace context carried by a handler's incoming
/// `traceparent` / `tracestate` headers, so the span and its children join the
/// caller's trace across the Restate boundary. The receiving-side convenience
/// over [`parent_context_from`] — it keeps the `tracing-opentelemetry`
/// dependency in this one crate instead of every workflow handler. A no-op
/// (fresh root) when no `traceparent` is present.
pub fn set_span_parent(span: &tracing::Span, traceparent: Option<&str>, tracestate: Option<&str>) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    // `set_parent` returns a `Result` as of tracing-opentelemetry 0.33; parenting
    // is best-effort telemetry, so a failure to attach is intentionally ignored.
    let _ = span.set_parent(parent_context_from(traceparent, tracestate));
}

#[cfg(test)]
mod tests {
    use super::{
        build_export_providers, current_trace_context_headers, normalize_endpoint,
        openobserve_export_config, parent_context_from, trace_context_headers,
        SanitizingSubscriber,
    };

    #[test]
    fn openobserve_config_is_stdout_only_when_the_endpoint_is_unset() {
        assert_eq!(
            openobserve_export_config(None, None, None, None, None),
            Ok(None)
        );
    }

    #[test]
    fn openobserve_config_keeps_the_complete_direct_otlp_contract() {
        let config = openobserve_export_config(
            Some("http://openobserve:5081".to_string()),
            Some("root@example.com".to_string()),
            Some("secret".to_string()),
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect("complete OpenObserve configuration is valid")
        .expect("an endpoint enables export");

        assert_eq!(config.endpoint, "http://openobserve:5081");
        assert_eq!(config.organization, "navigator");
        assert_eq!(config.stream, "default");
        assert_eq!(
            config.authorization,
            "Basic cm9vdEBleGFtcGxlLmNvbTpzZWNyZXQ="
        );
    }

    #[test]
    fn openobserve_config_refuses_an_incomplete_credential_contract() {
        let error = openobserve_export_config(
            Some("http://openobserve:5081".to_string()),
            Some("root@example.com".to_string()),
            None,
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect_err("an endpoint without complete credentials must not export");

        assert!(error.contains("NAVIGATOR_OPENOBSERVE_PASSWORD"));
    }

    #[test]
    fn normalize_endpoint_treats_unset_empty_and_blank_as_no_export() {
        assert_eq!(normalize_endpoint(None), None);
        assert_eq!(normalize_endpoint(Some(String::new())), None);
        assert_eq!(normalize_endpoint(Some("   ".to_string())), None);
    }

    #[test]
    fn normalize_endpoint_keeps_a_real_endpoint() {
        assert_eq!(
            normalize_endpoint(Some("http://openobserve:5081".to_string())),
            Some("http://openobserve:5081".to_string())
        );
    }

    /// Building the three providers must not open a connection (tonic connects
    /// lazily), so this constructs them against an unreachable endpoint and
    /// shuts them down — proving the export path wires logs alongside traces +
    /// metrics with no network.
    ///
    /// **Must run on a multi-thread runtime.** The batch span/log processors
    /// and the periodic metric reader each own a background flush task on the
    /// Tokio runtime; `shutdown()` blocks until that task acknowledges. On the
    /// default current-thread `#[tokio::test]` runtime the blocking shutdown
    /// starves the very task it waits on — a deadlock. Two worker threads let
    /// the flush task make progress while shutdown blocks.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn export_providers_build_all_three_signals_offline() {
        let config = openobserve_export_config(
            Some("http://127.0.0.1:5081".to_string()),
            Some("root@example.com".to_string()),
            Some("secret".to_string()),
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect("complete test config is valid")
        .expect("test endpoint enables export");
        let providers = build_export_providers(&config, "telemetry-test", Some("26.6.23"));
        // All three signals are present; shutting down flushes (no-op here,
        // nothing batched) without panicking or requiring a live OpenObserve.
        let _ = providers.tracer.shutdown();
        let _ = providers.meter.shutdown();
        let _ = providers.logger.shutdown();
    }

    /// The cross-service propagation contract, fully offline: a known span
    /// context injects to a well-formed W3C `traceparent`, and extracting that
    /// header back yields a parent context with the SAME trace id. This is the
    /// invariant `workflows::trigger` (inject) and the `Archives` / `Notation`
    /// handlers (extract) depend on across the Restate boundary.
    #[test]
    fn trace_context_round_trips_through_w3c_headers() {
        use opentelemetry::trace::{
            SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
        };

        // Without an explicit propagator the global default is a no-op; set the
        // W3C propagator so inject/extract actually run.
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let trace_id = TraceId::from_bytes([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]);
        let span_id = SpanId::from_bytes([0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18]);
        let sc = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        let cx = opentelemetry::Context::new().with_remote_span_context(sc);

        let headers = trace_context_headers(&cx);
        let traceparent = headers
            .iter()
            .find(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.as_str());
        assert!(traceparent.is_some(), "traceparent must be injected");
        let tp = traceparent.unwrap();
        // W3C shape: version-traceid-spanid-flags, and it carries our ids.
        assert!(tp.starts_with("00-"), "W3C version prefix: {tp}");
        assert!(
            tp.contains("0102030405060708090a0b0c0d0e0f10"),
            "carries the trace id: {tp}"
        );

        let parent = parent_context_from(traceparent, None);
        assert_eq!(
            parent.span().span_context().trace_id(),
            trace_id,
            "extracted parent must share the injected trace id"
        );
        assert!(
            parent.span().span_context().is_remote(),
            "extracted context is a remote parent"
        );
    }

    /// With no active span (and the no-op default propagator path), the current
    /// helper returns no headers — the graceful-degradation property that keeps
    /// dev/CI/OSS forks zero-cost and never attaches a malformed header.
    #[test]
    fn current_headers_empty_without_an_active_span() {
        assert!(current_trace_context_headers().is_empty());
    }

    fn emit_synthetic_records() {
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            email = "client@example.com",
            "unsafe email must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            phone = "+1 (212) 555-0199",
            "unsafe phone must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            government_id = "123-45-6789",
            "unsafe government id must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            "unsafe message client@example.com must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            "unsafe message +1 (212) 555-0199 must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            "unsafe message government id 123-45-6789 must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            body = "CONFIDENTIAL CLIENT AGREEMENT: the party shall indemnify the client.",
            "unsafe document body must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            "CONFIDENTIAL CLIENT AGREEMENT: the party shall indemnify the client."
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            "approved telemetry survives unchanged"
        );
    }

    fn assert_safe_output(rendered: &str) {
        assert!(!rendered.contains("client@example.com"));
        assert!(!rendered.contains("212"));
        assert!(!rendered.contains("123-45-6789"));
        assert!(!rendered.contains("CONFIDENTIAL CLIENT AGREEMENT"));
        assert!(rendered.contains("opaque-person-id"));
        assert!(rendered.contains("accepted"));
        assert!(rendered.contains("approved telemetry survives unchanged"));
        assert_eq!(
            rendered
                .matches("approved telemetry survives unchanged")
                .count(),
            1
        );
    }

    #[test]
    fn direct_export_subscriber_rejects_unsafe_values_but_keeps_safe_fields() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<String>>);

        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(self.0.clone())
            }
        }

        struct BufferWriter(Arc<Mutex<String>>);

        impl std::io::Write for BufferWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let text = String::from_utf8_lossy(bytes);
                self.0
                    .lock()
                    .expect("test buffer is not poisoned")
                    .push_str(&text);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let stdout = Arc::new(Mutex::new(String::new()));
        let openobserve = Arc::new(Mutex::new(String::new()));
        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(Buffer(stdout.clone()))
            .with_target(false);
        let openobserve_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(Buffer(openobserve.clone()))
            .with_target(false);
        let subscriber = SanitizingSubscriber::new(
            tracing_subscriber::registry()
                .with(stdout_layer)
                .with(openobserve_layer),
        );

        tracing::subscriber::with_default(subscriber, emit_synthetic_records);

        for rendered in [
            stdout
                .lock()
                .expect("stdout test buffer is not poisoned")
                .clone(),
            openobserve
                .lock()
                .expect("OpenObserve test buffer is not poisoned")
                .clone(),
        ] {
            assert_safe_output(&rendered);
        }
    }
}
