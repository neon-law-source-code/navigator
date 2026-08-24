//! Grounding tests for the environment tables in the Operating workshop.
//!
//! The workshop is the operator's teaching surface; `.env.example` is the
//! detailed committed contract. These checks keep every variable needed to
//! bring up local KIND, start `web` / `workflows-service`, and provision or
//! ship the reference GKE deployment visible in both places.

use std::path::Path;

fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} — {e}", path.display()))
}

fn environment_matrix() -> String {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let after = deploy
        .split_once("\n## Environment Matrix")
        .expect("DEPLOY.md must carry an Environment Matrix section")
        .1;
    match after.split_once("\n## ") {
        Some((body, _)) => body.to_string(),
        None => after.to_string(),
    }
}

const LOCAL_CONTROL_VARS: &[&str] = &[
    "NAVIGATOR_KIND_CLUSTER",
    "NAVIGATOR_K8S_NAMESPACE",
    "NAVIGATOR_KIND_DEPS_OVERLAY",
    "NAVIGATOR_KIND_OVERLAY",
    "NAVIGATOR_GKE_OVERLAY",
    "NAVIGATOR_PRIVATE_MODE",
    "NAVIGATOR_KIND_SURREAL_PORT",
    "NAVIGATOR_KIND_RESTATE_INGRESS_PORT",
    "NAVIGATOR_KIND_RESTATE_ADMIN_PORT",
    "NAVIGATOR_KIND_RAUTHY_PORT",
    "NAVIGATOR_KIND_GARAGE_S3_PORT",
    "NAVIGATOR_KIND_WEB_PORT",
    "NAVIGATOR_KIND_OPENOBSERVE_PORT",
    "NAVIGATOR_KIND_OPENOBSERVE_OTLP_PORT",
    "NAVIGATOR_GARAGE_ACCESS_KEY",
    "NAVIGATOR_GARAGE_SECRET_KEY",
    "NAVIGATOR_GARAGE_ASSETS_ACCESS_KEY",
    "NAVIGATOR_GARAGE_ASSETS_SECRET_KEY",
    "NAVIGATOR_GARAGE_APPLICATIONS_ACCESS_KEY",
    "NAVIGATOR_GARAGE_APPLICATIONS_SECRET_KEY",
    "NAVIGATOR_GARAGE_LFS_ACCESS_KEY",
    "NAVIGATOR_GARAGE_LFS_SECRET_KEY",
    "NAVIGATOR_IMAGE_TAG",
];

const GENERATED_LOCAL_VARS: &[&str] = &[
    "PORT",
    "NAVIGATOR_ENVIRONMENT",
    "NAVIGATOR_CI_HARNESS",
    "NAVIGATOR_GIT_REPO_ROOT",
    "NAVIGATOR_SURREAL_ENDPOINT",
    "NAVIGATOR_SURREAL_NAMESPACE",
    "NAVIGATOR_SURREAL_DATABASE",
    "NAVIGATOR_SURREAL_USER",
    "NAVIGATOR_SURREAL_PASSWORD",
    "NAVIGATOR_STORAGE_BACKEND",
    "NAVIGATOR_STORAGE_ENDPOINT",
    "NAVIGATOR_STORAGE_BUCKET",
    "NAVIGATOR_ASSETS_BUCKET",
    "NAVIGATOR_APPLICATIONS_BUCKET",
    "NAVIGATOR_LFS_BUCKET",
    "NAVIGATOR_STORAGE_REGION",
    "NAVIGATOR_STORAGE_ACCESS_KEY",
    "NAVIGATOR_STORAGE_SECRET_KEY",
    "NAVIGATOR_ASSETS_ACCESS_KEY",
    "NAVIGATOR_ASSETS_SECRET_KEY",
    "NAVIGATOR_APPLICATIONS_ACCESS_KEY",
    "NAVIGATOR_APPLICATIONS_SECRET_KEY",
    "NAVIGATOR_LFS_ACCESS_KEY",
    "NAVIGATOR_LFS_SECRET_KEY",
    "OAUTH_ISSUER_URL",
    "OAUTH_CLIENT_ID",
    "OAUTH_CLIENT_SECRET",
    "OAUTH_REDIRECT_URI",
    "SESSION_SECRET",
    "RESTATE_BROKER_URL",
    "SENDGRID_API_KEY",
    "SENDGRID_INBOUND_SECRET",
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "NAVIGATOR_OPENOBSERVE_URL",
    "NAVIGATOR_OPENOBSERVE_USERNAME",
    "NAVIGATOR_OPENOBSERVE_PASSWORD",
    "NAVIGATOR_OPENOBSERVE_ORGANIZATION",
    "NAVIGATOR_OPENOBSERVE_STREAM",
];

