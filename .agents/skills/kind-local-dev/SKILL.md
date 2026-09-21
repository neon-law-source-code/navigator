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
CI-shaped flow — there is no Makefile. This skill is the authority for that loop. The facts it does not restate live
where they are executable rather than described: the cluster config and `extraPortMappings` in
[`k8s/kind-config.yaml`](../../../k8s/kind-config.yaml), and the port-forward table, the registry pull/retag/`kind load`
image flow, the per-worktree environment, and the teardown in `cli::devx` — read them through `cargo run -p cli -- dev
--help`. In-cluster Store specifics are in [`docs/test-database.md`](../../../docs/test-database.md).

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
