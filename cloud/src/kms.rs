//! The dedicated runtime KMS seam for Firm integration credentials.
//!
//! Deployment configuration encryption and Firm integration credentials are
//! separate trust boundaries. This module accepts only the explicitly
//! configured runtime key and binds every wrapped data key to the Firm and
//! typed provider context that requested it.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use aes_gcm::{
    aead::{consts::U12, Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

const NONCE_LEN: usize = 12;
const RUNTIME_KMS_KEY_ENV: &str = "NAVIGATOR_RUNTIME_KMS_KEY";
const KMS_ENDPOINT: &str = "https://cloudkms.googleapis.com/v1";

/// The authenticated-data context attached to one Firm secret version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmsContext {
    pub firm_id: Uuid,
    pub provider: String,
    pub kind: String,
}

impl KmsContext {
    #[must_use]
    pub fn new(firm_id: Uuid, provider: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            firm_id,
            provider: provider.into(),
            kind: kind.into(),
        }
    }

    #[must_use]
    pub fn aad(&self) -> String {
        format!(
            "navigator-firm-secret\n{}\n{}\n{}",
            self.firm_id, self.provider, self.kind
        )
    }
}

/// A KMS-wrapped data-encryption key and the KMS key version that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedDataKey {
    pub ciphertext: Vec<u8>,
    pub key_version: String,
}

/// Errors intentionally contain no request body, token, ciphertext, or
/// provider credential. They are safe to carry through an operator-facing
/// error boundary and to record as an outcome.
#[derive(Debug, Error)]
pub enum KmsError {
    #[error("runtime KMS is unavailable")]
    Unavailable,
    #[error("runtime KMS context mismatch")]
    ContextMismatch,
    #[error("runtime KMS configuration is missing")]
    MissingConfiguration,
    #[error("runtime KMS must use a dedicated key")]
    DeploymentKeyNotAllowed,
    #[error("runtime KMS request failed")]
    Transport,
    #[error("runtime KMS returned HTTP {0}")]
    Http(StatusCode),
    #[error("runtime KMS returned an invalid response")]
    InvalidResponse,
    #[error("envelope encryption failed")]
    Envelope,
}

/// The only KMS operations the Firm-secret store needs.
#[async_trait]
pub trait RuntimeKms: Send + Sync {
    async fn wrap_data_key(
        &self,
        data_key: &[u8],
        context: &KmsContext,
    ) -> Result<WrappedDataKey, KmsError>;

    async fn unwrap_data_key(
        &self,
        wrapped: &WrappedDataKey,
        context: &KmsContext,
    ) -> Result<Vec<u8>, KmsError>;
}

/// Encrypt a provider credential with the per-version data key. The key is
/// never serialized by this function; only the returned ciphertext is stored.
pub fn seal(plaintext: &[u8], data_key: &[u8], context: &KmsContext) -> Result<Vec<u8>, KmsError> {
    let cipher = Aes256Gcm::new_from_slice(data_key).map_err(|_| KmsError::Envelope)?;
    let nonce_bytes: [u8; NONCE_LEN] = rand::random();
    let nonce: Nonce<U12> = nonce_bytes.into();
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: context.aad().as_bytes(),
            },
        )
        .map_err(|_| KmsError::Envelope)?;
    let mut result = nonce_bytes.to_vec();
    result.extend(ciphertext);
    Ok(result)
}

/// Decrypt one credential after KMS has authenticated and unwrapped its data
/// key. The context is authenticated data, so a different Firm/provider/kind
/// cannot open the value.
pub fn open(ciphertext: &[u8], data_key: &[u8], context: &KmsContext) -> Result<Vec<u8>, KmsError> {
    let (nonce_bytes, body) = ciphertext
        .split_at_checked(NONCE_LEN)
        .ok_or(KmsError::Envelope)?;
    let nonce_bytes: [u8; NONCE_LEN] = nonce_bytes.try_into().map_err(|_| KmsError::Envelope)?;
    let cipher = Aes256Gcm::new_from_slice(data_key).map_err(|_| KmsError::Envelope)?;
    cipher
        .decrypt(
            &Nonce::<U12>::from(nonce_bytes),
            Payload {
                msg: body,
                aad: context.aad().as_bytes(),
            },
        )
        .map_err(|_| KmsError::ContextMismatch)
}

/// A deterministic in-process KMS fake. It exercises the same envelope,
/// context, key-version, and unavailable-service paths as the runtime client.
#[derive(Clone)]
pub struct FakeKms {
    master_key: [u8; 32],
    key_version: String,
    available: Arc<AtomicBool>,
}