const DEPLOYED_RUNTIME_VARS: &[&str] = &[
    "NAVIGATOR_CREDENTIAL_ENVIRONMENT",
    "NAVIGATOR_CUSTOM_BRANDING",
    "NAVIGATOR_DOCUMENTS_BUCKET",
    "NAVIGATOR_EXPORTS_ACCESS_KEY",
    "NAVIGATOR_EXPORTS_SECRET_KEY",
    "NAVIGATOR_STORAGE_FS_ROOT",
    "NAVIGATOR_STORAGE_SESSION_TOKEN",
    "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
    "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
    "NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID",
    "NAVIGATOR_DRIVE_NEON_LAW_DELEGATED_USER",
    "NAVIGATOR_DRIVE_NEON_LAW_SERVICE_ACCOUNT_JSON",
    "NAV_BASE_URL",
    "CANONICAL_HOST",
    "NAVIGATOR_ASSET_BASE_URL",
    "NAVIGATOR_BOOTSTRAP_OWNER_EMAIL",
    "NAVIGATOR_BOOTSTRAP_COMPANY",
    "OIDC_JWKS_URL",
    "OIDC_AUDIENCE",
    "OIDC_ISSUER",
    "OIDC_HS256_SECRET",
    "OIDC_DISABLED",
    "GOOGLE_OAUTH_CLIENT_IDS",
    "GOOGLE_OAUTH_REQUIRED_HD",
    "GOOGLE_TOKENINFO_URL",
    "NAVIGATOR_IDENTITY_PLATFORM_API_KEY",
    "NAVIGATOR_IDENTITY_PLATFORM_ENDPOINT",
    "NAVIGATOR_GCP_METADATA_ENDPOINT",
    "NAVIGATOR_RATE_LIMIT_PER_MIN",
    "RESTATE_AUTH_TOKEN",
    "RESTATE_SERVICE",
    "RESTATE_INGRESS_URL",
    "WORKFLOWS_SERVICE_LISTEN",
    "WORKFLOWS_WEBHOOK_LISTEN",
    "NAVIGATOR_EMAIL_BACKEND",
    "SENDGRID_FROM_EMAIL",
    "SENDGRID_EVENTS_SECRET",
    "SENDGRID_EVENTS_PUBLIC_KEY",
    "NAVIGATOR_PARSE_HOST",
    "NAVIGATOR_LAWYER_NOTIFY_EMAIL",
    "NAVIGATOR_DKIM_REQUIRE_DOMAIN",
    "SLACK_WEBHOOK_URL",
    "DOCUSIGN_BASE_URL",
    "DOCUSIGN_ACCOUNT_ID",
    "DOCUSIGN_INTEGRATION_KEY",
    "DOCUSIGN_USER_ID",
    "DOCUSIGN_PRIVATE_KEY",
    "DOCUSIGN_OAUTH_BASE",
    "DOCUSIGN_ACCESS_TOKEN",
    "DOCUSIGN_SIGNER_EMAIL",
    "DOCUSIGN_SIGNER_NAME",
    "DOCUSIGN_HMAC_KEY",
    "DOCUSIGN_WEBHOOK_SECRET",
    "XERO_TENANT_ID",
    "XERO_BASE_URL",
    "XERO_CLIENT_ID",
    "XERO_CLIENT_SECRET",
    "XERO_TOKEN_URL",
    "XERO_SCOPE",
    "XERO_ACCESS_TOKEN",
    "NAVIGATOR_GIT_HOST",
    "NAVIGATOR_GITHUB_ORG",
    "NAVIGATOR_GITHUB_APP_ID",
    "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
    "NAVIGATOR_GITHUB_INSTALLATION_ID",
    "NAVIGATOR_GITHUB_API_BASE",
    "OTEL_SERVICE_NAME",
    "RUST_LOG",
    "NAVIGATOR_RELEASE_TAG",
    "NAVIGATOR_GIT_SHA",
    "NAVIGATOR_BUILD_TIME",
    "NAVIGATOR_PUBLIC_DIR",
    "NAVIGATOR_BLOG_DIR",
    "NAVIGATOR_WORKSHOPS_DIR",
    "NAVIGATOR_GCP_PROJECT_ID",
    "NAVIGATOR_GCP_LOCATION",
    "NAVIGATOR_ROUTER_MODEL",
    "NAVIGATOR_CONTRACT_REVIEW_MODEL",
    "GOOGLE_METADATA_URL",
    "NAVIGATOR_ONCHAIN_BACKEND",
    "SOLANA_RPC_URL",
    "SOLANA_PROGRAM_ID",
    "SOLANA_SIGNER_SECRET",
    "BILLING_EXPORT_TABLE",
    "BIGQUERY_PROJECT",
    "BILLING_CANARY_NOTIFY_EMAIL",
];

