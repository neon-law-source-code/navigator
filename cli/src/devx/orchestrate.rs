//! Machine-bound devx orchestration.
//!
//! Every function here drives KIND, Kubernetes, Docker, or a host process:
//! it shells out to `kind`/`kubectl`/`helm`/`docker`, spawns and reaps
//! port-forward children, probes host TCP ports, or reads and writes the
//! `.devx/` state directory. None of it is unit-testable — it is exercised
//! by the live `deploy.yml` / `dev up` gates against the persistent KIND
//! fixture, which is why the coverage gate in `ci.yml` excludes this file.
//! The pure/testable logic (`KindConfig` derivation, env and kind-config
//! rendering, `resolve_workflows_url`, the deploy-order guards) stays in the
//! parent module with its tests.
//!
//! `dispatch` and the sibling devx modules reach these functions through the
//! narrow `pub(super)` seam re-exported from `super`.

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use super::{
    garage, normalize_docker_arch, parse_terminating, render_env, render_kind_config,
    restate_crd_path, terminating_wedge_message, KindConfig, Terminating, DEFAULT_RAUTHY_HOST_PORT,
    GATEWAY_IMAGE, INGRESS_MANIFEST, RESTATE_CRDS, RESTATE_OPERATOR_VERSION, WEB_IMAGE,
    WORKFLOWS_SERVICE_IMAGE,
};

pub(super) fn up(cfg: &KindConfig) -> Result<()> {
    let root = workspace_root()?;
    up_in(&root, cfg)
}

/// Bring up a KIND dependency tier whose state belongs to `root`.
///
/// `dev worktree-env --path` may target a checkout other than the CLI's
/// current directory, so the port-forward PID ledger must live with the
/// target worktree rather than whichever checkout invoked the command.
pub(super) fn up_in(root: &Path, cfg: &KindConfig) -> Result<()> {
    require_tools(&["kind", "kubectl", "docker", "helm"])?;
    let state = StateDir::new(root)?;

    kind_up_steps(root, cfg)?;
    // The worker stays in KIND so Restate discovers and registers it through
    // the same RestateDeployment as production. Build its image from this
    // checkout instead of consulting the private Artifact Registry: ordinary
    // local development must work with an empty `.env` and must not run a
    // worker from a different release than host-side `web`.
    build_and_load_worker(root, cfg)?;
    if super::private_mode_from_env() {
        build_and_load_gateway(root, cfg)?;
    }

    ensure_namespace(&cfg.namespace)?;
    garage::prepare(&cfg.namespace)?;
    retire_legacy_identity_provider(cfg)?;
    eprintln!("==> applying dependency manifests (skipping k8s/base/web)");
    apply_kustomize(root, &cfg.deps_overlay)?;
    // Before `wait_for_dep_rollouts` below, so the rollout this may trigger is
    // the one that wait already covers, and every port-forward opened after it
    // lands on the final pod.
    align_rauthy_public_url(cfg)?;
    wait_rollout("statefulset", "garage", cfg)?;
    hydrate_garage_environment(cfg)?;
    wait_for_dep_rollouts(cfg)?;

    eprintln!("==> opening port-forwards");
    state.kill_pids();
    let mut pids = Vec::new();
    let forwards = (|| -> Result<()> {
        // Restate lives in its own `restate` namespace (the Operator's
        // CR places the StatefulSet there). The other deps run in
        // `navigator`. Forward both Restate ports — ingress (8080) for
        // the workflow client, admin (9070) for `restate-cli` /
        // dashboard.
        pids.push(port_forward_two_in(
            "restate",
            "svc/restate",
            cfg.restate_ingress_port,
            8080,
            cfg.restate_admin_port,
            9070,
            &state,
        )?);
        pids.push(port_forward(
            "svc/clamav",
            cfg.clamav_port,
            3310,
            cfg,
            &state,
        )?);
        pids.push(port_forward(
            "svc/navigator-garage",
            cfg.garage_s3_port,
            3900,
            cfg,
            &state,
        )?);
        // SurrealDB, the store the workspace is porting onto (#1093).
        // The in-cluster worker reaches the same engine through the
        // Service; this forward is what lets host `web` join it.
        pids.push(port_forward(
            "svc/surreal",
            cfg.surreal_port,
            super::surreal::SERVICE_PORT,
            cfg,
            &state,
        )?);
        // OpenObserve serves its UI and direct OTLP gRPC listener from one
        // Rust process. Host `web` exports to the latter; the operator opens
        // the former.
        pids.push(port_forward_two_in(
            &cfg.namespace,
            "svc/openobserve",
            cfg.openobserve_port,
            5080,
            cfg.openobserve_otlp_port,
            5081,
            &state,
        )?);
        state.write_pids(&pids)
    })();
    if let Err(err) = forwards {
        StateDir::kill(&pids);
        return Err(err);
    }

    // Sanity check: probe each port the local `web` will use.
    wait_for_tcp("127.0.0.1", cfg.restate_ingress_port)?;
    wait_for_tcp("127.0.0.1", cfg.clamav_port)?;
    wait_for_tcp("127.0.0.1", cfg.rauthy_port)?;
    wait_for_tcp("127.0.0.1", cfg.garage_s3_port)?;
    wait_for_tcp("127.0.0.1", cfg.surreal_port)?;
    wait_for_tcp("127.0.0.1", cfg.openobserve_otlp_port)?;

    // The tier is reachable now, so the schema can be applied before
    // anything is told the environment is ready. Idempotent: a reused
    // cluster converges on the current definitions.
    super::surreal::apply_schema(cfg, "navigator")?;

    // Development always refreshes every sample matter's application before
    // the environment is written. The generated variable below then points the
    // host-side web process at this exact set of staged bundles.
    super::sample_project::run_for_root(false, root)?;

    state.write_env(&render_env(cfg, root))?;

    print_chrome_summary(cfg);
    Ok(())
}

