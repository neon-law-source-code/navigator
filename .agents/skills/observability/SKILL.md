--- name: observability description: > How Neon Law Navigator emits telemetry and how to debug a durable-execution
failure fast. Every binary shares one seam, `telemetry::init` (stdout logs — JSON in prod — plus OTLP traces + metrics
when `OTEL_EXPORTER_OTLP_ENDPOINT` is set), and everything lands in BigQuery (structured logs via a Cloud Logging sink;
traces/metrics via an OTel Collector to Cloud Trace/Monitoring). Trigger when adding a span/metric/log field,
instrumenting a handler or workflow, wiring a new binary's main, debugging a missing nightly/periodic job or a silent
trigger, running `navigator ops doctor`, or touching the OTel Collector / BigQuery sink. THE ONE RULE, stated first
because it is load-bearing: instrument identifiers and counts, NEVER content — a notation_id, service, outcome,
duration, status code are fine; a client name, answer body, email address, or document body must never enter a span,
metric, or log. Telemetry crosses the firm's trust boundary; client content does not. Skip for one-off println
debugging. ---

# observability

The single most important rule, before anything else:

> **Identifiers and counts, never content.** A span attribute, metric label, or log field may carry a `notation_id`, a
  `service` name, an `outcome`, a duration, an HTTP status — never a client name, an answer body, an email address, or a
  document body. Telemetry leaves the firm's trust boundary; privileged client content does not. This is a standing
  engineering- and legal-council order. When in doubt, log the id and look the rest up.

## The seam

Every binary calls `telemetry::init("navigator-<name>")` once in `main` and **holds the returned guard to end of
`main`** (the drop flushes batched export — critical for short-lived trigger jobs). Never hand-roll `tracing_subscriber`
in a binary again; the crate is the one seam.

- `OTEL_EXPORTER_OTLP_ENDPOINT` **unset** → human `fmt` logs to stdout, no OTLP. Zero cost (dev/CI/forks). **set** →
  JSON logs to stdout (Cloud Logging parses them) **plus** OTLP traces + metrics to the collector.

Wiring a new binary: add `telemetry.workspace = true`, then in `main` bind the guard to a name (never `let _ =`, which
drops instantly):

```rust
let _telemetry = telemetry::init("navigator-<name>");
```

**Opt-out by design.** The seam is for the *service* binaries — the 7 that wire it are `web`, `workflows-service`, the
four `*-trigger` jobs, and `redirect`. The interactive / short-lived CLIs — `cli` (navigator), `navigator-lsp` —
deliberately do **not** init it: their output is for a human at a terminal (or an LSP client over stdio), not the lake,
so instrumenting them would only add noise. Wiring a new *service* binary is the rule; a new CLI staying stdout-only is
the exception, and it is a choice, not an oversight.

## Instrumenting work

- Prefer instrumenting the **one shared chokepoint** over N call sites. Workflow triggers are instrumented once in
  `workflows::start_workflow` (span `workflow.trigger` + metric `navigator.workflow.trigger.fired{service,outcome}`), so
  every trigger gets it free.
- The two agent surfaces over the **one shared tool catalog** are each instrumented at their single dispatch
  chokepoint, so neither protocol is blind: A2A in `portal::a2a` (audit spans), and `/mcp` in
  `mcp::server::handle_tools_call` (span `mcp.tool.call` + metric `navigator.mcp.tool.called{tool,outcome}`). Tool name
  and outcome only — never the `arguments`.
- Spans: `#[tracing::instrument(skip(secret_or_body), fields(service = ..., key = ...))]`. Skip bodies and tokens.
  Metrics: `telemetry::record_trigger_fired(service, outcome)` is the model — create the counter from
  `opentelemetry::global::meter(...)`, add with `KeyValue` labels that are ids/enums only. Safe to call when OTLP is off
  (the global meter is a no-op).
- Events: `tracing::error!(service, status = code, "…")` — fields are ids/counts; the message names the condition.

## Where it lands

Logs → Cloud Logging → a **BigQuery sink** (day one); traces + metrics → Cloud Trace / Monitoring via the **OTel
Collector**, whose **fail-closed redaction processor** (`allow_all_keys: false`) is the last line enforcing the
no-content rule. It complements the Iceberg archive: telemetry is the *operational* half, the archive the *data* half.

Full landing map, the sink, and the collector config live in
[`docs/observability.md`](../../../docs/observability.md#where-it-lands-direct-openobserve) — don't restate the
architecture here.

**Locally** the same OTLP export goes straight to an in-cluster **OpenObserve** — `navigator dev up` wires host and
in-cluster `web` to it and writes the whole contract (UI URL, OTLP endpoint, credentials, organization, stream) into
`.devx/env`, on ports derived per worktree. To *see* a span, metric, or trace you just emitted, follow
[`docs/observability.md`](../../../docs/observability.md#seeing-telemetry-locally-openobserve); this skill is the *emit*
side.

## Debugging a missing periodic job (the playbook)

1. **`navigator ops doctor`** — names any trigger Job wedged in `ImagePullBackOff` / `CrashLoopBackOff` or
   Active-too-long (which, under a `CronJob`'s `concurrencyPolicy: Forbid`, silently skips every run) and any unready
   workload, each with the exact `kubectl` fix. First stop for a missing nightly/periodic job.
2. **BigQuery / logs** — `navigator.workflow.trigger.fired` by `service`/`outcome`: `rejected status=401` is a stale
   `RESTATE_AUTH_TOKEN`; `404` is a service not re-registered; `transport_error` is a hung/unreachable ingress.
3. **Restate Cloud console → Invocations** — did the worker run it; which step failed.
4. **`Heartbeat` email** (six-hourly) — its *absence* means durable execution itself may be down.

See [`docs/observability.md`](../../../docs/observability.md) for the full architecture and
[`docs/durable-workflows.md`](../../../docs/durable-workflows.md) for the durable-execution model.
