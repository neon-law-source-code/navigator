//! Provision the shared image **hub** — and nothing else.
//!
//! `docs/environments.md` splits four projects into one hub and three runtime
//! projects containing three deployments. The hub (`ghcr`) holds the Artifact Registry every
//! deployment pulls from, the CI service account that pushes to it, and the
//! GitHub Workload Identity pool that lets Actions impersonate that account
//! keyless. Nothing runs there: no buckets, no GKE cluster, no IAP.
//!
//! That distinction is the whole reason this module exists. [`super::run`]
//! provisions an environment, and pointing it at the hub would create four
//! buckets and an Autopilot cluster in a project whose point is not to hold
//! them. So the hub gets its own entry point, and
//! [`super::tenants`] refuses either command aimed at the other's project
//! before the first GCP call.
//!
//! The steps themselves are the already-tested `artifact_registry` functions,
//! run against the hub project in the same order and with the same
//! idempotency contract: creates treat HTTP 409 as success, IAM bindings read
//! the live policy and skip a no-op write, and the cleanup policy is a PATCH.
//! A re-run after a partial failure converges.

use super::artifact_registry::{self, ci_service_account_email, wif_principal_set, WRITER_ROLE};
use super::client::GcpClient;
use super::error::SetupResult;
use super::tenants::{self, TenantRole};
use super::{
    services, DEFAULT_ARTIFACT_REGISTRY_REPO, DEFAULT_CI_PUSHER_ACCOUNT_ID, DEFAULT_GITHUB_REPO,
    DEFAULT_REGION,
};

/// The APIs the hub needs and nothing more. Compute, Storage, Container,
/// Config Management, Secret Manager, and IAP are deliberately absent —
/// enabling them here would advertise a capability the hub must not grow.
pub const REQUIRED_SERVICES: &[&str] = &[
    "artifactregistry.googleapis.com",
    "iam.googleapis.com",
    "iamcredentials.googleapis.com",
    "sts.googleapis.com",
];

/// Per-deployment overrides for `ops gcp hub setup`. Kept separate from
/// [`super::SetupConfig`] so environment-only resources — a VPC, a bucket, a
/// cluster — are unrepresentable from this command.
#[derive(Debug, Clone)]
pub struct HubSetupConfig {
    /// Artifact Registry location. Default: `us-west4`.
    pub region: String,
    /// Repository name that holds every container image. Default: `navigator`.
    pub artifact_registry_repo: String,
    /// `owner/repo` slug the Workload Identity provider trusts for keyless CI
    /// pushes. Default: `neon-law-source-code/navigator`.
    pub github_repo: String,
    /// Account id (local part of the SA email) of the CI pusher service
    /// account. Default: `navigator-ci-pusher`.
    pub ci_pusher_account_id: String,
}

impl Default for HubSetupConfig {
    fn default() -> Self {
        Self {
            region: DEFAULT_REGION.to_string(),
            artifact_registry_repo: DEFAULT_ARTIFACT_REGISTRY_REPO.to_string(),
            github_repo: DEFAULT_GITHUB_REPO.to_string(),
            ci_pusher_account_id: DEFAULT_CI_PUSHER_ACCOUNT_ID.to_string(),
        }
    }
}

