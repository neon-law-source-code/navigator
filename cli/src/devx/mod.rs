// devx — developer-experience tool that brings up Neon Law Navigator
// dependency stack (Garage, Rauthy, Restate, SurrealDB, OpenObserve, and
// ClamAV) inside a KIND cluster while leaving the `web` binary on the host so
// it can be restarted in-process during a Rust edit-compile
// loop.
//
// The KIND cluster + manifests are also what `devx deploy` uses.
// Both flows drive Kustomize overlays under `k8s/overlays/`:
//   - `kind-deps` — base + deps + workflows-service (no `web`),
//                   used by `devx up` for host-side iteration
//   - `kind`      — full local stack including `web`,
//                   used by `devx deploy`
//   - `gke`       — production overlay; `ops ship` renders and
//                   reconciles this in GKE Autopilot

use std::env;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Subcommand;

pub mod brand;
mod browser_e2e;
mod chrome;
pub mod deployments;
mod dns;
mod doctor;
mod e2e;
mod garage;
mod gcp;
pub(crate) mod github_setup;
mod lifecycle;
mod native;
mod observability;
mod orchestrate;
pub(crate) mod registry;
mod runtime;
mod sample_project;
mod ship;
mod staging;
mod surreal;
mod webapp;
mod worktree_env;

pub use github_setup::RepositoryTarget;
pub use lifecycle::Action as StagingAction;
pub use worktree_env::WorktreeEnvCmd;

// Machine-bound orchestration lives in `orchestrate` (excluded from the
// coverage gate in `ci.yml` because every one of its functions drives
// KIND/Kubernetes/Docker or a host process). Re-export its `pub(super)`
// entry points so `dispatch` below and the sibling devx modules reach them
// as `super::<name>`, exactly as when they lived here.
use orchestrate::{
    align_rauthy_public_url, configure_worktree_kubeconfig, deploy, down, down_in,
    hydrate_garage_environment, kind_down_only, kind_up_only, kustomize_render, logs, print_env,
    rauthy_deployment_exists, reload_worker, require_auth, require_tools, run, status, undeploy,
    up, up_in, use_kind_context, wait_for_condition, wait_for_tcp, wait_rollout,
};

// KIND/local defaults. Each pairs with a `KindConfig` field and a
// `NAVIGATOR_*` env var (see `KindConfig::from_env`). The constants are
// the fallback an empty `.env` resolves to, so default behavior is
// byte-for-byte what the old inline `const`s gave.
const DEFAULT_CLUSTER_NAME: &str = "navigator";
const DEFAULT_NAMESPACE: &str = "navigator";
// Exact upstream KIND manifest from ingress-nginx controller-v1.11.2:
// deploy/static/provider/kind/deploy.yaml
// SHA-256: dc850e38ca4abcb08625f1601f0656c81b6b5e34cc23d5458c332060732c14e0
const INGRESS_MANIFEST: &str = "k8s/vendor/ingress-nginx-controller-v1.11.2.yaml";
#[cfg(test)]
const INGRESS_MANIFEST_SHA256: &str =
    "dc850e38ca4abcb08625f1601f0656c81b6b5e34cc23d5458c332060732c14e0";

// Restate Operator — same chart drives KIND and GKE. Release notes:
// https://github.com/restatedev/restate-operator/releases. Kept aligned
// with the server image in `k8s/staging/restate.yaml` and
// `RESTATE_CLI_VERSION` below (2.8.1 / 1.7.2), and with the chart
// version in `.github/workflows/deploy.yml`. NOTE: the local "restate
// won't provision" wedge was NOT a version skew — it was the operator's
// own `deny-all` NetworkPolicy being enforced by recent kindnet, which
// blocked the operator→node `:5122` provisioning dial. The fix lives in
// `restate.yaml` (`security.disableNetworkPolicies: true`), not here.
const RESTATE_OPERATOR_VERSION: &str = "2.8.1";

// The Operator's CRDs, applied explicitly by `install_restate_operator`.
//
// Chart 2.8.1 moved these into a native Helm `crds/` directory (gated by
// the chart's `installCrds`, default on). Helm installs a native CRD once
// and never touches it again on `helm upgrade` — a deliberately
// conservative lifecycle — so the chart alone would freeze a long-lived
// cluster's CRD schema at whatever version first created it while a fresh
// cluster got the current one. That silent split is exactly the drift the
// pinned constants exist to prevent, so apply the release's CRD artifacts
// on every `up` and let both converge.
const RESTATE_CRDS: [&str; 3] = [
    "restateclusters",
    "restatedeployments",
    "restatecloudenvironments",
];
// Exact v2.8.1 release artifact SHA-256 digests.
#[cfg(test)]
const RESTATE_CRD_SHA256: [(&str, &str); 3] = [
    (
        "restateclusters",
        "f8fa6a9f6cb0233b90e7a8860f62fd6c6cd6b0633776b9a269496ffe9d1fb5a5",
    ),
    (
        "restatedeployments",
        "a8de718e448c99856303553403dce047ed884b26dd0312cd36d9c5a1dda45dfa",
    ),
    (
        "restatecloudenvironments",
        "c63f92508ec4a3862ee399f629fab58f63813d02de08d190e63c2c772f15c01b",
    ),
];
const RESTATE_CRD_DIR: &str = "k8s/vendor/restate-operator-v2.8.1";

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write as _;

    let mut output = String::with_capacity(64);
    for byte in sha2::Sha256::digest(bytes) {
        write!(output, "{byte:02x}").expect("write a SHA-256 digest to a string");
    }
    output
}

#[cfg(test)]
mod ingress_manifest_tests {
    use super::*;

    #[test]
    fn ingress_manifest_is_vendored_and_pinned() {
        assert!(
            !INGRESS_MANIFEST.contains("://"),
            "worktree bootstrap must not ask kubectl to download the ingress manifest"
        );

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent");
        let manifest = std::fs::read(root.join(INGRESS_MANIFEST))
            .expect("read the vendored ingress-nginx manifest");
        assert_eq!(
            sha256_hex(&manifest),
            INGRESS_MANIFEST_SHA256,
            "vendored manifest must match the recorded upstream digest"
        );
        assert!(
            std::str::from_utf8(&manifest)
                .expect("vendored ingress-nginx manifest is UTF-8")
                .contains("registry.k8s.io/ingress-nginx/controller:v1.11.2"),
            "vendored manifest must match the pinned ingress-nginx release"
        );
    }
}

// Restate CLI — pinned so operator-laptop and CI-runner versions
// don't drift. The CLI talks to Restate Cloud's admin API; a
// mismatch silently desyncs `deployments register`. 1.6.x renamed
// `deployment` to `deployments` and `--env` to `--environment`.
// Tracks the server line (see `RESTATE_OPERATOR_VERSION` above and
// the server image in `restate.yaml`) — keep all three on 1.7.x.
// Only `check_restate_cli_version` reads this: it warns when the CLI
// on PATH drifts. There is no CI env to keep in step.
const RESTATE_CLI_VERSION: &str = "1.7.2";

// Last-resort public HTTPS endpoint Restate Cloud uses to reach the
// `workflows-service` worker. Backed by the ingress + managed cert
// under `examples/deploy/k8s/gke/ingress/workflows-*.yaml`. This is a
// placeholder of *last* resort: the URL is resolved by
// [`resolve_workflows_url`], which prefers an explicit `--url` /
// `NAVIGATOR_WORKFLOWS_URL`, then derives `https://workflows.<domain>/`
// from the resolved `brand.primary_domain` (Neon Law by default, or a custom
// bundle's), and only falls back to this constant when none of those are set.
// Hitting this constant in prod means the
// re-register silently no-ops (the 2026-06-10 ship symptom).
pub(crate) const WORKFLOWS_PUBLIC_URL: &str = "https://workflows.example.com/";

// Local `:dev` image tags KIND loads. CI publishes the real images to
// the private Artifact Registry (`YY.M.D`); `pull_retag_load` pulls one
// and retags it to the `:dev` name the manifests reference, so the
// overlays stay unchanged. The trigger images
// (archives/billing/heartbeat) are pulled straight from the
// registry by their CronJobs in prod and are never loaded into the local
// cluster, so they need no local tag constant here.
const WEB_IMAGE: &str = "navigator-web:dev";
const WORKFLOWS_SERVICE_IMAGE: &str = "navigator-workflows-service:dev";
const GATEWAY_IMAGE: &str = "navigator-gateway:dev";

// Kustomize overlay roots. `Up` applies the deps-only overlay (no
// in-cluster `web`). `Deploy` applies the full overlay including
// `web`. The `gke` overlay is reconciled by Config Sync in
// production — `dev kustomize gke` renders it locally for inspection.
const DEFAULT_KUSTOMIZE_KIND_DEPS: &str = "k8s/overlays/kind-deps";
const DEFAULT_KUSTOMIZE_KIND: &str = "k8s/overlays/kind";
// The full overlay again, plus the `private-mode` component: a Pingora
// sidecar in front of `web` that checks network then HTTP basic auth. Selected
// instead of the plain full overlay when `NAVIGATOR_PRIVATE_MODE` is
// affirmative. `kubectl kustomize` has no "add a component" flag, so the
// toggle is overlay selection — which keeps `dev kustomize kind` rendering
// exactly what `dev deploy` would apply.
const DEFAULT_KUSTOMIZE_KIND_PRIVATE: &str = "k8s/overlays/kind-private";
// GKE overlay lives under `examples/deploy/k8s/gke/` (moved out of
// the canonical `k8s/` tree for the product release — the prod
// overlay is now an example users adapt, not a hard-coded part of
// the workspace surface). Config Sync in NeonLaw's prod cluster
// reconciles the same path, just under the new directory.
const DEFAULT_KUSTOMIZE_GKE: &str = "examples/deploy/k8s/gke";

// Host-side ports the locally-run `web` binary connects to.
// Restate's ingress port (8080 in-cluster) is remapped to host 9080
// because KIND already binds host 8080 to its nginx ingress.
const DEFAULT_INGRESS_HTTP_HOST_PORT: u16 = 8080;
const DEFAULT_INGRESS_HTTPS_HOST_PORT: u16 = 8443;
const DEFAULT_RESTATE_INGRESS_HOST_PORT: u16 = 9080;
const DEFAULT_RESTATE_ADMIN_HOST_PORT: u16 = 9070;
const DEFAULT_CLAMAV_HOST_PORT: u16 = 13310;
// Rauthy is exposed through KIND's fixed NodePort mapping. Garage S3
// uses a normal `kubectl port-forward`, so existing clusters pick up a
// changed host port without being recreated.
const DEFAULT_RAUTHY_HOST_PORT: u16 = 30080;
const LOCAL_RAUTHY_CLIENT_SECRET: &str =
    "navigatorwebsecretnavigatorwebsecretnavigatorwebsecretnavigatorw";
const DEFAULT_GARAGE_S3_HOST_PORT: u16 = 30900;
// SurrealDB (the `surreal` Service in `navigator`), the store the
// workspace is porting onto (#1093). Its own port 8000 is a crowded
// default on a developer's machine, and 8080 is already KIND's ingress,
// so the host mapping is 18000 — clear of every other port this module
// binds.
const DEFAULT_SURREAL_HOST_PORT: u16 = 18000;
/// The namespace every local environment selects. One namespace per
/// deployment is the production shape (#1093); locally there is one.
const SURREAL_NAMESPACE: &str = "navigator";
/// Root credentials for the disposable in-cluster engine. They match
/// `k8s/overlays/kind/surreal/surreal.yaml`'s Secret — a KIND-only
/// convenience, never a deployment's credentials.
const SURREAL_LOCAL_USER: &str = "root";
const SURREAL_LOCAL_PASSWORD: &str = "root";

// Local `web` defaults — matches `cargo run -p neon` defaults.
const DEFAULT_LOCAL_WEB_PORT: u16 = 3001;

// OpenObserve's UI (5080) and direct OTLP gRPC ingest port (5081) are
// port-forwarded to the host for the operator and host-side `web` process.
const DEFAULT_OPENOBSERVE_HOST_PORT: u16 = 5080;
const DEFAULT_OPENOBSERVE_OTLP_HOST_PORT: u16 = 5081;
const KIND_OPENOBSERVE_USERNAME: &str = "root@example.com";
const KIND_OPENOBSERVE_PASSWORD: &str = "NavigatorKindOpenObserve1!";
const KIND_OPENOBSERVE_ORGANIZATION: &str = "default";
const KIND_OPENOBSERVE_STREAM: &str = "default";

/// Every KIND/local knob `devx` reads, resolved once in `main()` and
/// threaded into the subcommands. Each field falls back to a
/// `DEFAULT_*` constant, so an empty `.env` reproduces prior behavior
/// exactly. New local knobs are added here and in `from_env`, never as
/// a scattered `env::var` at a call site. See `docs/env-driven-devx.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KindConfig {
    cluster: String,
    namespace: String,
    deps_overlay: String,
    full_overlay: String,
    gke_overlay: String,
    ingress_http_port: u16,
    ingress_https_port: u16,
    restate_ingress_port: u16,
    restate_admin_port: u16,
    clamav_port: u16,
    rauthy_port: u16,
    garage_s3_port: u16,
    surreal_port: u16,
    web_port: u16,
    openobserve_port: u16,
    openobserve_otlp_port: u16,
}

impl KindConfig {
    /// The kubectl context for this cluster. Every cluster-mutating command
    /// passes it explicitly rather than relying on the current context: a
    /// stale GKE/EKS context is otherwise indistinguishable from a fresh KIND
    /// boot, and the command lands on the wrong cluster.
    fn kind_context(&self) -> String {
        format!("kind-{}", self.cluster)
    }

    /// Resolve the KIND/local config from the environment, falling back
    /// to the `DEFAULT_*` constants for any var that is unset or empty.
    fn from_env() -> Self {
        Self {
            cluster: env_string("NAVIGATOR_KIND_CLUSTER", DEFAULT_CLUSTER_NAME),
            namespace: env_string("NAVIGATOR_K8S_NAMESPACE", DEFAULT_NAMESPACE),
            deps_overlay: env_string("NAVIGATOR_KIND_DEPS_OVERLAY", DEFAULT_KUSTOMIZE_KIND_DEPS),
            full_overlay: env_string(
                "NAVIGATOR_KIND_OVERLAY",
                if private_mode_from_env() {
                    DEFAULT_KUSTOMIZE_KIND_PRIVATE
                } else {
                    DEFAULT_KUSTOMIZE_KIND
                },
            ),
            gke_overlay: env_string("NAVIGATOR_GKE_OVERLAY", DEFAULT_KUSTOMIZE_GKE),
            ingress_http_port: env_port(
                "NAVIGATOR_KIND_INGRESS_HTTP_PORT",
                DEFAULT_INGRESS_HTTP_HOST_PORT,
            ),
            ingress_https_port: env_port(
                "NAVIGATOR_KIND_INGRESS_HTTPS_PORT",
                DEFAULT_INGRESS_HTTPS_HOST_PORT,
            ),
            restate_ingress_port: env_port(
                "NAVIGATOR_KIND_RESTATE_INGRESS_PORT",
                DEFAULT_RESTATE_INGRESS_HOST_PORT,
            ),
            restate_admin_port: env_port(
                "NAVIGATOR_KIND_RESTATE_ADMIN_PORT",
                DEFAULT_RESTATE_ADMIN_HOST_PORT,
            ),
            clamav_port: env_port("NAVIGATOR_KIND_CLAMAV_PORT", DEFAULT_CLAMAV_HOST_PORT),
            rauthy_port: env_port("NAVIGATOR_KIND_RAUTHY_PORT", DEFAULT_RAUTHY_HOST_PORT),
            garage_s3_port: env_port("NAVIGATOR_KIND_GARAGE_S3_PORT", DEFAULT_GARAGE_S3_HOST_PORT),
            surreal_port: env_port("NAVIGATOR_KIND_SURREAL_PORT", DEFAULT_SURREAL_HOST_PORT),
            web_port: env_port("NAVIGATOR_KIND_WEB_PORT", DEFAULT_LOCAL_WEB_PORT),
            openobserve_port: env_port(
                "NAVIGATOR_KIND_OPENOBSERVE_PORT",
                DEFAULT_OPENOBSERVE_HOST_PORT,
            ),
            openobserve_otlp_port: env_port(
                "NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT",
                DEFAULT_OPENOBSERVE_OTLP_HOST_PORT,
            ),
        }
    }
}

