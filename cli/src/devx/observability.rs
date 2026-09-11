//! `devx observability` — stand up the `OTel` Collector seam in a prod
//! cluster and wire the long-running binaries to it.
//!
//! This is the deterministic, in-binary form of the operator steps in
//! [`examples/deploy/k8s/observability/README.md`]. Production is
//! managed by direct `kubectl`/`gcloud` (no Config Sync, and the
//! `navigator-otel-env` `ConfigMap` the deployment manifests `envFrom` is
//! *not* part of any overlay `ship` applies), so without this
//! command the collector never gets deployed and every binary's
//! `OTEL_EXPORTER_OTLP_ENDPOINT` stays unset — telemetry silently never
//! leaves the pod. `devx observability apply` closes that gap in one
//! idempotent command:
//!
//! 1. **GSA + IAM** — ensure the `navigator-otel` Google service account
//!    exists, carries `roles/cloudtrace.agent` +
//!    `roles/monitoring.metricWriter` + `roles/logging.logWriter`, and is
//!    bound to the in-cluster `otel-collector` KSA via Workload Identity.
//! 2. **Collector** — render the bundled manifests with the project id
//!    substituted and `kubectl apply` them: the Collector Deployment +
//!    Service, the `otel-collector-config`, the shared
//!    `navigator-otel-env` `ConfigMap`, and the GMP self-monitoring
//!    (`PodMonitoring` + alert `Rules`).
//! 3. **Wire the binaries** — patch `navigator-web` and
//!    `workflows-service` to `envFrom` the `navigator-otel-env` `ConfigMap`
//!    so `OTEL_EXPORTER_OTLP_ENDPOINT` reaches `telemetry::init`. This is
//!    a one-time `kubectl patch` — the shared `navigator-otel-env`
//!    `ConfigMap` reference already lives in the manifests `ship` renders
//!    and reconciles, so subsequent ships keep the wiring in place.
//!
//! Everything per-deployment flows through the environment via the same
//! [`ShipConfig`] `ship` uses — there is no literal project
//! id, region, cluster, namespace, or context in this file.
//!
//! ## Testing
//!
//! The orchestration shells out to `gcloud`/`kubectl`, so it isn't
//! unit-tested. The pure pieces — the project-id substitution and the
//! `envFrom` patch builder — are covered by the `tests` module below.

use std::fs;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use k8s_openapi::api::core::v1::Endpoints;
use kube::{api::Api, Client as KubernetesClient, Config as KubernetesConfig};
use serde::Deserialize;

use super::gcp::{
    auth::adc_token_provider,
    client::{GcpClient, GcpService},
};
use super::ship::ShipConfig;
use super::{require_auth, require_tools, run};

/// In-cluster Deployment + container names (workspace conventions, same
/// as `ship`). Each long-running binary gets the collector
/// endpoint via `envFrom` the shared `ConfigMap`.
const WEB_DEPLOYMENT: &str = "navigator-web";
const WEB_CONTAINER: &str = "web";
const WORKFLOWS_DEPLOYMENT: &str = "workflows-service";
const WORKFLOWS_CONTAINER: &str = "worker";

/// The Collector's Google service account short name + the KSA it backs.
const OTEL_GSA: &str = "navigator-otel";
const OTEL_KSA: &str = "otel-collector";
/// The shared `ConfigMap` that carries `OTEL_EXPORTER_OTLP_ENDPOINT` — one
/// source of truth for the collector URL, `envFrom`'d by every binary.
const OTEL_ENV_CONFIGMAP: &str = "navigator-otel-env";
/// The telemetry-write roles the Collector's GSA needs to fan OTLP out to
/// Google Cloud (traces, metrics, logs respectively).
const OTEL_ROLES: &[&str] = &[
    "roles/cloudtrace.agent",
    "roles/monitoring.metricWriter",
    "roles/logging.logWriter",
];
/// Fresh GKE and IAM control-plane components can acknowledge creation before
/// their admission or policy services accept a dependent request. Retry only
/// idempotent operations through that bounded propagation window.
const CONTROL_PLANE_PROPAGATION_ATTEMPTS: usize = 6;
/// A cold Autopilot cluster can take a few minutes to start the managed
/// Prometheus admission webhook after the control plane reports RUNNING.
/// This is a readiness poll, not a blind apply retry.
const GMP_WEBHOOK_PROPAGATION_ATTEMPTS: usize = 37;
const CONTROL_PLANE_PROPAGATION_DELAY: Duration = Duration::from_secs(5);
const GMP_SYSTEM_NAMESPACE: &str = "gke-gmp-system";
const GMP_OPERATOR_ENDPOINTS: &str = "gmp-operator";

