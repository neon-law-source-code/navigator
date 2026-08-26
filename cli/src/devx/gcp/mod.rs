//! `devx gcp setup --project-id <PROJECT_ID>` — provision the GCP
//! resources `web` depends on (VPC, GCS buckets, runtime Workload
//! Identity, and a GKE Autopilot cluster) from a single Rust binary. Config
//! Sync is an explicit optional seam.
//!
//! Five buckets are unconditional — assets, documents, exports, logs, and the
//! private applications bucket that holds each Project's published client-portal
//! bundle. Two more are created only when the deployment names them: the
//! long-term Iceberg archive of its Surreal store, and the 90-day telemetry
//! landing zone the nightly lane promotes into that archive. A deployment naming
//! neither keeps exactly the five-bucket shape.
//!
//! Every step is **idempotent**: each `ensure_*` function issues a
//! create call and treats HTTP 409 Conflict (REST steps) or
//! `"already exists"` stderr (gcloud shell-out steps) as success. The
//! same `setup` invocation can therefore be re-run after a partial
//! failure without producing duplicates.
//!
//! ## Pipeline order
//!
//! 1. [`services::enable_services`] — `serviceusage.batchEnable`.
//!    Must run first; nothing else works without the APIs enabled.
//! 2. [`network::ensure_network`] — custom-mode VPC.
//! 3. [`buckets::ensure_bucket`] for assets, documents, exports, logs, and the
//!    applications bucket, then — when named — the archive and telemetry
//!    buckets, each followed by [`buckets::ensure_lifecycle`] so the telemetry
//!    expiry is in place before the first object lands.
//! 4. [`workload_identity::ensure_runtime_identity`] — per-deployment GSA
//!    and direct GCP access.
//! 5. Registry access, then
//!    [`gke::ensure_autopilot_cluster_foundation`] — GKE Autopilot cluster
//!    and Gateway static IP.
//! 6. [`workload_identity::bind_kubernetes_accounts`] — workload identity
//!    bindings after the cluster has created the project pool.
//! 7. [`gke::ensure_cluster_integrations`] — Fleet membership and optional
//!    Config Sync `RootSync`.
//! 8. [`kms::ensure`] — the Cloud KMS key this deployment's
//!    `secrets.enc.yaml` is encrypted against, in this deployment's own
//!    project. Last because it is cheap and depends on nothing above it
//!    except step 1; it is not a prerequisite of any earlier stage.
//!
//! Steps 1–3 talk to GCP REST APIs via [`client::GcpClient`]; steps 4–5
//! shell out to `gcloud` and `kubectl` (the Container API alone is
//! ~200 lines of cluster JSON). Tests stand up wiremock and override
//! base URLs per service for the REST steps, and use the dry-run
//! recorder for the shell-out step — no traffic ever leaves the host,
//! no GCP credentials needed. See `docs/cloud-operations.md` for the
//! layered CI strategy.

pub mod app_publisher;
pub mod artifact_registry;
pub mod auth;
pub mod buckets;
pub mod client;
pub mod error;
pub mod gke;
pub mod hub;
pub mod iap;
pub mod kms;
pub mod lro;
pub mod network;
pub mod secret_manager;
pub mod services;
pub mod tenants;
pub mod workload_identity;

pub use error::{SetupError, SetupResult};

use self::client::GcpClient;

const SETUP_STAGE_COUNT: usize = 15;

fn progress_line(project_id: &str, stage: usize, stage_count: usize, detail: &str) -> String {
    format!("gcp setup [{project_id}] {stage:02}/{stage_count:02} {detail}")
}

fn progress(project_id: &str, stage: usize, detail: &str) {
    eprintln!(
        "{}",
        progress_line(project_id, stage, SETUP_STAGE_COUNT, detail)
    );
}

/// Default region. Overridable via `NAVIGATOR_GCP_LOCATION`.
pub const DEFAULT_REGION: &str = "us-west4";

/// Bucket name suffixes appended to the project ID.
pub const ASSETS_BUCKET_SUFFIX: &str = "-assets";
pub const DOCUMENTS_BUCKET_SUFFIX: &str = "-documents";
pub const EXPORTS_BUCKET_SUFFIX: &str = "-exports";
pub const LOGS_BUCKET_SUFFIX: &str = "-logs";
/// The private, per-deployment bucket that holds each Project's published
/// client-portal bundle at `<project_code>/portal/`. Mandatory like the four
/// above; `web` streams its bytes same-origin, so it is never public.
pub const APPLICATIONS_BUCKET_SUFFIX: &str = "-applications";
pub const TELEMETRY_BUCKET_SUFFIX: &str = "-telemetry";
/// Archive buckets are `neon-law-archives-<deployment>`, a PREFIX rather than the
/// suffix every other lane uses. The archive of a project is named for the
/// archive family first and the deployment second, matching the existing
/// `neon-law-archives-neon-law-420305`.
pub const ARCHIVES_BUCKET_PREFIX: &str = "neon-law-archives-";

/// Default Artifact Registry repository name that holds every image.
pub const DEFAULT_ARTIFACT_REGISTRY_REPO: &str = "navigator";
/// Default `owner/repo` the Workload Identity provider trusts.
pub const DEFAULT_GITHUB_REPO: &str = "neon-law-source-code/navigator";
/// Default account id of the CI service account that pushes images.
pub const DEFAULT_CI_PUSHER_ACCOUNT_ID: &str = "navigator-ci-pusher";

