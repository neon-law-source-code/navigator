//! Resolving a Firm's provider client for one Project.
//!
//! The provider adapters in `cloud` take a token; the credential lives as an
//! encrypted row keyed by Firm. This module is the seam between them, and it
//! is a factory rather than a single injected client for one reason: the
//! token differs per Project, because it is resolved from that Project's
//! owning Firm. A process-wide `Arc<dyn NotionService>` would have to hold
//! one deployment-wide token, which is the arrangement the Firm-secret
//! boundary exists to replace.
//!
//! Errors here are mechanism-only. A caller learns that a credential is not
//! configured, that the Project has no owning Firm, or that the provider
//! refused — never the token, the ciphertext, or the provider's own body.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use uuid::Uuid;

use store::firm_secrets::{IntegrationProvider, IntegrationSecretKind};
use store::surreal::SurrealDb;

/// Why a Project's provider client could not be built or used.
#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("this deployment has no Firm integration runtime configured")]
    NotConfigured,
    #[error("the Firm has not configured this provider credential")]
    CredentialMissing,
    #[error("the project has no owning Firm")]
    NoFirm,
    #[error("no such project")]
    NoSuchProject,
    #[error("the stored credential could not be produced")]
    CredentialUnusable,
    #[error("the provider refused or was unreachable")]
    Provider,
    #[error("database: {0}")]
    Db(String),
}

impl IntegrationError {
    /// The stable slug a door reports. Distinct from the message so a caller
    /// can branch on the outcome without matching prose.
    #[must_use]
    pub const fn slug(&self) -> &'static str {
        match self {
            Self::NotConfigured => "runtime_not_configured",
            Self::CredentialMissing => "credential_missing",
            Self::CredentialUnusable => "credential_unusable",
            Self::NoFirm => "no_owning_firm",
            Self::NoSuchProject => "no_such_project",
            Self::Provider => "provider_unavailable",
            Self::Db(_) => "database_error",
        }
    }
}

fn secret_error(error: store::firm_secrets::SecretStoreError) -> IntegrationError {
    use store::firm_secrets::SecretStoreError as Error;
    match error {
        Error::NotConfigured { .. } => IntegrationError::CredentialMissing,
        Error::ProjectHasNoFirm => IntegrationError::NoFirm,
        Error::NoSuchProject(_) => IntegrationError::NoSuchProject,
        Error::Db(error) => IntegrationError::Db(error.to_string()),
        // A KMS fault, a context mismatch, or a payload that will not decode
        // are a credential that exists and cannot be produced — a different
        // operator action from one that was never written, so a different
        // outcome. None of them may carry their detail outward: the detail is
        // about ciphertext.
        _ => IntegrationError::CredentialUnusable,
    }
}

/// Build one Project's provider client from its Firm's stored credential.
#[async_trait]
pub trait IntegrationProviders: Send + Sync {
    async fn notion(
        &self,
        surreal: &SurrealDb,
        project_id: Uuid,
    ) -> Result<Arc<dyn cloud::NotionService>, IntegrationError>;

    async fn slack(
        &self,
        surreal: &SurrealDb,
        project_id: Uuid,
    ) -> Result<Arc<dyn cloud::SlackService>, IntegrationError>;
}

/// The default for a deployment with no runtime KMS key and no provider
/// account — every local checkout and every KIND cluster.
///
/// It refuses rather than falling back to an environment token. A fallback is
/// exactly the shape the Firm-secret boundary forbids: it would let a Project
/// reach a credential its Firm never wrote.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredIntegrations;

#[async_trait]
impl IntegrationProviders for UnconfiguredIntegrations {
    async fn notion(
        &self,
        _surreal: &SurrealDb,
        _project_id: Uuid,
    ) -> Result<Arc<dyn cloud::NotionService>, IntegrationError> {
        Err(IntegrationError::NotConfigured)
    }

    async fn slack(
        &self,
        _surreal: &SurrealDb,
        _project_id: Uuid,
    ) -> Result<Arc<dyn cloud::SlackService>, IntegrationError> {
        Err(IntegrationError::NotConfigured)
    }
}

/// The production resolver: one KMS handle and one parent Notion database,
/// with the per-Firm token read fresh for each call.
///
/// The credential is deliberately not cached. A revoked credential has to
/// stop working on the next call, and `firm_secrets::revoke` writes a row —
/// it does not call the provider — so a cache would keep a revoked token
/// live for as long as it survived.
pub struct FirmIntegrations {
    kms: Arc<dyn cloud::RuntimeKms>,
    notion_database_id: String,
}

impl std::fmt::Debug for FirmIntegrations {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FirmIntegrations")
            .field("notion_database_id", &self.notion_database_id)
            .finish_non_exhaustive()
    }
}

impl FirmIntegrations {
    #[must_use]
    pub fn new(kms: Arc<dyn cloud::RuntimeKms>, notion_database_id: impl Into<String>) -> Self {
        Self {
            kms,
            notion_database_id: notion_database_id.into(),
        }
    }