/// The Collector + `ConfigMap`s + Service + self-monitoring manifests,
/// bundled into the binary so the command is self-contained. Both carry
/// the `YOUR_PROJECT_ID` placeholder convention (`otel-collector.yaml`
/// twice: the WI GSA annotation + the `googlecloud` exporter project;
/// `collector-monitoring.yaml` has none) — `render_manifest` substitutes
/// the real project id.
const OTEL_COLLECTOR_YAML: &str =
    include_str!("../../../examples/deploy/k8s/observability/otel-collector.yaml");
const COLLECTOR_MONITORING_YAML: &str =
    include_str!("../../../examples/deploy/k8s/observability/collector-monitoring.yaml");

/// The placeholder every deploy-side manifest carries for the GCP project.
const PROJECT_PLACEHOLDER: &str = "YOUR_PROJECT_ID";
const NAMESPACE_PLACEHOLDER: &str = "YOUR_NAMESPACE";

/// The narrow GKE control-plane response required to connect a Rust
/// Kubernetes client without depending on an operator's kubeconfig or a
/// `kubectl` subprocess. The Google Cloud SDK-backed ADC token authorizes
/// both the Container API read and the Kubernetes API request.
#[derive(Debug, Deserialize)]
struct GkeClusterConnectionResponse {
    endpoint: String,
    status: String,
    #[serde(rename = "masterAuth")]
    master_auth: GkeMasterAuth,
}

#[derive(Debug, Deserialize)]
struct GkeMasterAuth {
    #[serde(rename = "clusterCaCertificate")]
    cluster_ca_certificate: String,
}

struct GkeClusterConnection {
    endpoint: String,
    /// One DER body per certificate in the cluster CA bundle — the shape
    /// `kube::Config::root_cert` takes, not the PEM the API returns.
    ca_certificate: Vec<Vec<u8>>,
}

/// Options parsed from the `devx observability apply` flags.
#[derive(Debug, Clone)]
pub struct ObservabilityOpts {
    /// Deployment directory under `deployments/` to stand the Collector up
    /// in. Explicit like `ops ship`'s — never inherited from a shell.
    pub deployment: String,
    /// The directory holding the `deployments/` tree, when it is not the
    /// workspace. See [`super::deployments::root`].
    pub deployments_dir: Option<std::path::PathBuf>,
    /// Print every command instead of running it.
    pub dry_run: bool,
}

/// Entry point for `Cmd::Observability`.
pub fn run_observability(opts: &ObservabilityOpts) -> Result<()> {
    let root = super::deployments::root(opts.deployments_dir.as_deref())?;
    let deployment = super::deployments::Deployment::load(&root, &opts.deployment)?;
    let cfg = ShipConfig::from_deployment(&deployment)?;
    require_tools(&["kubectl", "gcloud"])?;
    if !opts.dry_run {
        require_auth(&["gcloud"])?;
    }
    eprintln!(
        "==> observability: standing up the `OTel` collector in {} ({})",
        cfg.project_id, cfg.context
    );
    ensure_gsa_iam(&cfg, opts.dry_run)?;
    apply_manifests(&cfg, opts.dry_run)?;
    wire_binaries(&cfg, opts.dry_run)?;
    eprintln!(
        "==> observability ready. Roll the binaries so they pick up the endpoint \
         (a `devx ship` set-image, or `kubectl rollout restart`), then look for \
         traces in Cloud Trace + the `navigator.workflow.trigger.fired` metric."
    );
    Ok(())
}

