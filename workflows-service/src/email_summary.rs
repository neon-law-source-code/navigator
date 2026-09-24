//! Durable execution for one archived support-email summary.
//!
//! The webhook submits only an opaque receipt ID and immutable run
//! configuration. Every archive read, provider request, and Slack delivery is
//! a named Restate step so replay uses the journal instead of repeating an
//! ordinary successful side effect.

use std::sync::Arc;
use std::time::Duration;

use cloud::{GeminiVertexAdapter, MetadataTokenSource, StorageService, VertexError, VertexRequest};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use store::email_receipts::EmailReceipt;
use workflows::{
    build_prompt, deliver_summary, normalize_email, parse_summary, scoped_email_digest,
    DeliveryResult, EmailSummaryRequest, EmailSummaryRunConfig, ProviderDelivery, SlackBot,
    SummaryDeliveryError, SummaryDeliveryMessage, SummaryProvider, DEFAULT_MAX_INPUT_CHARS,
    DEFAULT_MAX_OUTPUT_TOKENS,
};

const DEFAULT_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const PROVIDER_ATTEMPTS: usize = 3;
const SUMMARY_OUTPUT_MAX_CHARS: usize = 8_000;

/// The provider transport, authenticated with the worker's Workload Identity
/// token source. Gemini is the only summary provider; see `cloud::vertex`.
#[derive(Clone)]
pub struct SummaryProviders {
    pub gemini: GeminiVertexAdapter,
}

impl SummaryProviders {
    /// Build the production transports only when the shared summary feature
    /// configuration is enabled and complete. The default-off row leaves the
    /// service registered but provider-disabled.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if workflows::EmailSummaryConfig::from_env()?.is_none() {
            return Ok(None);
        }
        let metadata_url = std::env::var("GOOGLE_METADATA_URL")
            .unwrap_or_else(|_| DEFAULT_METADATA_TOKEN_URL.to_string());
        let token_source = Arc::new(MetadataTokenSource::new(metadata_url)?);
        Ok(Some(Self {
            gemini: GeminiVertexAdapter::new(token_source)?,
        }))
    }
}

/// Invocation result contains identifiers and status only. Provider content
/// is retained in the journaled result and the bounded private Slack message,
/// never in worker logs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct EmailSummaryReport {
    pub receipt_id: uuid::Uuid,
    pub delivery_state: String,
}

#[derive(Clone)]
pub struct EmailSummaryService {
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn StorageService>,
    slack: Arc<dyn SlackBot>,
    providers: Option<SummaryProviders>,
}

impl EmailSummaryService {
    #[must_use]
    pub fn new(
        surreal: store::surreal::SurrealDb,
        storage: Arc<dyn StorageService>,
        slack: Arc<dyn SlackBot>,
        providers: Option<SummaryProviders>,
    ) -> Self {
        Self {
            surreal,
            storage,
            slack,
            providers,
        }
    }
}

fn request_for(
    receipt: &EmailReceipt,
    config: &EmailSummaryRunConfig,
    project_id: &str,
    prompt: String,
) -> VertexRequest {
    VertexRequest {
        project_id: project_id.to_string(),
        model: config.model.clone(),
        location: config.location.clone(),
        prompt_version: config.prompt_version.clone(),
        input_digest: receipt.raw_digest.clone(),
        prompt,
        max_output_tokens: config.max_output_tokens.min(DEFAULT_MAX_OUTPUT_TOKENS),
    }
}

async fn call_provider(
    providers: &SummaryProviders,
    provider: SummaryProvider,
    request: &VertexRequest,
) -> Result<cloud::VertexResponse, VertexError> {
    match provider {
        SummaryProvider::Gemini => providers.gemini.generate_content(request).await,
    }
}

fn retryable(error: &VertexError) -> bool {
    matches!(
        error,
        VertexError::Retryable { .. } | VertexError::TransportRetryable
    )
}

