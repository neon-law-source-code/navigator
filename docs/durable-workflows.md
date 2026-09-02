---
publish: true
---

# Durable workflows

How Neon Law Navigator runs long-lived, crash-safe work — retainer intake, Drive sync, the nightly Archives backup — on
[Restate](https://restate.dev), and how an operator tells *why one didn't run*.

> **The one rule that costs two hours when forgotten:** a registered Restate deployment is a **snapshot, not a
  subscription**. Rolling a new worker image does **not** re-register it. A service you just added (or its new handlers)
  stays invisible at the ingress — `404 "service not found"` — until you **re-register the deployment**. See [The
  registration gotcha](#the-registration-gotcha).

> **The mental model:** **Kubernetes owns the clock; Restate owns the journal.** A trigger fires an invocation once;
  Restate makes its execution durable — journals every step, retries failures, runs it to completion on the worker.

## Two sides: submit vs. run

Durable execution is split across two crates so the rest of the workspace never binds to `restate-sdk`.

| | `workflows` (lib) | `workflows-service` (bin) |
| --- | --- | --- |
| Role | **Outbound** — *submit* a job | **Inbound** — *run* the handlers |
| Who calls it | `web` and the `archives` trigger | Restate dials *into* it |
| Runtime | `InMemoryRuntime` (dev/CI) or `RestateRuntime` | the worker itself |
| `restate-sdk` | no | **yes — the only crate with it** |
| Tested via | `wiremock` (exact HTTP shape) | `cargo test -p workflows-service` |

One worker pod hosts every service — new workflows bind onto the worker endpoint, never a new pod. Today that worker
serves three virtual objects — `notation` (questionnaire + workflow timelines on one journal), `devx-pr`
(per-pull-request notifications), and `devx-guardrails` (the GitHub-automation guardrail state) — and the durable
workflows `Archives`, `BillingCanary`, `BillingDigest`, `ReconcileInvoices`, `DriDigest`, `Heartbeat`,
`GitHubAutomationHeartbeat`, and `DevxIssueTriage`. The exact set is the single source of truth in
`workflows_service::registry`, whose tests assert every workflow name is PascalCase (template filenames follow the
separate snake_case convention `N103` enforces) and that the registry never drifts from the worker's actual `.bind(...)`
calls. The DevX Slack-notice services `DevxIssueTriage` and `devx-pr` are no exception: they bind into this same worker
— one worker, every service. They own the engineering Slack notifier and alone read `SLACK_WEBHOOK_URL`. In the
reference deploy the worker runs behind `workflows.your-domain.example`.

The runtime is chosen by `RESTATE_BROKER_URL`: unset means in-process / in-memory, so KIND works with zero config; set
means the `RestateRuntime` adapter posts to the broker over HTTP. The same selection is used in `portal::main` and the
`archives` trigger.

## Three ways a workflow starts

Every workflow is kicked off in exactly one of three ways. All three land on the worker that owns its trust domain.

| Mode | Fired by | Example | Code |
| --- | --- | --- | --- |
| **Event-driven** | `web`, on a user action | retainer intake; Drive sync | `portal::retainer_walk` |
| **Scheduled** | a Kubernetes `CronJob` | Archives; canary | `archives`/`billing-workflows` |
| **Manual** | a Cron schedules button | `POST /app/admin/schedules/:job/run` | `portal::cron_schedules` |

The submit shape is identical in all three: `POST {ingress}/{Service}/{key}/run` (append `/send` for one-way), with the
optional bearer. The shared helper is `workflows::start_workflow`.

## Where the schedule lives

**Restate has no cron.** The nightly schedule is a **Kubernetes `CronJob`** named `archives-trigger` in the `navigator`
namespace — stored in the cluster (etcd), evaluated by the kube-controller-manager, sourced from
`examples/deploy/k8s/exports/cron-archives-trigger.yaml` (`schedule: "0 10 * * *"`, UTC, = 02:00 PST). Each firing runs
the thin `navigator-archives-trigger` image — one `POST` to the ingress, then it exits. Restate owns the retry schedule
from there.

```text
kube CronJob (0 10 * * *) --fires--> trigger pod --POST /Archives/<date>/run/send--> Restate ingress
                                                                                          | Accepted
                                                                                          v
                                                          worker runs: snapshot -> cost -> notify (journaled)
```

To inspect or fire the schedule by hand:

```bash
kubectl -n navigator get cronjob archives-trigger
kubectl -n navigator create job --from=cronjob/archives-trigger archives-trigger-manual-001
```

## Idempotency is the workflow key

Restate admits **at most one invocation per workflow key**. The key choice *is* the idempotency policy:

- **Nightly Archives** keys on the **UTC run date**, so a double-fire on the same day is a silent no-op — exactly what
  a backup wants.
- **Manual runs** key on a unique `manual-<uuid>`, so every click actually executes and notifies — a test button that
  deduped against the nightly run would look broken.

## Auth: two tokens, two ports

The single most error-prone area: there are **two different credentials** on **two different ports**, and conflating
them is what silently broke prod.

- **Submit / trigger** an invocation — ingress, port **`:8080`**. Credential: `RESTATE_AUTH_TOKEN`, the Restate Cloud
  **`key_…`** API key (72 chars). Lives in the k8s secret `navigator-web-secrets`, sourced from the deployment's
  `secrets.enc.yaml` via Secret Manager.
- **Register** a deployment — admin API, port **`:9070`**. Credential: the **SSO access token** (a long JWT written by
  `restate cloud login`), in `~/.config/restate/config.toml` under `[global.cloud] access_token`.

The secret takes the **ingress `key_`**, never the SSO JWT — they look nothing alike. If `navigator-web-secrets` holds a
long `eyJ…` JWT, it is **wrong** and the ingress answers `401 Unauthenticated`. The deployment's
`deployments/<name>/secrets.enc.yaml` is the source of truth (see [deployment secrets](deployment-secrets.md)). We once
had this token drift across three places — the provider config's `key_`, Secret Manager `stub`, and a stale SSO JWT in
the k8s secret — which is the failure this section exists to prevent.

## The registration gotcha

Restate routes the ingress to **registered** services. Registration is a **snapshot of the worker's handler list at
register time** — it does not follow new deploys.

- In **KIND**, `cargo run -p cli -- ops restate register` wires the worker URL into the in-cluster broker, so the dev
  loop just works.
- In **Restate Cloud**, registration is an **explicit admin operation**. Rolling a new worker image does **not**
  re-register, so a service or handler added since the last registration is invisible at the ingress:

```text
POST :8080/Archives/<key>/run/send
404 {"message":"service 'Archives' not found, make sure to register the service before calling it."}
```

**Fix — re-register the deployment** (re-runs discovery against the live worker and picks up every service). Either use
the Restate Cloud console (your env, Deployments, Register deployment, overwrite the existing endpoint), or the admin
REST API authenticated with the SSO token:

```bash
ADMIN="https://<env>.env.<region>.restate.cloud:9070"
TOK=$(sed -n 's/^access_token = "\(.*\)"/\1/p' ~/.config/restate/config.toml | head -1)
# dry-run first — confirm the discovered service list before committing:
curl -s -X POST "$ADMIN/deployments" -H "Authorization: Bearer $TOK" \
  -d '{"uri":"https://workflows.your-domain.example/","force":true,"dry_run":true}' | jq '.services[].name'
# then commit (drop dry_run):
curl -s -X POST "$ADMIN/deployments" -H "Authorization: Bearer $TOK" \
  -d '{"uri":"https://workflows.your-domain.example/","force":true}'
```

The `restate` CLI is configured for the env but may report "Unable to connect" to `:9070` even when the host is
reachable; the admin REST API above works directly with the same SSO token.

### How `ship` re-registers (step 7d)

After rolling the service deployments, `ship` re-registers the worker so any handler added since the last registration
is reachable. Two design points:

- **It targets the real worker URL, not the placeholder.** The `navigator` CLI resolves the register target in
  precedence order: explicit `--url` → `NAVIGATOR_WORKFLOWS_URL` → derived `https://workflows.<brand.primary_domain>/` →
  the `workflows.example.com` placeholder of last resort. The derivation step exists because the 2026-06-10 ship had a
  domain configured but no explicit `NAVIGATOR_WORKFLOWS_URL`, fell through to the placeholder, and silently no-op'd the
  register. `ops ship` now passes the selected deployment's explicit `NAVIGATOR_WORKFLOWS_URL` from its `config.toml`;
  the derivation is the belt-and-suspenders default for an operator who hasn't set it.
- **It picks its transport from the environment.** When `RESTATE_ADMIN_URL` **and** `RESTATE_ADMIN_TOKEN` are both set
  (operator-session credentials exported for a headless run — never repository coordinates), `restate_register` POSTs
  `{"uri":<worker>,"force":true}` straight to the admin REST API — headless, needs no `restate cloud env configure`
  (which requires a TTY) and works with a non-expiring admin-scoped API key. Otherwise it shells out to the pinned
  `restate` CLI, which only reaches Restate Cloud when the operator has a selected environment in
  `~/.config/restate/config.toml` and a fresh SSO token; with neither, the CLI defaults to the `local` environment
  (`localhost:9070`) and the step fails. Wiring the two env vars is what makes an unattended ship re-register reliably.
- **It warns and continues — it does not gate the ship (yet).** A `:9070` admin endpoint that is firewalled from the
  operator's network, or an expired SSO token, would otherwise block every ship. Re-register is therefore best-effort: a
  failure prints `WARN: Restate re-register failed (continuing)` and the ship completes. The cost of warn-and-continue
  is a silent `404` on a *newly added* service until someone re-registers by hand; the `Notation` (retainer) service is
  already registered and unaffected. Flip step 7d to fail-the-ship only once `:9070` reachability is proven from the
  ship host and an **admin-scoped** token (`RESTATE_ADMIN_TOKEN` + `RESTATE_ADMIN_URL`, exported by the operator for a
  headless run) is wired — until then a hard gate trades a rare silent 404 for a frequent blocked ship.

## Adding a workflow

1. Author the spec in a notation template's `workflow:` frontmatter (see [notation authoring](notation-authoring.md))
   or, for non-notation flows, bind a new Restate service in `workflows-service`.
2. Signal it from `web` (event-driven) or add a trigger (scheduled / manual).
3. Ship the worker — see [GKE production](gke-prod.md) and [cloud operations](cloud-operations.md). Always ship
   `navigator-web` and `workflows-service` at one SHA; the DevX Slack services `DevxIssueTriage` and `devx-pr` now bind
   into `workflows-service` too.
4. **Re-register the deployment** (above) — otherwise the new service `404`s at the ingress no matter how clean the
   deploy was. `ship` registers `workflows.<domain>`. This step is invisible in `kubectl` and easy to forget.

## The heartbeat: proving the engine itself is alive

Every other scheduled workflow proves an *integration* — `Archives` proves the database and GCS are reachable,
`BillingCanary` proves Xero still agrees with us. None answers the bluntest operator question: *is durable execution
itself alive right now?* A silent `Archives` is ambiguous (engine down, or just a GCS outage?).

`Heartbeat` removes the ambiguity. It is a two-step Restate workflow (`beat` → `notify`) that depends on **nothing** —
no database, no object storage, no third-party API — so a green run can only mean the engine accepted an invocation,
journaled step one, and ran step two to completion. It fires **every six hours** (`0 */6 * * *` UTC), keyed on the UTC
date + hour so the four daily runs each get a distinct workflow key (a date-only key would dedupe three of four into
no-ops). Each run posts firm ops a **single line** to the engineering Slack channel — a heart glyph, `Durable execution
OK`, and the beat timestamp — straight through the Slack notifier with no email framing. The recurring message stays a
terse liveness ping; the operator runbook for debugging a *missing* beat is the chain below, not the message itself.
(Ops notices go to Slack only; the duplicate email was dropped once Slack proved itself.)

The signal that matters most is the missing one: **a six-hour window with no heartbeat notice in Slack means the engine
may be down** — walk the chain below. Like every new service, `Heartbeat` is invisible at the ingress until the
deployment is **re-registered** (see [the registration gotcha](#the-registration-gotcha)); the absence of its first
notice after a ship is itself the test that re-register happened.

## Debugging "the workflow didn't run"

Work down the chain; the break is almost always near the top:

1. **Did the trigger fire?** `kubectl -n navigator get cronjob archives-trigger` (last schedule) plus the trigger pod
   logs. For manual: did the admin button return the confirmation page or an error?
2. **Did the ingress accept it?** A `401` is a wrong or stale `RESTATE_AUTH_TOKEN`; a `404 service not found` is the
   registration gotcha — both have dedicated sections above.
3. **Did the worker run it?** Check the invocation in the Restate Cloud console (Invocations) or via the admin API; a
   failing step retries and surfaces there.
4. **Did the side effect happen?** Email transmits through SendGrid only when `NAVIGATOR_EMAIL_BACKEND=sendgrid` and
   `SENDGRID_API_KEY` are present; otherwise the worker silently uses a capturing backend that logs "sent" without
   sending. See [cloud operations](cloud-operations.md) for manifest-drift notes.

## When a workflow didn't run, follow the evidence

Don't pattern-match against a catalog of past outages — read what the system is telling you now, and trace the request
through the stages above. Four sources, in the order that usually pays off:

- **Commit history.** `git log` / `git blame` on `workflows-service`, the `*-trigger` manifests, and
  `images/Containerfile.*`. A durable-execution break is almost always a recent diff, not spontaneous — find what
  changed since the last run that worked.
- **GitHub Actions.** The `ci` and `deploy` runs (`gh run list`, `gh run view <id> --log`). Did the worker and trigger
  images actually build and publish, and did the deploy job succeed? A red or skipped deploy explains a stale image
  faster than any cluster probe.
- **Logs.** The trigger pod and worker logs (`kubectl -n navigator dev logs ...`) for the live failure; Cloud Logging /
  BigQuery for history. The worker prints its backends and per-invocation status; the trigger logs whether the ingress
  accepted the call.
- **Google Cloud.** Cloud Trace for the invocation's spans (`web` → ingress → handler), GKE workload state, and the
  Restate Cloud console's Invocations view for which step failed and retried.

`navigator ops doctor` is a shortcut for the cluster slice — it surfaces wedged trigger Jobs and unready workloads in
plain language. Triage with it, then confirm against the evidence above.

## Guardrails: keep a fixed bug fixed

We don't re-litigate the same outage. Every failure we've actually hit is pinned by a test or a manifest field so it
can't silently recur; when you fix a new one, add its guard in the same PR.

- **Every workspace member is in every workspace-building Containerfile.** Add a crate → add `COPY <crate> <crate>` to
  `images/Containerfile.{trigger,workflows-service,web}`. Guarded by
  `every_workspace_staging_containerfile_copies_every_member` in `cli/tests/containerfile_docs_copy.rs`.
- **Registered workflow names are PascalCase** and the registry matches `main.rs`'s `.bind(...)` calls — guarded by the
  `workflows_service::registry` tests (template filenames follow the separate snake_case convention `N103` enforces).
- **Trigger CronJobs carry `activeDeadlineSeconds` + `startingDeadlineSeconds`** so a stuck Job can't wedge `Forbid`,
  and `start_workflow` has a 30s HTTP timeout so a hung ingress can't keep a trigger pod alive.
- **Host `web` and the worker share one database inside their KIND tier.** The Restate worker journals each transition
  to the store its own `NAVIGATOR_SURREAL_*` names — `navigator` in that cluster. Both `dev up` and `dev worktree-env`
  put host `web` on the worker's database ([AGENTS.md](../AGENTS.md#default-worktree-loop)), so every Restate-backed
  flow (the retainer walk, contract review) reaches the worker. Point host `web` at any other database and a Notation it
  commits is invisible to the worker, and the flow 500s with `journal: RecordNotFound`. The `matter_open_restate`
  regression in `server/tests/` proves the flow reaches the matter when the databases agree. There is currently no
  automated guard comparing host `web`'s and the worker's databases at `worktree-env status` or `ops doctor` time; a
  mismatch surfaces only as the `journal: RecordNotFound` 500 above.
- **Debugging stays identifier-and-status only — never client content** (the standing no-content rule; see
  [observability](observability.md)).

## See also

- The *what* of each individual workflow: [notation](notation.md), [retainer intake](retainer_intake.md).
- Scheduling any periodic job (the CronJob pattern, both flavors): [Scheduled jobs](cronjobs.md). Deploy and secret
  mechanics: [GKE production](gke-prod.md), [deployment secrets](deployment-secrets.md). Crate entry points:
  [`workflows/README.md`](../workflows/README.md) and [`workflows-service/README.md`](../workflows-service/README.md).
