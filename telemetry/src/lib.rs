#![allow(clippy::doc_markdown)]
//! The one observability seam for every Neon Law Navigator binary.
//!
//! [`init`] wires `tracing` once and returns a [`TelemetryGuard`] whose drop
//! flushes any pending OpenTelemetry export. Every `main` calls it with its
//! service name; nothing else hand-rolls a subscriber.
//!
//! Three export decisions, chosen by `OTEL_EXPORTER_OTLP_ENDPOINT` and the
//! OpenObserve variables:
//!
//! - **Unset (dev / CI / OSS fork)** — a human-readable `fmt` layer to stdout
//!   and nothing else. Zero OTel cost, no network.
//! - **Set with no OpenObserve variables** — stdout switches to structured JSON
//!   and the process exports all three signals over plain OTLP/gRPC to a
//!   collector. The collector owns backend credentials and fan-out.
//! - **Set with the complete OpenObserve contract** — stdout switches to
//!   **structured JSON** and the process exports **traces, metrics, and logs**
//!   directly over OTLP/gRPC to OpenObserve. `OTEL_EXPORTER_OTLP_ENDPOINT`
//!   selects the endpoint; `NAVIGATOR_OPENOBSERVE_USERNAME`,
//!   `NAVIGATOR_OPENOBSERVE_PASSWORD`, `NAVIGATOR_OPENOBSERVE_ORGANIZATION`,
//!   and `NAVIGATOR_OPENOBSERVE_STREAM` authenticate and route every signal.
//!   A partial OpenObserve contract falls back safely to stdout instead of
//!   attempting an unauthenticated export. The stdout JSON layer stays on in
//!   both exporting modes, so an export outage never means a lost local log
//!   line.
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
use opentelemetry::metrics::Meter;
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
    #[cfg(test)]
    resource: Resource,
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

        let unsafe_source_path = name == "source_path"
            && value
                .chars()
                .any(|character| matches!(character, '?' | '@' | '='));
        self.unsafe_value = self.unsafe_value
            || identity_like
            || (body_like && !value.is_empty())
            || unsafe_source_path
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

/// The process-side OTLP contract. A plain collector deliberately carries no
/// metadata; direct OpenObserve is the only backend-specific branch here.
#[derive(Debug, Eq, PartialEq)]
enum OtlpExportConfig {
    Collector { endpoint: String },
    OpenObserve(OpenObserveExportConfig),
}

impl OtlpExportConfig {
    fn endpoint(&self) -> &str {
        match self {
            Self::Collector { endpoint } => endpoint,
            Self::OpenObserve(config) => &config.endpoint,
        }
    }

    fn metadata(&self) -> opentelemetry_otlp::tonic_types::metadata::MetadataMap {
        match self {
            Self::Collector { .. } => opentelemetry_otlp::tonic_types::metadata::MetadataMap::new(),
            Self::OpenObserve(config) => config.metadata(),
        }
    }
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

/// Build the process-side OTLP contract. An unset endpoint preserves stdout-
/// only development. An endpoint with no OpenObserve values is a plain
/// collector contract. Once any OpenObserve value is set, all four credential
/// and routing values are mandatory: partial configuration must never cause an
/// unauthenticated export attempt.
fn otlp_export_config(
    endpoint: Option<String>,
    username: Option<String>,
    password: Option<String>,
    organization: Option<String>,
    stream: Option<String>,
) -> Result<Option<OtlpExportConfig>, String> {
    let Some(endpoint) = normalize_endpoint(endpoint) else {
        return Ok(None);
    };

    let configured = [&username, &password, &organization, &stream]
        .into_iter()
        .filter(|value| value.as_ref().is_some_and(|value| !value.trim().is_empty()))
        .count();
    if configured == 0 {
        return Ok(Some(OtlpExportConfig::Collector { endpoint }));
    }

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
    Ok(Some(OtlpExportConfig::OpenObserve(
        OpenObserveExportConfig {
            endpoint,
            authorization,
            organization,
            stream,
        },
    )))
}

/// Build the trace / metric / log OTLP providers for `endpoint`, all sharing a
/// single [`Resource`] (DRY: one resource, three providers — never three
/// resources that can drift). Building an exporter does **not** open a
/// connection — tonic connects lazily on first export — so this is safe to call
/// offline (and the unit tests do exactly that).
fn build_export_providers(
    config: &OtlpExportConfig,
    service_name: &str,
    release: Option<&str>,
) -> ExportProviders {
    // Tag every signal with the deployed release (`YY.M.D`) under the OTel
    // `service.version` convention, so a span/metric/log in OpenObserve says
    // which release emitted it. This is the headless
    // counterpart to `web`'s `GET /version`: the worker and the trigger
    // CronJobs have no HTTP surface, but they self-report their release here.
    let mut builder = Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attributes(resource_attributes_from_env());
    if let Some(release) = release {
        builder = builder.with_attribute(KeyValue::new("service.version", release.to_string()));
    }
    let resource = builder.build();

    // Traces — one batch span exporter.
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(config.endpoint())
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
        .with_endpoint(config.endpoint())
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
        .with_endpoint(config.endpoint())
        .with_metadata(config.metadata())
        .build()
        .expect("build OTLP log exporter");
    let logger = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource.clone())
        .build();