/// The `run` handler's per-provider step, outside `ctx.run`'s journaling —
/// pub so a full-pipeline test (`server`'s intake -> archive -> handler
/// coverage, ENG-889) can call the same digest-checking, provider-calling
/// logic the real workflow runs, without needing a live Restate broker.
pub async fn summarize_provider(
    storage: Arc<dyn StorageService>,
    providers: Option<SummaryProviders>,
    provider: SummaryProvider,
    receipt: EmailReceipt,
    config: EmailSummaryRunConfig,
    project_id: String,
) -> Result<ProviderDelivery, HandlerError> {
    let model = config.model.clone();
    let Some(providers) = providers else {
        return Ok(ProviderDelivery::Failed {
            model,
            status: "provider_not_configured".to_string(),
        });
    };

    let object = storage
        .get(&receipt.archive_key)
        .await
        .map_err(HandlerError::from)?;
    let input = match normalize_email(
        &object.bytes,
        config.max_input_chars.min(DEFAULT_MAX_INPUT_CHARS),
    ) {
        Ok(input) => input,
        Err(error) => {
            return Ok(ProviderDelivery::Failed {
                model,
                status: format!("invalid_input:{error}"),
            });
        }
    };
    // Prove the archived bytes are the ones this receipt admitted, under the
    // receipt's own deployment and mailbox scope, and that the immutable run
    // configuration was captured for the same receipt.
    let archived_digest = scoped_email_digest(
        &receipt.deployment,
        &receipt.receiving_mailbox,
        &object.bytes,
    );
    if archived_digest != receipt.raw_digest || config.input_digest != receipt.raw_digest {
        return Ok(ProviderDelivery::Failed {
            model,
            status: "input_digest_mismatch".to_string(),
        });
    }
    let prompt = build_prompt(&input, &config);
    let request = request_for(&receipt, &config, &project_id, prompt);
    let mut last_error = None;
    for attempt in 0..PROVIDER_ATTEMPTS {
        match call_provider(&providers, provider, &request).await {
            Ok(response) => {
                return Ok(
                    match parse_summary(&response.text, SUMMARY_OUTPUT_MAX_CHARS) {
                        Ok(result) => ProviderDelivery::Succeeded {
                            model: response.model,
                            result,
                        },
                        Err(error) => ProviderDelivery::Failed {
                            model,
                            status: format!("invalid_output:{error}"),
                        },
                    },
                );
            }
            Err(error) if retryable(&error) && attempt + 1 < PROVIDER_ATTEMPTS => {
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(100 * (attempt as u64 + 1))).await;
            }
            Err(error) => {
                last_error = Some(error);
                break;
            }
        }
    }
    Ok(ProviderDelivery::Failed {
        model,
        status: format!(
            "provider_error:{}",
            last_error.map_or_else(|| "unknown".into(), |e| e.to_string())
        ),
    })
}

async fn delivery_error(
    error: SummaryDeliveryError,
    db: &store::surreal::SurrealDb,
    receipt_id: uuid::Uuid,
) -> Result<Json<String>, HandlerError> {
    if matches!(error, SummaryDeliveryError::Slack(_)) {
        if let Some(delivery) = store::email_deliveries::find(db, receipt_id)
            .await
            .map_err(HandlerError::from)?
        {
            if delivery.state == store::email_deliveries::UNKNOWN {
                return Ok(Json("unknown".to_string()));
            }
        }
        return Err(TerminalError::new(error.to_string()).into());
    }
    Err(HandlerError::from(error))
}

