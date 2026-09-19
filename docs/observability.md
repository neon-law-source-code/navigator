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
share the one crate. The endpoint and OpenObserve variables select one of three process contracts:

| | Unset (stdout-only) | Plain collector contract | Complete OpenObserve contract |
| --- | --- | --- | --- |
| stdout | human-readable `fmt` | **structured JSON** and OTLP | **structured JSON** and OTLP |
| traces | — | OTLP/gRPC → collector | OTLP/gRPC → OpenObserve |
| metrics | — | OTLP/gRPC → collector | OTLP/gRPC → OpenObserve |
| logs | — | OTLP/gRPC → collector and stdout JSON | OTLP/gRPC → OpenObserve and stdout JSON |
| credentials | — | none in the process | Basic auth, organization, and stream |

`OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` name the exporter and service. The four `NAVIGATOR_OPENOBSERVE_*`
values supply Basic authentication, organization, and stream routing for the direct OpenObserve contract. When none of
those four values is set, an endpoint is a plain collector contract with no authentication metadata. When one to three
are set, the process rejects the partial OpenObserve contract and remains stdout-only. The guard's drop flushes batched
spans/metrics — important for the short-lived trigger jobs, which would otherwise exit before the periodic exporter
fires.

Nothing about the plain collector contract replaces the OpenTelemetry environment variables.
`OTEL_EXPORTER_OTLP_HEADERS` and its per-signal variants still apply in every exporting mode — the OpenObserve metadata
is merged with them rather than substituted for them — so pointing the collector contract at a backend that needs its
own header is a deployment change, not a Rust change.

## What is instrumented

Every workflow trigger funnels through `workflows::start_workflow`, instrumented once there so every trigger inherits
it:

- a span `workflow.trigger` with `service` / `key` / `handler` — never the request body; the metric
  **`navigator.workflow.trigger.fired`**, dimensioned by `service` and `outcome` ∈ {`accepted`, `rejected`,
  `transport_error`}. A flat line for a service that should fire on a schedule is the signal a trigger has silently
  stopped — the exact failure that hid for days;
- a structured event on each outcome (`status`, `service`) so a 401 / 404 / timeout is one log line, not a guess.

The worker and web emit their own spans through the same subscriber, so new handlers inherit tracing for free. Every
`web` request span additionally carries the HTTP semconv trio through the outermost `TraceLayer` in `portal/src/lib.rs`:
`http.request.method`, `http.route` (the matched route *template* from axum's `MatchedPath` — never the resolved path,
so a Project code or person id in the URL never rides a span attribute), and `http.response.status_code` (recorded once
the handler answers). All three are on the collector's allow-list already, so a Dash0 span query grouped by
`http.response.status_code` works without a collector change.

`store::surreal::ping` — the readiness probe's one query — runs on its own `tokio::spawn`ed task rather than inline
(ENG-709). `readyz`'s caller is a kubelet HTTP probe with a short timeout; when it fires before the query answers,
kubelet drops the connection and axum drops the handler future that was awaiting `ping`. Awaited inline, that drop would
drop the receiver half of the remote WS engine's internal response channel while the query was still in flight, and the
`surrealdb` client's own router task would log `Failed to send query results to channel: SendError(..)` at ERROR when it
tried to deliver a response nobody was waiting for — this was roughly 85% of staging's log volume. Spawning decouples
the two lifetimes: the caller giving up only stops it from *waiting*, not the query from *running*, so the engine always
finds its receiver.

Web also records first-party public website visits as aggregate analytics. The durable table and OTel counter
(`navigator.web.visit.count`) use bounded dimensions only: UTC day, Axum matched route pattern, trusted edge
country/region code, route-derived locale, coarse status class, and a source bucket derived from approved UTM/ref query
parameters or referrer host classification. Do not add raw IP addresses, user agents, session or person identifiers, raw
URL paths, arbitrary query parameters, full query strings, or full `Referer` URLs to visitor analytics. Unknown or
sensitive query parameters are ignored, invalid allowed values collapse to `invalid`, missing referrers collapse to
`direct`, same-site referrers to `internal`, and unrecognized external hosts to `other`. The operational view is
admin-only at `/app/admin/analytics`. The exported counter dimensions are `http.route`, `country`, `source`, `locale`,
and `status_class`; `http.route` is the matched template, never the resolved path.

Public lead capture stores the submitted inquiry in the `lead` table through `POST /leads`; its audit event carries only
the lead id, brand, source path, and outcome, never an email address, phone number, or form content. Conversion writes
the Person directory (`store::persons::create`) and points `lead.person_id` at that row.

## Neon Law funnel

The Neon Law funnel uses one identifier-only structured event family with the `funnel` target. Each event carries its
event name in the bounded `step` field. The five steps and their fields are:

| Event | Fields |
| --- | --- |
| `funnel.lead_captured` | `lead_id`, `brand`, `source_path`, `sms_consent` |
| `funnel.started` | `person_id`, `project_id`, `notation_id`, `service_id`, `brand` |
| `funnel.intake_complete` | `notation_id`, `project_id` |
| `funnel.review_entered` | `notation_id`, `project_id` |
| `funnel.sent` | `notation_id`, `project_id`, `channel` (`email` or `signature`) |