    ExportProviders {
        #[cfg(test)]
        resource: resource.clone(),
        tracer,
        meter,
        logger,
    }
}

/// Merge `OTEL_RESOURCE_ATTRIBUTES` explicitly because `Resource::builder()`
/// does not run the SDK's environment detector. The deployment supplies pod
/// identity here; malformed or empty entries are ignored rather than turning
/// a telemetry identity hint into a boot failure.
fn resource_attributes_from_env() -> Vec<KeyValue> {
    std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .ok()
        .into_iter()
        .flat_map(|attributes| attributes.split(',').map(str::to_owned).collect::<Vec<_>>())
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            (!key.is_empty() && !value.is_empty())
                .then(|| KeyValue::new(key.to_string(), value.to_string()))
        })
        .collect()
}

/// Initialize the global `tracing` subscriber and, when configured, OTLP
/// export. Call exactly once per process, early in `main`.
pub fn init(default_service_name: &str) -> TelemetryGuard {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let export_config = otlp_export_config(
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
        ..
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
/// `tool` is the namespaced tool name (e.g. `create_person`); `outcome` is
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

/// Attribute keys emitted by [`record_web_visit`]. Keep this beside the
/// recorder so the collector pinning test reads the source contract rather
/// than carrying a second hand-maintained list.
pub const WEB_VISIT_ATTRIBUTE_KEYS: &[&str] =
    &["http.route", "country", "source", "locale", "status_class"];

/// Record one public website visit. Safe to call unconditionally: when OTLP is
/// not configured the global meter is a no-op, so this costs nothing in dev.
/// `http.route` is the matched route pattern, `country` is a trusted edge-supplied
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
            KeyValue::new("http.route", route.to_string()),
            KeyValue::new("country", country.to_string()),
            KeyValue::new("source", source.to_string()),
            KeyValue::new("locale", locale.to_string()),
            KeyValue::new("status_class", status_class.to_string()),
        ],
    );
}

/// The instrumentation scope for Neon Law funnel metrics.
const FUNNEL_METER: &str = "navigator.funnel";

/// Counter: one increment for each real Neon Law funnel step, dimensioned by
/// the bounded step name. The event helper below emits the matching
/// identifier-only structured event.
pub const FUNNEL_STEP: &str = "navigator.funnel.step";

/// Attribute keys emitted by [`record_funnel_event`].
pub const FUNNEL_EVENT_ATTRIBUTE_KEYS: &[&str] = &[
    "step",
    "lead_id",
    "brand",
    "source_path",
    "sms_consent",
    "person_id",
    "project_id",
    "notation_id",
    "service_id",
    "channel",
];

/// Attribute keys emitted by the `navigator.funnel.step` counter.
pub const FUNNEL_STEP_ATTRIBUTE_KEYS: &[&str] = &["step"];

/// Neon Law funnel step names. These values are both the structured event's
/// `step` field and the metric's `step` attribute.
pub mod funnel_step {
    /// An anonymous lead was persisted.
    pub const LEAD_CAPTURED: &str = "funnel.lead_captured";
    /// A client started a service from the start door.
    pub const STARTED: &str = "funnel.started";
    /// A notation's questionnaire reached its terminal state.
    pub const INTAKE_COMPLETE: &str = "funnel.intake_complete";
    /// A notation entered the lawyer review gate.
    pub const REVIEW_ENTERED: &str = "funnel.review_entered";
    /// An approved notation moved into an outbound channel.
    pub const SENT: &str = "funnel.sent";
}