impl std::fmt::Debug for FakeKms {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FakeKms")
            .field("key_version", &self.key_version)
            .finish_non_exhaustive()
    }
}

impl FakeKms {
    #[must_use]
    pub fn new(key_version: impl Into<String>) -> Self {
        let key_version = key_version.into();
        let digest = Sha256::digest(key_version.as_bytes());
        let mut master_key = [0_u8; 32];
        master_key.copy_from_slice(&digest);
        Self {
            master_key,
            key_version,
            available: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::Relaxed);
    }
}

#[async_trait]
impl RuntimeKms for FakeKms {
    async fn wrap_data_key(
        &self,
        data_key: &[u8],
        context: &KmsContext,
    ) -> Result<WrappedDataKey, KmsError> {
        if !self.available.load(Ordering::Relaxed) {
            return Err(KmsError::Unavailable);
        }
        Ok(WrappedDataKey {
            ciphertext: seal(data_key, &self.master_key, context)?,
            key_version: self.key_version.clone(),
        })
    }

    async fn unwrap_data_key(
        &self,
        wrapped: &WrappedDataKey,
        context: &KmsContext,
    ) -> Result<Vec<u8>, KmsError> {
        if !self.available.load(Ordering::Relaxed) {
            return Err(KmsError::Unavailable);
        }
        if wrapped.key_version != self.key_version {
            return Err(KmsError::ContextMismatch);
        }
        open(&wrapped.ciphertext, &self.master_key, context)
    }
}

/// Runtime KMS configuration. It is intentionally explicit: there is no
/// default key and the deployment-config key is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleKmsConfig {
    pub key_name: String,
}

impl GoogleKmsConfig {
    pub fn from_env() -> Result<Self, KmsError> {
        let key_name = std::env::var(RUNTIME_KMS_KEY_ENV)
            .map_err(|_| KmsError::MissingConfiguration)?
            .trim()
            .to_string();
        if key_name.is_empty() {
            return Err(KmsError::MissingConfiguration);
        }
        if key_name.ends_with("/cryptoKeys/deployment-config") {
            return Err(KmsError::DeploymentKeyNotAllowed);
        }
        Ok(Self { key_name })
    }
}

/// Workload-identity token source injected by the process bootstrap. KMS does
/// not discover ambient credentials or fall back to `gcloud`.
#[async_trait]
pub trait KmsTokenSource: Send + Sync {
    async fn token(&self) -> Result<String, KmsError>;
}

/// Google Cloud KMS REST adapter. Its token source is injected so tests never
/// need provider credentials and production can supply workload identity.
pub struct GoogleKms {
    config: GoogleKmsConfig,
    http: reqwest::Client,
    token_source: Arc<dyn KmsTokenSource>,
    endpoint: String,
}

impl std::fmt::Debug for GoogleKms {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GoogleKms")
            .field("config", &self.config)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl GoogleKms {
    #[must_use]
    pub fn new(config: GoogleKmsConfig, token_source: Arc<dyn KmsTokenSource>) -> Self {
        Self::with_endpoint(config, token_source, KMS_ENDPOINT)
    }

    /// Same client against an explicit endpoint, so the request shape this
    /// adapter sends can be asserted without a cloud account.
    #[must_use]
    pub fn with_endpoint(
        config: GoogleKmsConfig,
        token_source: Arc<dyn KmsTokenSource>,
        endpoint: impl Into<String>,
    ) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            token_source,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
        }
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        key_name: &str,
        operation: &str,
        body: serde_json::Value,
    ) -> Result<T, KmsError> {
        let token = self.token_source.token().await?;
        let response = self
            .http
            .post(format!("{}/{key_name}:{operation}", self.endpoint))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| KmsError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(KmsError::Http(status));
        }
        response.json().await.map_err(|_| KmsError::InvalidResponse)
    }
}

#[derive(Debug, Deserialize)]
struct EncryptResponse {
    ciphertext: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DecryptResponse {
    plaintext: String,
}

#[async_trait]
impl RuntimeKms for GoogleKms {
    async fn wrap_data_key(
        &self,
        data_key: &[u8],
        context: &KmsContext,
    ) -> Result<WrappedDataKey, KmsError> {
        let response: EncryptResponse = self
            .post(
                &self.config.key_name,
                "encrypt",
                serde_json::json!({
                    "plaintext": BASE64.encode(data_key),
                    "additionalAuthenticatedData": BASE64.encode(context.aad()),
                }),
            )
            .await?;
        Ok(WrappedDataKey {
            ciphertext: BASE64
                .decode(response.ciphertext)
                .map_err(|_| KmsError::InvalidResponse)?,
            key_version: response
                .name
                .unwrap_or_else(|| self.config.key_name.clone()),
        })
    }

