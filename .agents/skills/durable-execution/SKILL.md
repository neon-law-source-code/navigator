---
name: durable-execution
description: >
  The operational contract for Neon Law Navigator's Restate-backed durable execution — how to keep the running workflows
  alive, diagnose why one didn't fire, and not break them. Covers the submit-vs-run split (workflows lib /
  workflows-service bin), the service inventory, the six-hourly Heartbeat liveness canary, the three start modes, and
  how to investigate a stalled workflow by following the evidence — commit history, GitHub Actions runs, pod and Cloud
  logs, and Google Cloud (Trace, GKE, the Restate console) — then pin the fix with a guard test so it can't recur.
  Trigger when touching workflows-service, a *-trigger CronJob, the image Containerfiles, when re-registering with
  Restate, when a scheduled or manual workflow "didn't run", or when adding a workspace crate (it must enter the
  Containerfile COPY lists). This engine executes BINDING legal artifacts (retainer dispatch, signatures, filings), so
  an outage is a diligence concern, not a backlog item. Debugging surfaces carry invocation ids and service names, NEVER
  client content. To *add* a new workflow use create-legal-workflow; this skill keeps the existing ones alive.
---

# durable-execution

Durable execution runs the firm's **binding obligations** — retainer dispatch, signatures, county / NV SoS filings. When
the engine silently stops, an engagement letter doesn't go out and a deadline can slip, so an outage is a
**competence-and-diligence** concern, not a backlog item. The **Heartbeat** is the auditable control: a six-hour gap
with no Slack notice is the alarm.

**Everything factual lives in the doc** — read [`docs/durable-workflows.md`](../../../docs/durable-workflows.md) and
keep it, not this skill, authoritative: the submit-vs-run split, the service inventory, the three start modes and
idempotency keys, the registration gotcha, the two-tokens/two-ports auth model, and the evidence-and-guardrails
playbook.

## How to treat it (the load-bearing rules)

- **Diagnose by evidence, not memory.** When one didn't fire, read what the system is telling you now — commit history,
  the GitHub Actions run, the logs, and Google Cloud — instead of guessing from a remembered failure.
  `navigator ops doctor` triages the cluster slice fast; confirm against the evidence.
- **Fix the root cause, then guard it.** Pin every fix with a test or manifest field, in the same PR, so the same outage
  can't recur — that is how the failure list shrinks instead of repeating.
- **Adding a workspace crate?** Add it to every workspace-building Containerfile's COPY list or the next trigger build
  takes every trigger image down (the `cli::devx` test guards this). Image builds belong to `deploy.yml`'s daily tag
  flow.
- **Debugging stays identifier-and-status only — never client content** (the standing no-content rule).

## Boundaries

- *Add* a new workflow (template + questionnaire + handlers): [[create-legal-workflow]]. *Author* a Restate handler
  without breaking durability (the `ctx.run` rule): [[restate-rust]]. Telemetry / the trigger metric / BigQuery:
  [[observability]].