/// The `kubectl` argv that switches the current context to a KIND cluster.
/// Pure so the pin — the guard that keeps a devx write off an ambient (possibly
/// prod) context — stays unit-tested in this coverage-counted module while
/// `orchestrate::use_kind_context` stays process plumbing.
fn use_context_args(context: &str) -> Vec<String> {
    ["config", "use-context", context]
        .iter()
        .map(|arg| (*arg).to_string())
        .collect()
}

/// The `kubectl` argv that deletes the KIND namespace boundary. Pure so the
/// `--context` pin keeps a unit test in this (coverage-counted) module while
/// `orchestrate::undeploy` stays process plumbing.
fn undeploy_args(context: &str, namespace: &str) -> Vec<String> {
    [
        "--context",
        context,
        "delete",
        "--ignore-not-found",
        "namespace",
        namespace,
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect()
}

/// Read a string env var, treating unset *and* empty as "use default".
/// Empty-as-default keeps a `FOO=` line in `.env` from blanking a path.
fn env_string(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Private mode — put a Pingora gateway in front of `web`. `/health` stays
/// open; every other request passes network allowlisting then HTTP basic auth.
/// The one flag both halves of the
/// orchestration read: `KindConfig::from_env` selects the
/// `kind-private` overlay with it, and `ship` appends the
/// `private-mode` component to the rendered GKE tree with it, so a
/// deployment is private in the same way locally and in production.
///
/// Off unless affirmatively on: unset, empty, `0`, and `false` all leave
/// the public topology alone, because a typo in this var must never
/// silently un-gate a deployment someone believes is private. The
/// affirmative set matches `portal::oauth::self_signup_enabled`.
pub(super) fn private_mode_from_env() -> bool {
    private_mode(env::var("NAVIGATOR_PRIVATE_MODE").ok().as_deref())
}

/// The parse behind [`private_mode_from_env`], split from the `env` read
/// so it is unit-tested without mutating the process environment.
fn private_mode(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Read a `u16` port env var, falling back to `default` when unset,
/// empty, or unparseable. An invalid value falls back rather than
/// crashing the dev loop — the default is always a working port.
fn env_port(key: &str, default: u16) -> u16 {
    env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(default)
}

#[derive(Subcommand)]
pub enum DnsCmd {
    /// Provision a domain's public-deploy DNS through `DNSimple`:
    /// reachability, the apex→www redirect, and both mail lanes (human
    /// mail on the apex, application mail on a `parse.` subdomain). Every
    /// group is opt-in via a flag and idempotent (matching record → no-op,
    /// single-valued drift → patched, missing → created; never deleted).
    /// Zone from `--domain` or `DNS_ZONE`; auth from `DNS_ACCT` +
    /// `DNS_SIMPLE` (`DNSIMPLE_API_TOKEN` is a legacy alias). The per-domain `SendGrid` / Google secrets are
    /// flags — never invented. Full recipe: `docs/dns.md`.
    Setup {
        /// Domain to configure; defaults to `DNS_ZONE`.
        #[arg(long, env = "DNS_ZONE")]
        domain: Option<String>,
        /// Selected host `A` records → this gateway IP. Falls back to
        /// `NAVIGATOR_GATEWAY_IP` from the deployment config.
        #[arg(long, env = "NAVIGATOR_GATEWAY_IP")]
        gateway_ip: Option<String>,
        /// Extra `A` host label → gateway IP (repeatable; default `www`, `workflows`).
        #[arg(long = "host")]
        hosts: Vec<String>,
        /// Apex `URL` record → `https://www.<domain>` (301 apex → www, via `DNSimple`'s redirector).
        #[arg(long)]
        redirect_apex_to_www: bool,
        /// Apex `MX` `smtp.google.com` + `_spf.google.com` in SPF (Google Workspace).
        #[arg(long)]
        google_workspace: bool,
        /// Apex `google-site-verification` token.
        #[arg(long)]
        google_site_verification: Option<String>,
        /// `parse.` subdomain `MX` `mx.sendgrid.net` + `sendgrid.net` in SPF.
        #[arg(long)]
        sendgrid: bool,
        /// `SendGrid` `DKIM` `CNAME` target (repeatable → `s1._domainkey`, `s2._domainkey`, … in order).
        #[arg(long = "dkim-target")]
        dkim_targets: Vec<String>,
        /// `SendGrid` link-branding `CNAME` as `label=target` (repeatable).
        #[arg(long = "sendgrid-link-brand", value_parser = parse_label_target)]
        sendgrid_link_brand: Vec<(String, String)>,
        /// Extra SPF `include:` mechanism (repeatable; e.g. `amazonses.com`).
        #[arg(long = "spf-include")]
        spf_includes: Vec<String>,
        /// `DMARC` policy; adds a `_dmarc` `TXT` record.
        #[arg(long, value_enum)]
        dmarc: Option<dns::DmarcPolicy>,
        /// `DMARC` report address (default `mailto:postmaster@<domain>`).
        #[arg(long)]
        dmarc_rua: Option<String>,
        /// Preview the `DNSimple` calls without sending any traffic.
        #[arg(long)]
        dry_run: bool,
    },
}

/// clap value parser for `--sendgrid-link-brand label=target`. Both sides
/// must be non-empty — an empty label or target is an invalid CNAME and is
/// rejected at the CLI boundary rather than failing later as a provider error.
fn parse_label_target(raw: &str) -> std::result::Result<(String, String), String> {
    match raw.split_once('=') {
        Some((label, target)) => {
            let label = label.trim();
            let target = target.trim();
            if label.is_empty() || target.is_empty() {
                return Err(format!(
                    "expected `label=target` with a non-empty label and target, got `{raw}`"
                ));
            }
            Ok((label.to_string(), target.to_string()))
        }
        _ => Err(format!(
            "expected `label=target` with a non-empty label and target, got `{raw}`"
        )),
    }
}

// Clap materializes this only while parsing one operator command. Keeping the
// fields together makes `ops gcp setup --help` and the dispatcher auditable;
// heap indirection on individual options would obscure that contract.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum GcpCmd {
    /// Provision network, five GCS buckets, and a GKE Autopilot
    /// cluster in the given Google Cloud project. Safe to re-run:
    /// every step ignores "already exists" responses.
    Setup {
        /// Google Cloud project ID (e.g. `your-project-id`). Falls back to
        /// `NAVIGATOR_GCP_PROJECT_ID` so coordinates exported from a
        /// deployment's `config.toml` can carry the complete target.
        #[arg(long, env = "NAVIGATOR_GCP_PROJECT_ID")]
        project_id: String,
        /// Canonical HTTPS origin allowed to fetch public assets. Falls back
        /// to `NAV_BASE_URL` and must not include a path.
        #[arg(long, env = "NAV_BASE_URL")]
        public_base_url: String,
        /// Region for GKE Autopilot and bucket location.
        /// Falls back to `NAVIGATOR_GCP_LOCATION` then to the
        /// workspace default.
        #[arg(long, env = "NAVIGATOR_GCP_LOCATION")]
        region: Option<String>,
        /// GKE Autopilot cluster name. Falls back to
        /// `NAVIGATOR_GKE_CLUSTER_NAME`.
        #[arg(long, env = "NAVIGATOR_GKE_CLUSTER_NAME")]
        cluster_name: Option<String>,
        /// VPC network name. Falls back to `NAVIGATOR_VPC_NAME`.
        #[arg(long, env = "NAVIGATOR_VPC_NAME")]
        vpc_name: Option<String>,
        /// Regional GKE subnetwork name. Falls back to
        /// `NAVIGATOR_SUBNETWORK_NAME`.
        #[arg(long, env = "NAVIGATOR_SUBNETWORK_NAME")]
        subnetwork_name: Option<String>,
        /// Reserved global static-IP name. Falls back to
        /// `NAVIGATOR_GATEWAY_IP_NAME`.
        #[arg(long, env = "NAVIGATOR_GATEWAY_IP_NAME")]
        gateway_ip_name: Option<String>,
        /// HTTPS URL of the Git repo `Config Sync` should reconcile
        /// from. Falls back to `NAVIGATOR_CONFIG_SYNC_REPO`. Omit to
        /// skip the `RootSync` step entirely — sensible for forks not
        /// running `GitOps` yet.
        #[arg(long, env = "NAVIGATOR_CONFIG_SYNC_REPO")]
        config_sync_repo: Option<String>,
        /// Path inside the repo Config Sync should watch. Falls back
        /// to `NAVIGATOR_CONFIG_SYNC_DIR`.
        #[arg(long, env = "NAVIGATOR_CONFIG_SYNC_DIR")]
        config_sync_dir: Option<String>,
        /// Artifact Registry repository that holds every container image.
        /// Falls back to `NAVIGATOR_GAR_REPO` then the workspace default
        /// (`navigator`). A fork overrides it so its images and IAM
        /// bindings target its own repository.
        #[arg(long, env = "NAVIGATOR_GAR_REPO")]
        artifact_registry_repo: Option<String>,
        /// `owner/repo` slug the Workload Identity provider trusts for
        /// keyless CI pushes. Falls back to `NAVIGATOR_GITHUB_REPO` then
        /// the workspace default (`neon-law-source-code/navigator`). A fork
        /// MUST set this to its own slug, or its GitHub Actions workflow
        /// cannot impersonate the CI pusher service account.
        #[arg(long, env = "NAVIGATOR_GITHUB_REPO")]
        github_repo: Option<String>,
        /// Account id (local part of the SA email) of the CI pusher
        /// service account. Falls back to `NAVIGATOR_CI_PUSHER_ACCOUNT_ID`
        /// then the workspace default (`navigator-ci-pusher`).
        #[arg(long, env = "NAVIGATOR_CI_PUSHER_ACCOUNT_ID")]
        ci_pusher_account_id: Option<String>,
        /// Hub project holding the shared Artifact Registry (for this
        /// workspace, `ghcr`). When set, this environment gets no
        /// registry, CI pusher, or Workload Identity pool of its own — it
        /// only takes `roles/artifactregistry.reader` on the hub repository,
        /// which `ops gcp hub setup` provisioned. Falls back to
        /// `NAVIGATOR_IMAGES_PROJECT_ID`. Omit for the single-project shape.
        #[arg(long, env = "NAVIGATOR_IMAGES_PROJECT_ID")]
        images_project_id: Option<String>,
        /// Public assets bucket for this deployment. Falls back to
        /// `<project-id>-assets`.
        #[arg(long, env = "NAVIGATOR_ASSETS_BUCKET")]
        assets_bucket: Option<String>,
        /// Private documents bucket for this deployment. Falls back to
        /// `<project-id>-documents`.
        #[arg(long, env = "NAVIGATOR_DOCUMENTS_BUCKET")]
        documents_bucket: Option<String>,
        /// Nightly Parquet/Iceberg exports bucket for this deployment. Falls
        /// back to `<project-id>-exports`.
        #[arg(long, env = "NAVIGATOR_EXPORTS_BUCKET")]
        exports_bucket: Option<String>,
        /// Audit-log archive bucket for this deployment. Falls back to
        /// `<project-id>-logs`.
        #[arg(long, env = "NAVIGATOR_LOGS_BUCKET")]
        logs_bucket: Option<String>,
        /// Private applications bucket holding each Project's published
        /// client-portal bundle. Falls back to `<project-id>-applications`.
        #[arg(long, env = "NAVIGATOR_APPLICATIONS_BUCKET")]
        applications_bucket: Option<String>,
        /// The GHE organization that owns this deployment's Project
        /// repositories. Provisions the publisher lane when set together with
        /// at least one `--applications-publisher-repo`; omit both to decline
        /// it. Singular because one Workload Identity provider serves the
        /// deployment and its attribute condition names one owner. Falls back
        /// to `NAVIGATOR_GITHUB_ORG`.
        #[arg(long, env = "NAVIGATOR_GITHUB_ORG")]
        applications_publisher_org: Option<String>,
        /// A Project repository the publisher lane provisions an identity for.
        /// Repeatable, once per Project, and each value is a repository name —
        /// which is also the Project code and the bucket object prefix. Each
        /// gets its own `nav-pub-<code>` service account, because the grant is
        /// conditioned on one prefix and one account can carry only one.
        /// Comma-separated through `NAVIGATOR_APP_PUBLISHER_REPOS`.
        #[arg(
            long = "applications-publisher-repo",
            env = "NAVIGATOR_APP_PUBLISHER_REPOS",
            value_delimiter = ',',
            num_args = 1
        )]
        applications_publisher_repos: Vec<String>,
        /// Long-term Iceberg archive of this deployment's Surreal store
        /// (`neon-law-archives-<deployment>`). No lifecycle rule: this is where
        /// long-term storage lives. Omit to skip the lane entirely — there is
        /// no derived default, because the name is prefix-shaped rather than
        /// `<project-id>-`-shaped.
        #[arg(long, env = "NAVIGATOR_ICEBERG_BUCKET")]
        archives_bucket: Option<String>,
        /// Telemetry landing zone (`<deployment>-telemetry`), where the `OTel`
        /// collector writes Parquet before the nightly lane promotes it into
        /// the archive. Provisioned with a flat 90-day object expiry. Omit to
        /// skip; no derived default, because the project may be named for the
        /// entity rather than the deployment.
        #[arg(long, env = "NAVIGATOR_TELEMETRY_BUCKET")]
        telemetry_bucket: Option<String>,
        /// Per-deployment Google service-account id used by GKE Workload
        /// Identity. Falls back to `navigator-web` for the single-project
        /// OSS shape.
        #[arg(long, env = "NAVIGATOR_GCP_SERVICE_ACCOUNT_ID")]
        google_service_account_id: Option<String>,
        /// Dedicated per-deployment Google service-account id used only for
        /// the delegated Workspace Drive credential. Falls back to
        /// `navigator-drive` for the single-project OSS shape.
        #[arg(long, env = "NAVIGATOR_DRIVE_GCP_SERVICE_ACCOUNT_ID")]
        drive_service_account_id: Option<String>,
        /// Kubernetes namespace containing the runtime service accounts.
        #[arg(long, env = "NAVIGATOR_K8S_NAMESPACE")]
        kubernetes_namespace: Option<String>,
        /// Preview the GCP API calls that would be made, without
        /// sending any traffic. `gcloud` has no universal
        /// equivalent of this flag, so we provide one ourselves.
        #[arg(long)]
        dry_run: bool,
    },
    /// Provision the shared image hub — the Artifact Registry every
    /// environment pulls from, the CI pusher service account, and the GitHub
    /// Workload Identity pool — and nothing else. The hub is not an
    /// environment: this command never creates buckets, GKE, or IAP
    /// resources, and it refuses a project ID recorded as an environment.
    #[command(subcommand)]
    Hub(GcpHubCmd),
    /// Identity-Aware Proxy operations for the `navigator-web`
    /// backend. Run after the GKE Ingress has provisioned the LB.
    /// See `docs/gemini-enterprise-mcp.md`.
    #[command(subcommand)]
    Iap(IapCmd),
}

