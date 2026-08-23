//! The Cloud KMS key each deployment's `secrets.enc.yaml` is encrypted
//! against, in that deployment's own project.
//!
//! This is the first link in the chain `docs/deployment-secrets.md` describes:
//!
//! ```text
//! KMS key -> sops -> secrets.enc.yaml (in the repo) -> ops secrets apply
//!         -> Secret Manager -> CSI -> Secret -> pod
//! ```
//!
//! ## Why the key lives in the deployment's own project
//!
//! `deployments/neon-law-stg/config.toml` states the rule: staging's key must
//! not be decryptable by production's principals, and the reverse. The IAM
//! binding therefore lives on the key, inside the project, and is never
//! inherited from the organization — an organization-level binding is lost
//! when a project moves between organizations, which would silently change who
//! can read the archive.
//!
//! A shared key would defeat that outright, so nothing here takes a key name
//! from a flag. The path is derived from the deployment's own project and
//! region, and a test asserts the derivation matches the `kms_key` every
//! deployment in the tree declares.
//!
//! ## Idempotency
//!
//! Both creates POST unconditionally and treat HTTP 409 Conflict as success,
//! the convention every other `ensure_*` in this pipeline follows.
//!
//! ## What this deliberately does not do
//!
//! No rotation schedule. A SOPS document is encrypted against a specific key
//! version, so rotating means re-encrypting every `secrets.enc.yaml` in the
//! tree — a reviewed operation, not a property of provisioning.

use serde_json::json;

use super::artifact_registry::EnsureOutcome;
use super::client::{GcpClient, GcpService};
use super::error::{SetupError, SetupResult};

/// The key ring every deployment's key sits in.
pub const KEY_RING: &str = "navigator-secrets";
/// The key `sops` encrypts a deployment's `secrets.enc.yaml` against.
pub const CRYPTO_KEY: &str = "deployment-config";

/// The fully qualified key name for one deployment, in the spelling
/// `config.toml` and `.sops.yaml` both carry.
#[must_use]
pub fn key_name(project_id: &str, location: &str) -> String {
    format!(
        "projects/{project_id}/locations/{location}/keyRings/{KEY_RING}/cryptoKeys/{CRYPTO_KEY}"
    )
}

/// Create the key ring if it is not already there.
pub async fn ensure_key_ring(
    client: &GcpClient,
    project_id: &str,
    location: &str,
) -> SetupResult<EnsureOutcome> {
    let response = client
        .post_json(
            GcpService::CloudKms,
            &format!(
                "/v1/projects/{project_id}/locations/{location}/keyRings?keyRingId={KEY_RING}"
            ),
            &json!({}),
        )
        .await?;

    match response.status_u16() {
        200..=299 => Ok(EnsureOutcome::Created),
        409 => Ok(EnsureOutcome::AlreadyExists),
        status => Err(SetupError::BadStatus {
            operation: format!("create key ring {KEY_RING} in {project_id}/{location}"),
            status,
            body: response.into_text(),
        }),
    }
}

/// Create the deployment-config key if it is not already there.
///
/// `ENCRYPT_DECRYPT` because `sops` wraps its own data key with this one.
pub async fn ensure_crypto_key(
    client: &GcpClient,
    project_id: &str,
    location: &str,
) -> SetupResult<EnsureOutcome> {
    let response = client
        .post_json(
            GcpService::CloudKms,
            &format!(
                "/v1/projects/{project_id}/locations/{location}/keyRings/{KEY_RING}\
                 /cryptoKeys?cryptoKeyId={CRYPTO_KEY}"
            ),
            &json!({ "purpose": "ENCRYPT_DECRYPT" }),
        )
        .await?;

    match response.status_u16() {
        200..=299 => Ok(EnsureOutcome::Created),
        409 => Ok(EnsureOutcome::AlreadyExists),
        status => Err(SetupError::BadStatus {
            operation: format!("create crypto key {CRYPTO_KEY} in {project_id}/{location}"),
            status,
            body: response.into_text(),
        }),
    }
}

