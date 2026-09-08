# DevX API

GitHub engineering automation is a singleton platform capability. Although the same Navigator image ships to every
persistent environment, only `neon-law-stg` mounts the GitHub receiver and registers `DevxIssueTriage` and `devx-pr`
with Restate. The guard is code, keyed to `NAVIGATOR_GCP_PROJECT_ID`, rather than a deployment convention: another
environment with copied credentials cannot consume the App's one webhook stream or create a competing global budget
counter.

The DevX API turns a GitHub request into an isolated engineering job. Its receiver and worker use the normal services;
each job gets a runner container so tools and Rust cache cannot leak between invocations.

## Shared-cluster guardrails

The authoritative worker requires three positive limits: `NAVIGATOR_GITHUB_MAX_CONCURRENT`,
`NAVIGATOR_GITHUB_MAX_REVISE_ROUNDS`, and `NAVIGATOR_GITHUB_MAX_DAILY_TOKENS`. Its `devx-guardrails/global` virtual
object serializes every reservation. It keeps only invocation identifiers and counts: duplicate reservations do not
spend twice; the concurrent cap defers new work; and an over-budget attempt pauses new token reservations until the next
UTC day. An active invocation keeps its concurrency slot across that rollover.

The authority-only `POST /devx-guardrails/global/status` Restate handler exposes a count-only operator projection: UTC
budget day, spent and remaining tokens, active-invocation count, pause condition, and configured caps. It never exposes
the invocation identifiers retained for reservation idempotency. `navigator ops doctor` reads that projection at the
automation home, warns when the daily budget has paused new work or the object is unavailable, and states explicitly
when another deployment is correctly non-authoritative. It remains read-only: no current notification workflow opens a
PR or invokes an agent.

`GitHubAutomationHeartbeat` is a separate six-hourly authority canary that runs only in `neon-law-stg`; it starts one
body-free Restate workflow keyed by its UTC six-hour slot. The worker journals `beat` and posts GitHub automation
authority OK through the ops notifier. The notice proves authority-only service registration, durable worker, and
notifier path; it deliberately does not receive a webhook, read repository content, mint a token, or invoke an agent.
Configure its dedicated export manifest only at the automation home with `RESTATE_INGRESS_URL` and the normal
`RESTATE_AUTH_TOKEN`; it reuses the pinned `navigator-heartbeat-trigger` image. A missing signal is investigated with
the same Restate registration and ingress checks as the general `Heartbeat` canary.

## Runner image inventory

`navigator-runner` is amd64-only. The integration-gated tag flow publishes it to GHCR (`ghcr.io/neon-law-source-code`)
with the release tag and `latest`; `.github/workflows/ghcr-retention.yml` owns its retention. It carries the pinned Rust
toolchain, Chrome/ChromeDriver pair, Node LTS, Claude Code CLI, `cargo-llvm-cov`, and a built `navigator` CLI with
dependency-warm Cargo layers.

Its dependency-first Cargo recipe warms the workspace target cache without making cache bytes authoritative.

At job start, the runner controller must fetch the exact immutable ref requested for that invocation into the job
worktree before any command runs. The baked `/workspace` tree is only a warm cache; a job must never execute its baked
source as a substitute for the requested ref.

## Agent harness and prompts

`github_webhooks::harness::AgentHarness` is the runner boundary. Durable workflows and their tests depend on its typed
task and outcome contract. The runner's `ClaudeCodeHarness` invokes the pinned `claude` binary in print mode with
stream-JSON output and the task-selected model. It selects Vertex AI with `CLAUDE_CODE_USE_VERTEX=1`,
`ANTHROPIC_VERTEX_PROJECT_ID`, and `CLOUD_ML_REGION`; Workload Identity remains the only credential source. The harness
reduces local process output to a typed outcome, rejects malformed results, and fails closed when the returned turn or
token usage exceeds the task limit. `StubHarness` returns scripted outcomes so workflow tests never make model calls or
consume tokens.

The versioned agent instructions in `github_webhooks/agent_instructions/` define the three engineering actions: issue
triage, implementation, and pull-request revision. Their shared preamble makes the headless, no-pause, prompt-injection
boundary explicit. Prompt frontmatter selects the shipped model and turn limit; the optional
`NAVIGATOR_GITHUB_MODEL_TRIAGE`, `NAVIGATOR_GITHUB_MODEL_IMPLEMENT`, and `NAVIGATOR_GITHUB_MODEL_REVISE` deployment
values override only the model.

## GitHub App contract

The receiver and durable worker use Navigator's existing GitHub App credentials: `NAVIGATOR_GITHUB_APP_ID`,
`NAVIGATOR_GITHUB_APP_PRIVATE_KEY`, and the per-repository installation selected for each invocation. The App's webhook
secret is `NAVIGATOR_GITHUB_WEBHOOK_SECRET`; GitHub sends subscribed deliveries to `POST /webhooks/github/{secret}` on
the public `workflows` host, served by `workflows-service`. The receiver verifies the raw-body HMAC and only then
submits identifier-only commands to Restate.

The App needs repository permissions `contents:write`, `issues:write`, `pull_requests:write`, `checks:read`,
`actions:read`, and `metadata:read`. It subscribes to issues, issue comments, pull-request reviews, pull-request review
comments, check runs, and workflow runs. These permissions let the worker retrieve scoped issue, review, and
failed-check content, but do not grant a merge bypass: the repository's human code-owner review and merge queue remain
authoritative.

The typed client mints a short-lived installation token for the invocation's repository. A runner receives that token
only through its isolated job credential URL; the URL type has no `Debug` or `Display` implementation and must never
enter telemetry, Restate keys, Slack notices, or durable workflow state.