    /// Decrypt against the configured `cryptoKeys/...` resource, never the
    /// `kms_key_version` recorded beside the row.
    ///
    /// Two reasons, and either one is enough. Google KMS symmetric `decrypt`
    /// is defined on the key, not on a key version — a `cryptoKeyVersions/N`
    /// path is only valid for the asymmetric and raw operations — so posting
    /// the recorded version resource never round-trips a wrapped data key.
    /// And the recorded value arrives from a database row, so honouring it as
    /// the request target would let stored data choose the key this process
    /// asks, which is exactly the choice [`GoogleKmsConfig::from_env`] refuses
    /// to leave open. The version stays as provenance for which key version
    /// wrapped the key; KMS selects it from the ciphertext.
    async fn unwrap_data_key(
        &self,
        wrapped: &WrappedDataKey,
        context: &KmsContext,
    ) -> Result<Vec<u8>, KmsError> {
        let response: DecryptResponse = self
            .post(
                &self.config.key_name,
                "decrypt",
                serde_json::json!({
                    "ciphertext": BASE64.encode(&wrapped.ciphertext),
                    "additionalAuthenticatedData": BASE64.encode(context.aad()),
                }),
            )
            .await?;
        BASE64
            .decode(response.plaintext)
            .map_err(|_| KmsError::InvalidResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/navigator-runtime";

    #[derive(Debug)]
    struct FakeToken;

    #[async_trait]
    impl KmsTokenSource for FakeToken {
        async fn token(&self) -> Result<String, KmsError> {
            Ok("test-token".to_string())
        }
    }

    /// Symmetric `decrypt` is defined on the key, not on a key version, and
    /// the recorded version arrives from a database row. Both say the request
    /// target is the configured key: this asserts the path the adapter posts,
    /// with a `kms_key_version` that names a version resource of a different
    /// key entirely.
    #[tokio::test]
    async fn decrypt_targets_the_configured_key_not_the_recorded_version() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/{KEY}:decrypt")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "plaintext": BASE64.encode(b"data-key") })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let kms = GoogleKms::with_endpoint(
            GoogleKmsConfig {
                key_name: KEY.to_string(),
            },
            Arc::new(FakeToken),
            server.uri(),
        );
        let wrapped = WrappedDataKey {
            ciphertext: b"wrapped".to_vec(),
            key_version: "projects/p/locations/l/keyRings/r/cryptoKeys/other/cryptoKeyVersions/9"
                .to_string(),
        };
        let context = KmsContext::new(Uuid::now_v7(), "notion", "notion_token");
        assert_eq!(
            kms.unwrap_data_key(&wrapped, &context).await.unwrap(),
            b"data-key"
        );
    }

    #[tokio::test]
    async fn fake_kms_binds_ciphertext_to_context_and_key_version() {
        let kms = FakeKms::new("runtime-v3");
        let context = KmsContext::new(Uuid::now_v7(), "notion", "notion_token");
        let wrapped = kms.wrap_data_key(b"data-key", &context).await.unwrap();
        assert_eq!(wrapped.key_version, "runtime-v3");
        assert_eq!(
            kms.unwrap_data_key(&wrapped, &context).await.unwrap(),
            b"data-key"
        );
        assert!(kms
            .unwrap_data_key(
                &wrapped,
                &KmsContext::new(context.firm_id, "slack", "slack_bot_token")
            )
            .await
            .is_err());
    }

    #[test]
    fn runtime_configuration_requires_a_dedicated_key() {
        let old = std::env::var(RUNTIME_KMS_KEY_ENV).ok();
        std::env::set_var(
            RUNTIME_KMS_KEY_ENV,
            "projects/p/locations/l/keyRings/r/cryptoKeys/deployment-config",
        );
        assert!(matches!(
            GoogleKmsConfig::from_env(),
            Err(KmsError::DeploymentKeyNotAllowed)
        ));
        match old {
            Some(value) => std::env::set_var(RUNTIME_KMS_KEY_ENV, value),
            None => std::env::remove_var(RUNTIME_KMS_KEY_ENV),
        }
    }
}