/// Rebuild the current checkout's worker image and replace the running KIND
/// worker. This is the focused edit loop for `workflows-service` and shared
/// workflow code; host-side `web` edits do not need an image rebuild.
pub(super) fn reload_worker(cfg: &KindConfig) -> Result<()> {
    require_tools(&["kind", "kubectl", "docker"])?;
    let root = workspace_root()?;
    // Pin every kubectl call below to this checkout's cluster before touching
    // it. `build_and_load_worker` loads the image into `cfg.cluster` by name,
    // but the pod delete and readiness wait use whatever kubeconfig context is
    // ambient, so a stale or unrelated context would restart the wrong worker
    // and leave the rebuilt image unused. This mirrors how `up_in` pins its own
    // kubectl operations via `kind_up_steps`.
    configure_worktree_kubeconfig(&root, cfg)?;
    build_and_load_worker(&root, cfg)?;
    eprintln!("==> restarting workflows-service in {}", cfg.cluster);
    // Deleting the pod forces the RestateDeployment's ReplicaSet to recreate it
    // against the freshly `kind load`ed `:dev` image (the tag is unchanged, so
    // only a new pod picks it up under `IfNotPresent`). `--wait` is kubectl's
    // default, so this blocks until the outgoing pod is gone.
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(&cfg.namespace)
        .arg("delete")
        .arg("pod")
        .arg("--selector")
        .arg("app=workflows-service")
        .arg("--ignore-not-found"))?;
    // Gate on the *replacement* pod's readiness before trusting the
    // RestateDeployment CR: its `Ready` condition lags pod deletion, so waiting
    // on it alone can return against the pre-deletion state while the new pod is
    // still starting. Once the new pod is Ready, confirm the operator has
    // finished reconciling the deployment too.
    wait_for_selected_pod_ready(&cfg.namespace, "app=workflows-service")?;
    wait_for_condition(
        &cfg.namespace,
        "restatedeployment/workflows-service",
        "Ready",
    )
}

/// Build the worker from `root` and make its local `:dev` tag available to
/// every node of this KIND cluster. The manifest's `IfNotPresent` policy then
/// starts this exact source image without contacting a registry.
fn build_and_load_worker(root: &Path, cfg: &KindConfig) -> Result<()> {
    build_and_load_local_image(
        root,
        cfg,
        "images/Containerfile.workflows-service",
        WORKFLOWS_SERVICE_IMAGE,
    )
}

/// Build the private-mode Pingora gateway only when the selected overlay uses
/// it, then make its local tag available to every node of this KIND cluster.
fn build_and_load_gateway(root: &Path, cfg: &KindConfig) -> Result<()> {
    build_and_load_local_image(root, cfg, "images/Containerfile.gateway", GATEWAY_IMAGE)
}

fn build_and_load_local_image(
    root: &Path,
    cfg: &KindConfig,
    containerfile_path: &str,
    image: &str,
) -> Result<()> {
    let containerfile = root.join(containerfile_path);
    eprintln!(
        "==> docker build -f {} -t {image} {}",
        containerfile.display(),
        root.display()
    );
    run(Command::new("docker")
        .arg("build")
        .arg("--file")
        .arg(containerfile)
        .arg("--tag")
        .arg(image)
        .arg(root))?;
    kind_load_image_into_cluster(image, cfg)
}