/// The outbound channel for [`FunnelEvent::Sent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunnelChannel {
    /// The reviewed draft was handed off by email.
    Email,
    /// The reviewed draft was handed off for e-signature.
    Signature,
}

impl FunnelChannel {
    /// The bounded value written to the event and metric dimensions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Signature => "signature",
        }
    }
}

/// One identifier-only event in the Neon Law funnel.
#[derive(Debug, Clone, Copy)]
pub enum FunnelEvent<'a> {
    /// A public lead was accepted and persisted.
    LeadCaptured {
        lead_id: &'a str,
        brand: &'a str,
        source_path: &'a str,
        sms_consent: bool,
    },
    /// A client opened a service start door successfully.
    Started {
        person_id: &'a str,
        project_id: &'a str,
        notation_id: &'a str,
        service_id: &'a str,
        brand: &'a str,
    },
    /// A questionnaire reached its terminal state.
    IntakeComplete {
        notation_id: &'a str,
        project_id: &'a str,
    },
    /// A notation entered lawyer review.
    ReviewEntered {
        notation_id: &'a str,
        project_id: &'a str,
    },
    /// An approved notation moved into an outbound channel.
    Sent {
        notation_id: &'a str,
        project_id: &'a str,
        channel: FunnelChannel,
    },
}

impl FunnelEvent<'_> {
    fn step(&self) -> &'static str {
        match self {
            Self::LeadCaptured { .. } => funnel_step::LEAD_CAPTURED,
            Self::Started { .. } => funnel_step::STARTED,
            Self::IntakeComplete { .. } => funnel_step::INTAKE_COMPLETE,
            Self::ReviewEntered { .. } => funnel_step::REVIEW_ENTERED,
            Self::Sent { .. } => funnel_step::SENT,
        }
    }
}

/// Emit one structured Neon Law funnel event and increment its counter.
///
/// The enum is deliberately closed so a call site can provide only the
/// identifiers and bounded values belonging to that step. No address, email,
/// phone, name, matter title, or Project code can enter this event family.
pub fn record_funnel_event(event: FunnelEvent<'_>) {
    let step = event.step();
    record_funnel_step(step);
    match event {
        FunnelEvent::LeadCaptured {
            lead_id,
            brand,
            source_path,
            sms_consent,
        } => tracing::info!(
            target: "funnel",
            step,
            lead_id,
            brand,
            source_path,
            sms_consent,
        ),
        FunnelEvent::Started {
            person_id,
            project_id,
            notation_id,
            service_id,
            brand,
        } => tracing::info!(
            target: "funnel",
            step,
            person_id,
            project_id,
            notation_id,
            service_id,
            brand,
        ),
        FunnelEvent::IntakeComplete {
            notation_id,
            project_id,
        } => tracing::info!(target: "funnel", step, notation_id, project_id),
        FunnelEvent::ReviewEntered {
            notation_id,
            project_id,
        } => tracing::info!(target: "funnel", step, notation_id, project_id),
        FunnelEvent::Sent {
            notation_id,
            project_id,
            channel,
        } => tracing::info!(
            target: "funnel",
            step,
            notation_id,
            project_id,
            channel = channel.as_str(),
        ),
    }
}

/// Increment the Neon Law funnel counter. Safe to call unconditionally: an
/// unconfigured OpenTelemetry global provider is a no-op.
pub fn record_funnel_step(step: &str) {
    let meter = opentelemetry::global::meter(FUNNEL_METER);
    record_funnel_step_with_meter(&meter, step);
}

fn record_funnel_step_with_meter(meter: &Meter, step: &str) {
    let counter = meter.u64_counter(FUNNEL_STEP).build();
    counter.add(1, &[KeyValue::new("step", step.to_string())]);
}

/// The instrumentation scope for browser sign-in metrics.
const AUTH_METER: &str = "navigator.auth";

/// Counter for completed browser sign-in callbacks, dimensioned by provider
/// and bounded outcome.
pub const AUTH_SIGN_IN: &str = "navigator.auth.sign_in";

/// Attribute keys emitted by [`record_auth_event`].
pub const AUTH_EVENT_ATTRIBUTE_KEYS: &[&str] = &[
    "event",
    "person_id",
    "provider",
    "brand",
    "first_link",
    "reason",
];