#[derive(Subcommand)]
pub enum GcpHubCmd {
    /// Create the Artifact Registry repository and its cleanup policy, the CI
    /// pusher service account and its repo-scoped writer binding, and the
    /// GitHub Workload Identity pool, provider, and impersonation binding.
    /// Safe to re-run: every step treats "already exists" as success.
    Setup {
        /// Google Cloud project that holds the shared registry. For this
        /// workspace, `ghcr`. Refused if it is one of the four
        /// environment projects — see `docs/environments.md`.
        #[arg(long)]
        project_id: String,
        /// Artifact Registry location. Falls back to
        /// `NAVIGATOR_GCP_LOCATION` then the workspace default.
        #[arg(long, env = "NAVIGATOR_GCP_LOCATION")]
        region: Option<String>,
        /// Repository name that holds every container image. Falls back to
        /// `NAVIGATOR_GAR_REPO` then the workspace default (`navigator`).
        #[arg(long, env = "NAVIGATOR_GAR_REPO")]
        artifact_registry_repo: Option<String>,
        /// `owner/repo` slug the Workload Identity provider trusts for
        /// keyless CI pushes. Falls back to `NAVIGATOR_GITHUB_REPO` then the
        /// workspace default. A fork MUST set this to its own slug.
        #[arg(long, env = "NAVIGATOR_GITHUB_REPO")]
        github_repo: Option<String>,
        /// Account id (local part of the SA email) of the CI pusher service
        /// account. Falls back to `NAVIGATOR_CI_PUSHER_ACCOUNT_ID`.
        #[arg(long, env = "NAVIGATOR_CI_PUSHER_ACCOUNT_ID")]
        ci_pusher_account_id: Option<String>,
        /// Print the exact hub plan without authenticating or contacting GCP.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum IapCmd {
    /// Print the IAP audience string `portal::iap::IapConfig` validates
    /// against. Format:
    /// `/projects/<PROJECT_NUMBER>/global/backendServices/<SERVICE_ID>`.
    /// Paste the value into `IAP_AUDIENCE` in your GKE overlay
    /// (the example overlay lives at
    /// `examples/deploy/k8s/gke/patches/web-env.yaml`).
    Audience {
        /// Google Cloud project ID (e.g. `your-project-id`).
        #[arg(long)]
        project_id: String,
        /// Compute backend-service name. Defaults to `navigator-web`,
        /// matching the example GKE overlay.
        #[arg(long, default_value = gcp::iap::DEFAULT_SERVICE_NAME)]
        service: String,
    },
    /// Add `--member` to `roles/iap.httpsResourceAccessor` on the
    /// IAP-protected backend service. Idempotent: a no-op when the
    /// principal is already bound. Accepted member formats:
    /// `user:libra@example.com`, `group:lawyer@example.com`,
    /// `serviceAccount:s@p.iam.gserviceaccount.com`, or a bare
    /// OAuth client ID like `12345-abc.apps.googleusercontent.com`.
    Grant {
        /// Google Cloud project ID (e.g. `your-project-id`).
        #[arg(long)]
        project_id: String,
        /// Principal to allow past IAP. See command docstring for
        /// supported formats.
        #[arg(long)]
        member: String,
        /// Compute backend-service name. Defaults to `navigator-web`.
        #[arg(long, default_value = gcp::iap::DEFAULT_SERVICE_NAME)]
        service: String,
    },
}

#[derive(Subcommand)]
pub enum RestateCmd {
    /// Register the `workflows-service` worker with the configured
    /// Restate Cloud environment. Equivalent to
    /// `restate -y deployment register <url>` against
    /// `NAVIGATOR_WORKFLOWS_URL` (or the prod default).
    Register {
        /// Override the public worker URL. When unset, falls back to
        /// `NAVIGATOR_WORKFLOWS_URL`, then derives
        /// `https://workflows.<brand.primary_domain>/`, and only
        /// then the workspace placeholder
        /// (`https://workflows.example.com/`).
        #[arg(long)]
        url: Option<String>,
    },
}

/// Dispatch the local `dev` and operator `ops` subsets of the `navigator` CLI.
/// `main` loads `.env` + `.devx/env` before parsing ordinary local commands,
/// so this only resolves the KIND config and routes. Deployment-scoped
/// commands read the `deployments/` tree through their explicit
/// `--deployment` flag instead. Non-orchestration commands never reach here.
///
/// One big match over every subcommand — readability comes from the flat
/// dispatch, not from splitting it across helpers.
#[allow(clippy::too_many_lines)]
pub fn dispatch(command: crate::Command) -> Result<()> {
    if let crate::Command::Dev(crate::DevCmd::SendgridOpenapi {
        verify,
        regenerate,
        root,
    }) = &command
    {
        if *regenerate {
            return crate::sendgrid_openapi::regenerate(root);
        }
        let _ = verify;
        return crate::sendgrid_openapi::verify(root);
    }
    let cfg = KindConfig::from_env();
    match command {
        crate::Command::Dev(crate::DevCmd::Install) => native::install(),
        crate::Command::Dev(crate::DevCmd::Up) => up(&cfg),
        crate::Command::Dev(crate::DevCmd::Down) => down(&cfg),
        crate::Command::Dev(crate::DevCmd::Env) => print_env(&cfg),
        crate::Command::Dev(crate::DevCmd::Status) => {
            status(&cfg);
            Ok(())
        }
        crate::Command::Dev(crate::DevCmd::WorkerReload) => reload_worker(&cfg),
        crate::Command::Dev(crate::DevCmd::BuildWebapp { release }) => webapp::build(release),
        crate::Command::Dev(crate::DevCmd::Staging(action)) => staging::dispatch_kind(action, &cfg),
        crate::Command::Dev(crate::DevCmd::Kind(crate::KindCmd::Up)) => kind_up_only(&cfg),
        crate::Command::Dev(crate::DevCmd::Kind(crate::KindCmd::Down)) => kind_down_only(&cfg),
        crate::Command::Dev(crate::DevCmd::WorktreeEnv(cmd)) => worktree_env::dispatch(cmd, &cfg),
        crate::Command::Dev(crate::DevCmd::Deploy) => deploy(&cfg, None),
        crate::Command::Dev(crate::DevCmd::Undeploy) => undeploy(&cfg),
        crate::Command::Dev(crate::DevCmd::E2e) => e2e::run_e2e(&cfg),
        crate::Command::Dev(crate::DevCmd::GarageBootstrap) => garage::bootstrap(&cfg),
        crate::Command::Dev(crate::DevCmd::GrantLawyer) => e2e::grant_lawyer(&cfg),
        crate::Command::Dev(crate::DevCmd::SampleProject {
            project,
            repo,
            git_ref,
            keep,
        }) => sample_project::run(
            project.as_deref(),
            repo.as_deref(),
            git_ref.as_deref(),
            keep,
        ),
        crate::Command::Dev(crate::DevCmd::BrowserE2e { base_url }) => {
            browser_e2e::run_browser_e2e(base_url.as_deref())
        }
        crate::Command::Dev(crate::DevCmd::Logs) => logs(&cfg),
        crate::Command::Dev(crate::DevCmd::Kustomize(crate::KustomizeCmd::Kind)) => {
            kustomize_render(&cfg.full_overlay)
        }
        crate::Command::Dev(crate::DevCmd::Kustomize(crate::KustomizeCmd::Gke)) => {
            kustomize_render(&cfg.gke_overlay)
        }
        crate::Command::Ops(crate::OpsCmd::Doctor { namespace }) => {
            doctor::run(namespace.as_deref().unwrap_or(&cfg.namespace))
        }
        crate::Command::Ops(crate::OpsCmd::SurrealArchive(crate::SurrealArchiveCmd::Export)) => {
            crate::surreal_archive::export()
        }
        crate::Command::Ops(crate::OpsCmd::SurrealArchive(
            crate::SurrealArchiveCmd::RestoreDrill { key },
        )) => crate::surreal_archive::restore_drill(&key),
        crate::Command::Ops(crate::OpsCmd::Github(crate::GithubCmd::Setup {
            repository,
            dry_run,
        })) => github_setup::run(&RepositoryTarget::resolve(repository)?, dry_run),
        crate::Command::Ops(crate::OpsCmd::Gcp(GcpCmd::Setup {
            project_id,
            public_base_url,
            region,
            cluster_name,
            vpc_name,
            subnetwork_name,
            gateway_ip_name,
            config_sync_repo,
            config_sync_dir,
            artifact_registry_repo,
            github_repo,
            ci_pusher_account_id,
            images_project_id,
            assets_bucket,
            documents_bucket,
            exports_bucket,
            logs_bucket,
            applications_bucket,
            applications_publisher_org,
            applications_publisher_repos,
            archives_bucket,
            telemetry_bucket,
            google_service_account_id,
            drive_service_account_id,
            kubernetes_namespace,
            dry_run,
        })) => {
            let mut config = gcp::SetupConfig {
                public_base_url: Some(public_base_url),
                images_project_id,
                assets_bucket,
                documents_bucket,
                exports_bucket,
                logs_bucket,
                applications_bucket,
                applications_publisher_org,
                applications_publisher_repos,
                archives_bucket,
                telemetry_bucket,
                ..gcp::SetupConfig::default()
            };
            if let Some(v) = google_service_account_id {
                config.google_service_account_id = v;
            }
            if let Some(v) = drive_service_account_id {
                config.drive_service_account_id = v;
            }
            if let Some(v) = kubernetes_namespace {
                config.kubernetes_namespace = v;
            }
            if let Some(v) = region {
                config.region = v;
            }
            if let Some(v) = cluster_name {
                config.cluster_name = v;
            }
            if let Some(v) = vpc_name {
                config.vpc_name = v;
            }
            if let Some(v) = subnetwork_name {
                config.subnetwork_name = v;
            }
            if let Some(v) = gateway_ip_name {
                config.gateway_ip_name = v;
            }
            if let Some(v) = config_sync_repo {
                config.config_sync_repo = Some(v);
            }
            if let Some(v) = config_sync_dir {
                config.config_sync_dir = v;
            }
            if let Some(v) = artifact_registry_repo {
                config.artifact_registry_repo = v;
            }
            if let Some(v) = github_repo {
                config.github_repo = v;
            }
            if let Some(v) = ci_pusher_account_id {
                config.ci_pusher_account_id = v;
            }
            gcp_setup(project_id, dry_run, config)
        }
        crate::Command::Ops(crate::OpsCmd::Gcp(GcpCmd::Hub(GcpHubCmd::Setup {
            project_id,
            region,
            artifact_registry_repo,
            github_repo,
            ci_pusher_account_id,
            dry_run,
        }))) => {
            let mut config = gcp::hub::HubSetupConfig::default();
            if let Some(v) = region {
                config.region = v;
            }
            if let Some(v) = artifact_registry_repo {
                config.artifact_registry_repo = v;
            }
            if let Some(v) = github_repo {
                config.github_repo = v;
            }
            if let Some(v) = ci_pusher_account_id {
                config.ci_pusher_account_id = v;
            }
            gcp_hub_setup(project_id, dry_run, config)
        }
        crate::Command::Ops(crate::OpsCmd::Gcp(GcpCmd::Iap(IapCmd::Audience {
            project_id,
            service,
        }))) => gcp_iap_audience(&project_id, &service),
        crate::Command::Ops(crate::OpsCmd::Gcp(GcpCmd::Iap(IapCmd::Grant {
            project_id,
            member,
            service,
        }))) => gcp_iap_grant(&project_id, &service, &member),
        crate::Command::Ops(crate::OpsCmd::Restate(RestateCmd::Register { url })) => {
            restate_register(url.as_deref())
        }
        crate::Command::Ops(crate::OpsCmd::Ship {
            deployment,
            deployments_dir,
            dry_run,
            restart_only,
            image_only,
            tag,
            assert_signing_iam,
        }) => ship::run_ship(&ship::ShipOpts {
            deployment,
            deployments_dir,
            dry_run,
            restart_only,
            image_only,
            tag,
            assert_signing_iam,
        }),
        crate::Command::Ops(crate::OpsCmd::Deployments { deployments_dir }) => {
            deployments::check(&deployments::root(deployments_dir.as_deref())?)
        }
        crate::Command::Ops(crate::OpsCmd::Secrets(crate::SecretsCmd::Apply {
            deployment,
            deployments_dir,
            dry_run,
        })) => deployments::apply(
            &deployments::root(deployments_dir.as_deref())?,
            &deployment,
            dry_run,
        ),
        crate::Command::Ops(crate::OpsCmd::Dns(DnsCmd::Setup {
            domain,
            gateway_ip,
            hosts,
            redirect_apex_to_www,
            google_workspace,
            google_site_verification,
            sendgrid,
            dkim_targets,
            sendgrid_link_brand,
            spf_includes,
            dmarc,
            dmarc_rua,
            dry_run,
        })) => dns_setup(
            domain,
            &dns::DnsSetupConfig {
                gateway_ip,
                hosts,
                redirect_apex_to_www,
                google_workspace,
                google_site_verification,
                sendgrid,
                dkim_targets,
                sendgrid_link_brand,
                spf_includes,
                dmarc,
                dmarc_rua,
            },
            dry_run,
        ),
        crate::Command::Ops(crate::OpsCmd::Rebrand(cmd)) => brand::run(cmd),
        crate::Command::Ops(crate::OpsCmd::Observability {
            deployment,
            deployments_dir,
            dry_run,
        }) => observability::run_observability(&observability::ObservabilityOpts {
            deployment,
            deployments_dir,
            dry_run,
        }),
        // `main` only routes the orchestration subset here; the notation /
        // live-site commands (validate, import, login, …) are handled there.
        _ => unreachable!("devx::dispatch received a non-orchestration command"),
    }
}

/// `ops dns setup`: reconcile the desired DNS record set on the zone via
/// `DNSimple`. Zone from `--domain` / `DNS_ZONE`; auth from the provider's
/// `DNS_ACCT` + `DNS_SIMPLE` (or legacy `DNSIMPLE_API_TOKEN`). Builds a private Tokio runtime
/// because the DNS provider is async.
fn dns_setup(domain: Option<String>, config: &dns::DnsSetupConfig, dry_run: bool) -> Result<()> {
    tracing_subscriber::fmt::try_init().ok();
    let zone = domain
        .filter(|z| !z.trim().is_empty())
        .context("no domain: pass --domain or set DNS_ZONE")?;
    config.validate().map_err(|msg| anyhow::anyhow!(msg))?;
    let desired = dns::desired_records(&zone, config);
    if desired.is_empty() {
        eprintln!(
            "no record groups selected — pass flags such as --gateway-ip / --redirect-apex-to-www / \
             --google-workspace / --sendgrid (see `ops dns setup --help`)"
        );
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        let mut provider = dns::DnsimpleProvider::from_env()?;
        if dry_run {
            provider = provider.with_dry_run();
        }
        let report = dns::run_setup(&provider, &zone, &desired).await?;
        for entry in &report {
            eprintln!(
                "==> {:5} {:20} {} : {:?}",
                entry.record_type.as_str(),
                if entry.name.is_empty() {
                    "(root)"
                } else {
                    entry.name.as_str()
                },
                entry.content,
                entry.outcome,
            );
        }
        if dry_run {
            eprintln!(
                "--- dry run: {} call(s) would be made ---",
                provider.recorded_calls().len()
            );
            for call in provider.recorded_calls() {
                eprintln!("{} {}", call.method, call.url);
                if let Some(body) = call.body {
                    eprintln!("  {body}");
                }
            }
        }
        if config.redirect_apex_to_www {
            eprintln!("\n{}", apex_redirect_certificate_notice(&zone));
        }
        Ok::<(), anyhow::Error>(())
    })
}

fn apex_redirect_certificate_notice(zone: &str) -> String {
    format!(
        "note: the apex→www redirect serves HTTPS only once a certificate covers the \
         apex. DNSimple does not auto-issue one for a URL record — issue an auto-renewing \
         Let's Encrypt certificate for {zone} (see docs/dns.md)."
    )
}

/// `devx gcp iap audience`: print the IAP audience string for
/// `IAP_AUDIENCE`. Requires the LB to already exist (apply the gke
/// overlay first); a 404 surfaces as a clear error.
fn gcp_iap_audience(project_id: &str, service: &str) -> Result<()> {
    use std::sync::Arc;

    use gcp::client::{GcpClient, TokenProvider};

    tracing_subscriber::fmt::try_init().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        let token: Arc<dyn TokenProvider> = gcp::auth::adc_token_provider().await?;
        let client = GcpClient::new(token);
        let project_number = gcp::iap::get_project_number(&client, project_id)
            .await
            .context("look up project number")?;
        let svc_id = gcp::iap::get_backend_service_id(&client, project_id, service)
            .await
            .with_context(|| {
                format!(
                    "look up backend service `{service}` (apply k8s/overlays/gke first \
and wait for the GKE Ingress controller to provision the LB)"
                )
            })?;
        let audience = gcp::iap::format_iap_audience(&project_number, &svc_id);
        // The audience string is the only thing the operator pastes
        // into web-env.yaml — print to stdout so it can be captured.
        println!("{audience}");
        eprintln!(
            "==> IAP_AUDIENCE for {service} in {project_id}: paste into \
k8s/overlays/gke/patches/web-env.yaml then kubectl apply -k k8s/overlays/gke"
        );
        Ok::<(), anyhow::Error>(())
    })
}