/// Point Rauthy's canonical public URL at this tier's browser-reachable origin.
///
/// Rauthy uses one `PUB_URL` for discovery, token validation, and browser
/// redirects. Host-run web reaches the mapped `NodePort` directly; full KIND web
/// reaches the same issuer through [`align_rauthy_in_cluster_client`].
///
/// The Deployment keeps `valueFrom` references to this Secret. Patching the
/// Secret instead of replacing those entries with literal values preserves a
/// shape that a later `kubectl apply` can reconcile. Restarting the Deployment
/// then makes the new values visible to the process.
pub(super) fn align_rauthy_public_url(cfg: &KindConfig) -> Result<()> {
    let origin = super::rauthy_origin(cfg.rauthy_port);
    eprintln!("==> aligning Rauthy public URL → {origin}");
    let output = Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args(["patch", "secret", "rauthy-secrets", "--type=merge", "-p"])
        .arg(super::rauthy_public_url_patch(cfg.rauthy_port))
        .output()
        .context("patch Rauthy public URL Secret")?;
    if !output.status.success() {
        bail!(
            "patch Rauthy public URL Secret failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprint!("{stdout}");
    if stdout.contains("(no change)") {
        return Ok(());
    }
    run(Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args(["rollout", "restart", "deployment/rauthy"]))
}

/// Return whether this local cluster has already converged on Rauthy.
///
/// Worktree reuse normally skips the expensive dependency apply when every
/// host port is reachable. A pre-Rauthy cluster also satisfies that probe
/// because its identity provider owns the same `NodePort`, so the deployment
/// identity is the additional convergence signal.
pub(super) fn rauthy_deployment_exists(cfg: &KindConfig) -> bool {
    Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args(["get", "deployment", "rauthy"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Release the local `NodePort` and remove the retired dependency-tier objects
/// before applying Rauthy. This is idempotent and touches only the disposable
/// identity-provider fixture in the selected KIND cluster.
fn retire_legacy_identity_provider(cfg: &KindConfig) -> Result<()> {
    run(Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args([
            "delete",
            "deployment/keycloak",
            "service/keycloak",
            "configmap/keycloak-realm",
            "secret/keycloak-realm-admin",
            "ingress/keycloak",
            "--ignore-not-found",
        ]))
}

/// Align a full-stack `navigator-web` pod with Rauthy's single public issuer.
///
/// The `ConfigMap` changes the sidecar's loopback listen port; the Deployment
/// patch changes web's issuer and triggers the rollout that remounts that
/// `ConfigMap`. Deps-only host-web environments never call this path.
fn align_rauthy_in_cluster_client(cfg: &KindConfig) -> Result<()> {
    let issuer = super::rauthy_issuer(cfg.rauthy_port);
    eprintln!("==> aligning in-cluster Rauthy client → {issuer}");
    run(Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args([
            "patch",
            "configmap",
            "rauthy-loopback-proxy",
            "--type=merge",
            "-p",
        ])
        .arg(super::rauthy_loopback_config_patch(cfg.rauthy_port)))?;
    run(Command::new("kubectl")
        .args(["--context", &cfg.kind_context(), "-n", &cfg.namespace])
        .args([
            "patch",
            "deployment",
            "navigator-web",
            "--type=strategic",
            "-p",
        ])
        .arg(super::navigator_web_rauthy_patch(cfg.rauthy_port)))
}

pub(super) fn hydrate_garage_environment(cfg: &KindConfig) -> Result<()> {
    garage::export(&garage::provision(&cfg.namespace)?);
    Ok(())
}

/// `dev kind up`: just create the cluster + install ingress +
/// install the Restate Operator. Don't apply application manifests.
pub(super) fn kind_up_only(cfg: &KindConfig) -> Result<()> {
    require_tools(&["kind", "kubectl", "helm"])?;
    let root = workspace_root()?;
    kind_up_steps(&root, cfg)
}

/// `dev kind down`: delete the KIND cluster.
pub(super) fn kind_down_only(cfg: &KindConfig) -> Result<()> {
    require_tools(&["kind"])?;
    let cluster = &cfg.cluster;
    if cluster_exists(cluster)? {
        eprintln!("==> deleting KIND cluster '{cluster}'");
        run(Command::new("kind")
            .arg("delete")
            .arg("cluster")
            .arg("--name")
            .arg(cluster))?;
    } else {
        eprintln!("==> KIND cluster '{cluster}' not found; nothing to delete");
    }
    Ok(())
}

/// `devx deploy`: full in-cluster stack from published Artifact Registry
/// images. Pulls both images at a resolved immutable release tag, retags + loads them
/// into KIND, applies every manifest under `k8s/`, waits for the
/// navigator-web rollout to settle. CI builds and publishes the images;
/// this pulls them. `tag_override` (e.g. `worktree-env --demo --tag`)
/// pins the release; else `NAVIGATOR_IMAGE_TAG`, else the latest
/// published tag is pulled.
pub(super) fn deploy(cfg: &KindConfig, tag_override: Option<&str>) -> Result<()> {
    require_tools(&["kind", "kubectl", "docker", "helm"])?;
    let root = workspace_root()?;
    // `kind_up_steps` is idempotent — safe to call when the cluster
    // is already up. Establishes the Operator + nginx-ingress
    // invariants `deploy` relies on.
    kind_up_steps(&root, cfg)?;
    let image_registry = super::registry::registry_from_env();
    let tag = resolve_local_image_tag(&image_registry, tag_override)?;
    // Fail fast before any apply if either service image is missing the
    // resolved tag — a missing image would wedge the rollout in
    // ImagePullBackOff.
    super::registry::ensure_tag_published(&image_registry, "navigator-web", &tag)?;
    super::registry::ensure_tag_published(&image_registry, "navigator-workflows-service", &tag)?;
    if cfg.full_overlay == super::DEFAULT_KUSTOMIZE_KIND_PRIVATE {
        super::registry::ensure_tag_published(&image_registry, "navigator-gateway", &tag)?;
    }
    pull_retag_load(&image_registry, "navigator-web", &tag, WEB_IMAGE, cfg)?;
    pull_retag_load(
        &image_registry,
        "navigator-workflows-service",
        &tag,
        WORKFLOWS_SERVICE_IMAGE,
        cfg,
    )?;
    if cfg.full_overlay == super::DEFAULT_KUSTOMIZE_KIND_PRIVATE {
        pull_retag_load(
            &image_registry,
            "navigator-gateway",
            &tag,
            GATEWAY_IMAGE,
            cfg,
        )?;
    }
    garage::prepare(&cfg.namespace)?;
    retire_legacy_identity_provider(cfg)?;
    apply_kustomize(&root, &cfg.full_overlay)?;
    align_rauthy_public_url(cfg)?;
    align_rauthy_in_cluster_client(cfg)?;
    wait_rollout("statefulset", "garage", cfg)?;
    garage::provision(&cfg.namespace)?;
    eprintln!("==> waiting for navigator-web rollout");
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(&cfg.namespace)
        .arg("rollout")
        .arg("status")
        .arg("deployment/navigator-web")
        .arg("--timeout=300s"))
}

/// `devx undeploy`: kubectl delete namespace navigator. Does NOT
/// touch the cluster — use `dev kind down` for that. The `--context` pin is
/// load-bearing: this deletes a whole namespace, and without it the delete
/// lands on whatever context is current, which on an operator's machine is as
/// likely to be prod as KIND.
pub(super) fn undeploy(cfg: &KindConfig) -> Result<()> {
    require_tools(&["kubectl"])?;
    run(Command::new("kubectl").args(super::undeploy_args(&cfg.kind_context(), &cfg.namespace)))
}

/// `devx logs`: tail navigator-web logs.
pub(super) fn logs(cfg: &KindConfig) -> Result<()> {
    require_tools(&["kubectl"])?;
    use_kind_context(cfg)?;
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(&cfg.namespace)
        .arg("logs")
        .arg("-f")
        .arg("deployment/navigator-web")
        .arg("-c")
        .arg("web"))
}

/// Switch kubectl to the KIND context for this cluster before any
/// cluster-mutating apply. `devx` is KIND-only; a stale GKE/EKS
/// context being current is otherwise indistinguishable from a fresh
/// KIND boot and would land manifests in the wrong place.
pub(super) fn use_kind_context(cfg: &KindConfig) -> Result<()> {
    let context = cfg.kind_context();
    let out = Command::new("kubectl")
        .args(["config", "get-contexts", "-o", "name"])
        .output()
        .with_context(|| "kubectl config get-contexts failed")?;
    if !out.status.success() {
        anyhow::bail!(
            "kubectl config get-contexts failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let contexts = String::from_utf8_lossy(&out.stdout);
    if !contexts.lines().any(|c| c == context) {
        anyhow::bail!(
            "kubectl context '{context}' not found; bring the KIND cluster up first \
             (`dev kind up`)."
        );
    }
    eprintln!("==> pinning kubectl context → {context}");
    run(Command::new("kubectl").args(super::use_context_args(&context)))
}

/// `dev kustomize kind` / `dev kustomize gke`: render a Kustomize
/// overlay to stdout. Inspect what `kubectl apply -k` would send
/// before sending it.
pub(super) fn kustomize_render(overlay: &str) -> Result<()> {
    require_tools(&["kubectl"])?;
    let root = workspace_root()?;
    run(Command::new("kubectl")
        .arg("kustomize")
        .arg(root.join(overlay)))
}

// ---------- shared helpers ----------

/// Idempotent cluster bring-up: `kind create cluster` (if missing),
/// `kubectl apply` the nginx-ingress manifest, then `helm install`
/// the Restate Operator. Safe to re-invoke.
fn kind_up_steps(root: &Path, cfg: &KindConfig) -> Result<()> {
    let cluster = &cfg.cluster;
    if cluster_exists(cluster)? {
        eprintln!("==> KIND cluster '{cluster}' already exists, reusing");
    } else {
        eprintln!("==> creating KIND cluster '{cluster}'");
        let config_path = kind_config_path(root, cfg)?;
        run(Command::new("kind")
            .arg("create")
            .arg("cluster")
            .arg("--name")
            .arg(cluster)
            .arg("--kubeconfig")
            .arg(worktree_kubeconfig(root))
            .arg("--config")
            .arg(&config_path))?;
    }
    configure_worktree_kubeconfig(root, cfg)?;

    eprintln!("==> installing nginx-ingress");
    run(Command::new("kubectl")
        .arg("apply")
        .arg("-f")
        .arg(root.join(INGRESS_MANIFEST)))?;
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg("ingress-nginx")
        .arg("wait")
        .arg("--for=condition=ready")
        .arg("pod")
        .arg("--selector=app.kubernetes.io/component=controller")
        // A cold KIND node can need several minutes just to pull the
        // controller image; on a loaded local Docker host its probes can take
        // longer still. Leave enough room for a healthy cold start.
        .arg("--timeout=1200s"))?;

    eprintln!("==> installing Restate Operator (chart v{RESTATE_OPERATOR_VERSION})");
    run(Command::new("helm")
        .arg("upgrade")
        .arg("--install")
        .arg("restate-operator")
        .arg("oci://ghcr.io/restatedev/restate-operator-helm")
        .arg("--version")
        .arg(RESTATE_OPERATOR_VERSION)
        .arg("--namespace")
        .arg("restate-operator")
        .arg("--create-namespace"))?;
    // Helm installs a native `crds/` entry once and never upgrades it, so the
    // chart bump alone leaves an existing cluster on its original CRD schema.
    // Server-side apply because the chart already owns these fields under the
    // `helm` field manager (`--force-conflicts` takes ownership) and because
    // `restateclusters` serializes to ~236 KB — close enough to the 262 KB
    // annotation ceiling that a client-side apply's `last-applied-configuration`
    // is one upstream field away from breaking.
    eprintln!("==> applying Restate CRDs (v{RESTATE_OPERATOR_VERSION})");
    for crd in RESTATE_CRDS {
        run(Command::new("kubectl")
            .arg("apply")
            .arg("--server-side")
            .arg("--force-conflicts")
            .arg("-f")
            .arg(root.join(restate_crd_path(crd))))?;
    }
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg("restate-operator")
        .arg("wait")
        .arg("--for=condition=available")
        .arg("--timeout=1200s")
        .arg("deployment")
        .arg("--all"))
}

/// Keep every Kubernetes subprocess for one local setup on a kubeconfig that
/// belongs to the target worktree. Unlike `kubectl config use-context`, this
/// never mutates the host-wide current context, so concurrent clones cannot
/// redirect one another's apply, Garage, or port-forward commands.
pub(super) fn configure_worktree_kubeconfig(root: &Path, cfg: &KindConfig) -> Result<()> {
    let kubeconfig = worktree_kubeconfig(root);
    fs::create_dir_all(root.join(".devx")).with_context(|| {
        format!(
            "create worktree state directory for {}",
            kubeconfig.display()
        )
    })?;
    if !kubeconfig.exists() {
        run(Command::new("kind")
            .arg("export")
            .arg("kubeconfig")
            .arg("--name")
            .arg(&cfg.cluster)
            .arg("--kubeconfig")
            .arg(&kubeconfig))?;
    }
    env::set_var("KUBECONFIG", &kubeconfig);
    use_kind_context(cfg)
}

fn worktree_kubeconfig(root: &Path) -> PathBuf {
    root.join(".devx/kubeconfig")
}

/// Create the application namespace before provisioning Garage's control
/// secret. A reused cluster already has it, while a fresh worktree cluster has
/// not yet had its Kustomize overlay applied.
fn ensure_namespace(namespace: &str) -> Result<()> {
    let exists = Command::new("kubectl")
        .args(["get", "namespace", namespace])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("inspect Kubernetes namespace {namespace}"))?;
    if exists.success() {
        return Ok(());
    }
    eprintln!("==> creating namespace {namespace}");
    run(Command::new("kubectl").args(["create", "namespace", namespace]))
}

/// Load a local Docker image tag into every KIND node.
///
/// `kind load docker-image` hardcodes `ctr images import --all-platforms
/// --digests`, which means it tries to import *every* manifest the local
/// image index references. Since CI began publishing multi-arch indexes
/// (`linux/amd64` + `linux/arm64` + buildx `unknown/unknown` attestation
/// manifests), that breaks on any single-arch host: `docker pull` only
/// materializes the host platform's blobs, so `--all-platforms` aborts with
/// `ctr: content digest <other-arch manifest>: not found`. `OrbStack` and
/// Docker 29 use the containerd image store, which keeps the full index, so
/// this is the *default* failure on Apple Silicon — not an opt-in misconfig.
///
/// Flatten to the daemon's own platform with `docker save --platform` first,
/// then `kind load image-archive` the single-platform tar — `--all-platforms`
/// then finds exactly one platform and succeeds.
///
/// Only a *failed `docker save`* triggers the legacy fallback: a save that
/// can't produce `<platform>` means the image is older/single-arch, where
/// `kind load docker-image` still works (one platform, nothing to mismatch).
/// A failure of the `kind load image-archive` step is deliberately *not*
/// caught — KIND being unreachable or out of disk would fail the legacy load
/// the same way, and on a multi-arch image the fallback would re-trigger the
/// very `--all-platforms` digest bug this exists to avoid. That error
/// propagates directly instead of hiding behind a "flatten failed" message.
fn kind_load_image_into_cluster(tag: &str, cfg: &KindConfig) -> Result<()> {
    let platform = format!("linux/{}", docker_daemon_arch());
    let archive = tempfile::Builder::new()
        .prefix("navigator-kind-load-")
        .suffix(".tar")
        .tempfile()
        .context("create temp image archive for kind load")?;
    eprintln!("==> docker save --platform {platform} {tag} (single-platform for kind)");
    let saved = run(Command::new("docker")
        .arg("save")
        .arg("--platform")
        .arg(&platform)
        .arg(tag)
        .arg("-o")
        .arg(archive.path()));
    if let Err(err) = saved {
        eprintln!(
            "==> `docker save --platform {platform}` failed ({err:#}); \
             falling back to `kind load docker-image` (older single-arch image?)"
        );
        return run(Command::new("kind")
            .arg("load")
            .arg("docker-image")
            .arg(tag)
            .arg("--name")
            .arg(&cfg.cluster));
    }
    eprintln!("==> kind load image-archive ({tag} → {})", cfg.cluster);
    let loaded = run(Command::new("kind")
        .arg("load")
        .arg("image-archive")
        .arg(archive.path())
        .arg("--name")
        .arg(&cfg.cluster));
    if let Err(err) = loaded {
        // `image-archive` is preferred (it is what makes the platform flatten
        // above worth doing), but it is not universally accepted: on Docker
        // 29.7.2 + containerd image store, `ctr images import` rejects the
        // archive `docker save --platform` produces with `unrecognized image
        // format`, even though the archive is well-formed — one manifest, one
        // platform, `manifest.json` present.
        //
        // `docker load`-ing the same image through `kind load docker-image`
        // succeeds against the identical daemon and cluster, so the fallback is
        // not a lesser path here — it is the one that works. Measured: the
        // archive load fails and the docker-image load then lands the image in
        // the node's containerd (`crictl images` shows it) in the same run.
        //
        // Kept as a fallback rather than a replacement so the flattened-archive
        // path stays primary where it works, and so this stops being reached
        // the moment the upstream mismatch is fixed.
        eprintln!(
            "==> `kind load image-archive` failed ({err:#}); \
             falling back to `kind load docker-image`"
        );
        return run(Command::new("kind")
            .arg("load")
            .arg("docker-image")
            .arg(tag)
            .arg("--name")
            .arg(&cfg.cluster));
    }
    Ok(())
}

/// Architecture the Docker daemon runs as (`arm64` / `amd64`), used to build
/// the `linux/<arch>` platform passed to `docker save`. Queries the daemon
/// directly (`docker version`) so it is correct even when the CLI binary runs
/// under a different arch (Rosetta); falls back to the host arch if the daemon
/// can't be reached.
fn docker_daemon_arch() -> String {
    Command::new("docker")
        .args(["version", "--format", "{{.Server.Arch}}"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|arch| !arch.is_empty())
        .unwrap_or_else(|| normalize_docker_arch(std::env::consts::ARCH))
}

/// Resolve the immutable GHCR release tag the local cluster should
/// pull, in precedence order: an explicit `override_tag` (e.g.
/// `worktree-env --demo --tag`), then `NAVIGATOR_IMAGE_TAG`, then the
/// latest published tag from the registry. CI builds and publishes the
/// images (`deploy.yml`); the local loop pulls them.
fn resolve_local_image_tag(registry: &str, override_tag: Option<&str>) -> Result<String> {
    if let Some(tag) = override_tag
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env::var("NAVIGATOR_IMAGE_TAG")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
    {
        super::registry::validate_release_tag(&tag)?;
        return Ok(tag);
    }
    super::registry::resolve_latest_tag(registry, "navigator-web")
}

/// Pull a published Artifact Registry image, retag it to the local `:dev`
/// tag the KIND manifests reference, and load it into the cluster.
/// Retagging (rather than rewriting every manifest to a registry ref)
/// keeps the overlays byte-identical whether an image was historically
/// built or is now pulled. Published images are amd64-only (see
/// `deploy.yml`), matching prod's amd64 GKE Autopilot nodes, so `docker
/// pull` fetches the amd64 variant on any host; an Apple-Silicon laptop
/// loads it under emulation into its KIND node. The registry is private,
/// so the host's docker must be authorized
/// (`gcloud auth configure-docker <region>-docker.pkg.dev`).
fn pull_retag_load(
    registry: &str,
    image: &str,
    tag: &str,
    dev_tag: &str,
    cfg: &KindConfig,
) -> Result<()> {
    let remote = super::registry::image_ref(registry, image, tag);
    eprintln!("==> docker pull {remote}");
    run(Command::new("docker").arg("pull").arg(&remote))?;
    run(Command::new("docker").arg("tag").arg(&remote).arg(dev_tag))?;
    kind_load_image_into_cluster(dev_tag, cfg)
}

/// Path to the `kind create cluster --config` file. At default host ports this
/// is the committed `k8s/kind-config.yaml` verbatim (so a standalone `kind
/// create` against it still works). When any mapped host port is overridden,
/// render a temp copy under `.devx/`.
fn kind_config_path(root: &Path, cfg: &KindConfig) -> Result<PathBuf> {
    let committed = root.join("k8s/kind-config.yaml");
    if cfg.ingress_http_port == super::DEFAULT_INGRESS_HTTP_HOST_PORT
        && cfg.ingress_https_port == super::DEFAULT_INGRESS_HTTPS_HOST_PORT
        && cfg.rauthy_port == DEFAULT_RAUTHY_HOST_PORT
    {
        return Ok(committed);
    }
    let template =
        fs::read_to_string(&committed).with_context(|| format!("read {}", committed.display()))?;
    let rendered = render_kind_config(&template, cfg);
    let dir = root.join(".devx");
    fs::create_dir_all(&dir).with_context(|| format!("create state dir {}", dir.display()))?;
    let path = dir.join("kind-config.yaml");
    fs::write(&path, rendered).with_context(|| format!("write {}", path.display()))?;
    eprintln!(
        "==> rendered kind-config with ingress={}/{} rauthy={} host ports → {}",
        cfg.ingress_http_port,
        cfg.ingress_https_port,
        cfg.rauthy_port,
        path.display()
    );
    Ok(path)
}

fn apply_kustomize(root: &Path, overlay: &str) -> Result<()> {
    eprintln!("==> kubectl apply -k {overlay}");
    run(Command::new("kubectl")
        .arg("apply")
        .arg("-k")
        .arg(root.join(overlay)))
}

fn wait_for_dep_rollouts(cfg: &KindConfig) -> Result<()> {
    eprintln!("==> waiting for rollouts");
    for dep in ["surreal", "rauthy", "openobserve", "clamav"] {
        wait_rollout("deployment", dep, cfg)?;
    }
    // Restate runs in its own `restate` namespace and the Operator
    // names the underlying StatefulSet from the CR spec, not literally
    // "restate". `workflows-service` is also a Restate Operator CR
    // (RestateDeployment), not a plain Deployment. Wait on each CR's
    // own `Ready` condition — that's the contract the Operator
    // exposes.
    wait_for_condition("restate", "restatecluster/restate", "Ready")?;
    wait_for_condition(
        &cfg.namespace,
        "restatedeployment/workflows-service",
        "Ready",
    )
}

pub(super) fn wait_for_condition(namespace: &str, resource: &str, condition: &str) -> Result<()> {
    if let Some(state) = terminating_state(namespace, resource) {
        bail!("{}", terminating_wedge_message(namespace, resource, &state));
    }
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(namespace)
        .arg("wait")
        .arg(format!("--for=condition={condition}"))
        .arg(resource)
        .arg("--timeout=300s"))
}

/// Probe whether `resource` is mid-deletion, so [`wait_for_condition`] can
/// fail fast with a real diagnosis instead of blocking for the full timeout.
///
/// Best-effort by construction: a probe that cannot answer (`NotFound`, no
/// cluster, no kubectl) yields `None` and lets the `wait` produce the
/// authoritative error. This only ever *adds* a diagnosis; it never invents a
/// failure of its own.
fn terminating_state(namespace: &str, resource: &str) -> Option<Terminating> {
    let out = Command::new("kubectl")
        .arg("--namespace")
        .arg(namespace)
        .arg("get")
        .arg(resource)
        .arg("-o")
        .arg("jsonpath={.metadata.deletionTimestamp}|{.metadata.finalizers}")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_terminating(&String::from_utf8_lossy(&out.stdout))
}

/// Wait for a pod matching `selector` in `namespace` to reach `Ready`.
///
/// `reload_worker` deletes the running worker pod and lets the
/// `RestateDeployment`'s `ReplicaSet` recreate it. A bare
/// `kubectl wait --for=condition=Ready pod` errors with "no matching resources
/// found" if it runs before the `ReplicaSet` has created the replacement, so
/// first poll until a pod exists, then block on its readiness. This observes
/// the actual replacement pod rather than the `RestateDeployment` CR's lagging
/// `Ready` condition.
fn wait_for_selected_pod_ready(namespace: &str, selector: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_mins(5);
    loop {
        let out = Command::new("kubectl")
            .arg("--namespace")
            .arg(namespace)
            .arg("get")
            .arg("pod")
            .arg("--selector")
            .arg(selector)
            .arg("--output")
            .arg("jsonpath={.items[*].metadata.name}")
            .output()
            .with_context(|| format!("list pods matching {selector} in {namespace}"))?;
        if out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for a pod matching '{selector}' to appear in {namespace}");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(namespace)
        .arg("wait")
        .arg("--for=condition=Ready")
        .arg("pod")
        .arg("--selector")
        .arg(selector)
        .arg("--timeout=300s"))
}

pub(super) fn down(cfg: &KindConfig) -> Result<()> {
    let root = workspace_root()?;
    down_in(&root, cfg)
}

/// Tear down a KIND dependency tier whose port-forward state belongs to
/// `root`. See [`up_in`] for why this cannot rely on the caller's CWD.
pub(super) fn down_in(root: &Path, cfg: &KindConfig) -> Result<()> {
    let state = StateDir::new(root)?;
    eprintln!("==> killing port-forwards");
    state.kill_pids();
    let cluster = &cfg.cluster;
    if cluster_exists(cluster)? {
        eprintln!("==> deleting KIND cluster '{cluster}'");
        run(Command::new("kind")
            .arg("delete")
            .arg("cluster")
            .arg("--name")
            .arg(cluster))?;
    }
    state.clear();
    Ok(())
}

pub(super) fn print_env(cfg: &KindConfig) -> Result<()> {
    print!("{}", render_env(cfg, &workspace_root()?));
    Ok(())
}

pub(super) fn status(cfg: &KindConfig) {
    let cluster = cluster_exists(&cfg.cluster).unwrap_or(false);
    println!("KIND cluster '{}': {}", cfg.cluster, yes_no(cluster));
    if let Ok(root) = workspace_root() {
        if let Ok(state) = StateDir::new(&root) {
            let pids = state.read_pids().unwrap_or_default();
            println!("Port-forward PIDs ({}): {pids:?}", pids.len());
            for &port in &[
                cfg.restate_ingress_port,
                cfg.restate_admin_port,
                cfg.clamav_port,
                cfg.rauthy_port,
                cfg.garage_s3_port,
                cfg.surreal_port,
            ] {
                let listening = std::net::TcpStream::connect_timeout(
                    &format!("127.0.0.1:{port}").parse().unwrap(),
                    Duration::from_millis(200),
                )
                .is_ok();
                println!("  127.0.0.1:{port}: {}", yes_no(listening));
            }
        }
    }
}

fn print_chrome_summary(cfg: &KindConfig) {
    let web = cfg.web_port;
    eprintln!();
    eprintln!("===========================================================");
    eprintln!(" devx up — full Neon Law Navigator stack running in KIND");
    eprintln!("===========================================================");
    eprintln!();
    eprintln!("Start the web server on the host:");
    eprintln!();
    eprintln!("    set -a; source .devx/env; set +a");
    eprintln!("    cargo run -p neon");
    eprintln!();
    eprintln!("Walk the retainer in Chrome:");
    eprintln!("  http://localhost:{web}                    — navigator home");
    eprintln!("  http://localhost:{web}/auth/login  — OIDC flow");
    eprintln!("  http://localhost:{web}/app/lawyer/retainers/new — start a stepwise walk");
    eprintln!();
    eprintln!("Inspect the workflow journal directly from the host:");
    eprintln!();
    eprintln!(
        "    surreal sql --endpoint ws://localhost:{} --namespace navigator --database navigator \\",
        cfg.surreal_port
    );
    eprintln!("        --pretty <<<'SELECT * FROM notation_event ORDER BY id'");
    eprintln!();
    eprintln!("Other admin UIs:");
    eprintln!(
        "  http://localhost:{}/auth/v1/admin    — Rauthy admin (nick@neonlaw.com/admin)",
        cfg.rauthy_port
    );
    eprintln!(
        "  http://localhost:{}                   — Garage S3 endpoint",
        cfg.garage_s3_port
    );
    eprintln!(
        "  http://localhost:{}                  — SurrealDB HTTP/health",
        cfg.surreal_port
    );
    eprintln!(
        "      in-cluster: {}  (workflows-service shares this engine)",
        super::surreal::IN_CLUSTER_ENDPOINT
    );
    eprintln!(
        "  http://localhost:{}/services         — Restate admin (registered services)",
        cfg.restate_admin_port
    );
    eprintln!(
        "  http://localhost:{}                   — OpenObserve UI",
        cfg.openobserve_port
    );
    eprintln!();
    eprintln!(
        "Telemetry: host `web` exports directly to OpenObserve at \
         http://localhost:{} (set in .devx/env).",
        cfg.openobserve_otlp_port
    );
    eprintln!(
        "Open OpenObserve and select the default organization and stream to inspect service.name."
    );
    eprintln!();
    eprintln!("Tear down with: cargo run --release -p cli -- dev down");
    eprintln!();
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

pub(super) fn wait_rollout(kind: &str, name: &str, cfg: &KindConfig) -> Result<()> {
    run(Command::new("kubectl")
        .arg("--namespace")
        .arg(&cfg.namespace)
        .arg("rollout")
        .arg("status")
        .arg(format!("{kind}/{name}"))
        .arg("--timeout=300s"))
}

fn port_forward(
    target: &str,
    host_port: u16,
    svc_port: u16,
    cfg: &KindConfig,
    state: &StateDir,
) -> Result<u32> {
    spawn_port_forward(
        &[
            "--namespace",
            &cfg.namespace,
            "port-forward",
            target,
            &format!("{host_port}:{svc_port}"),
        ],
        state,
    )
}

fn port_forward_two_in(
    namespace: &str,
    target: &str,
    host_a: u16,
    svc_a: u16,
    host_b: u16,
    svc_b: u16,
    state: &StateDir,
) -> Result<u32> {
    spawn_port_forward(
        &[
            "--namespace",
            namespace,
            "port-forward",
            target,
            &format!("{host_a}:{svc_a}"),
            &format!("{host_b}:{svc_b}"),
        ],
        state,
    )
}

fn spawn_port_forward(args: &[&str], state: &StateDir) -> Result<u32> {
    let log_offset = state.log_size();
    let log = state.open_log()?;
    let log_err = log.try_clone()?;
    let mut cmd = Command::new("kubectl");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    detach(&mut cmd);
    let child: Child = cmd
        .spawn()
        .with_context(|| format!("spawn kubectl {args:?}"))?;
    let pid = child.id();
    // Detach: don't wait on it. `Child::drop` does not kill the
    // process; the OS adopts it via the new process group.
    std::mem::forget(child);

    // Give kubectl a moment to either bind or fail, then scan the log
    // it appended. Without this check, a `bind: address already in
    // use` error is hidden by anything else that happens to be
    // listening on that port.
    std::thread::sleep(Duration::from_millis(800));
    if let Some(err) = state.log_tail_error(log_offset)? {
        bail!("kubectl port-forward {args:?} failed: {err}");
    }
    Ok(pid)
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn detach(_cmd: &mut Command) {}

pub(super) fn wait_for_tcp(host: &str, port: u16) -> Result<()> {
    let addr = format!("{host}:{port}");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(stream) = std::net::TcpStream::connect_timeout(
            &addr
                .parse()
                .with_context(|| format!("parse socket addr {addr}"))?,
            Duration::from_millis(500),
        ) {
            drop(stream);
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for {addr} to accept connections");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn cluster_exists(name: &str) -> Result<bool> {
    let out = Command::new("kind")
        .arg("get")
        .arg("clusters")
        .output()
        .context("run `kind get clusters`")?;
    if !out.status.success() {
        bail!(
            "kind get clusters failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.trim() == name))
}

pub(super) fn require_tools(tools: &[&str]) -> Result<()> {
    for tool in tools {
        let ok = Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {tool}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            bail!("required tool not on PATH: {tool}");
        }
    }
    Ok(())
}

/// Verify that auth-bearing CLIs are not just present but **authenticated**.
///
/// A present-but-unauthenticated `gcloud` / `restate` doesn't
/// fail until deep in a flow — mid-push to Artifact Registry, mid-Restate
/// re-register — where the error is cryptic and a half-finished ship is
/// already on the cluster. This runs the cheapest read-only probe per CLI
/// up front so the whole flow aborts in one clear line before it touches
/// anything. Each probe's output is discarded; only its exit status counts.
pub(super) fn require_auth(tools: &[&str]) -> Result<()> {
    for &tool in tools {
        // (probe command, what to run to fix it)
        let (probe, hint) = match tool {
            // Prints an access token iff a credential is active; non-zero
            // when logged out or no ADC / service account is available.
            "gcloud" => (
                "gcloud auth print-access-token",
                "run `gcloud auth login` (or activate a service account)",
            ),
            // `whoami` succeeds only when an environment is configured. This
            // catches "never logged in"; a stale Cloud token can still slip
            // through, so the register step remains the real proof.
            "restate" => (
                "restate whoami",
                "run `restate cloud login` then `restate cloud environments configure <env>`",
            ),
            // The daemon must be reachable to build/push images.
            "docker" => ("docker info", "start the Docker daemon"),
            other => bail!("require_auth: no auth probe defined for `{other}`"),
        };
        eprintln!("==> auth check: {tool}");
        let ok = Command::new("sh")
            .arg("-c")
            .arg(probe)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            bail!("`{tool}` is present but not authenticated — {hint}");
        }
    }
    Ok(())
}

pub(super) fn run(cmd: &mut Command) -> Result<()> {
    let program = Path::new(cmd.get_program()).display().to_string();
    let status = cmd.status().with_context(|| format!("spawn {program}"))?;
    if !status.success() {
        bail!("command failed ({status}): {program}");
    }
    Ok(())
}

pub(super) fn workspace_root() -> Result<PathBuf> {
    let mut dir = env::current_dir().context("get current directory")?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("k8s").is_dir() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => bail!("could not find workspace root containing Cargo.toml and k8s/"),
        }
    }
}

// ---------- state directory (.devx/) ----------

struct StateDir {
    dir: PathBuf,
}

impl StateDir {
    fn new(root: &Path) -> Result<Self> {
        let dir = root.join(".devx");
        fs::create_dir_all(&dir).with_context(|| format!("create state dir {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn pids_path(&self) -> PathBuf {
        self.dir.join("pids")
    }

    fn env_path(&self) -> PathBuf {
        self.dir.join("env")
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("port-forwards.log")
    }

    fn open_log(&self) -> Result<fs::File> {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
            .with_context(|| format!("open {}", self.log_path().display()))
    }

    fn log_size(&self) -> u64 {
        fs::metadata(self.log_path()).map_or(0, |m| m.len())
    }

    /// Return the first error-shaped line appended to the log past
    /// `since`, or None if nothing notable showed up. "Error-shaped"
    /// means kubectl's `error:` or `Unable to listen` prefixes — the
    /// happy path emits `Forwarding from …`.
    fn log_tail_error(&self, since: u64) -> Result<Option<String>> {
        let path = self.log_path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let tail = bytes
            .get(usize::try_from(since).unwrap_or(usize::MAX)..)
            .unwrap_or(&[]);
        let text = String::from_utf8_lossy(tail);
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("error:") || trimmed.starts_with("Unable to listen") {
                return Ok(Some(trimmed.to_string()));
            }
        }
        Ok(None)
    }

    fn write_pids(&self, pids: &[u32]) -> Result<()> {
        let mut f = fs::File::create(self.pids_path())
            .with_context(|| format!("write {}", self.pids_path().display()))?;
        for pid in pids {
            writeln!(f, "{pid}")?;
        }
        Ok(())
    }

    fn read_pids(&self) -> Result<Vec<u32>> {
        let path = self.pids_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let f = fs::File::open(&path).with_context(|| format!("read {}", path.display()))?;
        let mut out = Vec::new();
        for line in BufReader::new(f).lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            out.push(
                trimmed
                    .parse::<u32>()
                    .map_err(|e| anyhow!("malformed PID '{trimmed}': {e}"))?,
            );
        }
        Ok(out)
    }

    fn kill_pids(&self) {
        Self::kill(&self.read_pids().unwrap_or_default());
        let _ = fs::remove_file(self.pids_path());
    }

    fn kill(pids: &[u32]) {
        for pid in pids {
            // Best-effort: process may already have exited.
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    fn write_env(&self, body: &str) -> Result<()> {
        fs::write(self.env_path(), body)
            .with_context(|| format!("write {}", self.env_path().display()))
    }

    fn clear(&self) {
        let _ = fs::remove_file(self.pids_path());
        let _ = fs::remove_file(self.env_path());
    }
}
