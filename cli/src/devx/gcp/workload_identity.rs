//! Per-deployment Google service account and GKE Workload Identity bindings.
//!
//! The embedded manifests bind both runtime Kubernetes service accounts to
//! this GSA. Provisioning it here keeps `ops gcp setup` honest for an empty
//! project: Secret Manager and bucket access no longer depend on undocumented
//! manual IAM.

use super::client::{GcpClient, ShellResult};
use super::error::{SetupError, SetupResult};
use super::SetupConfig;

const PROJECT_ROLES: &[&str] = &[
    "roles/secretmanager.secretAccessor",
    "roles/aiplatform.user",
];
const KUBERNETES_SERVICE_ACCOUNTS: &[&str] = &["navigator-web", "workflows-service"];

/// Create the runtime GSA and idempotently bind its direct GCP access. The GKE
/// workload identity bindings are a separate step because the cluster must
/// first create the project's `<project>.svc.id.goog` pool.
pub async fn ensure_runtime_identity(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
    buckets: &[&str],
) -> SetupResult<()> {
    let account_id = &config.google_service_account_id;
    let gsa = format!("{account_id}@{project_id}.iam.gserviceaccount.com");

    create_service_account(
        client,
        project_id,
        account_id,
        &format!("Navigator deployment {account_id}"),
        "create deployment Google service account",
    )
    .await?;
    create_service_account(
        client,
        project_id,
        &config.drive_service_account_id,
        &format!(
            "Navigator Workspace Drive {}",
            config.drive_service_account_id
        ),
        "create deployment Workspace Drive service account",
    )
    .await?;
    bind_project_roles(client, project_id, &gsa).await?;
    bind_bucket_roles(client, &gsa, buckets).await?;
    bind_self_signing(client, project_id, &gsa).await
}

async fn create_service_account(
    client: &GcpClient,
    project_id: &str,
    account_id: &str,
    display_name: &str,
    operation: &'static str,
) -> SetupResult<()> {
    classify(
        client
            .shell_out(
                "gcloud",
                &[
                    "iam",
                    "service-accounts",
                    "create",
                    account_id,
                    "--project",
                    project_id,
                    "--display-name",
                    display_name,
                ],
            )
            .await?,
        operation,
    )
}

async fn bind_project_roles(client: &GcpClient, project_id: &str, gsa: &str) -> SetupResult<()> {
    for role in PROJECT_ROLES {
        classify(
            client
                .shell_out(
                    "gcloud",
                    &[
                        "projects",
                        "add-iam-policy-binding",
                        project_id,
                        "--member",
                        &format!("serviceAccount:{gsa}"),
                        "--role",
                        role,
                        "--condition=None",
                    ],
                )
                .await?,
            "bind deployment project role",
        )?;
    }
    Ok(())
}

async fn bind_bucket_roles(client: &GcpClient, gsa: &str, buckets: &[&str]) -> SetupResult<()> {
    for bucket in buckets {
        classify(
            client
                .shell_out(
                    "gcloud",
                    &[
                        "storage",
                        "buckets",
                        "add-iam-policy-binding",
                        &format!("gs://{bucket}"),
                        "--member",
                        &format!("serviceAccount:{gsa}"),
                        "--role",
                        "roles/storage.objectAdmin",
                    ],
                )
                .await?,
            "bind deployment bucket role",
        )?;
    }
    Ok(())
}

pub async fn bind_kubernetes_accounts(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
) -> SetupResult<()> {
    let gsa = format!(
        "{}@{project_id}.iam.gserviceaccount.com",
        config.google_service_account_id
    );
    for ksa in KUBERNETES_SERVICE_ACCOUNTS {
        let member = format!(
            "serviceAccount:{project_id}.svc.id.goog[{}/{ksa}]",
            config.kubernetes_namespace
        );
        classify(
            client
                .shell_out(
                    "gcloud",
                    &[
                        "iam",
                        "service-accounts",
                        "add-iam-policy-binding",
                        &gsa,
                        "--project",
                        project_id,
                        "--role",
                        "roles/iam.workloadIdentityUser",
                        "--member",
                        &member,
                    ],
                )
                .await?,
            "bind deployment Kubernetes service account",
        )?;
    }
    Ok(())
}

async fn bind_self_signing(client: &GcpClient, project_id: &str, gsa: &str) -> SetupResult<()> {
    classify(
        client
            .shell_out(
                "gcloud",
                &[
                    "iam",
                    "service-accounts",
                    "add-iam-policy-binding",
                    gsa,
                    "--project",
                    project_id,
                    "--role",
                    "roles/iam.serviceAccountTokenCreator",
                    "--member",
                    &format!("serviceAccount:{gsa}"),
                ],
            )
            .await?,
        "bind deployment self-signing role",
    )
}

fn classify(result: ShellResult, operation: &'static str) -> SetupResult<()> {
    if result.succeeded() || result.is_already_exists() {
        return Ok(());
    }
    Err(SetupError::ShellFailed {
        operation,
        command: result.command_line,
        exit: result.exit,
        stderr: result.stderr,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::devx::gcp::client::StaticToken;

    #[tokio::test]
    async fn dry_run_records_per_deployment_identity_and_bindings() {
        let client = GcpClient::new(Arc::new(StaticToken("t".into()))).with_dry_run();
        let config = SetupConfig {
            google_service_account_id: "example-a-web".into(),
            drive_service_account_id: "example-a-drive".into(),
            kubernetes_namespace: "example-a".into(),
            ..SetupConfig::default()
        };

        ensure_runtime_identity(
            &client,
            "neon-law-stg",
            &config,
            &[
                "example-a-assets",
                "example-a-documents",
                "example-a-exports",
                "example-a-logs",
                "example-a-applications",
            ],
        )
        .await
        .unwrap();
        bind_kubernetes_accounts(&client, "neon-law-stg", &config)
            .await
            .unwrap();

        let plan = client
            .recorded_calls()
            .iter()
            .map(|call| format!("{} {}", call.url, call.body.as_deref().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plan.contains("example-a-web"));
        assert!(plan.contains("example-a-drive"));
        assert!(plan.contains("roles/secretmanager.secretAccessor"));
        assert!(plan.contains("roles/aiplatform.user"));
        assert!(plan.contains("roles/storage.objectAdmin"));
        assert!(plan.contains("gs://example-a-applications"));
        assert!(!plan.contains("allUsers"));
        assert!(!plan.contains("roles/storage.objectViewer"));
        assert!(plan.contains("neon-law-stg.svc.id.goog[example-a/navigator-web]"));
        assert!(plan.contains("neon-law-stg.svc.id.goog[example-a/workflows-service]"));
        assert!(plan.contains("roles/iam.serviceAccountTokenCreator"));
        assert_eq!(client.recorded_calls().len(), 12);
    }
}