The matching `navigator.funnel.step` counter carries only the `step` attribute, so Dash0 can chart conversion and
drop-off without querying event logs. The workflow-service events are emitted from journaled Restate side effects, so a
replay reuses the recorded side effect instead of incrementing the counter or writing a duplicate event. The signature
send path uses its existing idempotent request record and emits only after the provider succeeds.

## Browser sign-in

Browser OAuth callbacks emit one `auth.signed_in` event after a session is created, with `person_id`, `provider`
(`google`, `microsoft`, or `apple`), `brand`, and `first_link`. A callback that cannot resolve an admitted Person, or
whose token fails verification, emits `auth.sign_in_refused` with `provider`, `brand`, and one of
`no_subject_match_no_email`, `email_unmatched`, `not_admitted`, or `token_invalid`. The matching
`navigator.auth.sign_in` counter carries only `provider` and `outcome` (`signed_in`, `refused`, or `failed`). A store
failure during Person resolution remains an HTTP 500 and emits `auth.sign_in_failed` with `provider`, `brand`, and the
bounded `error_class=store`. These events and logs carry identifiers and bounded values only: no email, name, address,
provider subject, or raw store error.

A sign-in that converges a Person row written before sign-in identifiers were split per provider also emits
`auth.legacy_subject_relinked` with `provider` and nothing else. It is a migration signal, not a sign-in outcome, so it
increments no counter and the sign-in that triggered it emits its own `auth.signed_in`. Each affected row can produce it
at most once — see ["Legacy convergence"](oidc.md#legacy-convergence) — so the event falling silent everywhere is the
condition ENG-783 waits on before removing the branch that emits it.

The GET and form-post callbacks share one completion path. The pre-auth cookie is consumed before token processing, and
the event is emitted once at the session-creation or refusal boundary. `first_link` is true when the sign-in creates a
Person or links the presenting provider to an existing Person, and false on a repeat login through that provider.

The key set emitted by every visit, funnel, and sign-in recorder is pinned to the collector's fail-closed allow-list by
`cli/tests/audit_fields_exported.rs`.

## Where it lands: direct OpenObserve

Traces, metrics, and logs speak OTLP/gRPC directly to the OpenObserve organization and stream named in their
environment. There is no collector hop and therefore no shared collector credential or backend fan-out to operate. The
telemetry contract is deliberately fail-closed at the source: the shared subscriber rejects a log event before either
the stdout formatter or the direct OTLP log bridge sees it when a value is an email address, phone number, government
identifier, document/body-like string, or an explicitly content-bearing field. Approved opaque identifiers and bounded
enum/status fields remain unchanged. This control applies to audit and non-audit events alike; OpenObserve's stream
retention and access policy is a separate deployment control, not the privacy boundary.

## Where it lands: collector fan-out

The `examples/deploy` process path uses the plain collector contract: binaries send OTLP/gRPC to the in-cluster
collector Service without OpenObserve credentials. The collector runs the existing `memory_limiter`, resource detection,
fail-closed `redaction`, and `batch` processors before the exporters. Traces also retain tail sampling. Dash0 is an
optional, staging-only integration declared by a nonblank `DASH0_ENDPOINT` in the selected deployment row's `[env]`
coordinates. A row without that endpoint need not carry `DASH0_DATASET` or `DASH0_TOKEN`; the deployment plan reports
the token as `integration not declared by this deployment` and `ops ship` removes it from that row's
`SecretProviderClass`. When the endpoint is present, `DASH0_DATASET` must also be a nonblank coordinate and
`DASH0_TOKEN` must be present in the encrypted Secret Manager input. The deployment, Secret Manager, and ship gates
refuse a missing value by name, so a half-configured row cannot be silently rendered as a Google-only pipeline. With all
three values present, the renderer substitutes the endpoint and dataset and includes `otlp/dash0` alongside
`googlecloud` in all three pipelines. The token remains a `secretKeyRef` and never enters application arguments or
committed plaintext.

The collector's own metrics (`otelcol_exporter_sent_*`, `otelcol_exporter_send_failed_*`, `otelcol_processor_dropped_*`,
…) reach the same fan-out as every other signal: a `prometheus/self` receiver scrapes the collector's own `:8888`
`telemetry.metrics` endpoint into the `metrics` pipeline, so exporter health redacts, batches, and exports to
`googlecloud` and `otlp/dash0` exactly like application metrics, instead of reaching Cloud Monitoring (via Google
Managed Prometheus's separate external scrape of the same port) only. `exporter` is on the allow-list for this reason —
it names a collector component (e.g. `otlp/dash0`), never client data.

The collector exporter uses OTLP/gRPC with `Authorization: Bearer …` and a `Dash0-Dataset` header. The transport and
header names are inferred from the repository's OTLP/gRPC seam and the implementation brief; confirm the account's exact
endpoint and header contract before rollout. The account is time-boxed, so the operator must choose the environment and
complete the configuration before relying on a live export. The existing staging direct OpenObserve contract remains
available and unchanged.

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