/// Attribute keys emitted by the `navigator.auth.sign_in` counter.
pub const AUTH_SIGN_IN_ATTRIBUTE_KEYS: &[&str] = &["provider", "outcome"];

/// Bounded outcome values for [`AUTH_SIGN_IN`].
pub mod auth_outcome {
    /// A provider callback created a session.
    pub const SIGNED_IN: &str = "signed_in";
    /// A provider callback was refused.
    pub const REFUSED: &str = "refused";
}

/// The provider values allowed in sign-in telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProvider {
    /// Google sign-in through the primary OIDC slot.
    Google,
    /// Microsoft Entra sign-in.
    Microsoft,
    /// Sign in with Apple.
    Apple,
}

impl AuthProvider {
    /// The bounded value written to the event and metric dimensions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
            Self::Apple => "apple",
        }
    }
}

/// The refusal reasons allowed in sign-in telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSignInReason {
    /// No subject matched and the token carried no usable email claim.
    NoSubjectMatchNoEmail,
    /// No admitted person matched the token's email claim.
    EmailUnmatched,
    /// A matching person is not admitted for sign-in.
    NotAdmitted,
    /// The provider token failed verification.
    TokenInvalid,
}

impl AuthSignInReason {
    /// The bounded value written to the refusal event and log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoSubjectMatchNoEmail => "no_subject_match_no_email",
            Self::EmailUnmatched => "email_unmatched",
            Self::NotAdmitted => "not_admitted",
            Self::TokenInvalid => "token_invalid",
        }
    }
}

/// One identifier-only browser sign-in outcome.
#[derive(Debug, Clone, Copy)]
pub enum AuthEvent<'a> {
    /// A provider callback created an application session.
    SignedIn {
        /// The local Person identifier.
        person_id: &'a str,
        /// The provider that issued the callback token.
        provider: AuthProvider,
        /// The resolved house brand.
        brand: &'a str,
        /// Whether this sign-in is known to have persisted a provider link.
        first_link: bool,
    },
    /// A provider callback was refused after token processing.
    SignInRefused {
        /// The provider that issued the callback token.
        provider: AuthProvider,
        /// The resolved house brand.
        brand: &'a str,
        /// The bounded refusal reason.
        reason: AuthSignInReason,
    },
}

/// Emit one structured browser sign-in event and increment its counter.
pub fn record_auth_event(event: AuthEvent<'_>) {
    match event {
        AuthEvent::SignedIn {
            person_id,
            provider,
            brand,
            first_link,
        } => {
            record_auth_sign_in(provider, auth_outcome::SIGNED_IN);
            tracing::info!(
                target: "auth",
                event = "auth.signed_in",
                person_id,
                provider = provider.as_str(),
                brand,
                first_link,
            );
        }
        AuthEvent::SignInRefused {
            provider,
            brand,
            reason,
        } => {
            record_auth_sign_in(provider, auth_outcome::REFUSED);
            tracing::info!(
                target: "auth",
                event = "auth.sign_in_refused",
                provider = provider.as_str(),
                brand,
                reason = reason.as_str(),
            );
        }
    }
}

/// Increment the browser sign-in counter. An unconfigured OpenTelemetry
/// global provider is a no-op.
pub fn record_auth_sign_in(provider: AuthProvider, outcome: &str) {
    let meter = opentelemetry::global::meter(AUTH_METER);
    record_auth_sign_in_with_meter(&meter, provider, outcome);
}