const PROVISION_AND_SHIP_VARS: &[&str] = &[
    "NAVIGATOR_GKE_CLUSTER_NAME",
    "NAVIGATOR_GKE_CONTEXT",
    "NAVIGATOR_IMAGE_REGISTRY",
    "NAVIGATOR_VPC_NAME",
    "NAVIGATOR_GATEWAY_IP_NAME",
    "NAVIGATOR_CONFIG_SYNC_REPO",
    "NAVIGATOR_CONFIG_SYNC_DIR",
    "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
    "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
    "NAVIGATOR_PRIMARY_DOMAIN",
    "NAVIGATOR_WEB_SECRET_NAME",
    "NAVIGATOR_WORKFLOWS_URL",
    "RESTATE_ADMIN_URL",
    "RESTATE_ADMIN_TOKEN",
];

#[test]
fn operating_workshop_and_env_contract_list_every_startup_variable() {
    let workshop = environment_matrix();
    let env_example = repo_file(".env.example");

    for variable in LOCAL_CONTROL_VARS
        .iter()
        .chain(GENERATED_LOCAL_VARS)
        .chain(DEPLOYED_RUNTIME_VARS)
        .chain(PROVISION_AND_SHIP_VARS)
    {
        assert!(
            workshop.contains(variable),
            "Operating workshop Environment Matrix must list `{variable}`",
        );
        assert!(
            env_example.contains(variable),
            ".env.example must document `{variable}` named by the Operating workshop",
        );
    }
}

#[test]
fn operating_workshop_lists_every_committed_environment_variable() {
    let workshop = environment_matrix();
    let env_example = repo_file(".env.example");
    let variables = env_example.lines().filter_map(|line| {
        let assignment = line.trim_start().trim_start_matches('#').trim_start();
        let (key, _) = assignment.split_once('=')?;
        (!key.is_empty()
            && key.chars().all(|character| {
                character == '_' || character.is_ascii_uppercase() || character.is_ascii_digit()
            }))
        .then_some(key)
    });

    for variable in variables {
        assert!(
            workshop.contains(variable),
            "Operating workshop Environment Matrix must list `{variable}` from .env.example",
        );
    }
}

#[test]
fn operating_workshop_teaches_profiles_precedence_and_simulation() {
    let workshop = environment_matrix();
    for phrase in [
        "process environment",
        ".env",
        ".devx/env",
        "Test",
        "Dev",
        "Production",
        "canonical seed",
        "Test-local database fixtures",
        "StubBillingProvider",
        "StubContractReviewer",
        "CapturingEmail",
    ] {
        assert!(
            workshop.contains(phrase),
            "Operating workshop Environment Matrix must explain `{phrase}`",
        );
    }
}