/// Step 1 — ensure the Collector's GSA exists, carries the three
/// telemetry-write roles, and is bound to the in-cluster KSA via Workload
/// Identity. Every call is idempotent: the GSA is created only when absent
/// (`describe` probe), and the IAM bindings are no-ops when already present.
fn ensure_gsa_iam(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    let gsa = gsa_email(&cfg.project_id);
    if gsa_exists(cfg, &gsa)? {
        eprintln!("==> GSA {gsa} already exists");
    } else {
        eprintln!("==> creating GSA {gsa}");
        exec(
            dry_run,
            Command::new("gcloud")
                .args(["iam", "service-accounts", "create", OTEL_GSA])
                .arg(format!("--project={}", cfg.project_id))
                .args(["--display-name", "Neon Law Navigator `OTel` Collector"]),
        )?;
    }
    // Bind the telemetry-write roles one at a time. `add-iam-policy-binding`
    // is read-modify-write on the project policy, so a tight loop can lose
    // an etag race; running them sequentially (each its own gcloud call)
    // avoids that, and a repeat binding is a documented no-op.
    for role in OTEL_ROLES {
        eprintln!("==> binding {role} → {gsa}");
        exec_with_control_plane_retry(
            "IAM binding",
            dry_run,
            CONTROL_PLANE_PROPAGATION_ATTEMPTS,
            || {
                let mut command = Command::new("gcloud");
                command
                    .args(["projects", "add-iam-policy-binding", &cfg.project_id])
                    .arg(format!("--member=serviceAccount:{gsa}"))
                    .arg(format!("--role={role}"))
                    .args(["--condition", "None"]);
                command
            },
        )?;
    }
    eprintln!("==> binding Workload Identity {OTEL_KSA} KSA → {gsa}");
    exec_with_control_plane_retry(
        "Workload Identity binding",
        dry_run,
        CONTROL_PLANE_PROPAGATION_ATTEMPTS,
        || {
            let mut command = Command::new("gcloud");
            command
                .args(["iam", "service-accounts", "add-iam-policy-binding", &gsa])
                .arg(format!("--project={}", cfg.project_id))
                .args(["--role", "roles/iam.workloadIdentityUser"])
                .arg(format!(
                    "--member=serviceAccount:{}.svc.id.goog[{}/{OTEL_KSA}]",
                    cfg.project_id, cfg.namespace
                ));
            command
        },
    )
}