/// Provision the hub. Steps, in order:
///
/// 1. Enable [`REQUIRED_SERVICES`] — nothing below works without them.
/// 2. The Docker-format Artifact Registry repository and its keep-last-10 cleanup
///    policy.
/// 3. The CI pusher service account, plus a repo-scoped
///    `roles/artifactregistry.writer` binding for it.
/// 4. The GitHub Workload Identity pool and `github-oidc` provider.
/// 5. `roles/iam.workloadIdentityUser` for the federated GitHub principal on
///    the CI service account, so Actions can impersonate it keyless.
///
/// Reader grants for the environments are *not* here: each environment writes
/// its own via `ops gcp setup --images-project-id`, so an environment's
/// provisioning run does not depend on re-running the hub's.
pub async fn run(
    client: &GcpClient,
    hub_project_id: &str,
    config: &HubSetupConfig,
) -> SetupResult<()> {
    tenants::validate_target(TenantRole::Hub, hub_project_id)?;

    services::enable(client, hub_project_id, REQUIRED_SERVICES).await?;

    let location = &config.region;
    let repo = &config.artifact_registry_repo;
    artifact_registry::ensure_repository(client, hub_project_id, location, repo).await?;
    artifact_registry::ensure_cleanup_policy(client, hub_project_id, location, repo).await?;

    let ci_sa = ci_service_account_email(&config.ci_pusher_account_id, hub_project_id);
    artifact_registry::ensure_ci_service_account(
        client,
        hub_project_id,
        &config.ci_pusher_account_id,
    )
    .await?;
    artifact_registry::ensure_repo_iam_member(
        client,
        hub_project_id,
        location,
        repo,
        WRITER_ROLE,
        &format!("serviceAccount:{ci_sa}"),
    )
    .await?;

    artifact_registry::ensure_wif_pool(client, hub_project_id).await?;
    artifact_registry::ensure_wif_provider(client, hub_project_id, &config.github_repo).await?;

    let project_number = artifact_registry::project_number(client, hub_project_id).await?;
    artifact_registry::ensure_wif_impersonation(
        client,
        hub_project_id,
        &ci_sa,
        &wif_principal_set(&project_number, &config.github_repo),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpClient, GcpService, StaticToken};
    use super::*;

    /// A dry-run client with every service the hub could reach pointed at an
    /// unreachable address, so a real HTTP call fails loudly instead of
    /// escaping the test. `Storage` and `Compute` are included deliberately:
    /// if the hub ever grew a bucket or a network, the call would be recorded
    /// here and the "and nothing else" assertion would fail rather than
    /// silently hitting a default production URL.
    fn offline_dry_run_client() -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ServiceUsage, "http://127.0.0.1:1")
            .with_base_url(GcpService::ArtifactRegistry, "http://127.0.0.1:1")
            .with_base_url(GcpService::Iam, "http://127.0.0.1:1")
            .with_base_url(GcpService::CloudResourceManager, "http://127.0.0.1:1")
            .with_base_url(GcpService::Storage, "http://127.0.0.1:1")
            .with_base_url(GcpService::Compute, "http://127.0.0.1:1")
            .with_dry_run()
    }

    #[test]
    fn required_services_provision_no_environment_capability() {
        for forbidden in [
            "storage.googleapis.com",
            "compute.googleapis.com",
            "container.googleapis.com",
            "anthosconfigmanagement.googleapis.com",
            "secretmanager.googleapis.com",
        ] {
            assert!(
                !REQUIRED_SERVICES.contains(&forbidden),
                "the hub is not an environment; it must not enable {forbidden}",
            );
        }
    }

    #[tokio::test]
    async fn dry_run_records_registry_wif_and_pusher_and_nothing_else() {
        let client = offline_dry_run_client();
        run(&client, "ghcr", &HubSetupConfig::default())
            .await
            .unwrap();

        let calls = client.recorded_calls();
        let urls: Vec<&str> = calls.iter().map(|c| c.url.as_str()).collect();

        // batchEnable + repo create + cleanup patch + SA create + writer
        // get/set + WIF pool + WIF provider + impersonation get/set = 10.
        // `project_number` short-circuits in dry-run, so no CRM call.
        assert_eq!(calls.len(), 10, "unexpected hub dry-run plan: {calls:?}");

        assert!(
            urls[0].ends_with("/projects/ghcr/services:batchEnable"),
            "step 1 enables APIs first: {}",
            urls[0]
        );
        assert!(
            urls[1]
                .contains("/projects/ghcr/locations/us-west4/repositories?repositoryId=navigator"),
            "step 2a repository: {}",
            urls[1]
        );
        assert_eq!(
            calls[2].method, "PATCH",
            "step 2b cleanup policy: {}",
            urls[2]
        );
        assert!(
            urls[3].ends_with("/projects/ghcr/serviceAccounts"),
            "step 3a CI pusher service account: {}",
            urls[3]
        );
        assert!(
            urls[5].ends_with(":setIamPolicy") && urls[5].contains("/repositories/navigator"),
            "step 3b repo writer binding: {}",
            urls[5]
        );
        assert!(
            urls[6].contains("workloadIdentityPools?workloadIdentityPoolId=github"),
            "step 4a WIF pool: {}",
            urls[6]
        );
        assert!(
            urls[7].contains("workloadIdentityPools/github/providers"),
            "step 4b WIF provider: {}",
            urls[7]
        );
        assert!(
            urls[9].contains("/serviceAccounts/navigator-ci-pusher@ghcr")
                && urls[9].ends_with(":setIamPolicy"),
            "step 5 impersonation binding: {}",
            urls[9]
        );

        // Nothing else: the hub is not an environment.
        for call in &calls {
            for forbidden in [
                "/b?project", // GCS bucket insert
                "/global/networks",
                "clusters create-auto",
                "/backendServices",
            ] {
                assert!(
                    !call.url.contains(forbidden),
                    "hub setup must not touch `{forbidden}`: {call:?}",
                );
            }
            assert_ne!(
                call.method, "SHELL",
                "hub setup shells out to nothing: {call:?}",
            );
        }
    }

    #[tokio::test]
    async fn the_hub_command_refuses_an_environment_project_before_any_call() {
        let client = offline_dry_run_client();
        let err = run(&client, "neon-law", &HubSetupConfig::default())
            .await
            .expect_err("neon-law is an environment, not the hub");

        assert!(err.to_string().contains("neon-law"), "{err}");
        assert!(
            client.recorded_calls().is_empty(),
            "the tenant guard must precede every GCP call, got {:?}",
            client.recorded_calls(),
        );
    }

    /// Mount the read side of a hub run: the project-number lookup and the
    /// two policy reads. The verbs differ by service — Artifact Registry
    /// routes `:getIamPolicy` on `GET`, IAM's service-account policies on
    /// `POST` — so the repository read needs its own mock rather than
    /// falling through the POST catch-all.
    async fn mount_reads(server: &MockServer) {
        // The impersonation binding needs a real project number, so the
        // CloudResourceManager lookup runs outside dry-run.
        Mock::given(method("GET"))
            .and(path("/v3/projects/ghcr"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "name": "projects/464694154887" })),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/ghcr/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(server)
            .await;
        // Remaining `getIamPolicy` reads: policies with no bindings, so every
        // binding is written rather than skipped. `done` keeps the same
        // response usable for any LRO that falls through to this catch-all.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn repository_and_pusher_land_in_the_hub_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/ghcr/services:batchEnable"))
            .and(body_partial_json(
                json!({ "serviceIds": REQUIRED_SERVICES }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/ghcr/locations/us-west4/repositories"))
            .and(query_param("repositoryId", "navigator"))
            .and(body_partial_json(json!({ "format": "DOCKER" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/v1/projects/ghcr/locations/us-west4/repositories/navigator",
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/ghcr/serviceAccounts"))
            .and(body_partial_json(
                json!({ "accountId": "navigator-ci-pusher" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/ghcr/locations/global/workloadIdentityPools/github/providers",
            ))
            .and(query_param("workloadIdentityPoolProviderId", "github-oidc"))
            .and(body_partial_json(json!({
                "oidc": { "issuerUri": artifact_registry::GITHUB_OIDC_ISSUER },
                "attributeCondition": artifact_registry::wif_attribute_condition("neon-law-source-code/navigator")
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/ghcr/serviceAccounts/navigator-ci-pusher@ghcr.iam.gserviceaccount.com:setIamPolicy",
            ))
            .and(body_partial_json(json!({
                "policy": { "bindings": [{
                    "role": "roles/iam.workloadIdentityUser",
                    "members": [wif_principal_set("464694154887", "neon-law-source-code/navigator")]
                }] }
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/ghcr/locations/global/workloadIdentityPools",
            ))
            .and(query_param("workloadIdentityPoolId", "github"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        mount_reads(&server).await;

        let mut client = GcpClient::new(Arc::new(StaticToken("t".into())));
        for service in [
            GcpService::ServiceUsage,
            GcpService::ArtifactRegistry,
            GcpService::Iam,
            GcpService::CloudResourceManager,
        ] {
            client = client.with_base_url(service, server.uri());
        }
        run(&client, "ghcr", &HubSetupConfig::default())
            .await
            .unwrap();
    }
}