/// Both halves, in order. The ring must exist before the key.
pub async fn ensure(client: &GcpClient, project_id: &str, location: &str) -> SetupResult<()> {
    ensure_key_ring(client, project_id, location).await?;
    ensure_crypto_key(client, project_id, location).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::StaticToken;
    use super::*;

    fn client(server: &MockServer) -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("test-token".into())))
            .with_base_url(GcpService::CloudKms, server.uri())
    }

    #[tokio::test]
    async fn the_key_ring_is_created_in_the_deployments_own_project_and_region() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/neon-law-stg/locations/us-west4/keyRings",
            ))
            .and(query_param("keyRingId", KEY_RING))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let outcome = ensure_key_ring(&client(&server), "neon-law-stg", "us-west4")
            .await
            .expect("the key ring is created");
        assert_eq!(outcome, EnsureOutcome::Created);
    }

    #[tokio::test]
    async fn the_crypto_key_declares_encrypt_decrypt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/neon-law-stg/locations/us-west4/keyRings/navigator-secrets/cryptoKeys",
            ))
            .and(query_param("cryptoKeyId", CRYPTO_KEY))
            .and(body_json(json!({ "purpose": "ENCRYPT_DECRYPT" })))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let outcome = ensure_crypto_key(&client(&server), "neon-law-stg", "us-west4")
            .await
            .expect("the crypto key is created");
        assert_eq!(outcome, EnsureOutcome::Created);
    }

    /// Re-running setup against a provisioned project must converge, not fail.
    #[tokio::test]
    async fn an_existing_ring_and_key_converge() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(409).set_body_string("ALREADY_EXISTS"))
            .mount(&server)
            .await;

        let client = client(&server);
        assert_eq!(
            ensure_key_ring(&client, "neon-law", "us-west4")
                .await
                .expect("409 converges"),
            EnsureOutcome::AlreadyExists
        );
        assert_eq!(
            ensure_crypto_key(&client, "neon-law", "us-west4")
                .await
                .expect("409 converges"),
            EnsureOutcome::AlreadyExists
        );
    }

    #[tokio::test]
    async fn a_permission_failure_names_the_operation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("PERMISSION_DENIED"))
            .mount(&server)
            .await;

        let error = ensure_key_ring(&client(&server), "neon-law-stg", "us-west4")
            .await
            .expect_err("403 is not converged away");
        let message = error.to_string();
        assert!(message.contains(KEY_RING), "{message}");
        assert!(message.contains("neon-law-stg"), "{message}");
    }

    /// The assertion this module exists for.
    ///
    /// `each_deployment_declares_the_kms_key_its_sops_rule_uses` proves
    /// `.sops.yaml` and `config.toml` agree with each other, but both are
    /// hand-written — they could agree on a key nothing provisions. This ties
    /// the provisioner to them, so a deployment cannot declare one key while
    /// `ops gcp setup` creates another.
    #[test]
    fn the_derived_key_name_is_what_every_deployment_declares() {
        use super::super::super::deployments::{names, Deployment};

        // The synthetic tree, because the real rows moved to a private
        // repository. `navigator ops deployments check` runs this same
        // agreement against them; see
        // `cli/tests/fixtures/deployment-tree/README.md`.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("deployment-tree");
        let names = names(&root).expect("the deployments tree is readable");
        assert!(!names.is_empty(), "the tree must describe a deployment");

        for name in names {
            let deployment = Deployment::load(&root, &name).expect("the deployment loads");
            let project_id = deployment
                .coordinates
                .get("NAVIGATOR_GCP_PROJECT_ID")
                .expect("every deployment names its project");
            let location = deployment
                .coordinates
                .get("NAVIGATOR_GCP_LOCATION")
                .expect("every deployment names its location");
            assert_eq!(
                deployment.kms_key,
                key_name(project_id, location),
                "deployments/{name}/config.toml declares a kms_key that `ops gcp setup` would \
                 not create"
            );
        }
    }
}
