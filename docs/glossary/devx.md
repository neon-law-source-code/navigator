---
title: "`devx`"
description: >-
  The developer-environment orchestration for this workspace, part of the navigator CLI (the cli crate), implemented in
  the cli/src/devx/ module — there is no separate devx crate or binary.
---

The **developer-environment orchestration** for this workspace, part of the `navigator` CLI (the `cli` crate),
implemented in the [`cli/src/devx/`](../../cli/src/devx/) module — there is no separate `devx` crate or binary. Brings a
complete dependency stack up inside a local KIND cluster — SurrealDB, Garage, Rauthy, Restate (operator-managed),
embedded Rego, plus the `workflows-service` Restate worker — opens host port-forwards and writes `.devx/env`. That file
has the connection details the host-side `cargo run -p neon` needs.

```bash
cargo run --release -p cli -- dev up      # bring it all up
set -a; source .devx/env; set +a       # connection env vars
cargo run -p neon                       # host-side web on :3001
cargo run --release -p cli -- dev down    # tear it all down
```

Subcommands:

- `dev up` — KIND + nginx-ingress + Restate Operator + every dep + workflows-service + port-forwards + env file. The
  `web` binary is left for the host to run.
- `dev down` — kill port-forwards and delete the KIND cluster. `dev env`, `dev status` — print the env file / show
  whether port-forwards are alive. `dev kind up`, `dev kind down` — just the cluster + ingress + Operator (no
  application manifests). `dev deploy` — full in-cluster stack including `navigator-web`. It idempotently sets the
  cluster up, **pulls** the published service images (`NAVIGATOR_IMAGE_TAG` or the latest `YY.M.D`), retags them to
  `:dev`, `kind load`s, applies every manifest, waits for the navigator-web rollout. CI builds the images; the local
  loop no longer builds them.
- `dev undeploy` — `kubectl delete namespace navigator`. `dev worktree-env up/down/status` stands up or tears down a
  per-worktree KIND environment (its own dependencies, Restate journal, `navigator` database, and host ports; `--branch`
  branches a supplied checkout in place or creates a sibling worktree; `--demo` runs the in-cluster stack). `dev e2e`
  smoke-tests rollouts, `/health`, embedded Rego decisions, and local seed counts. `dev grant-lawyer` pre-seeds the
  Lawyer demo user for browser e2e with the `lawyer` role. `ops ship` — one-shot production roll that pins service
  deployments to a named `--tag` and re-registers. `dev logs` — tails navigator-web logs.

The workspace has no Makefile — the `navigator` CLI is the only entry point.