/// Per-deployment overrides for `devx gcp setup`. Every field defaults
/// to the workspace's preferred name; OSS forks change them via CLI
/// flags or env vars without forking the code. The struct is built by
/// `devx/src/main.rs::gcp_setup` from clap-parsed `--region`,
/// `--cluster-name`, etc. flags that fall back to env vars.
#[derive(Clone)]
pub struct SetupConfig {
    /// GCP region used for GKE Autopilot and bucket location.
    /// Default: `us-west4`.
    pub region: String,
    /// GKE Autopilot cluster name. Default: `navigator-prod`.
    pub cluster_name: String,
    /// VPC network name. Default: `navigator-vpc`.
    pub vpc_name: String,
    /// Regional subnetwork used by the GKE cluster. Default:
    /// `navigator-subnet`.
    pub subnetwork_name: String,
    /// Reserved global static-IP name attached to the Gateway.
    /// Default: `navigator-gateway-ip`.
    pub gateway_ip_name: String,
    /// HTTPS URL of the GitHub (or other Git host) repo that `Config
    /// Sync` should reconcile from. `None` skips the `RootSync` step —
    /// the right default for OSS forks that don't run `GitOps` yet.
    pub config_sync_repo: Option<String>,
    /// Path inside `config_sync_repo` that the `RootSync` watches.
    /// Default: `examples/deploy/k8s/gke` to match the parameterized
    /// overlay shipped with the workspace.
    pub config_sync_dir: String,
    /// Canonical HTTPS origin allowed to fetch public bucket fonts. Supplied
    /// by the `NAV_BASE_URL` deployment setting.
    pub public_base_url: Option<String>,
    /// Artifact Registry repository name that holds every container
    /// image. Default: `navigator`.
    pub artifact_registry_repo: String,
    /// `owner/repo` slug the Workload Identity provider trusts for
    /// keyless CI pushes. Default: `neon-law-source-code/navigator`.
    pub github_repo: String,
    /// Account id (local part of the SA email) of the CI pusher service
    /// account. Default: `navigator-ci-pusher`.
    pub ci_pusher_account_id: String,
    /// Hub project holding the shared Artifact Registry. When set, this
    /// environment creates no registry, CI identity, or WIF pool of its own —
    /// it only takes `roles/artifactregistry.reader` on the hub repository.
    /// `None` keeps the single-project shape, where the registry lives
    /// alongside the workloads that pull from it.
    pub images_project_id: Option<String>,
    /// Per-deployment bucket names. When absent, the single-project OSS
    /// convention (`<project>-{assets,documents,exports,logs}`) is retained. The
    /// staging project sets these explicitly so its three deployments never
    /// share an object-storage lane.
    pub assets_bucket: Option<String>,
    pub documents_bucket: Option<String>,
    pub exports_bucket: Option<String>,
    pub logs_bucket: Option<String>,
    /// Private applications bucket holding each Project's published
    /// client-portal bundle. Mandatory like the four above; when absent it
    /// derives `<project>-applications`.
    pub applications_bucket: Option<String>,
    /// Long-term Iceberg archive for this deployment's Surreal store, and the
    /// 90-day telemetry landing zone the nightly lane promotes into it.
    ///
    /// Neither derives from `project_id`. `neon-law-archives-<deployment>` is
    /// prefix-shaped, and `<deployment>-telemetry` names the deployment while
    /// the project may be named for the entity — `neon-law-stg` lives in
    /// `neon-law`, so a derived `neon-law-telemetry` would be a bucket nobody
    /// configured. Absent means the deployment declines that lane.
    pub archives_bucket: Option<String>,
    pub telemetry_bucket: Option<String>,
    /// Google service-account id used by the web and workflows Kubernetes
    /// service accounts through GKE Workload Identity. Deployments sharing a
    /// GCP project set this explicitly so their principals stay isolated.
    pub google_service_account_id: String,
    /// Dedicated service-account id whose JSON credential is granted Google
    /// Workspace domain-wide delegation. It deliberately receives no runtime
    /// GCP roles; a leaked Drive key must not also unlock SQL, secrets, or GCS.
    pub drive_service_account_id: String,
    /// Kubernetes namespace containing the workload service accounts.
    pub kubernetes_namespace: String,
    /// The applications organization and every Project repository allowed to
    /// publish to this deployment's applications bucket.
    ///
    /// The org must be set and the list non-empty to provision the publisher
    /// lane; absent, the deployment declines it (its portal bundles are then
    /// published by hand or not yet).
    ///
    /// **The org is singular and the repositories are plural, and that asymmetry
    /// is the shape of the resources rather than an oversight.** One Workload
    /// Identity provider serves the deployment and its `attributeCondition`
    /// names exactly one `repository_owner`, so a second org would need a second
    /// provider. Each repository, by contrast, gets its own service account:
    /// the publisher's grant is conditioned on one object prefix and a condition
    /// lives on a binding, so one account carries exactly one Project's portal.
    /// Each entry is a repository name, which *is* the Project code.
    pub applications_publisher_org: Option<String>,
    pub applications_publisher_repos: Vec<String>,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            region: DEFAULT_REGION.to_string(),
            cluster_name: gke::DEFAULT_CLUSTER_NAME.to_string(),
            vpc_name: network::DEFAULT_NETWORK_NAME.to_string(),
            subnetwork_name: network::DEFAULT_SUBNETWORK_NAME.to_string(),
            gateway_ip_name: gke::DEFAULT_GATEWAY_IP_NAME.to_string(),
            config_sync_repo: None,
            config_sync_dir: "examples/deploy/k8s/gke".to_string(),
            public_base_url: None,
            artifact_registry_repo: DEFAULT_ARTIFACT_REGISTRY_REPO.to_string(),
            github_repo: DEFAULT_GITHUB_REPO.to_string(),
            ci_pusher_account_id: DEFAULT_CI_PUSHER_ACCOUNT_ID.to_string(),
            images_project_id: None,
            assets_bucket: None,
            documents_bucket: None,
            exports_bucket: None,
            logs_bucket: None,
            applications_bucket: None,
            archives_bucket: None,
            telemetry_bucket: None,
            google_service_account_id: "navigator-web".to_string(),
            drive_service_account_id: "navigator-drive".to_string(),
            kubernetes_namespace: "navigator".to_string(),
            applications_publisher_org: None,
            applications_publisher_repos: Vec::new(),
        }
    }
}