    async fn credential(
        &self,
        surreal: &SurrealDb,
        project_id: Uuid,
        provider: IntegrationProvider,
        kind: IntegrationSecretKind,
    ) -> Result<String, IntegrationError> {
        let credential = store::firm_secrets::resolve_project_credential(
            surreal,
            project_id,
            provider,
            kind,
            self.kms.as_ref(),
        )
        .await
        .map_err(secret_error)?;
        Ok(credential.expose_for_provider_call().to_string())
    }
}

#[async_trait]
impl IntegrationProviders for FirmIntegrations {
    async fn notion(
        &self,
        surreal: &SurrealDb,
        project_id: Uuid,
    ) -> Result<Arc<dyn cloud::NotionService>, IntegrationError> {
        let token = self
            .credential(
                surreal,
                project_id,
                IntegrationProvider::Notion,
                IntegrationSecretKind::NotionToken,
            )
            .await?;
        Ok(Arc::new(cloud::NotionClient::new(
            token,
            self.notion_database_id.clone(),
        )))
    }

    async fn slack(
        &self,
        surreal: &SurrealDb,
        project_id: Uuid,
    ) -> Result<Arc<dyn cloud::SlackService>, IntegrationError> {
        let token = self
            .credential(
                surreal,
                project_id,
                IntegrationProvider::Slack,
                IntegrationSecretKind::SlackBotToken,
            )
            .await?;
        Ok(Arc::new(cloud::SlackClient::new(token)))
    }
}

/// Select the resolver from the environment.
///
/// Both coordinates are required together. A runtime KMS key with no parent
/// Notion database would build a resolver whose Notion arm cannot create a
/// page, and a database id with no KMS key cannot decrypt a token, so a half
/// configuration is refused as unconfigured rather than half-working.
pub async fn from_env() -> Arc<dyn IntegrationProviders> {
    let Ok(config) = cloud::GoogleKmsConfig::from_env() else {
        tracing::info!("firm integrations: unconfigured (NAVIGATOR_RUNTIME_KMS_KEY unset)");
        return Arc::new(UnconfiguredIntegrations);
    };
    let Ok(database) = cloud::NotionDatabaseConfig::from_lookup(|key| std::env::var(key).ok())
    else {
        tracing::info!("firm integrations: unconfigured (NAVIGATOR_NOTION_DATABASE_ID unset)");
        return Arc::new(UnconfiguredIntegrations);
    };
    let Ok(tokens) = cloud::AdcTokenSource::new().await else {
        tracing::warn!(
            "firm integrations: unconfigured (a runtime KMS key is set but no \
             application-default credential resolved)"
        );
        return Arc::new(UnconfiguredIntegrations);
    };
    tracing::info!("firm integrations: FirmIntegrations (runtime KMS + Notion database)");
    Arc::new(FirmIntegrations::new(
        Arc::new(cloud::GoogleKms::new(config, Arc::new(tokens))),
        database.database_id,
    ))
}

/// In-process providers for dev and tests. One shared fake per provider, so a
/// test drives the door and then reads what the provider saw.
#[derive(Debug, Default, Clone)]
pub struct FakeIntegrations {
    pub notion: cloud::FakeNotion,
    pub slack: cloud::FakeSlack,
    notion_calls: Arc<AtomicUsize>,
    slack_calls: Arc<AtomicUsize>,
}

impl FakeIntegrations {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn notion_calls(&self) -> usize {
        self.notion_calls.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn slack_calls(&self) -> usize {
        self.slack_calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl IntegrationProviders for FakeIntegrations {
    async fn notion(
        &self,
        _surreal: &SurrealDb,
        _project_id: Uuid,
    ) -> Result<Arc<dyn cloud::NotionService>, IntegrationError> {
        self.notion_calls.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(self.notion.clone()))
    }

    async fn slack(
        &self,
        _surreal: &SurrealDb,
        _project_id: Uuid,
    ) -> Result<Arc<dyn cloud::SlackService>, IntegrationError> {
        self.slack_calls.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(self.slack.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{IntegrationError, IntegrationProviders, UnconfiguredIntegrations};
    use uuid::Uuid;

    /// A deployment with no runtime key refuses rather than reaching for a
    /// deployment-wide provider token. The refusal is the contract: a Project
    /// must never reach a credential its Firm did not write.
    #[tokio::test]
    async fn an_unconfigured_deployment_refuses_both_providers() {
        let surreal = store::test_support::mem_surreal().await;
        let providers = UnconfiguredIntegrations;
        assert!(matches!(
            providers.notion(&surreal, Uuid::now_v7()).await.err(),
            Some(IntegrationError::NotConfigured)
        ));
        assert!(matches!(
            providers.slack(&surreal, Uuid::now_v7()).await.err(),
            Some(IntegrationError::NotConfigured)
        ));
    }

    #[test]
    fn every_error_carries_a_slug_and_no_credential_detail() {
        for error in [
            IntegrationError::NotConfigured,
            IntegrationError::CredentialMissing,
            IntegrationError::NoFirm,
            IntegrationError::NoSuchProject,
            IntegrationError::CredentialUnusable,
            IntegrationError::Provider,
        ] {
            assert!(!error.slug().is_empty());
            let rendered = error.to_string();
            assert!(!rendered.contains("token"), "{rendered}");
            assert!(!rendered.contains("ciphertext"), "{rendered}");
        }
    }
}