/// `devx gcp iap grant`: add a principal to `roles/iap.httpsResourceAccessor`
/// on the IAP-protected backend service. Safe to re-run — checks the
/// existing policy and skips setIamPolicy when the binding is already there.
fn gcp_iap_grant(project_id: &str, service: &str, member: &str) -> Result<()> {
    use std::sync::Arc;

    use gcp::client::{GcpClient, TokenProvider};
    use gcp::iap::BindingOutcome;

    tracing_subscriber::fmt::try_init().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        let token: Arc<dyn TokenProvider> = gcp::auth::adc_token_provider().await?;
        let client = GcpClient::new(token);
        let project_number = gcp::iap::get_project_number(&client, project_id)
            .await
            .context("look up project number")?;
        let outcome = gcp::iap::ensure_iap_iam_binding(&client, &project_number, service, member)
            .await
            .with_context(|| format!("bind {member} on {service}"))?;
        match outcome {
            BindingOutcome::Added => {
                eprintln!("==> added {member} to roles/iap.httpsResourceAccessor on {service}");
            }
            BindingOutcome::AlreadyPresent => {
                eprintln!("==> {member} already bound on {service} (no change)");
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Resolve the public worker URL Restate Cloud dials, in precedence
/// order:
///   1. an explicit `--url` override,
///   2. `NAVIGATOR_WORKFLOWS_URL`,
///   3. derived from mounted `brand.primary_domain` as
///      `https://workflows.<domain>/`,
///   4. the [`WORKFLOWS_PUBLIC_URL`] placeholder — only when none of the
///      above are set.
///
/// Step 3 is the hardening from the 2026-06-10 ship: an operator who has
/// a bundle domain but never set the explicit workflows URL
/// now targets their real ingress instead of `workflows.example.com`,
/// which silently no-op'd the re-register. Pure (takes its inputs as
/// args) so it is unit-testable without mutating the process env.
pub(crate) fn resolve_workflows_url(
    url_override: Option<&str>,
    workflows_url_env: Option<&str>,
    primary_domain: Option<&str>,
) -> String {
    fn nonblank(v: &str) -> Option<String> {
        let t = v.trim();
        (!t.is_empty()).then(|| t.to_string())
    }
    url_override
        .and_then(nonblank)
        .or_else(|| workflows_url_env.and_then(nonblank))
        .or_else(|| {
            primary_domain
                .and_then(nonblank)
                .map(|d| format!("https://workflows.{d}/"))
        })
        .unwrap_or_else(|| WORKFLOWS_PUBLIC_URL.to_string())
}

/// `devx restate register [--url <URL>]`: register the `workflows-service`
/// worker with the caller's Restate Cloud environment. The URL is resolved
/// by [`resolve_workflows_url`] (override → `NAVIGATOR_WORKFLOWS_URL` →
/// `https://workflows.<brand.primary_domain>/` → placeholder).
///
/// Two transports, chosen by environment:
/// - When `RESTATE_ADMIN_URL` **and** `RESTATE_ADMIN_TOKEN` are both set
///   (operator-session credentials exported for a headless run — never
///   repository coordinates), register via the
///   admin REST API. This is headless: it needs no `restate cloud env
///   configure` (which requires a TTY) and works with a non-expiring
///   admin-scoped API key, so an unattended `ship` from a fresh
///   machine re-registers without the SSO token or a configured CLI env.
/// - Otherwise shell out to the pinned `restate` CLI (the KIND dev loop and
///   operators who keep the `restate cloud login` SSO token fresh).
///
/// See [`docs/durable-workflows.md`] "step 7d".
fn restate_register(url_override: Option<&str>) -> Result<()> {
    let workflows_url_env = env::var("NAVIGATOR_WORKFLOWS_URL").ok();
    let primary_domain = brand::primary_domain()?;
    let url = resolve_workflows_url(
        url_override,
        workflows_url_env.as_deref(),
        Some(&primary_domain),
    );

    let admin_url = env::var("RESTATE_ADMIN_URL").ok();
    let admin_token = env::var("RESTATE_ADMIN_TOKEN").ok();
    if let (Some(admin_url), Some(admin_token)) = (
        admin_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        admin_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
    ) {
        return register_via_admin_api(admin_url, admin_token, &url);
    }

    require_tools(&["restate"])?;
    check_restate_cli_version();
    eprintln!("==> restate -y deployments register {url}");
    run(Command::new("restate")
        .arg("-y")
        .arg("deployments")
        .arg("register")
        .arg(&url))?;
    eprintln!("==> restate -y deployments list");
    run(Command::new("restate")
        .arg("-y")
        .arg("deployments")
        .arg("list"))
}

/// Force-register the worker deployment via the Restate Cloud admin REST
/// API (`POST {admin}/deployments` with `force: true`), bearer-authenticated.
/// `force` re-runs discovery against the live worker, so every service it
/// exposes is (re)registered; the call is idempotent and safe on every ship.
fn register_via_admin_api(admin_base_url: &str, token: &str, worker_url: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("create tokio runtime")?;
    runtime.block_on(register_via_admin_api_async(
        admin_base_url,
        token,
        worker_url,
    ))
}

/// Force-register attempts before ship gives up. The Restate Cloud admin
/// server proxies discovery to the public worker URL through the GKE load
/// balancer; for a window after a rollout the NEG readiness gate lags and the
/// GFE returns a transient `502` (surfaced as a direct `502`/`503`/`504`, or
/// as an admin `500`/`META0003` wrapping one). The worker pod is healthy, so
/// retry rather than abort the ship on the LB window.
const REGISTER_MAX_ATTEMPTS: u32 = 5;

/// Initial backoff between force-register attempts; doubles each retry
/// (3s → 6s → 12s → 24s), so five attempts span ~45s — past the ~30s the GFE
/// error page itself asks the caller to wait.
const REGISTER_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(3);

/// Per-attempt HTTP timeout for a force-register POST, covering connect, send,
/// and reading the response body. Without it a connection that stalls in the LB
/// window (rather than returning a `502`) would hang the whole ship, defeating
/// the bounded-retry design; a timed-out attempt is retryable like any other
/// transport error, so the loop still makes progress.
const REGISTER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// One re-register attempt failed. `retryable` distinguishes a transient
/// gateway/LB window (worth another attempt) from a real registration error
/// (a bad request shape, auth failure, or a genuine worker-side error) that
/// more attempts cannot fix.
struct RegisterAttemptError {
    retryable: bool,
    message: String,
}

/// Whether a failed admin-register HTTP outcome is a transient gateway/LB
/// window worth retrying rather than a real registration error. The Restate
/// Cloud admin API surfaces the worker-side discovery failure back to us: a
/// direct `502`/`503`/`504`, or a `500` whose body carries the proxied gateway
/// error (Restate `META0003` wrapping a `5xx`, or a raw GFE "502 Server Error"
/// / "Bad Gateway" page). A `4xx` (bad request/auth/conflict) or a `500` with a
/// real registration body is a genuine error — never mask it with retries.
fn is_transient_register_failure(status: reqwest::StatusCode, body: &str) -> bool {
    match status.as_u16() {
        // A direct gateway status from the GFE/LB is the rollout window itself.
        502..=504 => true,
        // A Restate admin `500` is only the LB window when its body actually
        // carries the proxied gateway failure. A genuine registration error
        // answers `500` with a JSON body carrying a `restate_code`, so:
        //   - an empty/whitespace body is the proxy or LB dropping/truncating
        //     the response mid-rollout → transient; and
        //   - a non-empty body is transient only when it names a gateway
        //     failure ("bad gateway"/"service unavailable"/"gateway timeout",
        //     or a GFE "<5xx> Server Error" page).
        // We deliberately do NOT retry on a bare "502"/"503"/"504" digit run
        // that merely appears inside a real error body (a port, id, offset, or
        // line number), which would delay and misreport the true failure by
        // ~45s.
        500 => {
            let b = body.trim().to_ascii_lowercase();
            b.is_empty()
                || b.contains("bad gateway")
                || b.contains("service unavailable")
                || b.contains("gateway timeout")
                || b.contains("502 server error")
                || b.contains("503 server error")
                || b.contains("504 server error")
        }
        _ => false,
    }
}

async fn register_via_admin_api_async(
    admin_base_url: &str,
    token: &str,
    worker_url: &str,
) -> Result<()> {
    let endpoint = format!("{}/deployments", admin_base_url.trim_end_matches('/'));
    eprintln!("==> POST {endpoint} (force re-register {worker_url})");
    let client = reqwest::Client::builder()
        .timeout(REGISTER_REQUEST_TIMEOUT)
        .build()
        .context("build Restate admin HTTP client")?;
    let mut backoff = REGISTER_BACKOFF_BASE;
    for attempt in 1..=REGISTER_MAX_ATTEMPTS {
        match register_attempt(&client, &endpoint, token, worker_url).await {
            Ok(names) => {
                eprintln!(
                    "==> re-registered {worker_url} ({} services{}{})",
                    names.len(),
                    if names.is_empty() { "" } else { ": " },
                    names.join(", ")
                );
                return Ok(());
            }
            Err(err) if err.retryable && attempt < REGISTER_MAX_ATTEMPTS => {
                eprintln!(
                    "==> transient re-register failure (attempt {attempt}/{REGISTER_MAX_ATTEMPTS}): \
                     {}; retrying in {}s",
                    err.message,
                    backoff.as_secs()
                );
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
            Err(err) if err.retryable => {
                bail!(
                    "Restate admin register still failing after {REGISTER_MAX_ATTEMPTS} attempts \
                     (last: {}); the worker endpoint {worker_url} is not reachable through the \
                     load balancer — check the rollout and NEG health before retrying the ship",
                    err.message
                );
            }
            Err(err) => bail!("Restate admin register failed: {}", err.message),
        }
    }
    unreachable!("the loop returns or bails on the final attempt")
}

/// A single force-register POST. Returns the discovered service names on
/// success, or a classified error the caller's loop uses to decide whether to
/// retry. A transport error (connection reset/timeout mid-rollout) is treated
/// as retryable — the bounded attempt count still surfaces a genuinely
/// unreachable admin URL as a hard failure.
async fn register_attempt(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    worker_url: &str,
) -> std::result::Result<Vec<String>, RegisterAttemptError> {
    let body = serde_json::json!({ "uri": worker_url, "force": true });
    let resp = match client
        .post(endpoint)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            return Err(RegisterAttemptError {
                retryable: true,
                message: format!("POST to Restate Cloud admin /deployments failed: {err}"),
            });
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(parse_registered_service_names(&text));
    }
    Err(RegisterAttemptError {
        retryable: is_transient_register_failure(status, &text),
        message: format!("HTTP {status}: {text}"),
    })
}

/// Pull the registered service names out of the admin API's success body, for
/// the operator-facing confirmation line. A body that does not match the
/// expected shape yields an empty list rather than an error — the registration
/// already succeeded.
fn parse_registered_service_names(text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("services").and_then(|s| s.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Warn (don't fail) if the on-PATH Restate CLI doesn't match the
/// pinned version. Drift between operator-laptop and CI is the
/// primary failure mode the pin guards against; surface it loudly
/// without blocking the operator who may be deliberately ahead.
fn check_restate_cli_version() {
    let Ok(out) = Command::new("restate").arg("--version").output() else {
        return;
    };
    let banner = String::from_utf8_lossy(&out.stdout);
    if !banner.contains(RESTATE_CLI_VERSION) {
        eprintln!(
            "warning: restate CLI on PATH is `{}`; devx pins {RESTATE_CLI_VERSION}",
            banner.trim()
        );
    }
}

/// `devx gcp setup --project-id <ID> [--dry-run]`: provision the GCP
/// resources `web` depends on. Builds a private Tokio runtime so the
/// rest of `devx` can stay sync — the entire `gcp` module is async
/// (it talks to GCP REST APIs).
///
/// The pipeline generates no credential of its own, so a run prints no secret
/// to record. A deployment's own store, vendor, and OIDC credentials are
/// written into `deployments/<name>/secrets.enc.yaml` with `sops set` and
/// rotated at the provider first — see `docs/deployment-secrets.md`.
fn gcp_setup(project_id: String, dry_run: bool, config: gcp::SetupConfig) -> Result<()> {
    use std::sync::Arc;

    use gcp::client::{GcpClient, StaticToken, TokenProvider};

    tracing_subscriber::fmt::try_init().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        // Dry-run uses a stub token — we never authenticate.
        let token: Arc<dyn TokenProvider> = if dry_run {
            Arc::new(StaticToken("dry-run".into()))
        } else {
            gcp::auth::adc_token_provider().await?
        };
        let mut client = GcpClient::new(token);
        if dry_run {
            client = client.with_dry_run();
        }
        gcp::run(&client, &project_id, &config).await?;
        if dry_run {
            eprintln!(
                "--- dry run: {} call(s) would be made ---",
                client.recorded_calls().len()
            );
            for call in client.recorded_calls() {
                eprintln!("{} {}", call.method, call.url);
                if let Some(body) = call.body {
                    eprintln!("  {body}");
                }
            }
        } else {
            tracing::info!(project = %project_id, "setup complete");
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// `ops gcp hub setup --project-id <ID> [--dry-run]`: provision the shared
/// image hub. Uses its own typed configuration and pipeline, so no flag on
/// this command can reach the environment resources — buckets, GKE, IAP —
/// that the hub must never hold.
fn gcp_hub_setup(
    project_id: String,
    dry_run: bool,
    config: gcp::hub::HubSetupConfig,
) -> Result<()> {
    use std::sync::Arc;

    use gcp::client::{GcpClient, StaticToken, TokenProvider};

    // Refuse an environment project before attempting ADC authentication or
    // any cloud operation. `gcp::hub::run` validates again for direct callers.
    gcp::tenants::validate_target(gcp::tenants::TenantRole::Hub, &project_id)?;
    tracing_subscriber::fmt::try_init().ok();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        // Dry-run uses a stub token — we never authenticate.
        let token: Arc<dyn TokenProvider> = if dry_run {
            Arc::new(StaticToken("dry-run".into()))
        } else {
            gcp::auth::adc_token_provider().await?
        };
        let mut client = GcpClient::new(token);
        if dry_run {
            client = client.with_dry_run();
        }
        gcp::hub::run(&client, &project_id, &config).await?;
        if dry_run {
            eprintln!(
                "--- dry run: {} call(s) would be made ---",
                client.recorded_calls().len()
            );
            for call in client.recorded_calls() {
                eprintln!("{} {}", call.method, call.url);
                if let Some(body) = call.body {
                    eprintln!("  {body}");
                }
            }
        } else {
            tracing::info!(project = %project_id, "hub setup complete");
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Vendored path for one of the Operator's CRD artifacts. Tied to
/// [`RESTATE_OPERATOR_VERSION`] so the CRD schema can never drift from the
/// chart that reconciles it.
fn restate_crd_path(name: &str) -> String {
    format!("{RESTATE_CRD_DIR}/{name}.yaml")
}

/// An object the API server is trying to delete but cannot, because a
/// finalizer is still held.
#[derive(Debug, PartialEq, Eq)]
struct Terminating {
    deleted_at: String,
    finalizers: String,
}

/// Read a `{.metadata.deletionTimestamp}|{.metadata.finalizers}` jsonpath
/// probe. `None` means "not mid-deletion" — the timestamp is empty on a
/// healthy object, whereas finalizers are present either way (a Restate CR
/// carries `deployments.restate.dev` for its whole life), so the timestamp
/// is the only discriminator.
fn parse_terminating(raw: &str) -> Option<Terminating> {
    let (deleted_at, finalizers) = raw.split_once('|')?;
    let deleted_at = deleted_at.trim();
    if deleted_at.is_empty() {
        return None;
    }
    Some(Terminating {
        deleted_at: deleted_at.to_string(),
        finalizers: finalizers.trim().to_string(),
    })
}

/// Explain a `Terminating` wedge in terms of the operator action that clears
/// it.
///
/// Worth the words because the failure it replaces is actively misleading:
/// `kubectl wait` discards a condition whose `status.observedGeneration`
/// trails `metadata.generation`, treating it as stale. A CR stuck
/// `Terminating` never reconciles a re-applied spec, so its generation
/// climbs while `observedGeneration` sticks — and the wait blocks for the
/// full `--timeout` before dying with a bare `timed out waiting for the
/// condition`. That names neither the deletion nor the finalizer, and it
/// reports a `Ready: True` object as un-Ready.
fn terminating_wedge_message(namespace: &str, resource: &str, state: &Terminating) -> String {
    format!(
        "{resource} in namespace {namespace} is stuck Terminating (deletionTimestamp \
         {deleted_at}, finalizers {finalizers}).\n\
         Its controller is holding the finalizer, so a re-applied spec never reconciles and \
         `kubectl wait --for=condition=…` would block for the full timeout and then fail — \
         even while the object still reports Ready.\n\
         See why the finalizer is held:\n    \
         kubectl --namespace {namespace} describe {resource}\n\
         If the finalizer cannot be satisfied (e.g. the Restate Operator reports \
         `CleanupFailed(DeploymentInUse)` because this CR still backs the active service \
         versions), drop it and let `dev up` recreate the CR:\n    \
         kubectl --namespace {namespace} patch {resource} --type=merge -p \
         '{{\"metadata\":{{\"finalizers\":[]}}}}'",
        deleted_at = state.deleted_at,
        finalizers = state.finalizers,
    )
}

/// Map a Rust `std::env::consts::ARCH` value to the Docker/OCI arch suffix.
/// Only the two arches the workspace builds for are remapped; anything else
/// passes through unchanged.
fn normalize_docker_arch(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

/// Substitute configurable `hostPort:` values in a `kind-config.yaml` body.
/// The container ports stay fixed, so Service manifests remain in sync. At
/// default ports the output is byte-identical to the input.
fn render_kind_config(template: &str, cfg: &KindConfig) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(template.lines().count());
    // The `hostPort:` line follows its `containerPort:` line; track
    // which mapping we're inside so we rewrite the right host port.
    let mut pending: Option<u16> = None;
    for line in template.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- containerPort:") {
            pending = if trimmed.contains("containerPort: 80") {
                Some(cfg.ingress_http_port)
            } else if trimmed.contains("containerPort: 443") {
                Some(cfg.ingress_https_port)
            } else if trimmed.contains(&format!("containerPort: {DEFAULT_RAUTHY_HOST_PORT}")) {
                Some(cfg.rauthy_port)
            } else {
                None
            };
            lines.push(line.to_string());
        } else if trimmed.starts_with("hostPort:") {
            match pending.take() {
                Some(port) => {
                    let indent = &line[..line.len() - trimmed.len()];
                    lines.push(format!("{indent}hostPort: {port}"));
                }
                None => lines.push(line.to_string()),
            }
        } else {
            lines.push(line.to_string());
        }
    }
    let mut out = lines.join("\n");
    if template.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The browser-facing Rauthy origin for a local tier.
///
/// This is the one origin a browser can actually reach: KIND maps
/// `cfg.rauthy_port` straight through to the Rauthy container
/// (`render_kind_config` above rewrites that mapping per worktree), and
/// [`render_env`] derives `OAUTH_ISSUER_URL` from the same port. Deriving both
/// from `cfg` is what keeps the issuer host-side `web` is configured against
/// and the endpoint it redirects the browser to from drifting apart.
fn rauthy_origin(rauthy_port: u16) -> String {
    format!("http://localhost:{rauthy_port}")
}

fn rauthy_issuer(rauthy_port: u16) -> String {
    format!("{}/auth/v1/", rauthy_origin(rauthy_port))
}

/// Merge patch pointing Rauthy's public URL and `WebAuthn` origin at this tier's
/// browser-reachable port.
///
/// Rauthy intentionally has one canonical URL for discovery endpoints, token
/// issuers, and browser redirects. Host-run web reaches the mapped `NodePort`
/// directly; full in-cluster web uses the loopback proxy rendered below.
fn rauthy_public_url_patch(rauthy_port: u16) -> String {
    let authority = format!("localhost:{rauthy_port}");
    format!(
        r#"{{"stringData":{{"PUB_URL":"{authority}","RP_ORIGIN":"{origin}"}}}}"#,
        authority = authority,
        origin = rauthy_origin(rauthy_port),
    )
}

/// Nginx config for the full-stack pod's Rauthy loopback bridge.
///
/// The browser and the in-cluster web process must call the identical issuer.
/// A worktree changes the host-side `NodePort`, so the proxy listen port and Host
/// header follow that same derived value.
fn rauthy_loopback_config(rauthy_port: u16) -> String {
    format!(
        "worker_processes 1;\n\
         error_log /dev/stderr warn;\n\
         \n\
         events {{\n\
           worker_connections 128;\n\
         }}\n\
         \n\
         http {{\n\
           access_log /dev/stdout combined;\n\
           server {{\n\
             listen 127.0.0.1:{rauthy_port};\n\
             location / {{\n\
               proxy_http_version 1.1;\n\
               proxy_set_header Host localhost:{rauthy_port};\n\
               proxy_pass http://rauthy:8080;\n\
             }}\n\
           }}\n\
         }}\n"
    )
}

fn rauthy_loopback_config_patch(rauthy_port: u16) -> String {
    serde_json::json!({
        "data": {
            "nginx.conf": rauthy_loopback_config(rauthy_port),
        }
    })
    .to_string()
}

fn navigator_web_rauthy_patch(rauthy_port: u16) -> String {
    format!(
        concat!(
            r#"{{"spec":{{"template":{{"spec":{{"containers":[{{"#,
            r#""name":"web","env":[{{"#,
            r#""name":"OAUTH_ISSUER_URL","value":"{issuer}","valueFrom":null"#,
            r#"}}]}}]}}}}}}}}"#,
        ),
        issuer = rauthy_issuer(rauthy_port),
    )
}

// ---------- helpers ----------

/// Single-quote a `.devx/env` value so both a POSIX shell `source` and a
/// `dotenvy` parse read it literally. Worktree roots can contain spaces (and
/// other shell-significant characters); an unquoted `KUBECONFIG=/my path/…`
/// truncates at the first space when the file is sourced, so a destructive
/// `navigator dev staging reset` would silently fall back to the ambient
/// kubeconfig instead of the task-owned cluster. Single quotes are literal in
/// both consumers (unlike double quotes, which still expand `$`); a directory
/// containing a literal single quote is the one unsupported shape and is never
/// produced by a worktree path we create.
fn shell_single_quote(value: &str) -> String {
    format!("'{value}'")
}

fn render_env(cfg: &KindConfig, root: &Path) -> String {
    render_env_for(cfg, "navigator", cfg.web_port, root)
}

/// Like [`render_env`] but parameterized by the store database name and
/// host `web` port. The ordinary loop and each worktree tier use `navigator`
/// within their own KIND cluster, threaded through the same renderer so they
/// cannot drift in how they wire `web` to the worker.
/// `root` is the workspace (or worktree) root; the per-Project git repos
/// land under its `.devx/repos/<db_name>`, keyed by database so parallel
/// worktrees never share a repo volume.
#[allow(clippy::too_many_lines)]
fn render_env_for(cfg: &KindConfig, db_name: &str, web_port: u16, root: &Path) -> String {
    // The canonical seed is embedded from `store/seeds/*.yaml` and is
    // applied unconditionally and idempotently by `web` at startup.
    //
    // `RESTATE_BROKER_URL` is what `workflows::RestateRuntime::from_env`
    // reads. When set, the host-side `web` binary signals the
    // in-cluster `workflows-service` worker through the port-forwarded
    // Restate ingress; the worker journals each transition to the same
    // store. The renderer gives host `web` that same database name and the
    // tier's matching host port, so Restate-backed flows remain observable
    // from every worktree.
    // Host-side `web` runs `enforce_deployment_invariants` for the dev
    // profile. SENDGRID_API_KEY /
    // NAVIGATOR_CI_HARNESS explicitly authorizes the fake integration
    // values emitted by this automated KIND fixture. A normal dev
    // deployment supplies real non-production credentials instead.
    // `NAVIGATOR_GIT_REPO_ROOT` is where `repos::RepoStore` keeps each
    // Project's bare repo. It is optional — `portal::matter_documents`
    // returns `Ok(None)` on `RepoError::RootUnset`, and no web boot
    // invariant requires it (`store::deployment::WEB_REQUIREMENTS`) — but
    // host-side `web` sets it so the repo-writing paths stay exercised
    // locally while `repos::RepoStore` lives (ENG-108).
    // `.devx/env` is the complete task-local application environment. Keep
    // the orchestration coordinates alongside the host application's values:
    // a command such as `navigator dev deploy` must target the same isolated
    // worktree cluster that `dev worktree-env up` prepared, rather than
    // silently falling back to the shared `navigator` cluster.
    let lines = [
        ("PORT", web_port.to_string()),
        ("NAVIGATOR_ENVIRONMENT", "dev".into()),
        ("NAVIGATOR_CI_HARNESS", "1".into()),
        (
            "KUBECONFIG",
            shell_single_quote(&root.join(".devx").join("kubeconfig").display().to_string()),
        ),
        ("NAVIGATOR_KIND_CLUSTER", cfg.cluster.clone()),
        ("NAVIGATOR_K8S_NAMESPACE", cfg.namespace.clone()),
        ("NAVIGATOR_KIND_DEPS_OVERLAY", cfg.deps_overlay.clone()),
        ("NAVIGATOR_KIND_OVERLAY", cfg.full_overlay.clone()),
        ("NAVIGATOR_GKE_OVERLAY", cfg.gke_overlay.clone()),
        (
            "NAVIGATOR_KIND_INGRESS_HTTP_PORT",
            cfg.ingress_http_port.to_string(),
        ),
        (
            "NAVIGATOR_KIND_INGRESS_HTTPS_PORT",
            cfg.ingress_https_port.to_string(),
        ),
        (
            "NAVIGATOR_KIND_RESTATE_INGRESS_PORT",
            cfg.restate_ingress_port.to_string(),
        ),
        (
            "NAVIGATOR_KIND_RESTATE_ADMIN_PORT",
            cfg.restate_admin_port.to_string(),
        ),
        ("NAVIGATOR_KIND_CLAMAV_PORT", cfg.clamav_port.to_string()),
        ("NAVIGATOR_KIND_RAUTHY_PORT", cfg.rauthy_port.to_string()),
        (
            "NAVIGATOR_KIND_GARAGE_S3_PORT",
            cfg.garage_s3_port.to_string(),
        ),
        ("NAVIGATOR_KIND_WEB_PORT", cfg.web_port.to_string()),
        (
            "NAVIGATOR_KIND_OPENOBSERVE_PORT",
            cfg.openobserve_port.to_string(),
        ),
        (
            "NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT",
            cfg.openobserve_otlp_port.to_string(),
        ),
        (
            "NAVIGATOR_GIT_REPO_ROOT",
            shell_single_quote(
                &root
                    .join(".devx")
                    .join("repos")
                    .join(db_name)
                    .display()
                    .to_string(),
            ),
        ),
        // One directory holding every staged matter's bundle, each under its
        // own Project code. Boot re-reads each `navigator.yaml` rather than
        // trusting the directory name, so a bundle staged under the wrong
        // code is refused instead of published on another matter's portal.
        (
            store::sample_project::STAGE_ENV,
            shell_single_quote(
                &root
                    .join(".devx")
                    .join("sample-projects")
                    .display()
                    .to_string(),
            ),
        ),
        // The store. The in-cluster `workflows-service` worker reaches
        // the same engine at `surreal.navigator.svc.cluster.local:8000`,
        // so host `web` and the worker share one database.
        (
            "NAVIGATOR_SURREAL_ENDPOINT",
            format!("ws://localhost:{}", cfg.surreal_port),
        ),
        ("NAVIGATOR_SURREAL_NAMESPACE", SURREAL_NAMESPACE.into()),
        ("NAVIGATOR_SURREAL_DATABASE", db_name.to_string()),
        ("NAVIGATOR_SURREAL_USER", SURREAL_LOCAL_USER.into()),
        ("NAVIGATOR_SURREAL_PASSWORD", SURREAL_LOCAL_PASSWORD.into()),
        ("NAVIGATOR_STORAGE_BACKEND", "s3".into()),
        (
            "NAVIGATOR_STORAGE_ENDPOINT",
            format!("http://localhost:{}", cfg.garage_s3_port),
        ),
        ("NAVIGATOR_STORAGE_BUCKET", "navigator-documents".into()),
        ("NAVIGATOR_ASSETS_BUCKET", "navigator-assets".into()),
        (
            "NAVIGATOR_APPLICATIONS_BUCKET",
            "navigator-applications".into(),
        ),
        ("NAVIGATOR_LFS_BUCKET", "navigator-lfs".into()),
        ("NAVIGATOR_STORAGE_REGION", "garage".into()),
        (
            "NAVIGATOR_STORAGE_ACCESS_KEY",
            env_string("NAVIGATOR_GARAGE_ACCESS_KEY", "navigator-kind"),
        ),
        (
            "NAVIGATOR_STORAGE_SECRET_KEY",
            env_string("NAVIGATOR_GARAGE_SECRET_KEY", "navigator-kind-secret"),
        ),
        (
            "NAVIGATOR_ASSETS_ACCESS_KEY",
            env_string("NAVIGATOR_GARAGE_ASSETS_ACCESS_KEY", "navigator-assets"),
        ),
        (
            "NAVIGATOR_ASSETS_SECRET_KEY",
            env_string(
                "NAVIGATOR_GARAGE_ASSETS_SECRET_KEY",
                "navigator-assets-secret",
            ),
        ),
        (
            "NAVIGATOR_APPLICATIONS_ACCESS_KEY",
            env_string(
                "NAVIGATOR_GARAGE_APPLICATIONS_ACCESS_KEY",
                "navigator-applications",
            ),
        ),
        (
            "NAVIGATOR_APPLICATIONS_SECRET_KEY",
            env_string(
                "NAVIGATOR_GARAGE_APPLICATIONS_SECRET_KEY",
                "navigator-applications-secret",
            ),
        ),
        (
            "NAVIGATOR_LFS_ACCESS_KEY",
            env_string("NAVIGATOR_GARAGE_LFS_ACCESS_KEY", "navigator-lfs"),
        ),
        (
            "NAVIGATOR_LFS_SECRET_KEY",
            env_string("NAVIGATOR_GARAGE_LFS_SECRET_KEY", "navigator-lfs-secret"),
        ),
        ("OAUTH_ISSUER_URL", rauthy_issuer(cfg.rauthy_port)),
        ("OAUTH_CLIENT_ID", "navigator-web".into()),
        ("OAUTH_CLIENT_SECRET", LOCAL_RAUTHY_CLIENT_SECRET.into()),
        (
            "OAUTH_REDIRECT_URI",
            format!("http://localhost:{web_port}/auth/callback"),
        ),
        (
            "SESSION_SECRET",
            "dev-only-session-secret-change-in-prod".into(),
        ),
        (
            "NAVIGATOR_CLAMD_ADDR",
            format!("127.0.0.1:{}", cfg.clamav_port),
        ),
        (
            "RESTATE_BROKER_URL",
            format!("http://localhost:{}", cfg.restate_ingress_port),
        ),
        ("SENDGRID_API_KEY", "SG.kind-stub".into()),
        ("SENDGRID_INBOUND_SECRET", "kind-stub".into()),
        // OpenObserve's direct OTLP sink is port-forwarded to the host. To
        // run `web` with plain stdout and no export, set an empty endpoint in
        // `.env`, which loads before this generated file.
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://localhost:{}", cfg.openobserve_otlp_port),
        ),
        (
            "NAVIGATOR_OPENOBSERVE_URL",
            format!("http://localhost:{}", cfg.openobserve_port),
        ),
        (
            "NAVIGATOR_OPENOBSERVE_USERNAME",
            KIND_OPENOBSERVE_USERNAME.into(),
        ),
        (
            "NAVIGATOR_OPENOBSERVE_PASSWORD",
            KIND_OPENOBSERVE_PASSWORD.into(),
        ),
        (
            "NAVIGATOR_OPENOBSERVE_ORGANIZATION",
            KIND_OPENOBSERVE_ORGANIZATION.into(),
        ),
        (
            "NAVIGATOR_OPENOBSERVE_STREAM",
            KIND_OPENOBSERVE_STREAM.into(),
        ),
    ];
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `devx up`. Do not edit by hand — your edits are\n\
         # overwritten on the next `devx up`. Persistent / hand-edited\n\
         # values belong in `.env` at the workspace root, which is\n\
         # auto-loaded BEFORE this file by every binary's `main()`, so\n\
         # `.env` always wins on collisions.\n",
    );
    for (k, v) in lines {
        let _ = writeln!(out, "{k}={v}");
    }
    out
}

#[cfg(test)]
mod restate_pin_tests {
    use super::*;

    #[test]
    fn crd_artifacts_are_vendored_and_track_the_pinned_operator_chart() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent");

        assert_eq!(RESTATE_CRDS.len(), RESTATE_CRD_SHA256.len());

        for (name, expected_sha256) in RESTATE_CRD_SHA256 {
            assert!(RESTATE_CRDS.contains(&name));
            let artifact = restate_crd_path(name);
            assert!(
                !artifact.contains("://"),
                "worktree bootstrap must not ask kubectl to download Restate CRDs"
            );
            assert!(
                artifact.contains(&format!("v{RESTATE_OPERATOR_VERSION}/")),
                "CRD artifact must pin the operator version: {artifact}"
            );
            assert!(
                artifact.ends_with(&format!("/{name}.yaml")),
                "bad artifact path: {artifact}"
            );
            let manifest =
                std::fs::read(root.join(&artifact)).expect("read the vendored Restate CRD");
            assert_eq!(
                sha256_hex(&manifest),
                expected_sha256,
                "vendored CRD digest must match the recorded upstream artifact: {artifact}"
            );
            assert!(
                std::str::from_utf8(&manifest)
                    .expect("vendored Restate CRD is UTF-8")
                    .contains("restate.dev"),
                "vendored CRD must define a Restate API"
            );
        }
    }

    #[test]
    fn restate_cli_and_server_share_a_release_line() {
        // server image (k8s/staging/restate.yaml) ⇄ CLI ⇄ operator chart are
        // pinned together; the CLI tracks the server's minor line.
        assert!(
            RESTATE_CLI_VERSION.starts_with("1.7."),
            "CLI pin drifted off the 1.7 server line: {RESTATE_CLI_VERSION}"
        );
    }
}

#[cfg(test)]
mod terminating_guard_tests {
    use super::*;

    #[test]
    fn a_healthy_object_is_not_terminating() {
        // Real `kubectl -o jsonpath` output for a live RestateDeployment:
        // no deletionTimestamp, but the finalizer is present as always.
        assert_eq!(parse_terminating(r#"|["deployments.restate.dev"]"#), None);
        assert_eq!(parse_terminating("|"), None);
        assert_eq!(parse_terminating("  |[]"), None);
    }

    #[test]
    fn a_deleted_object_is_terminating() {
        // Real output from the wedged CR: `dev up` blocked 300s on this.
        let state =
            parse_terminating(r#"2026-07-15T06:32:52Z|["deployments.restate.dev"]"#).unwrap();
        assert_eq!(state.deleted_at, "2026-07-15T06:32:52Z");
        assert_eq!(state.finalizers, r#"["deployments.restate.dev"]"#);
    }

    #[test]
    fn unparseable_probe_output_is_not_treated_as_terminating() {
        // No separator at all — never claim a wedge we can't prove.
        assert_eq!(parse_terminating(""), None);
        assert_eq!(parse_terminating("garbage"), None);
    }

    #[test]
    fn the_wedge_message_names_the_cause_and_the_escape() {
        let state =
            parse_terminating(r#"2026-07-15T06:32:52Z|["deployments.restate.dev"]"#).unwrap();
        let msg =
            terminating_wedge_message("navigator", "restatedeployment/workflows-service", &state);
        assert!(msg.contains("stuck Terminating"), "{msg}");
        assert!(msg.contains("2026-07-15T06:32:52Z"), "{msg}");
        assert!(msg.contains("deployments.restate.dev"), "{msg}");
        // The operator's own reason, so the message is greppable against the
        // controller log that explains the refusal.
        assert!(msg.contains("DeploymentInUse"), "{msg}");
        // And the command that actually clears it.
        assert!(msg.contains("--type=merge"), "{msg}");
        assert!(msg.contains(r#"{"metadata":{"finalizers":[]}}"#), "{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).expect("valid status code")
    }

    /// A force re-register draws a `500`/`META0003` wrapping a GFE `502` from
    /// the admin API while the NEG readiness gate lags after a rollout. It must
    /// classify as transient so ship retries instead of aborting on a healthy
    /// worker.
    #[test]
    fn meta0003_wrapping_a_502_is_transient() {
        let body = r#"{"message":"[META0003] [META0003] got status code '502 Bad Gateway'. Response headers: {...}. Body: \n<html><head><title>502 Server Error</title></head><body><h1>Error: Server Error</h1></body></html>\n","restate_code":"META0003"}"#;
        assert!(is_transient_register_failure(status(500), body));
    }

    /// A direct gateway status from the load balancer is transient regardless of
    /// body — the GFE 502/503/504 pages are the LB window, not a real error.
    #[test]
    fn direct_gateway_statuses_are_transient() {
        assert!(is_transient_register_failure(status(502), ""));
        assert!(is_transient_register_failure(status(503), ""));
        assert!(is_transient_register_failure(status(504), ""));
        assert!(is_transient_register_failure(
            status(503),
            "<html><title>503 Service Unavailable</title></html>"
        ));
    }

    /// Real errors must NOT be masked by retries: a `4xx` (bad request shape,
    /// auth, conflict) and a `500` with no gateway marker are genuine failures
    /// the operator must see immediately, not five attempts later.
    #[test]
    fn real_errors_are_not_transient() {
        assert!(!is_transient_register_failure(status(400), "bad uri"));
        assert!(!is_transient_register_failure(status(401), "unauthorized"));
        assert!(!is_transient_register_failure(status(409), "conflict"));
        assert!(!is_transient_register_failure(
            status(500),
            r#"{"message":"internal error registering deployment","restate_code":"META0000"}"#
        ));
    }

    /// A genuine registration `500` whose body merely *mentions* a 5xx digit run
    /// (a port, an id, a line number) must classify as a real error, not the LB
    /// window — otherwise a broad numeric match would burn ~45s of retries and
    /// then misreport the true failure as an unreachable load balancer.
    #[test]
    fn incidental_5xx_digits_in_a_real_error_are_not_transient() {
        assert!(!is_transient_register_failure(
            status(500),
            r#"{"message":"failed to register deployment at https://workflows.example.com:8502/ (META0011, line 504)","restate_code":"META0011"}"#
        ));
    }

    /// A `500` with an empty or dropped body cannot be a genuine Restate
    /// registration error (those always answer with a JSON `restate_code`
    /// body); it is the proxy/LB truncating the response mid-rollout, so it must
    /// classify as transient rather than aborting the ship on a healthy worker.
    #[test]
    fn empty_500_body_is_transient() {
        assert!(is_transient_register_failure(status(500), ""));
        assert!(is_transient_register_failure(status(500), "   \n"));
    }

    /// The success-body parser feeds only the operator confirmation line, so a
    /// well-formed body yields the service names and a malformed one yields an
    /// empty list rather than failing an already-successful registration.
    #[test]
    fn parse_registered_service_names_reads_names_and_tolerates_junk() {
        let body = r#"{"services":[{"name":"workflows-service"},{"name":"DevxIssueTriage"},{"name":"devx-pr"}]}"#;
        assert_eq!(
            parse_registered_service_names(body),
            vec!["workflows-service", "DevxIssueTriage", "devx-pr"]
        );
        assert!(parse_registered_service_names("not json").is_empty());
        assert!(parse_registered_service_names("{}").is_empty());
    }

    #[test]
    fn parse_label_target_requires_both_sides() {
        assert_eq!(
            parse_label_target("em7475=target.sendgrid.net").unwrap(),
            ("em7475".to_string(), "target.sendgrid.net".to_string())
        );
        assert!(parse_label_target("=target").is_err());
        assert!(parse_label_target("label=").is_err());
        assert!(parse_label_target("   =target").is_err());
        assert!(parse_label_target("label=   ").is_err());
        assert!(parse_label_target("no-separator").is_err());
    }

    /// `deploy.yml`'s KIND integration deploys with a raw
    /// `kubectl apply -k` (not `dev deploy`), so it must run the Garage
    /// secret bootstrap itself — and *before* the smoke checks, or the
    /// Garage `StatefulSet` never starts and `navigator-web`/`workflows-service`
    /// sit in `CreateContainerConfigError` on the missing
    /// `navigator-garage-*` secrets. This guards the gap that failed the
    /// first release deploy after the Garage migration, which had no test.
    #[test]
    fn deploy_yml_bootstraps_garage_before_the_smoke_checks() {
        use std::path::Path;
        let deploy_yml = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent")
            .join(".github")
            .join("workflows")
            .join("deploy.yml");
        let body = std::fs::read_to_string(&deploy_yml)
            .unwrap_or_else(|e| panic!("read {}: {e}", deploy_yml.display()));
        let bootstrap = body
            .find("dev garage-bootstrap")
            .expect("deploy.yml must run `dev garage-bootstrap` to mint the Garage secrets");
        let smoke = body
            .find("dev e2e")
            .expect("deploy.yml must run `dev e2e` smoke checks");
        assert!(
            bootstrap < smoke,
            "the Garage bootstrap must run before `dev e2e`, else the rollout wait times out on unstarted pods"
        );
    }

    #[test]
    fn apex_redirect_certificate_notice_names_zone_and_certificate_step() {
        let notice = apex_redirect_certificate_notice("neonlaw.com");
        assert!(notice.contains("neonlaw.com"));
        assert!(notice.contains("URL record"));
        assert!(notice.contains("auto-renewing Let's Encrypt certificate"));
        assert!(notice.contains("docs/dns.md"));
    }

    /// The runner carries the browser pair, agent, and coverage tools the CI
    /// gates execute.
    /// Its caches are useful only when those tools match the tree that will
    /// run inside it, so fail at review time rather than quietly creating a
    /// second version pin.
    #[test]
    fn runner_uses_the_workspace_browser_pins() {
        use std::path::Path;

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent");
        let runner = std::fs::read_to_string(root.join("images/Containerfile.runner"))
            .expect("read runner Containerfile");
        let rust_toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml"))
            .expect("read rust toolchain pin");

        let rust_channel = rust_toolchain
            .split("channel = \"")
            .nth(1)
            .and_then(|channel| channel.split('"').next())
            .expect("Rust channel in rust-toolchain.toml");
        let rust_image_tag = rust_channel
            .strip_suffix(".0")
            .expect("the workspace Rust channel is a patch release");
        assert!(
            runner.contains(&format!("FROM rust:{rust_image_tag}-bookworm")),
            "the runner must use the Rust toolchain pinned in rust-toolchain.toml"
        );
        assert!(
            runner.contains(&format!(
                "ARG CHROME_FOR_TESTING_VERSION={}",
                chrome::CHROME_FOR_TESTING_VERSION
            )),
            "the runner must use cli::devx::chrome's Chrome/ChromeDriver pin"
        );
        assert!(
            runner.contains("ARG NODE_VERSION=24.18.0"),
            "the runner must pin its Node LTS release"
        );
        assert!(
            runner.contains("ARG CLAUDE_CODE_VERSION=2.1.220")
                && runner.contains("@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}"),
            "the runner must install an explicitly pinned Claude Code CLI"
        );
        assert!(
            runner.contains("ARG CARGO_LLVM_COV_VERSION=0.8.7")
                && runner
                    .contains("cargo-llvm-cov --version \"${CARGO_LLVM_COV_VERSION}\" --locked"),
            "the runner must install a reproducibly pinned coverage tool"
        );
        assert!(
            runner.contains("cargo build --locked -p cli")
                && runner.contains("/usr/local/bin/navigator"),
            "the runner must bake the Navigator CLI"
        );
        assert!(
            runner.contains("cargo build --locked -p github_webhooks --bin triage-runner")
                && runner.contains("/usr/local/bin/triage-runner"),
            "the runner must bake the isolated triage entrypoint"
        );
        let copies = |path: &str| {
            runner.lines().any(|line| {
                let words: Vec<_> = line.split_whitespace().collect();
                words == ["COPY", path, path]
            })
        };
        assert!(
            copies("k8s") && copies("examples"),
            "the runner must copy the CLI's compile-time embedded deployment assets"
        );
    }

    // The deploy workflow's "stub public assets" step generates
    // the placeholder `/public/img/...` and `/public/fonts/...` bytes the KIND
    // `assets verify` gate requires and removes the matching `.dockerignore`
    // rules so the baked image actually carries them. That step MUST run for whichever
    // image the KIND web pod runs — and since ENG-142 that is the single
    // distroless `navigator-web` (`k8s/base/web/web.yaml`), which no KIND
    // overlay patch overrides any more. Gating the stub step on the wrong
    // family leaves the served image empty and 404s all 18 paths in the
    // "content images are served by KIND web" gate.
    // The parsing/matching logic is factored into pure helpers below so both
    // the pass and the fail path are exercised directly — the guard is only
    // worth having if the failing case actually reports the drift.

    /// The web image family a manifest pins, parsed from its `image:` line
    /// (`:tag` stripped).
    fn kind_web_image_family(manifest_yaml: &str) -> Option<String> {
        manifest_yaml
            .lines()
            .find_map(|l| l.trim().strip_prefix("image:"))
            .map(|v| v.trim().split(':').next().unwrap_or_default().to_string())
    }

    /// The `build`-matrix leg that actually COMPILES `image` — the leg whose
    /// own `image` it is, or the leg carrying it as an `alias`.
    ///
    /// `navigator-web` is now a tag on the `neon-server` leg's image rather
    /// than a leg of its own, because both names always came from the same
    /// Containerfile and the second leg was a second metered compile of bytes
    /// the run already had. Every guard that asks "is the image the KIND pod
    /// runs built correctly?" has to follow that indirection, or it reads a
    /// leg that no longer exists and reports drift that is not there.
    fn build_leg_for_image(deploy_yml: &str, image: &str) -> Option<String> {
        let workflow: serde_yaml::Value = serde_yaml::from_str(deploy_yml).ok()?;
        let legs = workflow["jobs"]["build"]["strategy"]["matrix"]["include"].as_sequence()?;
        legs.iter()
            .find(|leg| {
                leg["image"].as_str() == Some(image) || leg["alias"].as_str() == Some(image)
            })
            .and_then(|leg| leg["image"].as_str())
            .map(str::to_string)
    }

    /// `Ok` iff the deploy workflow's public-asset stub step is gated to run
    /// for `kind_web_image`; `Err(explanation)` otherwise. This is the exact
    /// linkage that broke once: the step ran only for one family while the
    /// KIND web pod served `/public` from another.
    fn stub_step_covers_image(deploy_yml: &str, kind_web_image: &str) -> Result<(), String> {
        let step_pos = deploy_yml
            .find("name: stub public assets")
            .ok_or_else(|| "deploy.yml: stub step not found".to_string())?;
        let if_line = deploy_yml[step_pos..]
            .lines()
            .take(3)
            .find(|l| l.trim_start().starts_with("if:"))
            .ok_or_else(|| "deploy.yml: stub step has no `if:` condition".to_string())?;
        if if_line.contains(&format!("== '{kind_web_image}'")) {
            Ok(())
        } else {
            Err(format!(
                "the `stub public assets` step must run for the image the KIND web \
                 pod runs (`{kind_web_image}`), or the served image will 404 on \
                 `/public/img/...` and `/public/fonts/...`. Found `if:` = `{}`",
                if_line.trim()
            ))
        }
    }

    #[test]
    fn stub_step_covers_the_image_the_kind_web_pod_runs() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent")
            .to_path_buf();

        let base_web = std::fs::read_to_string(root.join("k8s/base/web/web.yaml"))
            .expect("read k8s/base/web/web.yaml");
        let kind_web_image =
            kind_web_image_family(&base_web).expect("base web.yaml pins a web image");
        assert_eq!(
            kind_web_image, "navigator-web",
            "the KIND web pod runs the single distroless web image (ENG-142)"
        );

        let deploy = std::fs::read_to_string(root.join(".github/workflows/deploy.yml"))
            .expect("read deploy.yml");
        // Through the alias, not around it: the KIND pod runs `navigator-web`,
        // which is a tag on whichever leg builds it. The stubs have to be in
        // THAT leg's image.
        let builder = build_leg_for_image(&deploy, &kind_web_image).unwrap_or_else(|| {
            panic!(
                "deploy.yml's build matrix must produce `{kind_web_image}`, as a leg's own image \
                 or as its `alias`"
            )
        });
        stub_step_covers_image(&deploy, &builder).expect(
            "deploy.yml stub step must cover the leg that builds the image the KIND web pod runs",
        );
    }

    /// No KIND overlay may override the web-container `image:`. The base pin
    /// is the single source of truth for what the in-cluster pod runs, and an
    /// override diverges from it silently, because the stub-step guard above
    /// reads the base.
    #[test]
    fn no_kind_overlay_overrides_the_web_image() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent")
            .to_path_buf();

        let mut offenders = Vec::new();
        let mut stack = vec![root.join("k8s/overlays")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read overlay dir") {
                let path = entry.expect("overlay dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).expect("read overlay manifest");
                // Only the navigator-web Deployment patch matters; the
                // dependency manifests (Garage, Rauthy, Surreal) legitimately
                // pin their own upstream images.
                if !body.contains("name: navigator-web") {
                    continue;
                }
                // Sidecars pin legitimate upstream images (the Rauthy loopback
                // proxy runs nginx), so only first-party `navigator-*` pins are
                // candidates for reintroducing a second web family.
                offenders.extend(
                    body.lines()
                        .filter_map(|l| l.trim().strip_prefix("image:"))
                        .map(|v| v.trim().split(':').next().unwrap_or_default())
                        .filter(|family| {
                            family.starts_with("navigator-") && *family != "navigator-web"
                        })
                        .map(|family| format!("{}: {family}", path.display())),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "a KIND overlay pins a non-`navigator-web` image for the web pod; \
             the base pin is the single source of truth: {offenders:?}"
        );
    }

    /// The release COMPILES the web application exactly once.
    ///
    /// Two ways that has been violated, both of them a second full-workspace
    /// build of the same `neon` binary. `navigator-git` was one, on a
    /// git-bearing base, costing 22m build + 13m publish per nightly run while
    /// nothing deployed consumed it (ENG-142). `navigator-web` as its own
    /// matrix leg was the other, measured at 263 min beside `neon-server`'s
    /// 272 over five days — half of `deploy.yml`'s whole runner bill for a
    /// byte-identical image.
    ///
    /// So the count that matters is legs compiling `Containerfile.neon`, not
    /// spellings of a name: the alias tag is free and must stay allowed, while
    /// a second compile must not.
    #[test]
    fn the_release_compiles_the_web_application_exactly_once() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent")
            .to_path_buf();
        let deploy = std::fs::read_to_string(root.join(".github/workflows/deploy.yml"))
            .expect("read deploy.yml");
        let workflow: serde_yaml::Value =
            serde_yaml::from_str(&deploy).expect("deploy.yml parses as YAML");

        let compiles_web: Vec<&str> = workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
            .as_sequence()
            .expect("the build job must declare an include matrix")
            .iter()
            .filter(|leg| leg["dockerfile"].as_str() == Some("images/Containerfile.neon"))
            .filter_map(|leg| leg["image"].as_str())
            .collect();
        assert_eq!(
            compiles_web,
            ["neon-server"],
            "deploy.yml must compile the web application exactly once"
        );

        // ...and the name the deployed manifests still pull has to keep
        // arriving, as a tag on that one compile.
        assert_eq!(
            build_leg_for_image(&deploy, "navigator-web").as_deref(),
            Some("neon-server"),
            "`k8s/base/web/web.yaml` still pulls navigator-web, so it must remain an alias of the \
             image that leg builds"
        );
        assert!(
            !deploy.contains("navigator-git") && !deploy.contains("Containerfile.git"),
            "deploy.yml must not reference a git-bearing image"
        );
    }

    /// The web boot invariants (#1172) refuse to start without the `SurrealDB`
    /// coordinates, so the KIND web patch must inject every one of them or
    /// the in-cluster pod crash-loops before its readiness probe can pass —
    /// deploy run 142136966 failed exactly this way while the local loops
    /// (which get the values from `.devx/env`) stayed green.
    #[test]
    fn kind_web_patch_carries_the_surreal_boot_invariants() {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root is cli/'s parent")
            .to_path_buf();

        let web_repos = std::fs::read_to_string(root.join("k8s/overlays/kind/web-surreal.yaml"))
            .expect("read web-surreal.yaml");
        for var in [
            "NAVIGATOR_SURREAL_ENDPOINT",
            "NAVIGATOR_SURREAL_NAMESPACE",
            "NAVIGATOR_SURREAL_DATABASE",
            "NAVIGATOR_SURREAL_USER",
            "NAVIGATOR_SURREAL_PASSWORD",
        ] {
            assert!(
                web_repos.contains(var),
                "KIND web patch must set {var}: the web boot invariants \
                 refuse to start without it"
            );
        }
    }

    /// The guard's fail path must actually fire on the regression it exists to
    /// catch (a stub step gated to a family the KIND web pod does not run) and
    /// stay quiet once the served image is covered.
    #[test]
    fn stub_step_guard_flags_a_gate_that_misses_the_served_image() {
        assert_eq!(
            kind_web_image_family("        image: navigator-web:dev\n").as_deref(),
            Some("navigator-web")
        );
        assert_eq!(kind_web_image_family("no image here\n"), None);

        let wrong_family = "      - name: stub public assets for the KIND web image\n        \
                        if: matrix.image == 'navigator-gateway'\n        run: |\n";
        let err = stub_step_covers_image(wrong_family, "navigator-web")
            .expect_err("a gate missing the served image must be flagged");
        assert!(
            err.contains("navigator-web") && err.contains("404"),
            "{err}"
        );

        let covered = "      - name: stub public assets for the KIND web image\n        \
                    if: matrix.image == 'navigator-web'\n";
        assert!(stub_step_covers_image(covered, "navigator-web").is_ok());

        let missing_step = "      - name: something else\n        if: always()\n";
        assert!(stub_step_covers_image(missing_step, "navigator-web")
            .unwrap_err()
            .contains("stub step not found"));

        let no_if = "      - name: stub public assets for the KIND web image\n        \
                     run: echo hi\n        env:\n";
        assert!(stub_step_covers_image(no_if, "navigator-web")
            .unwrap_err()
            .contains("no `if:` condition"));
    }

    /// Alias resolution is the indirection every guard above now walks, so it
    /// gets its own pass and fail paths: a name reached through `alias` must
    /// resolve to the leg that compiles it, a name that is a leg resolves to
    /// itself, and a name in neither resolves to nothing rather than to a
    /// confident wrong answer.
    #[test]
    fn build_leg_resolution_follows_an_alias_tag_to_the_leg_that_compiles_it() {
        let matrix = "jobs:\n  build:\n    strategy:\n      matrix:\n        include:\n\
                      \x20         - image: neon-server\n            dockerfile: images/Containerfile.neon\n\
                      \x20           alias: navigator-web\n\
                      \x20         - image: navigator-gateway\n            dockerfile: images/Containerfile.gateway\n";

        assert_eq!(
            build_leg_for_image(matrix, "navigator-web").as_deref(),
            Some("neon-server"),
            "an alias must resolve to the leg that compiles it"
        );
        assert_eq!(
            build_leg_for_image(matrix, "neon-server").as_deref(),
            Some("neon-server")
        );
        assert_eq!(
            build_leg_for_image(matrix, "navigator-gateway").as_deref(),
            Some("navigator-gateway"),
            "a leg with no alias still resolves to itself"
        );
        assert_eq!(build_leg_for_image(matrix, "navigator-git"), None);
        assert_eq!(
            build_leg_for_image("not: yaml: at: all", "navigator-web"),
            None
        );
    }

    // The `linux/<arch>` platform fed to `docker save` (in
    // `kind_load_image_into_cluster`) must use the OCI arch suffix, not the
    // Rust arch triple — `linux/aarch64` would never match an image's
    // `linux/arm64` manifest, defeating the multi-arch flatten.
    #[test]
    fn normalize_docker_arch_maps_rust_arches_to_oci_suffixes() {
        assert_eq!(normalize_docker_arch("x86_64"), "amd64");
        assert_eq!(normalize_docker_arch("aarch64"), "arm64");
        // Already-normalized or unknown values pass through unchanged.
        assert_eq!(normalize_docker_arch("amd64"), "amd64");
        assert_eq!(normalize_docker_arch("arm64"), "arm64");
        assert_eq!(normalize_docker_arch("riscv64"), "riscv64");
    }

    // Every env var `KindConfig::from_env` reads. Tests clear all of
    // them before asserting so a stray value from the developer's shell
    // (or a prior test) can't leak in.
    const KIND_ENV_VARS: &[&str] = &[
        "NAVIGATOR_KIND_CLUSTER",
        "NAVIGATOR_K8S_NAMESPACE",
        "NAVIGATOR_KIND_DEPS_OVERLAY",
        "NAVIGATOR_KIND_OVERLAY",
        "NAVIGATOR_GKE_OVERLAY",
        "NAVIGATOR_KIND_INGRESS_HTTP_PORT",
        "NAVIGATOR_KIND_INGRESS_HTTPS_PORT",
        "NAVIGATOR_KIND_RESTATE_INGRESS_PORT",
        "NAVIGATOR_KIND_RESTATE_ADMIN_PORT",
        "NAVIGATOR_KIND_CLAMAV_PORT",
        "NAVIGATOR_KIND_RAUTHY_PORT",
        "NAVIGATOR_KIND_GARAGE_S3_PORT",
        "NAVIGATOR_KIND_WEB_PORT",
        "NAVIGATOR_KIND_OPENOBSERVE_PORT",
        "NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT",
        "NAVIGATOR_PRIVATE_MODE",
    ];

    // Process env is global; `from_env` reads all of it. Serialize the
    // env-mutating tests so they don't race each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn clear_kind_env() {
        for key in KIND_ENV_VARS {
            env::remove_var(key);
        }
    }

    /// A `KindConfig` at its defaults, built without touching the
    /// process environment — for the pure render tests.
    pub(super) fn default_cfg() -> KindConfig {
        KindConfig {
            cluster: DEFAULT_CLUSTER_NAME.into(),
            namespace: DEFAULT_NAMESPACE.into(),
            deps_overlay: DEFAULT_KUSTOMIZE_KIND_DEPS.into(),
            full_overlay: DEFAULT_KUSTOMIZE_KIND.into(),
            gke_overlay: DEFAULT_KUSTOMIZE_GKE.into(),
            ingress_http_port: DEFAULT_INGRESS_HTTP_HOST_PORT,
            ingress_https_port: DEFAULT_INGRESS_HTTPS_HOST_PORT,
            restate_ingress_port: DEFAULT_RESTATE_INGRESS_HOST_PORT,
            restate_admin_port: DEFAULT_RESTATE_ADMIN_HOST_PORT,
            clamav_port: DEFAULT_CLAMAV_HOST_PORT,
            rauthy_port: DEFAULT_RAUTHY_HOST_PORT,
            garage_s3_port: DEFAULT_GARAGE_S3_HOST_PORT,
            surreal_port: DEFAULT_SURREAL_HOST_PORT,
            web_port: DEFAULT_LOCAL_WEB_PORT,
            openobserve_port: DEFAULT_OPENOBSERVE_HOST_PORT,
            openobserve_otlp_port: DEFAULT_OPENOBSERVE_OTLP_HOST_PORT,
        }
    }

    /// `undeploy` deletes a whole namespace. Unpinned it would delete it from
    /// whatever context is current — on an operator's machine as likely prod
    /// as KIND — so the pin is asserted, not assumed.
    #[test]
    fn undeploy_pins_the_kind_context() {
        assert_eq!(
            undeploy_args(&default_cfg().kind_context(), DEFAULT_NAMESPACE),
            [
                "--context",
                "kind-navigator",
                "delete",
                "--ignore-not-found",
                "namespace",
                "navigator",
            ]
        );
    }

    /// The worktree-env `up` reused-cluster path skips `up_in` and its
    /// `configure_worktree_kubeconfig` pin, then hydrates Garage over bare
    /// `kubectl`. The fix pins the context first; this asserts the pin is
    /// explicit and lands on *this worktree's* isolated KIND context — never
    /// the ambient one (which on an operator's machine can be prod).
    #[test]
    fn use_context_pins_the_isolated_kind_context() {
        // A worktree cluster carries a per-checkout suffix; the pin must target
        // that isolated cluster, not the shared `kind-navigator` or an ambient
        // GKE/EKS context.
        let mut worktree = default_cfg();
        worktree.cluster = "navigator-6e2e1eb5".to_string();
        let context = worktree.kind_context();
        assert_eq!(context, "kind-navigator-6e2e1eb5");
        assert_eq!(
            use_context_args(&context),
            ["config", "use-context", "kind-navigator-6e2e1eb5"]
        );
        assert!(
            !use_context_args(&context)
                .iter()
                .any(|arg| arg.starts_with("gke_")),
            "the pin must never resolve to a GKE context"
        );
    }

    #[test]
    fn resolve_workflows_url_prefers_explicit_override() {
        assert_eq!(
            resolve_workflows_url(
                Some("https://flag.example/"),
                Some("https://env.example/"),
                Some("neonlaw.com"),
            ),
            "https://flag.example/"
        );
    }

    #[test]
    fn resolve_workflows_url_falls_back_to_env() {
        assert_eq!(
            resolve_workflows_url(None, Some("https://env.example/"), Some("neonlaw.com")),
            "https://env.example/"
        );
    }

    #[test]
    fn resolve_workflows_url_derives_from_primary_domain() {
        // The 2026-06-10 hardening: domain set, explicit URL unset →
        // target the real ingress, never the placeholder.
        assert_eq!(
            resolve_workflows_url(None, None, Some("neonlaw.com")),
            "https://workflows.neonlaw.com/"
        );
    }

    #[test]
    fn resolve_workflows_url_treats_blank_as_unset() {
        // Empty/whitespace override and env must not win, and a blank
        // domain falls through to the placeholder rather than producing
        // `https://workflows.//`.
        assert_eq!(
            resolve_workflows_url(Some("  "), Some(""), Some(" neonlaw.com ")),
            "https://workflows.neonlaw.com/"
        );
        assert_eq!(
            resolve_workflows_url(None, None, Some("   ")),
            WORKFLOWS_PUBLIC_URL
        );
    }

    #[test]
    fn resolve_workflows_url_placeholder_only_when_nothing_set() {
        assert_eq!(
            resolve_workflows_url(None, None, None),
            WORKFLOWS_PUBLIC_URL
        );
    }

    #[tokio::test]
    async fn admin_api_register_posts_force_to_deployments_and_reports_services() {
        use wiremock::matchers::{body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let worker_url = "https://workflows.example.com/";
        Mock::given(method("POST"))
            .and(path("/deployments"))
            .and(header("authorization", "Bearer test-admin-key"))
            // The force re-register must send exactly {uri, force:true} — a
            // plain register (no force) would refuse to overwrite the
            // existing endpoint and a service added since last register would
            // stay invisible at the ingress.
            .and(body_json(
                serde_json::json!({ "uri": worker_url, "force": true }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "dp_test",
                "services": [{ "name": "notation" }, { "name": "Archives" }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        // A trailing slash on the admin base must not double up the path.
        register_via_admin_api_async(&format!("{}/", server.uri()), "test-admin-key", worker_url)
            .await
            .expect("admin-api register should succeed against the mock");
        // `.expect(1)` on the mock asserts exactly one matching POST on drop.
    }

    #[tokio::test]
    async fn admin_api_register_errors_on_non_2xx() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/deployments"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let err = register_via_admin_api_async(
            &server.uri(),
            "bad-token",
            "https://workflows.example.com/",
        )
        .await
        .expect_err("a 401 must surface as an error, not a silent success");
        assert!(
            err.to_string().contains("401"),
            "error should name the HTTP status, got: {err}"
        );
    }

    /// Load-bearing safety net: an empty environment must reproduce the
    /// exact pre-refactor constants. If this fails, `devx up` against a
    /// clean `.env` would behave differently than it always has.
    #[test]
    fn from_env_with_no_vars_equals_defaults() {
        let _guard = lock();
        clear_kind_env();
        assert_eq!(KindConfig::from_env(), default_cfg());
    }

    #[test]
    fn from_env_reads_string_overrides() {
        let _guard = lock();
        clear_kind_env();
        env::set_var("NAVIGATOR_KIND_CLUSTER", "fork-cluster");
        env::set_var("NAVIGATOR_K8S_NAMESPACE", "fork-ns");
        env::set_var("NAVIGATOR_KIND_DEPS_OVERLAY", "my/deps");
        env::set_var("NAVIGATOR_KIND_OVERLAY", "my/full");
        env::set_var("NAVIGATOR_GKE_OVERLAY", "my/gke");
        let cfg = KindConfig::from_env();
        clear_kind_env();
        assert_eq!(cfg.cluster, "fork-cluster");
        assert_eq!(cfg.namespace, "fork-ns");
        assert_eq!(cfg.deps_overlay, "my/deps");
        assert_eq!(cfg.full_overlay, "my/full");
        assert_eq!(cfg.gke_overlay, "my/gke");
    }

    #[test]
    fn private_mode_is_off_unless_affirmatively_on() {
        // The failure this guards is asymmetric: reading a stray value as
        // "on" costs a developer a basic-auth prompt, while reading a real
        // value as "off" publishes a deployment someone believes is
        // private. Everything outside the affirmative set is off, and
        // everything inside it is on regardless of case or padding.
        for on in ["1", "true", "TRUE", "yes", "on", "  true  "] {
            assert!(private_mode(Some(on)), "{on:?} must enable private mode");
        }
        for off in ["", "  ", "0", "false", "no", "off", "private", "maybe"] {
            assert!(!private_mode(Some(off)), "{off:?} must not enable it");
        }
        assert!(!private_mode(None), "unset must not enable it");
    }

    #[test]
    fn private_mode_selects_the_private_kind_overlay() {
        let _guard = lock();
        clear_kind_env();
        env::set_var("NAVIGATOR_PRIVATE_MODE", "1");
        let private = KindConfig::from_env();
        // An explicit overlay still wins — private mode only changes which
        // overlay is the DEFAULT, so a fork pointing at its own tree keeps
        // pointing at it.
        env::set_var("NAVIGATOR_KIND_OVERLAY", "my/full");
        let explicit = KindConfig::from_env();
        clear_kind_env();

        assert_eq!(private.full_overlay, DEFAULT_KUSTOMIZE_KIND_PRIVATE);
        // Nothing else about the topology moves.
        assert_eq!(private.deps_overlay, DEFAULT_KUSTOMIZE_KIND_DEPS);
        assert_eq!(private.gke_overlay, DEFAULT_KUSTOMIZE_GKE);
        assert_eq!(explicit.full_overlay, "my/full");
    }

    /// An overlay in this repository, built the way `dev deploy` applies it.
    ///
    /// Straight off disk, unlike the GKE pair in `ship.rs`: the KIND
    /// overlays carry no `${}` substitutions and are applied with
    /// `apply -k <path>`, so the on-disk tree already IS what gets applied.
    /// There is nothing for an embed-and-render step to add.
    fn kind_overlay(name: &str) -> String {
        let overlay = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../k8s/overlays")).join(name);
        ship::kustomize_build(&overlay)
            .unwrap_or_else(|err| panic!("k8s/overlays/{name} builds: {err:#}"))
    }

    #[test]
    fn kind_deps_use_direct_openobserve_without_lgtm() {
        let manifests = kind_overlay("kind-deps");

        assert!(
            !manifests.contains("grafana/otel-lgtm"),
            "the KIND dependency tier exports to OpenObserve directly, with no LGTM image"
        );
        assert!(
            !manifests.contains("name: lgtm"),
            "no LGTM Service or Deployment belongs in the KIND dependency tier"
        );

        let service = ship::manifest_doc(&manifests, "Service", "openobserve");
        assert_eq!(service["spec"]["ports"][0]["port"].as_u64(), Some(5080));
        assert_eq!(service["spec"]["ports"][1]["port"].as_u64(), Some(5081));

        let deployment = ship::manifest_doc(&manifests, "Deployment", "openobserve");
        let container = &deployment["spec"]["template"]["spec"]["containers"][0];
        assert!(
            container["image"]
                .as_str()
                .is_some_and(|image| image.starts_with("o2cr.ai/openobserve/openobserve@sha256:")),
            "OpenObserve must be pinned by immutable digest"
        );
        let env = container["env"].as_sequence().expect("OpenObserve has env");
        assert!(
            env.iter()
                .any(|entry| entry["name"].as_str() == Some("ZO_GRPC_PORT")
                    && entry["value"].as_str() == Some("5081")),
            "OpenObserve must expose direct OTLP gRPC on 5081"
        );
        assert!(
            manifests.contains("NAVIGATOR_OPENOBSERVE_PASSWORD"),
            "both Navigator binaries must receive the OpenObserve credential contract"
        );
        assert!(
            manifests.contains("openobserve_endpoint: http://openobserve:5081"),
            "the staging coordinates must point binaries directly at OpenObserve"
        );
    }

    #[test]
    fn private_mode_puts_the_basic_auth_gateway_in_front_of_kind_web() {
        // The KIND half of what `ship.rs`'s
        // `private_mode_puts_the_basic_auth_gateway_in_front_of_web`
        // proves for GKE. `private_mode_selects_the_private_kind_overlay`
        // above only proves the CLI names this path; it never builds it, so
        // a wrong `resources:`/`components:` path or a patch that missed
        // its target would leave both that test and every other per-PR
        // check green while `dev up` applied a public stack.
        let manifests = kind_overlay("kind-private");

        let service = ship::manifest_doc(&manifests, "Service", "navigator-web");
        assert_eq!(
            service["spec"]["ports"][0]["targetPort"].as_u64(),
            Some(8080),
            "the Service must reach `web` through the gateway, not directly: {:?}",
            service["spec"]["ports"]
        );

        let deployment = ship::manifest_doc(&manifests, "Deployment", "navigator-web");
        let containers = deployment["spec"]["template"]["spec"]["containers"]
            .as_sequence()
            .expect("the web pod has containers");
        let gateway = containers
            .iter()
            .find(|c| c["name"].as_str() == Some("private-gateway"))
            .expect("private mode adds the Pingora sidecar");
        assert_eq!(
            gateway["readinessProbe"]["httpGet"]["path"].as_str(),
            Some("/health"),
            "the probe must target the one unauthenticated location, or every probe 401s and \
             private mode reads as an outage rather than a password prompt"
        );
        assert_eq!(
            gateway["ports"][0]["containerPort"].as_u64(),
            service["spec"]["ports"][0]["targetPort"].as_u64(),
            "the gateway must listen on the port the Service targets. A numeric `targetPort` is \
             published for every selected pod whether or not anything there listens, so a \
             disagreement here is not a routing miss that fails closed — it is a live upstream \
             where nothing answers, and the ingress turns it into a 502."
        );
        assert!(
            manifests.contains("navigator-private-basic-auth"),
            "the basic-auth Secret must be part of the applied tree"
        );
    }

    #[test]
    fn the_default_kind_overlay_stays_public() {
        // The half that matters more: the overlay `dev up` applies without
        // `NAVIGATOR_PRIVATE_MODE` must be the stack it always was. A
        // component leaking into `../kind` would put a basic-auth prompt in
        // front of every local loop and the browser e2e gate, which sends
        // no `Authorization` header.
        let manifests = kind_overlay("kind");

        assert!(
            !manifests.contains("private-gateway"),
            "the plain overlay must not carry the gateway"
        );
        let service = ship::manifest_doc(&manifests, "Service", "navigator-web");
        assert_eq!(
            service["spec"]["ports"][0]["targetPort"].as_u64(),
            Some(3001),
            "the Service must reach the app directly"
        );
    }

    #[test]
    fn from_env_reads_port_overrides() {
        let _guard = lock();
        clear_kind_env();
        env::set_var("NAVIGATOR_KIND_INGRESS_HTTP_PORT", "18080");
        env::set_var("NAVIGATOR_KIND_INGRESS_HTTPS_PORT", "18443");
        env::set_var("NAVIGATOR_KIND_RESTATE_INGRESS_PORT", "19080");
        env::set_var("NAVIGATOR_KIND_RESTATE_ADMIN_PORT", "19070");
        env::set_var("NAVIGATOR_KIND_CLAMAV_PORT", "23310");
        env::set_var("NAVIGATOR_KIND_RAUTHY_PORT", "31080");
        env::set_var("NAVIGATOR_KIND_GARAGE_S3_PORT", "31900");
        env::set_var("NAVIGATOR_KIND_WEB_PORT", "4001");
        let cfg = KindConfig::from_env();
        clear_kind_env();
        assert_eq!(cfg.ingress_http_port, 18080);
        assert_eq!(cfg.ingress_https_port, 18443);
        assert_eq!(cfg.restate_ingress_port, 19080);
        assert_eq!(cfg.restate_admin_port, 19070);
        assert_eq!(cfg.clamav_port, 23310);
        assert_eq!(cfg.rauthy_port, 31080);
        assert_eq!(cfg.garage_s3_port, 31900);
        assert_eq!(cfg.web_port, 4001);
    }

    #[test]
    fn empty_and_garbage_values_fall_back_to_defaults() {
        let _guard = lock();
        clear_kind_env();
        // Empty string → default (a `FOO=` line shouldn't blank a path).
        env::set_var("NAVIGATOR_KIND_CLUSTER", "");
        // Unparseable port → default rather than a crash.
        env::set_var("NAVIGATOR_KIND_WEB_PORT", "not-a-port");
        let cfg = KindConfig::from_env();
        clear_kind_env();
        assert_eq!(cfg.cluster, DEFAULT_CLUSTER_NAME);
        assert_eq!(cfg.web_port, DEFAULT_LOCAL_WEB_PORT);
    }

    #[test]
    fn render_env_threads_the_ports() {
        let mut cfg = default_cfg();
        cfg.cluster = "navigator-task".into();
        cfg.web_port = 4001;
        cfg.clamav_port = 23310;
        cfg.rauthy_port = 31080;
        cfg.garage_s3_port = 31900;
        cfg.restate_ingress_port = 19080;
        cfg.openobserve_port = 15_080;
        cfg.openobserve_otlp_port = 15_081;
        let env = render_env(&cfg, Path::new("/ws"));
        assert!(env.contains("PORT=4001"));
        assert!(env.contains("KUBECONFIG='/ws/.devx/kubeconfig'"));
        assert!(env.contains("NAVIGATOR_KIND_CLUSTER=navigator-task"));
        assert!(env.contains("NAVIGATOR_KIND_RESTATE_INGRESS_PORT=19080"));
        assert!(env.contains("NAVIGATOR_KIND_WEB_PORT=4001"));
        assert!(env.contains("NAVIGATOR_GIT_REPO_ROOT='/ws/.devx/repos/navigator'"));
        assert!(env.contains("OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:15081"));
        assert!(env.contains("NAVIGATOR_OPENOBSERVE_URL=http://localhost:15080"));
        assert!(env.contains("NAVIGATOR_OPENOBSERVE_USERNAME=root@example.com"));
        assert!(env.contains("NAVIGATOR_OPENOBSERVE_PASSWORD=NavigatorKindOpenObserve1!"));
        assert!(env.contains("NAVIGATOR_OPENOBSERVE_ORGANIZATION=default"));
        assert!(env.contains("NAVIGATOR_CLAMD_ADDR=127.0.0.1:23310"));
        assert!(env.contains("OAUTH_ISSUER_URL=http://localhost:31080/auth/v1/"));
        assert!(env.contains("NAVIGATOR_STORAGE_ENDPOINT=http://localhost:31900"));
        assert!(env.contains("NAVIGATOR_ASSETS_BUCKET=navigator-assets"));
        assert!(env.contains("NAVIGATOR_ASSETS_ACCESS_KEY=navigator-assets"));
        assert!(env.contains("NAVIGATOR_APPLICATIONS_BUCKET=navigator-applications"));
        assert!(env.contains("NAVIGATOR_APPLICATIONS_ACCESS_KEY=navigator-applications"));
        assert!(env.contains("NAVIGATOR_LFS_BUCKET=navigator-lfs"));
        assert!(env.contains("NAVIGATOR_LFS_ACCESS_KEY=navigator-lfs"));
        assert!(env.contains("RESTATE_BROKER_URL=http://localhost:19080"));
        assert!(env.contains("OAUTH_REDIRECT_URI=http://localhost:4001/auth/callback"));
    }

    /// `.devx/env` carries the store's whole connection contract, so a
    /// sourced environment reaches the same engine `web` was started
    /// against.
    #[test]
    fn render_env_carries_the_surreal_contract() {
        let mut cfg = default_cfg();
        cfg.surreal_port = 21_224;

        let env = render_env(&cfg, Path::new("/ws"));

        assert!(
            env.contains("NAVIGATOR_SURREAL_ENDPOINT=ws://localhost:21224"),
            "{env}"
        );
        assert!(
            env.contains("NAVIGATOR_SURREAL_NAMESPACE=navigator"),
            "{env}"
        );
        assert!(
            env.contains("NAVIGATOR_SURREAL_DATABASE=navigator"),
            "{env}"
        );
        assert!(env.contains("NAVIGATOR_SURREAL_USER=root"), "{env}");
        assert!(env.contains("NAVIGATOR_SURREAL_PASSWORD=root"), "{env}");
    }

    #[test]
    fn render_env_shell_quotes_worktree_paths() {
        // A worktree root with a space is the ordinary way an unquoted value
        // truncates when `.devx/env` is sourced, stranding destructive staging
        // commands on the ambient kubeconfig. The generated lines must survive
        // both a POSIX `source` and the `dotenvy` parse every binary runs.
        let cfg = default_cfg();
        let env = render_env(&cfg, Path::new("/ws with space"));
        assert!(
            env.contains("KUBECONFIG='/ws with space/.devx/kubeconfig'"),
            "{env}"
        );
        assert!(
            env.contains("NAVIGATOR_GIT_REPO_ROOT='/ws with space/.devx/repos/navigator'"),
            "{env}"
        );

        // `dotenvy` is the loader `main()` and the reset proof both use; parse
        // the rendered body back and confirm the quotes are stripped to the
        // exact path rather than being taken literally or split on the space.
        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_read_iter(env.as_bytes())
                .map(|item| item.expect("parse rendered .devx/env line"))
                .collect();
        assert_eq!(
            parsed.get("KUBECONFIG").map(String::as_str),
            Some("/ws with space/.devx/kubeconfig")
        );
        assert_eq!(
            parsed.get("NAVIGATOR_GIT_REPO_ROOT").map(String::as_str),
            Some("/ws with space/.devx/repos/navigator")
        );
    }

    // The committed cluster config; the render helper must reproduce it
    // byte-for-byte at default ports.
    const COMMITTED_KIND_CONFIG: &str = include_str!("../../../k8s/kind-config.yaml");

    /// Rauthy's issuer and `WebAuthn` origin must sit on this tier's own
    /// browser-reachable port.
    #[test]
    fn the_rauthy_public_url_patch_targets_this_tiers_own_port() {
        let patch = rauthy_public_url_patch(20_445);

        assert!(
            patch.contains(r#""PUB_URL":"localhost:20445""#),
            "patch must point PUB_URL at the tier's own port: {patch}"
        );
        assert!(
            patch.contains(r#""RP_ORIGIN":"http://localhost:20445""#),
            "patch must align RP_ORIGIN with the tier's own port: {patch}"
        );
        assert!(
            !patch.contains("localhost:8080"),
            "patch must not reintroduce the shared ingress origin: {patch}"
        );
        assert!(
            patch.contains(r#""stringData":"#),
            "the patch must preserve the Deployment's valueFrom contract: {patch}"
        );
    }

    /// The patch is a pure function of the port so repeated reconciliation
    /// cannot drift the issuer or `WebAuthn` origin.
    #[test]
    fn the_rauthy_public_url_patch_is_deterministic() {
        assert_eq!(
            rauthy_public_url_patch(20_445),
            rauthy_public_url_patch(20_445)
        );
        assert_ne!(
            rauthy_public_url_patch(20_445),
            rauthy_public_url_patch(30_080)
        );
    }

    /// `web` validates the issuer it was configured with against the one
    /// Rauthy advertises, so the public URL patch, host environment, and
    /// in-cluster loopback bridge are one fact.
    #[test]
    fn every_rauthy_channel_agrees_on_the_issuer_port() {
        let mut cfg = default_cfg();
        cfg.rauthy_port = 20_445;

        let origin = rauthy_origin(cfg.rauthy_port);
        let issuer = rauthy_issuer(cfg.rauthy_port);
        let env = render_env(&cfg, Path::new("/ws"));

        assert!(
            env.contains(&format!("OAUTH_ISSUER_URL={issuer}")),
            "`.devx/env` must carry the canonical issuer; env was:\n{env}"
        );
        assert!(
            rauthy_public_url_patch(cfg.rauthy_port).contains(&origin),
            "the Rauthy deployment patch must carry the same origin"
        );
        assert!(
            navigator_web_rauthy_patch(cfg.rauthy_port).contains(&issuer),
            "the in-cluster web patch must carry the same issuer"
        );
        let proxy = rauthy_loopback_config(cfg.rauthy_port);
        assert!(proxy.contains("listen 127.0.0.1:20445"), "{proxy}");
        assert!(proxy.contains("Host localhost:20445"), "{proxy}");
        assert!(
            rauthy_loopback_config_patch(cfg.rauthy_port).contains("20445"),
            "the ConfigMap patch must carry the same port"
        );
    }

    #[test]
    fn render_kind_config_is_byte_identical_at_defaults() {
        let rendered = render_kind_config(COMMITTED_KIND_CONFIG, &default_cfg());
        assert_eq!(rendered, COMMITTED_KIND_CONFIG);
    }

    #[test]
    fn render_kind_config_substitutes_the_configurable_hostports() {
        let mut cfg = default_cfg();
        cfg.ingress_http_port = 18080;
        cfg.ingress_https_port = 18443;
        cfg.rauthy_port = 31080;
        let rendered = render_kind_config(COMMITTED_KIND_CONFIG, &cfg);
        assert!(rendered.contains("hostPort: 18080"));
        assert!(rendered.contains("hostPort: 18443"));
        assert!(rendered.contains("hostPort: 31080"));
        assert!(!rendered.contains("hostPort: 8080"));
        assert!(!rendered.contains("hostPort: 8443"));
        assert!(!rendered.contains("hostPort: 30080"));
        assert!(rendered.contains("containerPort: 30080"));
        // Only host-port values change — line count is stable.
        assert_eq!(
            rendered.lines().count(),
            COMMITTED_KIND_CONFIG.lines().count()
        );
    }
}