/// Every bucket name this pipeline provisions, resolved once.
///
/// The first five fall back to the single-project OSS convention
/// `<project>-<lane>`. The last two never do — see
/// [`SetupConfig::archives_bucket`] — so `None` there means the deployment
/// declined that lane rather than that a default applies.
struct BucketNames {
    assets: String,
    documents: String,
    exports: String,
    logs: String,
    applications: String,
    archives: Option<String>,
    telemetry: Option<String>,
}

impl BucketNames {
    fn resolve(project_id: &str, config: &SetupConfig) -> Self {
        let derived = |configured: &Option<String>, suffix: &str| {
            configured
                .clone()
                .unwrap_or_else(|| format!("{project_id}{suffix}"))
        };
        Self {
            assets: derived(&config.assets_bucket, ASSETS_BUCKET_SUFFIX),
            documents: derived(&config.documents_bucket, DOCUMENTS_BUCKET_SUFFIX),
            exports: derived(&config.exports_bucket, EXPORTS_BUCKET_SUFFIX),
            logs: derived(&config.logs_bucket, LOGS_BUCKET_SUFFIX),
            applications: derived(&config.applications_bucket, APPLICATIONS_BUCKET_SUFFIX),
            archives: config.archives_bucket.clone(),
            telemetry: config.telemetry_bucket.clone(),
        }
    }

    /// Every bucket this deployment's runtime identity is granted `objectUser`
    /// on. A lane the deployment declined contributes no binding, so the grant
    /// set stays exactly as narrow as the bucket set.
    fn granted(&self) -> Vec<&str> {
        let mut granted: Vec<&str> = vec![
            &self.assets,
            &self.documents,
            &self.exports,
            &self.logs,
            &self.applications,
        ];
        granted.extend(self.archives.as_deref());
        granted.extend(self.telemetry.as_deref());
        granted
    }
}

/// Stages 4-10: the five buckets every deployment gets, then the two it may
/// have declined. Split out of [`run`] as one unit because they are one
/// concern, and because `run` is otherwise a list of unrelated stages.
async fn ensure_buckets(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
    names: &BucketNames,
    public_base_url: &str,
) -> SetupResult<()> {
    let region = &config.region;
    progress(
        project_id,
        4,
        &format!("private assets bucket {}", names.assets),
    );
    buckets::ensure_bucket(client, project_id, &names.assets, region).await?;
    buckets::ensure_assets_cors(client, &names.assets, public_base_url).await?;

    progress(
        project_id,
        5,
        &format!("private documents bucket {}", names.documents),
    );
    buckets::ensure_bucket(client, project_id, &names.documents, region).await?;

    progress(
        project_id,
        6,
        &format!("private exports bucket {}", names.exports),
    );
    buckets::ensure_bucket(client, project_id, &names.exports, region).await?;

    progress(
        project_id,
        7,
        &format!("private logs bucket {}", names.logs),
    );
    buckets::ensure_bucket(client, project_id, &names.logs, region).await?;

    progress(
        project_id,
        8,
        &format!("private applications bucket {}", names.applications),
    );
    buckets::ensure_bucket(client, project_id, &names.applications, region).await?;
    // The ten-year orphaned-asset expiry. The publish must still overwrite
    // unconditionally (see `APPLICATIONS_RETENTION_DAYS`).
    buckets::ensure_lifecycle(client, &names.applications).await?;

    ensure_optional_bucket(
        client,
        project_id,
        region,
        9,
        "Iceberg archive bucket",
        names.archives.as_deref(),
    )
    .await?;
    ensure_optional_bucket(
        client,
        project_id,
        region,
        10,
        &format!(
            "telemetry landing bucket ({}-day expiry)",
            buckets::TELEMETRY_RETENTION_DAYS
        ),
        names.telemetry.as_deref(),
    )
    .await
}

/// Create one bucket the deployment may have declined, and reconcile its
/// lifecycle to whatever its kind calls for.
///
/// `None` is a first-class answer, not an error: a deployment that names no
/// archive or telemetry bucket keeps the original four-bucket shape. It still
/// prints a stage line, so the operator sees that the stage ran and chose to
/// do nothing — a silently absent stage reads as a stage that was forgotten.
///
/// Lifecycle is reconciled immediately after the create rather than in a later
/// pass, so a telemetry bucket cannot accept its first object before the expiry
/// that governs it exists.
async fn ensure_optional_bucket(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    stage: usize,
    label: &str,
    name: Option<&str>,
) -> SetupResult<()> {
    match name {
        Some(name) => {
            progress(project_id, stage, &format!("{label} {name}"));
            buckets::ensure_bucket(client, project_id, name, region).await?;
            buckets::ensure_lifecycle(client, name).await?;
        }
        None => progress(
            project_id,
            stage,
            &format!("{label} (not configured; skipped)"),
        ),
    }
    Ok(())
}

