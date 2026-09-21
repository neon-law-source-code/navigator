---
name: kind-local-dev
description: >
  Local Kubernetes-in-Docker (KIND) workflow for the navigator workspace — cluster lifecycle, ingress, port- forwarding,
  the "host runs `web`, deps run in cluster" iteration pattern via the `navigator` CLI. Trigger when running any
  `navigator` orchestration subcommand (`dev up`, `dev down`, `dev deploy`, `dev kind up`, `dev kind down`, `dev e2e`,
  `dev logs`, `dev worktree-env`, `dev serve`), editing `k8s/kind-config.yaml`, debugging an in- cluster service from
  the host, or onboarding the cluster from a fresh machine. Also trigger before installing a different local-Kubernetes
  flavor — we standardize on KIND. Also trigger before proposing to run, preview, screenshot, or manually exercise the
  app, or before local runtime orchestration. Follow the current AGENTS.md authorization and worktree-runtime contract.
---

# KIND-based local development

The `navigator` CLI (`cli::devx`) drives both the "host runs `web`" developer loop and the "full stack in KIND"
CI-shaped flow — there is no Makefile. This skill is the authority for that loop. Docker, KIND, `kubectl`, Helm, and the
Restate CLI must be installed and `docker info` must succeed. Do not add shell-script wrappers or substitute one-off
local containers for the Kubernetes topology. In-cluster Store specifics are in
[`docs/test-database.md`](../../../docs/test-database.md); the cluster config and its `extraPortMappings` are in
[`k8s/kind-config.yaml`](../../../k8s/kind-config.yaml).

## Worktree-first

Every code change starts in a **New Worktree** in Codex or Claude. A worktree is the isolated task checkout; a topic
branch is still the PR's Git reference, and they are complementary rather than alternatives. Codex starts its worktrees
at a detached `HEAD`; Claude may create a branch with its worktree. That difference is normal.

Before the first edit, inspect `git worktree list --porcelain`: the current `pwd -P` must be a non-primary `worktree`
entry. If it is not, do not create a branch, a worktree, or edit files — stop and say so. Never repair that mistake by
hand-creating another checkout.

A worktree opens at whatever `main` pointed to when it was created, and that tip is usually stale by the time the work
is ready to push. Fetch and rebase onto `origin/main` before the first edit and again before every push. Rebase rather
than merge — PRs squash and merge commits are disabled — and sign it, because an unsigned commit cannot enter the merge
queue.

```bash
git fetch origin
git rebase -S origin/main
```

Then run the CLI once with the PR topic. It attaches or creates that branch **in this worktree**, including a detached
Codex one; it creates a sibling `.worktrees/<topic>` checkout only when deliberately started from the primary checkout
outside Codex or Claude.

```bash
cargo run -p cli -- dev worktree-env up --branch <topic>
```

## The default worktree loop

Once the task branch exists, use its isolated dependency tier when editing `web`:

```bash
cargo run -p cli -- dev worktree-env up --path "$PWD"
set -a; source .devx/env; set +a
cargo run -p neon
```

`worktree-env up` creates or reuses a cluster keyed to the worktree path under `--runtime kind`, applies the SurrealDB
schema, assigns a stable port slot, and writes `.devx/env` plus `.devx/worktree.json`. The native lane (`--runtime
native`) keeps that descriptor and slot contract for the per-worktree Restate and web processes, while one host-level
registry owns the shared SurrealDB, Rauthy, and Garage processes; each native worktree gets its own database and Garage
bucket/key set, and the registry records process identity and worktree claims so teardown and sweep cannot signal a
recycled or live unrelated process.

Host `web` and its worker therefore share one store and one Restate journal per worktree, while parallel worktrees do
not. Source the generated environment before every local command that must target this checkout — it is the complete
local application environment, and a gitignored `.env` is only for optional live third-party sandbox credentials.

```bash
cargo run -p cli -- dev worktree-env status --path "$PWD"
cargo run -p cli -- dev worktree-env down --path "$PWD"
```

`worktree-env down` removes this checkout's port-forwards, cluster or native tenants, and `.devx` state, and never
touches another worktree's. Run it at handoff: a cluster left behind keeps binding its slot's ports, and a native claim
left behind keeps its tenant alive until `sweep` can identify it as orphaned.

## The shared dependency tier

`dev up` owns the dependency tier directly and binds `web` to the default port rather than a derived one:

```bash
cargo run --release -p cli -- dev up
set -a; source .devx/env; set +a
cargo run -p neon
```

It deploys SurrealDB, Rauthy, Garage, Restate, `workflows-service`, and telemetry in KIND, restores the host
port-forwards, and writes `.devx/env`. Re-run it after sleep or reboot to re-arm dead port-forwards; it reuses the
cluster. Restart the compiled `web` process after changing routes, handlers, views, or content.

| Surface | Shared tier port |
| --- | --- |
| Restate ingress | `9080` |
| Restate admin | `9070` |
| Rauthy | `30080` |
| Garage | `30900` |
| SurrealDB | `18000` |
| `web`, default brand | `3001` |
| `web`, `delete-your-data` | `3011` |
| `web`, lawyer-shook | `3021` |