/// Retry an idempotent control-plane operation while a just-created GSA or
/// managed admission webhook propagates. Permanent failures remain visible
/// after the bounded window instead of being silently swallowed.
fn exec_with_control_plane_retry(
    operation: &str,
    dry_run: bool,
    attempts: usize,
    mut command: impl FnMut() -> Command,
) -> Result<()> {
    for attempt in 1..=attempts {
        let mut operation_command = command();
        match exec(dry_run, &mut operation_command) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < attempts => {
                eprintln!(
                    "==> {operation} did not settle (attempt {attempt}/{attempts}); retrying in {}s",
                    CONTROL_PLANE_PROPAGATION_DELAY.as_secs()
                );
                sleep(CONTROL_PLANE_PROPAGATION_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded control-plane retry loop always returns")
}

/// Step 2 — render the bundled manifests with the project id substituted
/// and `kubectl apply` them (context-pinned, the manifests carry their own
/// namespace). The collector config is in a `ConfigMap`, so a server-side
/// apply can't catch a bad collector pipeline — the operator confirms the
/// rollout settles afterward (this command waits on it).
fn apply_manifests(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    for (name, template) in [
        ("otel-collector.yaml", OTEL_COLLECTOR_YAML),
        ("collector-monitoring.yaml", COLLECTOR_MONITORING_YAML),
    ] {
        let rendered = render_manifest(template, &cfg.project_id, &cfg.namespace);
        let path = std::env::temp_dir().join(format!("navigator-otel-{name}"));
        if dry_run {
            eprintln!(
                "DRY-RUN: would render {name} → {} and apply it",
                path.display()
            );
            continue;
        }
        fs::write(&path, rendered)
            .with_context(|| format!("write rendered {name} to {}", path.display()))?;
        if name == "collector-monitoring.yaml" {
            wait_for_gmp_operator(cfg, dry_run)?;
        }
        eprintln!("==> applying {name}");
        exec_with_control_plane_retry(name, false, CONTROL_PLANE_PROPAGATION_ATTEMPTS, || {
            let mut command = Command::new("kubectl");
            command
                .args(["--context", &cfg.context, "apply", "-f"])
                .arg(&path);
            command
        })?;
        let _ = fs::remove_file(&path);
    }
    if dry_run {
        eprintln!("DRY-RUN: would wait for the otel-collector rollout");
        return Ok(());
    }
    eprintln!("==> waiting for the otel-collector rollout");
    run(Command::new("kubectl").args([
        "--context",
        &cfg.context,
        "-n",
        &cfg.namespace,
        "rollout",
        "status",
        "deployment/otel-collector",
        "--timeout=120s",
    ]))
}

/// Wait for the actual managed Prometheus admission endpoint before applying
/// the manifest that triggers it. The Container API supplies the selected
/// cluster's endpoint and CA; the Rust Kubernetes client then observes the
/// only readiness signal that matters: a ready `gmp-operator` endpoint.
fn wait_for_gmp_operator(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!(
            "DRY-RUN: would use Google ADC + the GKE Container API to wait for \
             {GMP_SYSTEM_NAMESPACE}/{GMP_OPERATOR_ENDPOINTS} endpoints"
        );
        return Ok(());
    }

    eprintln!(
        "==> waiting for {GMP_SYSTEM_NAMESPACE}/{GMP_OPERATOR_ENDPOINTS} endpoints via Google ADC + GKE API"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create GKE readiness runtime")?;
    let token_provider = runtime.block_on(adc_token_provider())?;
    let gcp = GcpClient::new(token_provider.clone());
    let connection = runtime.block_on(gke_cluster_connection(&gcp, cfg))?;
    let token = runtime.block_on(token_provider.token())?;
    // Inside the runtime, not beside it. `kube` builds a tower buffer whose
    // worker is spawned at construction, so building the client off-runtime
    // panics with "there is no reactor running" before it ever reaches the
    // cluster — the constructor is not async, which is exactly what makes
    // the requirement easy to miss.
    let client = runtime.block_on(async { kubernetes_client(connection, token) })?;
    let endpoints: Api<Endpoints> = Api::namespaced(client, GMP_SYSTEM_NAMESPACE);

    for attempt in 1..=GMP_WEBHOOK_PROPAGATION_ATTEMPTS {
        let operator = runtime
            .block_on(endpoints.get_opt(GMP_OPERATOR_ENDPOINTS))
            .context("read GKE managed Prometheus operator endpoints")?;
        if gmp_operator_is_ready(operator.as_ref()) {
            eprintln!("==> {GMP_SYSTEM_NAMESPACE}/{GMP_OPERATOR_ENDPOINTS} endpoints are ready");
            return Ok(());
        }
        if attempt == GMP_WEBHOOK_PROPAGATION_ATTEMPTS {
            bail!(
                "GKE managed Prometheus operator did not publish ready endpoints after three minutes; inspect with: \
                 kubectl --context {} -n {GMP_SYSTEM_NAMESPACE} get deployment,pods,endpoints {GMP_OPERATOR_ENDPOINTS}",
                cfg.context,
            );
        }
        eprintln!(
            "==> {GMP_SYSTEM_NAMESPACE}/{GMP_OPERATOR_ENDPOINTS} has no ready endpoints \
             (attempt {attempt}/{GMP_WEBHOOK_PROPAGATION_ATTEMPTS}); retrying in {}s",
            CONTROL_PLANE_PROPAGATION_DELAY.as_secs()
        );
        sleep(CONTROL_PLANE_PROPAGATION_DELAY);
    }
    unreachable!("the bounded GMP endpoint readiness loop always returns")
}

async fn gke_cluster_connection(
    client: &GcpClient,
    cfg: &ShipConfig,
) -> Result<GkeClusterConnection> {
    let path = format!(
        "/v1/projects/{}/locations/{}/clusters/{}",
        cfg.project_id, cfg.location, cfg.cluster
    );
    let response = client
        .get(GcpService::Container, &path)
        .await
        .context("read GKE cluster connection from the Container API")?;
    if !(200..=299).contains(&response.status_u16()) {
        bail!("GKE Container API did not return the selected cluster connection");
    }
    parse_gke_cluster_connection(&response.into_text())
}

fn parse_gke_cluster_connection(response: &str) -> Result<GkeClusterConnection> {
    let response: GkeClusterConnectionResponse =
        serde_json::from_str(response).context("parse GKE cluster connection response")?;
    if response.status != "RUNNING" {
        bail!(
            "GKE cluster is not RUNNING yet (status: {})",
            response.status
        );
    }
    if response.endpoint.trim().is_empty() {
        bail!("GKE Container API returned an empty cluster endpoint");
    }
    let ca_certificate = BASE64_STANDARD
        .decode(response.master_auth.cluster_ca_certificate)
        .context("decode GKE cluster CA certificate")?;
    if ca_certificate.is_empty() {
        bail!("GKE Container API returned an empty cluster CA certificate");
    }
    Ok(GkeClusterConnection {
        endpoint: response.endpoint,
        ca_certificate: pem_certificates_to_der(&ca_certificate)?,
    })
}

/// Split a PEM certificate bundle into its DER bodies.
///
/// The Container API hands back `masterAuth.clusterCaCertificate` as
/// base64-encoded **PEM**, so decoding that base64 yields PEM text, not a
/// certificate. `kube::Config::root_cert` wants one DER blob per
/// certificate, and handing it PEM fails the handshake with rustls's
/// `invalid peer certificate: BadEncoding` — a message that describes the
/// GKE control plane's certificate rather than the encoding mistake that
/// is actually ours.
///
/// A bundle rather than a single certificate because a cluster CA may be
/// issued under a chain, and dropping every block but the first would
/// fail only for the clusters that have one.
fn pem_certificates_to_der(pem: &[u8]) -> Result<Vec<Vec<u8>>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let text = std::str::from_utf8(pem).context("GKE cluster CA certificate is not UTF-8 PEM")?;
    let mut der = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let body = &rest[start + BEGIN.len()..];
        let Some(end) = body.find(END) else {
            bail!("GKE cluster CA certificate has a PEM block with no END marker");
        };
        let base64: String = body[..end].split_whitespace().collect();
        der.push(
            BASE64_STANDARD
                .decode(&base64)
                .context("decode a GKE cluster CA PEM block")?,
        );
        rest = &body[end + END.len()..];
    }
    if der.is_empty() {
        bail!("GKE cluster CA certificate carries no PEM certificate block");
    }
    Ok(der)
}

fn kubernetes_client(connection: GkeClusterConnection, token: String) -> Result<KubernetesClient> {
    // The GKE API server is HTTPS, and `kube` handshakes through rustls,
    // which panics rather than choosing between the two providers this
    // workspace links. `store::surreal::connect` installs one for its own
    // endpoint; this is a different process seam and reaches no Surreal
    // engine, so it installs one too. Without it `ops observability`
    // aborts mid-run — after applying the collector and before the
    // `Rules` object that needs the operator it was about to wait for.
    store::surreal::install_tls_provider();
    let cluster_url = format!("https://{}", connection.endpoint)
        .parse()
        .context("parse GKE cluster endpoint")?;
    let mut config = KubernetesConfig::new(cluster_url);
    config.root_cert = Some(connection.ca_certificate);
    config.auth_info.token = Some(token.into());
    KubernetesClient::try_from(config).context("create GKE Kubernetes API client")
}

fn gmp_operator_is_ready(endpoints: Option<&Endpoints>) -> bool {
    endpoints
        .and_then(|endpoints| endpoints.subsets.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|subset| subset.addresses.as_ref())
        .any(|addresses| !addresses.is_empty())
}

/// Step 3 — patch `navigator-web` and `workflows-service` so the binary
/// container `envFrom`s the `navigator-otel-env` `ConfigMap` (alongside the
/// existing Secret). That supplies `OTEL_EXPORTER_OTLP_ENDPOINT`, which is
/// what flips `telemetry::init` from stdout-only to JSON + OTLP export.
fn wire_binaries(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    for (deployment, container) in [
        (WEB_DEPLOYMENT, WEB_CONTAINER),
        (WORKFLOWS_DEPLOYMENT, WORKFLOWS_CONTAINER),
    ] {
        if !dry_run && !deployment_exists(cfg, deployment)? {
            eprintln!(
                "==> {deployment} does not exist yet; ship will create it with \
                 {OTEL_ENV_CONFIGMAP} already wired"
            );
            continue;
        }
        let patch = envfrom_patch(container, &cfg.secret_name);
        eprintln!("==> wiring {deployment} ({container}) → {OTEL_ENV_CONFIGMAP}");
        exec(
            dry_run,
            Command::new("kubectl")
                .args(["--context", &cfg.context, "-n", &cfg.namespace])
                .args(["patch", "deployment", deployment, "--type=strategic", "-p"])
                .arg(&patch),
        )?;
    }
    Ok(())
}

fn deployment_exists(cfg: &ShipConfig, deployment: &str) -> Result<bool> {
    let output = Command::new("kubectl")
        .args(["--context", &cfg.context, "-n", &cfg.namespace])
        .args(["get", "deployment", deployment])
        .output()
        .with_context(|| format!("probe deployment/{deployment}"))?;
    if output.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("NotFound") || stderr.contains("not found") {
        return Ok(false);
    }
    bail!(
        "could not probe deployment/{deployment} in {}: {}",
        cfg.namespace,
        stderr.trim()
    )
}

/// The Collector GSA's full email for a project.
fn gsa_email(project_id: &str) -> String {
    format!("{OTEL_GSA}@{project_id}.iam.gserviceaccount.com")
}

/// True when the GSA already exists — `gcloud … describe` exits non-zero
/// when it does not, which we map to "create it". Under `--dry-run` we
/// assume absent so the create command is the one printed.
fn gsa_exists(cfg: &ShipConfig, gsa: &str) -> Result<bool> {
    let status = Command::new("gcloud")
        .args(["iam", "service-accounts", "describe", gsa])
        .arg(format!("--project={}", cfg.project_id))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("probe whether the navigator-otel GSA exists")?;
    Ok(status.success())
}

/// Render a deploy-side manifest by substituting the project and namespace
/// placeholders. Pure so the substitution is unit-testable.
#[must_use]
pub fn render_manifest(template: &str, project_id: &str, namespace: &str) -> String {
    template
        .replace(PROJECT_PLACEHOLDER, project_id)
        .replace(NAMESPACE_PLACEHOLDER, namespace)
}

/// Build the strategic-merge patch that rewrites a single container's
/// `envFrom` to carry the `OTel` `ConfigMap` first, then the existing Secret.
/// `envFrom` has no strategic-merge key, so the whole list is replaced —
/// hence both entries are restated. Pure + deterministic so it is
/// unit-testable.
#[must_use]
pub fn envfrom_patch(container: &str, secret_name: &str) -> String {
    format!(
        concat!(
            r#"{{"spec":{{"template":{{"spec":{{"containers":[{{"#,
            r#""name":"{container}","envFrom":["#,
            r#"{{"configMapRef":{{"name":"{cm}"}}}},"#,
            r#"{{"secretRef":{{"name":"{secret}"}}}}"#,
            r#"]}}]}}}}}}}}"#,
        ),
        container = container,
        cm = OTEL_ENV_CONFIGMAP,
        secret = secret_name,
    )
}

/// Run a command, or — under `--dry-run` — print it instead.
fn exec(dry_run: bool, cmd: &mut Command) -> Result<()> {
    if dry_run {
        let mut line = cmd.get_program().to_string_lossy().into_owned();
        for arg in cmd.get_args() {
            line.push(' ');
            line.push_str(&arg.to_string_lossy());
        }
        eprintln!("DRY-RUN $ {line}");
        Ok(())
    } else {
        run(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_every_project_placeholder() {
        let rendered = render_manifest(OTEL_COLLECTOR_YAML, "my-org-prod", "example-a");
        // The bundled collector manifest carries the placeholder exactly
        // twice (WI GSA annotation + googlecloud exporter project); both
        // must be substituted and none left behind.
        assert!(!rendered.contains(PROJECT_PLACEHOLDER));
        assert!(!rendered.contains(NAMESPACE_PLACEHOLDER));
        assert!(rendered.contains("navigator-otel@my-org-prod.iam.gserviceaccount.com"));
        assert!(rendered.contains("project: my-org-prod"));
        assert!(rendered.contains("namespace: example-a"));
        assert!(rendered.contains(
            "OTEL_EXPORTER_OTLP_ENDPOINT: \"http://otel-collector.example-a.svc.cluster.local:4317\""
        ));
        assert!(rendered.contains("name: DASH0_ENDPOINT"));
        assert!(rendered.contains("value: YOUR_DASH0_ENDPOINT"));
        assert!(rendered.contains("name: DASH0_DATASET"));
        assert!(rendered.contains("value: YOUR_DASH0_DATASET"));
        assert!(rendered.contains("name: DASH0_TOKEN"));
        assert!(rendered.contains("key: DASH0_TOKEN"));
    }

    #[test]
    fn render_substitutes_namespace_in_self_monitoring_manifest() {
        let rendered = render_manifest(COLLECTOR_MONITORING_YAML, "my-org-prod", "example-b");
        assert!(!rendered.contains(NAMESPACE_PLACEHOLDER));
        assert!(rendered.contains("namespace: example-b"));
    }

    #[test]
    fn gsa_email_follows_the_iam_convention() {
        assert_eq!(
            gsa_email("my-org-prod"),
            "navigator-otel@my-org-prod.iam.gserviceaccount.com"
        );
    }

    #[test]
    fn envfrom_patch_lists_configmap_then_secret_for_the_named_container() {
        let patch = envfrom_patch("web", "example-a-web-secrets");
        let parsed: serde_json::Value =
            serde_json::from_str(&patch).expect("envFrom patch must be valid JSON");
        let containers = &parsed["spec"]["template"]["spec"]["containers"];
        assert_eq!(containers[0]["name"], "web");
        let env_from = &containers[0]["envFrom"];
        // `ConfigMap` first (collector endpoint), Secret second (preserved).
        assert_eq!(env_from[0]["configMapRef"]["name"], OTEL_ENV_CONFIGMAP);
        assert_eq!(env_from[1]["secretRef"]["name"], "example-a-web-secrets");
    }

    #[test]
    fn envfrom_patch_targets_the_worker_container_for_workflows_service() {
        let patch = envfrom_patch("worker", "example-b-web-secrets");
        let parsed: serde_json::Value = serde_json::from_str(&patch).unwrap();
        assert_eq!(
            parsed["spec"]["template"]["spec"]["containers"][0]["name"],
            "worker"
        );
        assert_eq!(
            parsed["spec"]["template"]["spec"]["containers"][0]["envFrom"][1]["secretRef"]["name"],
            "example-b-web-secrets"
        );
    }

    #[test]
    fn gmp_operator_readiness_requires_a_ready_endpoint_address() {
        use k8s_openapi::api::core::v1::{EndpointAddress, EndpointSubset};

        assert!(!gmp_operator_is_ready(None));
        assert!(!gmp_operator_is_ready(Some(&Endpoints::default())));

        let not_ready = Endpoints {
            subsets: Some(vec![EndpointSubset {
                not_ready_addresses: Some(vec![EndpointAddress {
                    ip: "10.0.0.1".into(),
                    ..EndpointAddress::default()
                }]),
                ..EndpointSubset::default()
            }]),
            ..Endpoints::default()
        };
        assert!(!gmp_operator_is_ready(Some(&not_ready)));

        let ready = Endpoints {
            subsets: Some(vec![EndpointSubset {
                addresses: Some(vec![EndpointAddress {
                    ip: "10.0.0.1".into(),
                    ..EndpointAddress::default()
                }]),
                ..EndpointSubset::default()
            }]),
            ..Endpoints::default()
        };
        assert!(gmp_operator_is_ready(Some(&ready)));
    }

    /// A cluster CA the way the Container API hands it over: base64 of PEM
    /// *text*, not base64 of a certificate. The distinction is the whole
    /// bug — a fixture that skipped the PEM armor let a client that cannot
    /// parse PEM pass its test and then fail every real handshake with
    /// rustls's `invalid peer certificate: BadEncoding`.
    fn base64_pem(bodies: &[&[u8]]) -> String {
        use std::fmt::Write as _;
        let mut pem = String::new();
        for body in bodies {
            let _ = writeln!(
                pem,
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
                BASE64_STANDARD.encode(body)
            );
        }
        BASE64_STANDARD.encode(pem)
    }

    #[test]
    fn gke_connection_uses_the_container_api_endpoint_and_ca() {
        let connection = parse_gke_cluster_connection(&format!(
            r#"{{
                "endpoint":"10.0.0.1",
                "status":"RUNNING",
                "masterAuth":{{"clusterCaCertificate":"{}"}}
            }}"#,
            base64_pem(&[b"ca-bytes"])
        ))
        .expect("running cluster connection must parse");
        assert_eq!(connection.endpoint, "10.0.0.1");
        assert_eq!(connection.ca_certificate, vec![b"ca-bytes".to_vec()]);
    }

    #[test]
    fn a_ca_chain_keeps_every_certificate() {
        // Dropping every block but the first would fail only for the
        // clusters whose CA is issued under a chain — the worst shape of
        // bug to carry into one deployment out of several.
        let der = pem_certificates_to_der(
            &BASE64_STANDARD
                .decode(base64_pem(&[b"leaf", b"intermediate"]))
                .expect("fixture decodes"),
        )
        .expect("a chain parses");
        assert_eq!(der, vec![b"leaf".to_vec(), b"intermediate".to_vec()]);
    }

    #[test]
    fn a_ca_with_no_pem_block_is_refused() {
        // What the old fixture actually encoded. It must not be mistaken
        // for a certificate: failing here names the encoding, while
        // passing it through blames the cluster's certificate instead.
        let error = pem_certificates_to_der(b"ca-bytes")
            .expect_err("a payload with no PEM armor is not a certificate");
        assert!(
            error.to_string().contains("no PEM certificate block"),
            "the abort must name the missing armor: {error}"
        );
    }

    #[test]
    fn a_truncated_pem_block_is_refused() {
        let error = pem_certificates_to_der(b"-----BEGIN CERTIFICATE-----\nY2E=\n")
            .expect_err("a block with no END marker is malformed");
        assert!(error.to_string().contains("no END marker"), "{error}");
    }
}