/// Run the full setup pipeline. See module docs for the order.
pub async fn run(client: &GcpClient, project_id: &str, config: &SetupConfig) -> SetupResult<()> {
    tenants::validate_target(tenants::TenantRole::Environment, project_id)?;
    if let Some(images_project_id) = config.images_project_id.as_deref() {
        tenants::validate_images_project(project_id, images_project_id)?;
    }
    let public_base_url = config
        .public_base_url
        .as_deref()
        .ok_or(SetupError::MissingConfiguration("NAV_BASE_URL"))?;
    buckets::validate_public_base_url(public_base_url)?;
    let names = BucketNames::resolve(project_id, config);

    progress(project_id, 1, "enable required APIs");
    services::enable_services(client, project_id).await?;

    progress(project_id, 2, &format!("VPC {}", config.vpc_name));
    network::ensure_network(client, project_id, config).await?;

    progress(
        project_id,
        3,
        &format!(
            "regional subnet {} ({})",
            config.subnetwork_name, config.region
        ),
    );
    network::ensure_named_subnetwork(
        client,
        project_id,
        &config.region,
        &config.vpc_name,
        &config.subnetwork_name,
    )
    .await?;

    ensure_buckets(client, project_id, config, &names, public_base_url).await?;

    progress(
        project_id,
        11,
        &format!(
            "runtime identities and IAM bindings ({})",
            config.google_service_account_id
        ),
    );
    workload_identity::ensure_runtime_identity(client, project_id, config, &names.granted())
        .await?;

    // One Project client-portal publisher identity per Project repository,
    // provisioned only when the deployment names the applications org and at
    // least one repository that may publish. It rides after the runtime identity
    // because each publisher binds the applications bucket the buckets stage
    // created, and the lane is optional in the same sense the archive and
    // telemetry buckets are: a deployment that declines it publishes its portal
    // bundles by hand or not yet.
    //
    // Every code is resolved here, before `ensure` makes its first call, so a
    // code too long to own an account id refuses the whole stage rather than
    // leaving the Projects ahead of it provisioned and the rest not.
    let publisher_repos = config.applications_publisher_repos.as_slice();
    let publisher_org = if publisher_repos.is_empty() {
        None
    } else {
        config.applications_publisher_org.as_deref()
    };
    if let Some(org) = publisher_org {
        for publisher in app_publisher::resolve_publishers(project_id, publisher_repos)? {
            eprintln!(
                "gcp setup [{project_id}] application publisher identity {} ({org}/{})",
                publisher.email, publisher.code
            );
        }
        app_publisher::ensure(
            client,
            project_id,
            org,
            publisher_repos,
            &names.applications,
        )
        .await?;
    }

    // Registry before the cluster: GKE nodes pull the app images from
    // it, and the reader binding must exist before the first pull.
    progress(project_id, 12, "container registry access");
    artifact_registry::ensure(client, project_id, config).await?;

    progress(
        project_id,
        13,
        &format!("GKE Autopilot cluster {}", config.cluster_name),
    );
    gke::ensure_autopilot_cluster_foundation(client, project_id, config).await?;

    progress(
        project_id,
        14,
        "Kubernetes workload identity and cluster integrations",
    );
    workload_identity::bind_kubernetes_accounts(client, project_id, config).await?;
    gke::ensure_cluster_integrations(client, project_id, config).await?;

    // The key this deployment's `secrets.enc.yaml` is encrypted against, in
    // this deployment's own project. It depends on nothing above it except
    // stage 1 enabling the API — it is last because it is cheap and
    // independent, not because anything waits on it.
    // Printed in full: this is the exact string that must appear as `kms_key`
    // in this deployment's `config.toml` and in its `.sops.yaml` creation
    // rule, so an operator can compare the three without deriving anything.
    progress(project_id, 15, &kms::key_name(project_id, &config.region));
    kms::ensure(client, project_id, &config.region).await?;

    eprintln!("gcp setup [{project_id}] COMPLETE");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::client::{GcpClient, GcpService, RecordedCall, StaticToken};

    /// A dry-run client with every service pointed at an unreachable address,
    /// so a real HTTP call would fail loudly instead of escaping the test.
    fn offline_dry_run_client() -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, "http://127.0.0.1:1")
            .with_base_url(GcpService::Compute, "http://127.0.0.1:1")
            .with_base_url(GcpService::Storage, "http://127.0.0.1:1")
            .with_base_url(GcpService::ArtifactRegistry, "http://127.0.0.1:1")
            .with_base_url(GcpService::Iam, "http://127.0.0.1:1")
            .with_base_url(GcpService::CloudResourceManager, "http://127.0.0.1:1")
            .with_base_url(GcpService::CloudKms, "http://127.0.0.1:1")
            .with_dry_run()
    }

    /// Assert a recorded request body mentions `needle`, naming the pipeline
    /// step in the failure so a mismatch says which call drifted.
    fn assert_body_contains(call: &RecordedCall, needle: &str, step: &str) {
        let body = call.body.as_deref().unwrap_or_default();
        assert!(
            body.contains(needle),
            "{step}: expected `{needle}` in {body:?}"
        );
    }

    #[test]
    fn progress_lines_name_the_project_stage_and_resource_without_secrets() {
        let line = super::progress_line(
            "neon-law-stg",
            4,
            super::SETUP_STAGE_COUNT,
            "private assets bucket example-a-assets",
        );
        assert_eq!(
            line,
            "gcp setup [neon-law-stg] 04/15 private assets bucket example-a-assets"
        );
        assert!(!line.contains("password"));
    }

    #[tokio::test]
    async fn a_malformed_public_base_url_is_rejected_before_any_gcp_call() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            config_sync_repo: Some("https://example.com/your-org/your-repo".into()),
            // A path-bearing origin: valid to `Url::parse`, invalid as a CORS
            // origin. The pipeline must refuse it up front rather than fail at
            // step 4a against an already half-provisioned project.
            public_base_url: Some("https://www.example.test/app".into()),
            ..super::SetupConfig::default()
        };
        let err = super::run(&client, "my-project", &config)
            .await
            .expect_err("a path-bearing NAV_BASE_URL must not provision anything");

        assert!(err.to_string().contains("NAV_BASE_URL"), "{err}");
        assert!(
            client.recorded_calls().is_empty(),
            "no GCP call may precede the NAV_BASE_URL check, got {:?}",
            client.recorded_calls()
        );
    }

    #[tokio::test]
    async fn the_environment_pipeline_refuses_the_hub_project_before_any_gcp_call() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            public_base_url: Some("https://www.example.test".into()),
            ..super::SetupConfig::default()
        };
        let err = super::run(&client, super::tenants::HUB_PROJECT_ID, &config)
            .await
            .expect_err("the hub must never receive buckets or GKE");

        assert!(err.to_string().contains("the image hub"), "{err}");
        assert!(
            client.recorded_calls().is_empty(),
            "the tenant guard must precede every GCP call, got {:?}",
            client.recorded_calls()
        );
    }

    #[tokio::test]
    async fn an_environment_may_not_name_itself_as_its_own_image_hub() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            public_base_url: Some("https://www.example.test".into()),
            images_project_id: Some("neon-law".into()),
            ..super::SetupConfig::default()
        };
        let err = super::run(&client, "neon-law", &config)
            .await
            .expect_err("an environment never hosts the shared registry");

        assert!(err.to_string().contains("neon-law"), "{err}");
        assert!(client.recorded_calls().is_empty(), "guard runs first");
    }

    /// With `--images-project-id`, the environment takes a reader binding on
    /// the *hub's* repository and creates no registry, CI pusher, or WIF pool
    /// of its own — those live in the hub, provisioned by `ops gcp hub setup`.
    #[tokio::test]
    async fn images_project_id_records_only_a_cross_project_reader_grant() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            public_base_url: Some("https://www.example.test".into()),
            images_project_id: Some(super::tenants::HUB_PROJECT_ID.into()),
            ..super::SetupConfig::default()
        };
        super::run(&client, "neon-law", &config).await.unwrap();

        let calls = client.recorded_calls();
        let registry_calls: Vec<_> = calls
            .iter()
            .filter(|c| c.url.contains("/repositories") || c.url.contains("workloadIdentityPools"))
            .collect();
        // Exactly the getIamPolicy read and the setIamPolicy write on the hub
        // repository — down from the eleven the single-project shape records.
        assert_eq!(
            registry_calls.len(),
            2,
            "expected only the hub reader binding, got {registry_calls:?}"
        );
        for call in &registry_calls {
            assert!(
                call.url.contains("/projects/ghcr/"),
                "the binding is written in the hub project, not the environment: {call:?}"
            );
        }
        assert!(
            registry_calls[1].url.ends_with(":setIamPolicy"),
            "{:?}",
            registry_calls[1]
        );
        assert_body_contains(
            registry_calls[1],
            "roles/artifactregistry.reader",
            "cross-project reader role",
        );
        assert_body_contains(
            registry_calls[1],
            "-compute@developer.gserviceaccount.com",
            "the environment's own workload puller is the granted principal",
        );

        // The service-account create path is a bare POST to `/serviceAccounts`;
        // the IAM verbs suffix the resource with `:getIamPolicy`/`:setIamPolicy`.
        assert!(
            !calls.iter().any(|c| c.url.ends_with("/serviceAccounts")),
            "the CI pusher lives in the hub, not the environment: {calls:?}"
        );
    }

    // One end-to-end assertion intentionally names every provisioning stage;
    // splitting it would weaken the guarantee that a dry run covers the full
    // empty-project pipeline without network traffic.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn dry_run_records_full_pipeline_with_no_network_traffic() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            config_sync_repo: Some("https://example.com/your-org/your-repo".into()),
            public_base_url: Some("https://www.example.test".into()),
            ..super::SetupConfig::default()
        };
        super::run(&client, "my-project", &config).await.unwrap();

        let calls = client.recorded_calls();
        // REST: 2 services.batchEnable + network + subnet + 5 storage inserts
        // (assets, documents, exports, logs, applications) + 1 assets CORS read
        // + 1 assets CORS patch + 1 applications lifecycle patch = 12.
        // Runtime identity: runtime GSA + isolated Drive GSA + 1 project role
        // + 5 bucket roles + self-signing role = 9.
        // Artifact Registry: repo
        // create + cleanup patch + SA create +
        // writer get/set + reader get/set + WIF pool + WIF provider +
        // impersonation get/set = 11 (project number is short-circuited in
        // dry-run). SHELL (gke): gateway IP + create-auto = 2, followed by 2
        // KSA bindings, then fleet-enable + fleet-register + RootSync = 3.
        // KMS: key ring + crypto key = 2.
        assert_eq!(calls.len(), 41, "expected 41 calls, got {calls:?}");
        let urls: Vec<&str> = calls.iter().map(|c| c.url.as_str()).collect();
        let methods: Vec<&str> = calls.iter().map(|c| c.method).collect();

        // The claim in this test's name, asserted rather than assumed.
        //
        // `offline_dry_run_client` overrides one base URL per service, so a
        // service added to `GcpService` without a matching override keeps its
        // production URL and this "offline" pipeline aims at real GCP. Dry-run
        // only records, so nothing escapes today — but the omission is
        // invisible in a passing call count, and the next reader would inherit
        // a client that is offline only by coincidence. `CloudKms` arrived
        // exactly that way.
        for (method, url) in methods.iter().zip(urls.iter()) {
            assert!(
                *method == "SHELL" || url.starts_with("http://127.0.0.1:1"),
                "{method} {url} escapes the offline client — add its GcpService \
                 to offline_dry_run_client"
            );
        }

        assert!(
            urls[0].contains("services:batchEnable"),
            "step 1 services: {}",
            urls[0]
        );
        assert!(
            urls[1].contains("services:batchEnable"),
            "step 1b services: {}",
            urls[1]
        );
        assert!(
            urls[2].contains("/global/networks"),
            "step 2 network: {}",
            urls[2]
        );
        assert!(
            urls[3].contains("/regions/us-west4/subnetworks"),
            "step 2b subnet: {}",
            urls[3]
        );
        // No database stage sits between the subnet and the buckets: the
        // provisioner creates no instance, and opens no route to one.
        for (method, url) in methods.iter().zip(urls.iter()) {
            assert!(
                !url.contains("/instances") && !url.contains("sql"),
                "the retired managed-database stage must record nothing: {method} {url}"
            );
        }
        assert_body_contains(&calls[4], "my-project-assets", "step 3a assets bucket");
        assert_eq!(methods[5], "GET", "step 3a CORS read: {}", urls[5]);
        assert_eq!(methods[6], "PATCH", "step 3a CORS: {}", urls[6]);
        assert_body_contains(&calls[6], "maxAgeSeconds", "step 3a CORS body");
        assert_body_contains(
            &calls[6],
            "https://www.example.test",
            "step 3a CORS must use the configured public origin",
        );
        assert_body_contains(
            &calls[7],
            "my-project-documents",
            "step 3b documents bucket",
        );
        assert_body_contains(&calls[8], "my-project-exports", "step 3c exports bucket");
        assert_body_contains(&calls[9], "my-project-logs", "step 3d logs bucket");
        assert_body_contains(
            &calls[10],
            "my-project-applications",
            "step 3e applications bucket",
        );
        assert_eq!(
            methods[11], "PATCH",
            "step 3e applications lifecycle: {}",
            urls[11]
        );
        assert_body_contains(
            &calls[11],
            &format!("\"age\":{}", super::buckets::APPLICATIONS_RETENTION_DAYS),
            "step 3e applications bucket expires orphaned assets at the retention limit",
        );
        // Steps 12..=20 are direct runtime and Workspace identity shell-outs.
        for (i, m) in methods.iter().enumerate().take(21).skip(12) {
            assert_eq!(*m, "SHELL", "step {i} should be SHELL, got {m}");
        }
        assert!(urls[12].contains("service-accounts create navigator-web"));
        assert!(urls[13].contains("service-accounts create navigator-drive"));
        assert!(urls[14].contains("roles/secretmanager.secretAccessor"));
        assert!(urls[20].contains("roles/iam.serviceAccountTokenCreator"));

        // Steps 21..=31 are the Artifact Registry REST calls.
        assert!(
            urls[21].contains("/repositories?repositoryId=navigator"),
            "step 5a repo create: {}",
            urls[21]
        );
        assert_eq!(methods[22], "PATCH", "step 5b cleanup policy: {}", urls[22]);
        // Retention is a version COUNT, not an age. Both halves are asserted
        // because the DELETE half matches every version and would empty the
        // repository without its KEEP partner.
        assert_body_contains(&calls[22], "\"keepCount\":10", "step 5b retained versions");
        assert_body_contains(&calls[22], "\"action\":\"KEEP\"", "step 5b keep policy");
        assert!(
            urls[23].ends_with("/serviceAccounts"),
            "step 5c CI service account: {}",
            urls[23]
        );
        assert!(
            urls[29].contains("workloadIdentityPools/github/providers"),
            "step 5j WIF provider: {}",
            urls[29]
        );
        assert_body_contains(
            &calls[29],
            &super::artifact_registry::wif_attribute_condition(super::DEFAULT_GITHUB_REPO),
            "step 5j WIF provider repository condition",
        );
        assert_body_contains(
            &calls[29],
            super::artifact_registry::GITHUB_OIDC_ISSUER,
            "step 5j WIF provider issuer",
        );
        // Steps 32..=38 create GKE, bind its pool, then add integrations.
        // Bounded rather than open-ended: these are the shell-out steps, not
        // "everything after 32" — the KMS stage below is REST and follows them.
        for (i, m) in methods.iter().enumerate().take(39).skip(32) {
            assert_eq!(*m, "SHELL", "step {i} should be SHELL, got {m}");
        }
        assert!(
            urls[32].contains("compute addresses create"),
            "step 6a static IP: {}",
            urls[32]
        );
        assert!(
            urls[33].contains("container clusters create-auto"),
            "step 6b cluster: {}",
            urls[33]
        );
        assert!(urls[34].contains("navigator/navigator-web"));
        assert!(urls[35].contains("navigator/workflows-service"));
        assert!(
            urls[36].contains("fleet config-management enable"),
            "step 6c fleet enable: {}",
            urls[36]
        );
        assert!(
            urls[37].contains("container clusters update navigator-prod")
                && urls[37].contains("--enable-fleet"),
            "step 6d fleet reconciliation through the GKE cluster API: {}",
            urls[37]
        );
        assert!(
            urls[38].starts_with("kubectl apply"),
            "step 6e kubectl apply: {}",
            urls[38]
        );
        // Step 8: the key this deployment's `secrets.enc.yaml` is encrypted
        // against. Ring before key — the key create 404s otherwise.
        assert!(
            urls[39].contains("keyRings?keyRingId=navigator-secrets"),
            "step 8a key ring: {}",
            urls[39]
        );
        assert!(
            urls[40]
                .contains("keyRings/navigator-secrets/cryptoKeys?cryptoKeyId=deployment-config"),
            "step 8b crypto key: {}",
            urls[40]
        );
        assert_body_contains(&calls[40], "ENCRYPT_DECRYPT", "step 8b key purpose");
    }

    #[tokio::test]
    async fn cluster_pool_exists_before_kubernetes_service_account_bindings() {
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            public_base_url: Some("https://www.example.test".into()),
            images_project_id: Some(super::tenants::HUB_PROJECT_ID.into()),
            ..super::SetupConfig::default()
        };
        super::run(&client, "my-project", &config).await.unwrap();

        let calls = client.recorded_calls();
        let cluster = calls
            .iter()
            .position(|call| call.url.contains("container clusters create-auto"))
            .expect("the pipeline creates the Autopilot cluster");
        let navigator_web_binding = calls
            .iter()
            .position(|call| {
                call.url.contains("roles/iam.workloadIdentityUser")
                    && call.url.contains("navigator/navigator-web")
            })
            .expect("the pipeline binds the navigator-web Kubernetes account");
        let workflows_binding = calls
            .iter()
            .position(|call| {
                call.url.contains("roles/iam.workloadIdentityUser")
                    && call.url.contains("navigator/workflows-service")
            })
            .expect("the pipeline binds the workflows-service Kubernetes account");

        assert!(
            cluster < navigator_web_binding,
            "the GKE workload identity pool must exist before navigator-web is bound: {calls:?}"
        );
        assert!(
            cluster < workflows_binding,
            "the GKE workload identity pool must exist before workflows-service is bound: {calls:?}"
        );
    }

    #[track_caller]
    fn assert_live_run_sheet(prose: &str) {
        assert!(
            prose.matches("navigator ops gcp hub setup").count() >= 2,
            "DEPLOY.md must show both dry-run and live image-hub setup commands",
        );
        // One live setup per deployments/ directory: the coordinates are
        // exported from that directory's config.toml, so the command sheet
        // never hardcodes a provider shell-out or a deployment that has no
        // config to provision from.
        assert!(
            prose.contains("deployments/neon-law-stg/config.toml"),
            "DEPLOY.md must export setup coordinates from the deployment's config.toml",
        );
        assert!(
            prose.contains("navigator ops gcp setup --dry-run"),
            "DEPLOY.md must show the per-deployment dry-run setup command",
        );
        assert!(
            prose.contains("navigator ops gcp setup\n"),
            "DEPLOY.md must show the per-deployment live setup command",
        );
        assert!(
            prose.contains(
                "record the public value as `NAVIGATOR_GATEWAY_IP` in the matching `config.toml`"
            ),
            "DEPLOY.md must record every resolved gateway IP as a committed coordinate",
        );
    }

    /// Cross-reference the "Deploy the Neon Law Navigator" workshop prose
    /// (`server/content/workshops/navigator/DEPLOY.md`) against the pipeline
    /// it teaches. If the prose names a service, bucket, or command the
    /// dry-run does not actually use — or omits one it does — this test
    /// fails and the workshop is stale. The *renderable* half of the
    /// contract (route, brand, stepped-content shape) lives in
    /// `features/tests/features/deploy_the_navigator_walkthrough.feature`;
    /// this is the half that can only run where `super::run` is reachable.
    #[tokio::test]
    async fn deploy_workshop_prose_matches_the_dry_run_pipeline() {
        use super::services::REQUIRED_SERVICES;

        let deploy_md = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/content/workshops/navigator/DEPLOY.md"
        );
        let prose = std::fs::read_to_string(deploy_md)
            .expect("read DEPLOY.md — the deploy workshop must exist for this grounding test");

        // Use the same placeholder project id the prose prints, so the
        // recorded bucket names line up with the names the workshop shows.
        let project_id = "your-project-id";
        let client = offline_dry_run_client();
        let config = super::SetupConfig {
            public_base_url: Some("https://www.example.test".into()),
            ..super::SetupConfig::default()
        };
        super::run(&client, project_id, &config).await.unwrap();
        let calls = client.recorded_calls();

        // 1. The command the workshop prints is the real invocation —
        //    `cargo run -p cli -- ops gcp setup`, with cargo's `--`
        //    separator (the orchestration commands collapsed into the
        //    `navigator` CLI; there is no longer a separate `devx` binary).
        assert!(
            prose.contains("cargo run -p cli -- ops gcp setup --project-id"),
            "DEPLOY.md must print the real `cargo run -p cli -- ops gcp setup --project-id` command",
        );
        assert!(
            prose.contains("--dry-run"),
            "DEPLOY.md must teach the --dry-run preview",
        );

        // 2. The prose's API count matches REQUIRED_SERVICES exactly,
        //    and each short name is named in the prose.
        assert_eq!(
            REQUIRED_SERVICES.len(),
            22,
            "the workshop says twenty-two APIs; keep prose and code in lockstep",
        );
        assert!(
            prose.contains("twenty-two"),
            "DEPLOY.md must state the API count in words (twenty-two)",
        );
        for svc in REQUIRED_SERVICES {
            let short = svc.strip_suffix(".googleapis.com").unwrap_or(svc);
            assert!(
                prose.contains(short),
                "DEPLOY.md must name the {svc} API (looked for `{short}`)",
            );
        }
        assert!(
            prose.contains("batchEnable"),
            "DEPLOY.md must name the serviceusage.batchEnable call",
        );

        // 3. Exactly five buckets are created, and the prose names each
        //    (this is what kills the stale "two buckets" drift).
        let mut bucket_names: Vec<String> = calls
            .iter()
            .filter_map(|c| c.body.as_deref())
            .flat_map(|body| {
                [
                    super::ASSETS_BUCKET_SUFFIX,
                    super::DOCUMENTS_BUCKET_SUFFIX,
                    super::EXPORTS_BUCKET_SUFFIX,
                    super::LOGS_BUCKET_SUFFIX,
                    super::APPLICATIONS_BUCKET_SUFFIX,
                ]
                .into_iter()
                .map(|suffix| format!("{project_id}{suffix}"))
                .filter(|name| body.contains(name.as_str()))
                .collect::<Vec<_>>()
            })
            .collect();
        bucket_names.sort();
        bucket_names.dedup();
        assert_eq!(
            bucket_names.len(),
            5,
            "pipeline must create exactly five buckets, got {bucket_names:?}",
        );
        for name in &bucket_names {
            assert!(
                prose.contains(name.as_str()),
                "DEPLOY.md must name the {name} bucket the pipeline creates",
            );
        }

        // 4. Idempotency: the prose teaches the 409-is-success rule.
        assert!(
            prose.contains("409"),
            "DEPLOY.md must explain that HTTP 409 means already-exists/success",
        );

        // 5. The run sheet contains one live command for the shared
        //    image hub and every isolated deployment. Matching the command
        //    through its newline distinguishes the live setup from the
        //    preceding `--dry-run` copy.
        assert_live_run_sheet(&prose);

        // 6. Scorpio's trust claim: the provisioner now generates no secret at
        //    all, so the workshop must not teach one being printed — and no
        //    literal credential may be baked into the prose either. A live
        //    secret is long and unbroken; honest prose is not, so a
        //    32-character alphanumeric run is the tell.
        assert!(
            !prose.contains("printed exactly once"),
            "setup prints no generated credential any more; DEPLOY.md must not promise one",
        );
        assert!(
            prose.contains("secrets.enc.yaml"),
            "DEPLOY.md must still name the sops destination for a deployment's own credentials",
        );
        let longest_alnum_run = prose
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::len)
            .max()
            .unwrap_or(0);
        assert!(
            longest_alnum_run < 32,
            "DEPLOY.md must not bake in a literal credential (found a {longest_alnum_run}-char token)",
        );
    }

    #[test]
    fn deploy_workshop_records_the_substrate_checkpoint_and_unshipped_production() {
        let deploy_md = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/content/workshops/navigator/DEPLOY.md"
        );
        let raw = std::fs::read_to_string(deploy_md)
            .expect("read DEPLOY.md — the deploy workshop must exist for this grounding test");
        // Collapse whitespace before matching. These are prose sentences, and
        // `navigator validate` reflows prose to fill 120 columns — so a wording
        // edit anywhere earlier in a paragraph can push a pinned sentence across
        // a line break and fail this test for a reason that is not a regression.
        // The sibling grounding test in `cli/tests/workshop_command_grounding.rs`
        // already normalizes; this one matched raw bytes and broke exactly that
        // way. Line breaks are not part of the claim being pinned.
        let prose = raw.split_whitespace().collect::<Vec<_>>().join(" ");

        for observed in [
            "the two production substrates have completed every setup stage",
            "Both production GKE Autopilot clusters are `RUNNING`",
            // Pinned on the provisioner invariant rather than on instance
            // state. The previous claim — that both instances were `RUNNABLE`
            // — was a fact about live infrastructure that this test could not
            // observe, so deleting an instance turned the assertion into a
            // fiction it kept passing on. These two survive the decommission:
            // `setup` creating no instance is a property of the tree, and the
            // operator's remaining step is a statement about the runbook.
            "`ops gcp setup` provisions no Cloud SQL instance",
            "An operator must export the two legacy production Postgres 15 instances",
            "created but unprovisioned project",
            "The two production clusters currently have no application namespace",
            "their public hosts do not yet answer TLS",
            "An operator must ship one immutable release",
        ] {
            assert!(
                prose.contains(observed),
                "DEPLOY.md must preserve the observed substrate checkpoint `{observed}`",
            );
        }
        for forbidden_domain in ["@neonlaw.com", "@neon-law-stg.iam.gserviceaccount.com"] {
            assert!(
                !prose.contains(forbidden_domain),
                "DEPLOY.md must describe IAM principals without shipping email-shaped addresses from `{forbidden_domain}`",
            );
        }
        assert!(
            !prose.contains("remaining production rollout is incomplete")
                && !prose.contains("public DNS records are not reconciled")
                && !prose.contains("`neon-law-stg` completed stages 1–11")
                && !prose.contains("No Navigator release has been shipped")
                && !prose.contains("no Deployments, Pods, Services, Gateways, or HTTPRoutes"),
            "DEPLOY.md must not regress the completed substrates, DNS cutover, or staging rollout",
        );
    }
}