Locally there is no DNS standing in for those brand hostnames, so `web` answers them on their own ports instead of a
`Host:` header. A worktree instead selects a free slot in the `20000`–`21299` ranges — including slots held by stopped
or orphaned clusters, so worktrees never share one — and its three brand ports move together with that slot. Always read
the selected values from that worktree's `.devx/env`.

SurrealDB is the store. Its connection contract is `NAVIGATOR_SURREAL_ENDPOINT`, `_NAMESPACE`, and `_DATABASE`, written
into `.devx/env`, and its schema is applied rather than migrated: one idempotent `DEFINE` file
(`store/src/schema/navigator.surql`) plus a `schema_version` record.

## Verification

Run the browser and accessibility gate after starting `web` with the correct `.devx/env`. The command downloads and
caches the pinned Chrome for Testing build, starts ChromeDriver on a free port, grants Lawyer, and runs both suites with
`NAV_REQUIRE_HARNESS=1`. It reads the base URL from the sourced environment; override it only when driving a topology it
did not generate.

```bash
cargo run -p cli -- dev browser-e2e
cargo run -p cli -- dev browser-e2e --base-url http://localhost:3001
```

For a full in-cluster demo from published images, run `cargo run -p cli -- dev worktree-env up --demo`, optionally with
`--tag YY.M.D`; the ingress is at `http://localhost:8080`.

## Troubleshooting and cleanup

```bash
kubectl --namespace navigator get pods
kubectl --namespace navigator describe pod <name>
kubectl logs --namespace navigator <name> --all-containers --tail=100
```

Leave the ordinary `dev up` tier running between sessions. Run `navigator dev worktree-env down` at handoff to remove a
worktree's task-owned cluster and port-forwards. Never prune Docker volumes without explicit approval, and use `cargo
run --release -p cli -- dev down` only for a deliberate clean rebuild — it deletes the ordinary cluster. Cleanup details
live in [`docs/agent-workflows.md`](../../../docs/agent-workflows.md#resource-cleanup).

On Codex-provisioned macOS worktrees, Gatekeeper may open repeated `Verifying "<binary>"…` windows the first time each
freshly built executable runs. That is a host-side property, not a Navigator defect, and needs no action: the sandboxed
execution host stamps `com.apple.provenance` on every executable its Cargo/rustc process writes, and Gatekeeper scans
such binaries once on first launch. Do not strip the attribute, disable Gatekeeper or SIP, notarize ephemeral test
binaries, or wrap Cargo in a shell script — each either weakens macOS security or cannot reach the first binary anyway.
Builds are unaffected. Tracked in [navigator#570](https://github.com/neon-law-source-code/navigator/issues/570).

## How to treat it (the load-bearing rules)

- **Use the CLI's watching server for interactive previews.** Reuse the configured worktree environment, source
  `.devx/env`, then run `cargo run -p cli -- dev serve`. Rust and catalog saves rebuild and restart the server; static
  assets refresh the browser without compilation. Keep it running through the edit-and-check loop and use the assigned
  brand port from `.devx/env`. Follow [web-preview](../web-preview/SKILL.md) to verify automatic refresh and the page.
- **We standardize on KIND.** Before installing another local-Kubernetes flavor (minikube, k3d, Docker Desktop k8s),
  stop — the manifests, port mappings, and `cli::devx` orchestration all assume KIND.
- **The in-cluster store is ephemeral by design.** SurrealDB runs memory-backed and fake-gcs-server uses `emptyDir`, so
  restarting their pod wipes the database and bucket — every developer starts from the same blank state and "works on my
  machine" drift can't accrue. Persistence in dev is a non-goal; production shape is a hosted SurrealDB, pointed at via
  `NAVIGATOR_SURREAL_ENDPOINT`.
- **`web` reaches the in-cluster store over a host port-forward.** The host-side `dev serve` process needs the
  environment that `dev up` writes to `.devx/env`: `set -a; source .devx/env; set +a` — the port-forward
  (`127.0.0.1:18000`) plus that env block is the bridge. "Ready cluster but `web` can't connect" is almost always the
  un-sourced env.
- **Screenshots go to `/tmp`, never the repo** — `/tmp/navigator-screenshots/` (`mkdir -p` first). The working tree
  stays clean.

## Anti-patterns

- Editing `k8s/` manifests for a one-off debug flag and forgetting to revert — use `kubectl patch` or a kustomize
  overlay for a transient change.
- Expecting the host cluster and CI's KIND cluster to behave identically — the host has whatever you `kind load`ed, CI
  starts clean.
- Using `:latest` image tags — tag with a content hash or `YY.MM.DD` so `kind load` + rollout are deterministic.
- `kubectl delete pod` to "fix" a `CrashLoopBackOff` — that hides the failure mode; read `kubectl logs --previous` then
  `kubectl describe pod` instead.

## Boundaries

- The browser half of the local loop (drive Chrome, screenshot, verify a UI change): [[web-preview]].
- OPA authz, Rauthy/OIDC, and Restate durable execution each have their own skill — this one owns the cluster, not the
  service.