fn record_auth_sign_in_with_meter(meter: &Meter, provider: AuthProvider, outcome: &str) {
    let counter = meter.u64_counter(AUTH_SIGN_IN).build();
    counter.add(
        1,
        &[
            KeyValue::new("provider", provider.as_str().to_string()),
            KeyValue::new("outcome", outcome.to_string()),
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
        otlp_export_config, parent_context_from, trace_context_headers, AuthEvent, AuthProvider,
        AuthSignInReason, FunnelChannel, FunnelEvent, OtlpExportConfig, SanitizingSubscriber,
        AUTH_SIGN_IN,
    };

    #[test]
    fn funnel_events_have_the_declared_fields_and_no_content_fields() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

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

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            super::record_funnel_event(FunnelEvent::LeadCaptured {
                lead_id: "00000000-0000-0000-0000-000000000001",
                brand: "neon",
                source_path: "/services",
                sms_consent: false,
            });
            super::record_funnel_event(FunnelEvent::Started {
                person_id: "00000000-0000-0000-0000-000000000002",
                project_id: "00000000-0000-0000-0000-000000000003",
                notation_id: "00000000-0000-0000-0000-000000000004",
                service_id: "llc-file",
                brand: "neon",
            });
            super::record_funnel_event(FunnelEvent::IntakeComplete {
                notation_id: "00000000-0000-0000-0000-000000000004",
                project_id: "00000000-0000-0000-0000-000000000003",
            });
            super::record_funnel_event(FunnelEvent::ReviewEntered {
                notation_id: "00000000-0000-0000-0000-000000000004",
                project_id: "00000000-0000-0000-0000-000000000003",
            });
            super::record_funnel_event(FunnelEvent::Sent {
                notation_id: "00000000-0000-0000-0000-000000000004",
                project_id: "00000000-0000-0000-0000-000000000003",
                channel: FunnelChannel::Signature,
            });
        });

        let rendered = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");
        let lines: Vec<serde_json::Value> = rendered
            .lines()
            .map(|line| serde_json::from_str(line).expect("funnel event is JSON"))
            .collect();
        assert_eq!(lines.len(), 5);
        for line in lines {
            let fields = line
                .get("fields")
                .and_then(serde_json::Value::as_object)
                .expect("JSON formatter nests event fields");
            for forbidden in ["address", "email", "name", "phone", "project_code"] {
                assert!(!fields.contains_key(forbidden), "forbidden field in {line}");
            }
        }
        assert!(rendered.contains("funnel.lead_captured"));
        assert!(rendered.contains("funnel.started"));
        assert!(rendered.contains("funnel.intake_complete"));
        assert!(rendered.contains("funnel.review_entered"));
        assert!(rendered.contains("funnel.sent"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn funnel_metric_registers_and_increments_with_the_step_attribute() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            data::AggregatedMetrics, InMemoryMetricExporter, SdkMeterProvider,
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let meter = provider.meter(super::FUNNEL_METER);

        super::record_funnel_step_with_meter(&meter, "funnel.review_entered");
        provider.force_flush().expect("metric flush");

        let metrics = exporter
            .get_finished_metrics()
            .expect("metric export succeeds");
        let metric = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .find(|metric| metric.name() == super::FUNNEL_STEP)
            .expect("funnel metric is registered");
        let AggregatedMetrics::U64(sum) = metric.data() else {
            panic!("funnel metric is not a sum");
        };
        let rendered = format!("{sum:?}");
        assert!(rendered.contains("value: 1"), "counter value: {rendered}");
        assert!(
            rendered.contains("step") && rendered.contains("funnel.review_entered"),
            "counter attributes: {rendered}"
        );
        provider.shutdown().expect("metric provider shuts down");
    }

    #[test]
    fn auth_events_have_the_declared_fields_and_no_identity_fields() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

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

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            super::record_auth_event(AuthEvent::SignedIn {
                person_id: "00000000-0000-0000-0000-000000000001",
                provider: AuthProvider::Google,
                brand: "neon",
                first_link: true,
            });
            for reason in [
                AuthSignInReason::NoSubjectMatchNoEmail,
                AuthSignInReason::EmailUnmatched,
                AuthSignInReason::NotAdmitted,
                AuthSignInReason::TokenInvalid,
            ] {
                super::record_auth_event(AuthEvent::SignInRefused {
                    provider: AuthProvider::Apple,
                    brand: "neon",
                    reason,
                });
            }
        });

        let rendered = String::from_utf8(output.lock().expect("capture lock").clone())
            .expect("capture is UTF-8");
        let lines: Vec<serde_json::Value> = rendered
            .lines()
            .map(|line| serde_json::from_str(line).expect("auth event is JSON"))
            .collect();
        assert_eq!(lines.len(), 5);
        for line in &lines {
            let fields = line
                .get("fields")
                .and_then(serde_json::Value::as_object)
                .expect("JSON formatter nests event fields");
            for key in fields.keys() {
                let key = key.to_ascii_lowercase();
                assert!(
                    !["email", "name", "sub", "subject", "address"]
                        .iter()
                        .any(|forbidden| key.contains(forbidden)),
                    "forbidden field key in {line}"
                );
            }
        }
        assert!(rendered.contains("auth.signed_in"));
        assert!(rendered.contains("auth.sign_in_refused"));
        assert!(rendered.contains("no_subject_match_no_email"));
        assert!(rendered.contains("email_unmatched"));
        assert!(rendered.contains("not_admitted"));
        assert!(rendered.contains("token_invalid"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn auth_metric_registers_and_increments_with_provider_and_outcome_attributes() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            data::AggregatedMetrics, InMemoryMetricExporter, SdkMeterProvider,
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let meter = provider.meter(super::AUTH_METER);

        super::record_auth_sign_in_with_meter(
            &meter,
            AuthProvider::Microsoft,
            super::auth_outcome::REFUSED,
        );
        provider.force_flush().expect("metric flush");

        let metrics = exporter
            .get_finished_metrics()
            .expect("metric export succeeds");
        let metric = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .find(|metric| metric.name() == AUTH_SIGN_IN)
            .expect("auth metric is registered");
        let AggregatedMetrics::U64(sum) = metric.data() else {
            panic!("auth metric is not a sum");
        };
        let rendered = format!("{sum:?}");
        assert!(rendered.contains("value: 1"), "counter value: {rendered}");
        assert!(
            rendered.contains("provider")
                && rendered.contains("microsoft")
                && rendered.contains("outcome")
                && rendered.contains("refused"),
            "counter attributes: {rendered}"
        );
        provider.shutdown().expect("metric provider shuts down");
    }

    #[test]
    fn an_unset_endpoint_is_stdout_only() {
        assert_eq!(otlp_export_config(None, None, None, None, None), Ok(None));
    }

    #[test]
    fn a_complete_openobserve_contract_keeps_its_direct_credentials() {
        let config = otlp_export_config(
            Some("http://openobserve:5081".to_string()),
            Some("root@example.com".to_string()),
            Some("secret".to_string()),
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect("complete OpenObserve configuration is valid")
        .expect("an endpoint enables export");

        let OtlpExportConfig::OpenObserve(config) = config else {
            panic!("complete OpenObserve configuration must use the direct contract");
        };
        assert_eq!(config.endpoint, "http://openobserve:5081");
        assert_eq!(config.organization, "navigator");
        assert_eq!(config.stream, "default");
        assert_eq!(
            config.authorization,
            "Basic cm9vdEBleGFtcGxlLmNvbTpzZWNyZXQ="
        );
        let metadata = config.metadata();
        assert_eq!(
            metadata
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Basic cm9vdEBleGFtcGxlLmNvbTpzZWNyZXQ=")
        );
        assert_eq!(
            metadata
                .get("organization")
                .and_then(|value| value.to_str().ok()),
            Some("navigator")
        );
        assert_eq!(
            metadata
                .get("stream-name")
                .and_then(|value| value.to_str().ok()),
            Some("default")
        );
    }

    #[test]
    fn an_endpoint_without_openobserve_values_is_a_plain_collector_contract() {
        let config = otlp_export_config(
            Some("http://otel-collector:4317".to_string()),
            None,
            None,
            None,
            None,
        )
        .expect("a plain collector endpoint is valid")
        .expect("an endpoint enables export");

        assert!(matches!(
            &config,
            OtlpExportConfig::Collector { endpoint } if endpoint == "http://otel-collector:4317"
        ));
        assert!(config.metadata().is_empty());
    }

    #[test]
    fn a_partial_openobserve_contract_is_refused() {
        let error = otlp_export_config(
            Some("http://openobserve:5081".to_string()),
            Some("root@example.com".to_string()),
            None,
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect_err("an endpoint without complete credentials must not export");

        assert!(error.contains("NAVIGATOR_OPENOBSERVE_PASSWORD"));
    }

    /// A single OpenObserve value is still a partial contract.
    ///
    /// This is the boundary the plain-collector branch is one step away from:
    /// it reads "no OpenObserve values at all" as a collector, so the very
    /// next case — exactly one value, which is what a half-applied Secret or
    /// a typo'd key looks like — is the one that must not be mistaken for a
    /// collector and exported unauthenticated. Each of the four is checked,
    /// because the count is what decides and any one of them must trip it.
    #[test]
    fn a_single_openobserve_value_is_still_a_partial_contract() {
        let endpoint = || Some("http://openobserve:5081".to_string());
        let set = || Some("only-one".to_string());

        for (name, config) in [
            (
                "NAVIGATOR_OPENOBSERVE_USERNAME",
                otlp_export_config(endpoint(), set(), None, None, None),
            ),
            (
                "NAVIGATOR_OPENOBSERVE_PASSWORD",
                otlp_export_config(endpoint(), None, set(), None, None),
            ),
            (
                "NAVIGATOR_OPENOBSERVE_ORGANIZATION",
                otlp_export_config(endpoint(), None, None, set(), None),
            ),
            (
                "NAVIGATOR_OPENOBSERVE_STREAM",
                otlp_export_config(endpoint(), None, None, None, set()),
            ),
        ] {
            let error =
                config.expect_err("one OpenObserve value must never export as a plain collector");
            assert!(
                !error.contains(name),
                "{name} is the value that was set; the error must name the missing three"
            );
        }
    }

    /// Whitespace is not configuration.
    ///
    /// `required_config_value` already treats a blank value as missing, so a
    /// contract of four blanks has to reach the collector branch rather than
    /// the error branch — otherwise a Secret projected with an empty key
    /// turns a working collector export into stdout-only.
    #[test]
    fn blank_openobserve_values_are_a_plain_collector_contract() {
        let config = otlp_export_config(
            Some("http://otel-collector:4317".to_string()),
            Some("  ".to_string()),
            Some(String::new()),
            Some("\t".to_string()),
            Some(String::new()),
        )
        .expect("blank OpenObserve values are absent, not partial")
        .expect("an endpoint enables export");

        assert!(matches!(config, OtlpExportConfig::Collector { .. }));
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
        let previous_resource_attributes = std::env::var_os("OTEL_RESOURCE_ATTRIBUTES");
        std::env::set_var(
            "OTEL_RESOURCE_ATTRIBUTES",
            "k8s.pod.name=telemetry-test-pod,service.instance.id=telemetry-test-pod",
        );
        let config = otlp_export_config(
            Some("http://127.0.0.1:5081".to_string()),
            Some("root@example.com".to_string()),
            Some("secret".to_string()),
            Some("navigator".to_string()),
            Some("default".to_string()),
        )
        .expect("complete test config is valid")
        .expect("test endpoint enables export");
        let providers = build_export_providers(&config, "telemetry-test", Some("26.6.23"));
        assert_eq!(
            providers
                .resource
                .get(&opentelemetry::Key::new("k8s.pod.name"))
                .map(|value| value.to_string()),
            Some("telemetry-test-pod".to_string())
        );
        // All three signals are present; shutting down flushes (no-op here,
        // nothing batched) without panicking or requiring a live OpenObserve.
        let _ = providers.tracer.shutdown();
        let _ = providers.meter.shutdown();
        let _ = providers.logger.shutdown();
        match previous_resource_attributes {
            Some(value) => std::env::set_var("OTEL_RESOURCE_ATTRIBUTES", value),
            None => std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES"),
        }
    }

    /// The same three providers build from the plain collector contract.
    ///
    /// This is the contract every deployed binary now uses — the OpenObserve
    /// branch is the exception — so the empty metadata map has to survive the
    /// exporter builders' `expect`s for all three signals, not just compile.
    ///
    /// Multi-thread for the reason the OpenObserve case above documents.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn export_providers_build_all_three_signals_for_a_collector() {
        let config = otlp_export_config(
            Some("http://127.0.0.1:4317".to_string()),
            None,
            None,
            None,
            None,
        )
        .expect("a collector endpoint is a valid contract")
        .expect("test endpoint enables export");
        assert!(matches!(config, OtlpExportConfig::Collector { .. }));

        let providers = build_export_providers(&config, "telemetry-test", None);
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
            source_path = "/services?utm_source=campaign",
            "unsafe source path must not be exported"
        );
        tracing::info!(
            person_id = "opaque-person-id",
            outcome = "accepted",
            source_path = "/services",
            "safe source path survives unchanged"
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
        assert!(!rendered.contains("unsafe source path must not be exported"));
        assert!(rendered.contains("safe source path survives unchanged"));
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
        // `without_time` keeps the wall clock out of the rendered lines: the
        // formatter's default timestamp carries sub-second digits, and the
        // `212` assertion below would otherwise trip whenever those digits
        // happened to contain it.
        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .without_time()
            .with_writer(Buffer(stdout.clone()))
            .with_target(false);
        let openobserve_layer = tracing_subscriber::fmt::layer()
            .json()
            .without_time()
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
