# Observability

How Neon Law Navigator emits telemetry, where it lands for analysis, and how an operator debugs a durable-execution
failure fast. Born from an incident: a trigger Job sat in `ImagePullBackOff` for days while a `CronJob`'s
`concurrencyPolicy: Forbid` silently skipped every run, and *nothing emitted a queryable signal* — the only telemetry
was the nightly email, which was the thing that broke. This page exists so that never repeats.

> **The one rule for anyone adding a span, metric, or log field — identifiers and counts, never content.** A
  `notation_id`, a `service` name, an `outcome`, a duration, a status code: yes. A client name, an answer body, an email
  address, a document body: never. Telemetry crosses the firm's trust boundary; client content does not. This is a
  standing engineering- and legal-council order, not a style preference.

## One seam: `telemetry::init`

Every binary calls [`telemetry::init`](../telemetry/src/lib.rs) once in `main` and holds the returned guard until exit.
There is no per-binary subscriber wiring anymore — web, the `workflows-service` worker, and all six `*-trigger` jobs
share the one crate. Two modes, chosen by whether `OTEL_EXPORTER_OTLP_ENDPOINT` is set:

| | Unset (stdout-only) | Complete OpenObserve contract |
| --- | --- | --- |
| stdout | human-readable `fmt` | **structured JSON** and OTLP |
| traces | — | OTLP/gRPC → OpenObserve |
| metrics | — | OTLP/gRPC → OpenObserve |
| cost | zero — no network | one batch span + periodic metric push |

`OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` name the exporter and service. The four `NAVIGATOR_OPENOBSERVE_*`
values supply Basic authentication, organization, and stream routing. An endpoint without all four is rejected and the
process remains stdout-only. The guard's drop flushes batched spans/metrics — important for the short-lived trigger
jobs, which would otherwise exit before the periodic exporter fires.

## What is instrumented

Every workflow trigger funnels through `workflows::start_workflow`, instrumented once there so every trigger inherits
it:

- a span `workflow.trigger` with `service` / `key` / `handler` — never the request body; the metric
  **`navigator.workflow.trigger.fired`**, dimensioned by `service` and `outcome` ∈ {`accepted`, `rejected`,
  `transport_error`}. A flat line for a service that should fire on a schedule is the signal a trigger has silently
  stopped — the exact failure that hid for days;
- a structured event on each outcome (`status`, `service`) so a 401 / 404 / timeout is one log line, not a guess.

The worker and web emit their own spans through the same subscriber, so new handlers inherit tracing for free.

Web also records first-party public website visits as aggregate analytics. The durable table and OTel counter
(`navigator.web.visit.count`) use bounded dimensions only: UTC day, Axum matched route pattern, trusted edge
country/region code, route-derived locale, coarse status class, and a source bucket derived from approved UTM/ref query
parameters or referrer host classification. Do not add raw IP addresses, user agents, session or person identifiers, raw
URL paths, arbitrary query parameters, full query strings, or full `Referer` URLs to visitor analytics. Unknown or
sensitive query parameters are ignored, invalid allowed values collapse to `invalid`, missing referrers collapse to
`direct`, same-site referrers to `internal`, and unrecognized external hosts to `other`. The operational view is
admin-only at `/app/admin/analytics`.

## Where it lands: direct OpenObserve

Traces, metrics, and logs speak OTLP/gRPC directly to the OpenObserve organization and stream named in their
environment. There is no collector hop and therefore no shared collector credential or backend fan-out to operate. The
telemetry contract is deliberately fail-closed: identifiers and bounded counts enter the Rust instrumentation; the
production organization then applies its own stream retention, access, and redaction policy.

The Iceberg archive ([iceberg-archive guide](iceberg-archive.md)) remains distinct. Its nightly `Archives` workflow
snapshots SurrealDB tables to Parquet on GCS for BigQuery external-table analysis; it is not an operational telemetry
sink. Any decision to retain application logs in BigQuery is a separate deployment-operator decision, recorded with that
deployment rather than implied by the exporter.

## Seeing telemetry locally: OpenObserve

`navigator dev up` creates an OpenObserve Deployment and Service in KIND, waits for its rollout, and port-forwards the
UI and direct OTLP/gRPC endpoints. It writes this complete development-only contract to `.devx/env`:

```text
NAVIGATOR_OPENOBSERVE_URL=http://localhost:5080
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5081
NAVIGATOR_OPENOBSERVE_USERNAME=root@example.com
NAVIGATOR_OPENOBSERVE_PASSWORD=…
NAVIGATOR_OPENOBSERVE_ORGANIZATION=default
NAVIGATOR_OPENOBSERVE_STREAM=default
```

Use `NAVIGATOR_KIND_OPENOBSERVE_PORT` and `NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT` to select different host ports. KIND
storage is `emptyDir`, so recreating the pod removes local telemetry. Host `web` started after sourcing `.devx/env`
exports directly; in-cluster `web` and `workflows-service` receive the same complete contract from
`navigator-openobserve`.

Open `NAVIGATOR_OPENOBSERVE_URL`, sign in with the generated development credentials, and use Explorer to select the
`default` organization and the stream for traces, logs, or metrics. Search by `service_name = neon-server` for the host
loop, `navigator-web` for the in-cluster web process, or `workflows-service` for the worker. A Restate-triggered
workflow should retain its W3C trace id across the boundary. Use `RUST_LOG=info` for that verification so the caller
span is enabled. Set an empty `OTEL_EXPORTER_OTLP_ENDPOINT` to intentionally run stdout-only.

## Debugging "the workflow didn't run"

Work down the chain (full version in the [durable-workflows guide](durable-workflows.md)); each rung now has telemetry:

1. **Did the trigger fire — and is a job wedged?** Run **`navigator ops doctor`**. It reads the cluster and names, in
   plain language, any trigger Job stuck in `ImagePullBackOff` / `CrashLoopBackOff` or Active too long (which, under
   `Forbid`, skips every subsequent run) and any unready workload. It prints the exact `kubectl` command that fixes each
   finding. First stop for a missing nightly/periodic job.
2. **Did the ingress accept it?** Query OpenObserve for `navigator.workflow.trigger.fired` by `service` and `outcome`,
   or read the trigger-outcome log events: `rejected` with `status=401` is a stale `RESTATE_AUTH_TOKEN`; `status=404` is
   the registration gotcha; `transport_error` is an unreachable/hung ingress (now capped by a 30s client timeout +
   `activeDeadlineSeconds`).
3. **Did the worker run it?** The Restate Cloud console → Invocations shows the journal; a failing step retries and
   surfaces there. Open it for this deployment's environment; the `Heartbeat` and `Archives` emails name the invocation
   but do not link to it.
4. **Is durable execution alive at all?** The six-hourly `Heartbeat` email is the liveness signal; its *absence* is the
   alert.

## Tracing across the Restate boundary

A workflow kicked off from `web` continues the caller's trace into the durable handler, so a single trace spans "button
click → ingress POST → snapshot/dispatch steps." `workflows::trigger` injects the current span's W3C `traceparent` into
the outbound ingress POST (`telemetry::current_trace_context_headers`); instrumented handlers extract it from
`ctx.headers()` and parent their span on the result (`telemetry::set_span_parent`, used by `Archives::run` and every
`Notation` handler). Only opaque trace context crosses — never a client field (LEGAL #2). When OTLP is unconfigured the
inject/extract pair is a no-op, so an explicitly stdout-only environment stays zero-cost.

The Rust contract — inject produces a well-formed `traceparent`, extract recovers the same trace id — is covered by
`telemetry`'s round-trip test and `workflows`' `trace_propagation` integration test. The **one** thing only a live
cluster confirms is that Restate forwards the ingress `traceparent` onto the handler invocation headers; verify once in
KIND/prod by checking a `web`-initiated workflow and its steps share a trace id in OpenObserve. If a future Restate
version stops forwarding it, the fallback is to carry a `trace_id` in the request body and link (rather than parent) the
handler span — no other code changes.

## The hardening that came with this

- **HTTP timeout** in `start_workflow` (30s) so a hung ingress can't keep a trigger pod running forever.
  **`activeDeadlineSeconds: 120` + `startingDeadlineSeconds`** on the trigger `CronJob`s, so a stuck trigger
  self-terminates instead of holding the `Forbid` lock and skipping every run — the precise failure mode that stopped
  the nightly archives email for days.
- **`navigator ops doctor`** so the next operator sees the wedge in one command instead of `kubectl` archaeology.

## See also

- The [durable-workflows guide](durable-workflows.md) — the durable-execution model and the registration gotcha. The
  [Iceberg archive guide](iceberg-archive.md) — the nightly store → Parquet → BigQuery table archive.
  [`cloud-operations.md`](cloud-operations.md) — the deployment and operator boundary.