#[restate_sdk::workflow(name = "EmailSummary")]
impl EmailSummaryService {
    #[restate_sdk::handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        Json(request): Json<EmailSummaryRequest>,
    ) -> Result<Json<EmailSummaryReport>, HandlerError> {
        let receipt_id = request.receipt_id;
        let surreal = self.surreal.clone();
        let receipt: EmailReceipt = ctx
            .run(move || async move {
                let receipt = store::email_receipts::find_by_id(&surreal, receipt_id)
                    .await
                    .map_err(HandlerError::from)?
                    .ok_or_else(|| TerminalError::new("email receipt not found"))?;
                Ok(Json(receipt))
            })
            .name("load-receipt")
            .await?
            .into_inner();

        if request.gemini.input_digest != receipt.raw_digest {
            return Err(
                TerminalError::new("summary run configuration does not match receipt").into(),
            );
        }

        let gemini = {
            let storage = Arc::clone(&self.storage);
            let providers = self.providers.clone();
            let receipt = receipt.clone();
            let config = request.gemini.clone();
            let project_id = request.project_id.clone();
            ctx.run(move || async move {
                Ok(Json(
                    summarize_provider(
                        storage,
                        providers,
                        SummaryProvider::Gemini,
                        receipt,
                        config,
                        project_id,
                    )
                    .await?,
                ))
            })
            .name("summarize-gemini")
            .await?
            .into_inner()
        };

        let message = SummaryDeliveryMessage { receipt_id, gemini };
        let surreal = self.surreal.clone();
        let slack = Arc::clone(&self.slack);
        let channel_id = request.channel_id.clone();
        let delivery = ctx
            .run(move || async move {
                match deliver_summary(&surreal, slack, receipt_id, &channel_id, &message).await {
                    Ok(DeliveryResult::Sent(_) | DeliveryResult::AlreadyConfirmed) => {
                        Ok(Json("confirmed".to_string()))
                    }
                    Ok(DeliveryResult::Unknown) => Ok(Json("unknown".to_string())),
                    Err(error) => delivery_error(error, &surreal, receipt_id).await,
                }
            })
            .name("deliver-slack")
            .await?
            .into_inner();

        Ok(Json(EmailSummaryReport {
            receipt_id,
            delivery_state: delivery,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{request_for, retryable, summarize_provider, SummaryProviders};
    use async_trait::async_trait;
    use chrono::Utc;
    use cloud::{StaticTokenSource, StorageError, StoredObject, VertexError};
    use std::sync::Arc;
    use store::email_receipts::EmailReceipt;
    use uuid::Uuid;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use workflows::{EmailSummaryRunConfig, ProviderDelivery, SummaryProvider};

    #[derive(Clone)]
    struct MemoryStorage {
        bytes: Option<Vec<u8>>,
    }

    #[async_trait]
    impl cloud::StorageService for MemoryStorage {
        async fn put(
            &self,
            _key: &str,
            _bytes: &[u8],
            _content_type: &str,
        ) -> Result<(), StorageError> {
            Err(StorageError::Unsupported("test"))
        }

        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            self.bytes
                .clone()
                .map(|bytes| StoredObject {
                    key: key.to_string(),
                    bytes,
                    content_type: "message/rfc822".to_string(),
                })
                .ok_or_else(|| StorageError::NotFound(key.to_string()))
        }

        async fn delete(&self, _key: &str) -> Result<(), StorageError> {
            Err(StorageError::Unsupported("test"))
        }

        async fn signed_url(
            &self,
            _key: &str,
            _expires_in: std::time::Duration,
        ) -> Result<String, StorageError> {
            Err(StorageError::Unsupported("test"))
        }
    }

    fn receipt(raw_digest: &str) -> EmailReceipt {
        let now = Utc::now();
        EmailReceipt {
            id: Uuid::now_v7(),
            receiving_mailbox: "support@example.com".to_string(),
            deployment: "staging".to_string(),
            raw_digest: raw_digest.to_string(),
            source_message_id: None,
            archive_key: "inbound/example.eml".to_string(),
            letter_id: Uuid::now_v7(),
            processing_state: "pending".to_string(),
            delivery_state: "not_attempted".to_string(),
            inserted_at: now,
            updated_at: now,
        }
    }

    /// The digest intake stores for `archive` under [`receipt`]'s scope.
    fn intake_digest(archive: &[u8]) -> String {
        workflows::scoped_email_digest("staging", "support@example.com", archive)
    }

    const ARCHIVE: &[u8] = b"From: sender@example.com\r\n\
        To: intake@example.com\r\n\
        Content-Type: text/plain\r\n\r\n\
        A bounded summary body\r\n";

    const GEMINI_PATH: &str = "/v1/projects/synthetic-project/locations/global/publishers/google/models/summary-model:generateContent";

    fn config(provider: SummaryProvider, digest: &str) -> EmailSummaryRunConfig {
        EmailSummaryRunConfig::new(provider, "summary-model", "global", "summary-v1", digest)
            .expect("valid test config")
    }

    fn providers(base_url: &str) -> SummaryProviders {
        let token = Arc::new(StaticTokenSource::new("test-token"));
        SummaryProviders {
            gemini: cloud::GeminiVertexAdapter::new(token)
                .expect("gemini adapter")
                .with_base_url(base_url),
        }
    }

    #[test]
    fn only_transient_vertex_failures_retry() {
        assert!(retryable(&VertexError::TransportRetryable));
        assert!(retryable(&VertexError::Retryable {
            status: None,
            retry_after: None,
        }));
        assert!(!retryable(&VertexError::Configuration));
        assert!(!retryable(&VertexError::InvalidModelOutput));
    }

    #[test]
    fn request_carries_the_receipt_digest_and_bounded_output_limit() {
        let digest = "a".repeat(64);
        let receipt = receipt(&digest);
        let config = config(SummaryProvider::Gemini, &digest).with_limits(4_000, 99_999);
        let request = request_for(&receipt, &config, "synthetic-project", "prompt".to_string());
        assert_eq!(request.project_id, "synthetic-project");
        assert_eq!(request.input_digest, digest);
        assert_eq!(
            request.max_output_tokens,
            workflows::DEFAULT_MAX_OUTPUT_TOKENS
        );
        assert_eq!(request.prompt, "prompt");
    }

    #[tokio::test]
    async fn missing_provider_configuration_is_a_bounded_failure() {
        let digest = "a".repeat(64);
        let result = summarize_provider(
            Arc::new(MemoryStorage { bytes: None }),
            None,
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("missing provider is represented in the summary");
        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status == "provider_not_configured"
        ));
    }

    #[tokio::test]
    async fn malformed_archive_is_not_sent_to_a_provider() {
        let digest = "a".repeat(64);
        let server = MockServer::start().await;
        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(b"not a MIME message".to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("malformed archive is represented in the summary");
        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status.starts_with("invalid_input:")
        ));
    }

    #[tokio::test]
    async fn digest_mismatch_is_rejected_before_provider_call() {
        let archive = b"From: sender@example.com\r\n\
            To: intake@example.com\r\n\
            Content-Type: text/plain\r\n\r\n\
            A bounded summary body\r\n";
        let digest = "a".repeat(64);
        let different = "b".repeat(64);
        let server = MockServer::start().await;
        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(archive.to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&different),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("digest mismatch is represented in the summary");
        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status == "input_digest_mismatch"
        ));
    }

    #[tokio::test]
    async fn valid_gemini_output_becomes_a_succeeded_delivery() {
        let archive = b"From: sender@example.com\r\n\
            To: intake@example.com\r\n\
            Content-Type: text/plain\r\n\r\n\
            A bounded summary body\r\n";
        let digest = intake_digest(archive);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/synthetic-project/locations/global/publishers/google/models/summary-model:generateContent"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"candidates":[{"content":{"parts":[{"text":"{\"summary\":\"Useful\",\"requested_actions\":[],\"sender_stated_dates\":[],\"missing_information\":[]}"}]}}]}"#,
            ))
            .mount(&server)
            .await;
        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(archive.to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("provider response is represented in the summary");
        assert!(matches!(
            result,
            ProviderDelivery::Succeeded { model, result }
                if model == "summary-model" && result.summary == "Useful"
        ));
    }

    #[tokio::test]
    async fn archive_storage_errors_are_returned_without_provider_access() {
        let digest = "a".repeat(64);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let result = summarize_provider(
            Arc::new(MemoryStorage { bytes: None }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_provider_json_becomes_a_bounded_failure() {
        let archive = b"From: sender@example.com\r\n\
            To: intake@example.com\r\n\
            Content-Type: text/plain\r\n\r\n\
            A bounded summary body\r\n";
        let digest = intake_digest(archive);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"candidates":[{"content":{"parts":[{"text":"{}"}]}}]}"#),
            )
            .mount(&server)
            .await;

        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(archive.to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("invalid output is represented in the summary");

        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status.starts_with("invalid_output:")
        ));
    }

    #[tokio::test]
    async fn retryable_provider_failures_are_retried_then_quarantined() {
        let archive = b"From: sender@example.com\r\n\
            To: intake@example.com\r\n\
            Content-Type: text/plain\r\n\r\n\
            A bounded summary body\r\n";
        let digest = intake_digest(archive);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(archive.to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("provider failure is represented in the summary");

        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status.starts_with("provider_error:")
        ));
    }

    #[tokio::test]
    async fn permanent_provider_failures_are_not_retried() {
        let archive = b"From: sender@example.com\r\n\
            To: intake@example.com\r\n\
            Content-Type: text/plain\r\n\r\n\
            A bounded summary body\r\n";
        let digest = intake_digest(archive);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        let result = summarize_provider(
            Arc::new(MemoryStorage {
                bytes: Some(archive.to_vec()),
            }),
            Some(providers(&server.uri())),
            SummaryProvider::Gemini,
            receipt(&digest),
            config(SummaryProvider::Gemini, &digest),
            "synthetic-project".to_string(),
        )
        .await
        .expect("provider failure is represented in the summary");

        assert!(matches!(
            result,
            ProviderDelivery::Failed { status, .. } if status.starts_with("provider_error:")
        ));
    }

    #[tokio::test]
    async fn a_valid_archive_reaches_the_gemini_adapter() {
        let digest = intake_digest(ARCHIVE);
        let server = MockServer::start().await;
        let json = r#"{\"summary\":\"Useful\",\"requested_actions\":[],\"sender_stated_dates\":[],\"missing_information\":[]}"#;
        Mock::given(method("POST"))
            .and(path(GEMINI_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                r#"{{"candidates":[{{"content":{{"parts":[{{"text":"{json}"}}]}}}}]}}"#
            )))
            .expect(1)
            .mount(&server)
            .await;
        {
            let provider = SummaryProvider::Gemini;
            let result = summarize_provider(
                Arc::new(MemoryStorage {
                    bytes: Some(ARCHIVE.to_vec()),
                }),
                Some(providers(&server.uri())),
                provider,
                receipt(&digest),
                config(provider, &digest),
                "synthetic-project".to_string(),
            )
            .await
            .expect("provider response is represented in the summary");
            assert!(
                matches!(&result, ProviderDelivery::Succeeded { result, .. } if result.summary == "Useful"),
                "{provider:?} did not succeed"
            );
        }
    }

    #[tokio::test]
    async fn tampered_archive_bytes_are_rejected_before_provider_call() {
        let digest = intake_digest(ARCHIVE);
        let mut tampered = ARCHIVE.to_vec();
        tampered.extend_from_slice(b"appended\r\n");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        {
            let provider = SummaryProvider::Gemini;
            let result = summarize_provider(
                Arc::new(MemoryStorage {
                    bytes: Some(tampered.clone()),
                }),
                Some(providers(&server.uri())),
                provider,
                receipt(&digest),
                config(provider, &digest),
                "synthetic-project".to_string(),
            )
            .await
            .expect("tampering is represented in the summary");
            assert!(matches!(
                result,
                ProviderDelivery::Failed { status, .. } if status == "input_digest_mismatch"
            ));
        }
    }

    #[tokio::test]
    async fn archive_outside_the_receipt_scope_is_rejected() {
        // Same bytes, but the receipt claims another deployment or mailbox:
        // the scoped digest no longer matches, so nothing reaches Vertex.
        let digest = intake_digest(ARCHIVE);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let mut other_deployment = receipt(&digest);
        other_deployment.deployment = "production".to_string();
        let mut other_mailbox = receipt(&digest);
        other_mailbox.receiving_mailbox = "other@example.com".to_string();
        let mut raw_only = receipt(&digest);
        raw_only.raw_digest = workflows::normalize_email(ARCHIVE, 1_000)
            .expect("valid MIME")
            .content_digest;
        for receipt in [other_deployment, other_mailbox, raw_only] {
            let result = summarize_provider(
                Arc::new(MemoryStorage {
                    bytes: Some(ARCHIVE.to_vec()),
                }),
                Some(providers(&server.uri())),
                SummaryProvider::Gemini,
                receipt.clone(),
                config(SummaryProvider::Gemini, &receipt.raw_digest),
                "synthetic-project".to_string(),
            )
            .await
            .expect("scope mismatch is represented in the summary");
            assert!(matches!(
                result,
                ProviderDelivery::Failed { status, .. } if status == "input_digest_mismatch"
            ));
        }
    }
}
