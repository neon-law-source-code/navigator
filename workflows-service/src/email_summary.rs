//! Durable execution for one archived support-email summary.
//!
//! The webhook submits only an opaque receipt ID and immutable run
//! configuration. Every archive read, provider request, and Slack delivery is
//! a named Restate step so replay uses the journal instead of repeating an
//! ordinary successful side effect.

use std::sync::Arc;
use std::time::Duration;

use cloud::{
    ClaudeVertexAdapter, GeminiVertexAdapter, MetadataTokenSource, StorageService, VertexError,
    VertexRequest,
};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use store::email_receipts::EmailReceipt;
use workflows::{
    build_prompt, deliver_summary, normalize_email, parse_summary, DeliveryResult,
    EmailSummaryRequest, EmailSummaryRunConfig, ProviderDelivery, SlackBot, SummaryDeliveryError,
    SummaryDeliveryMessage, SummaryProvider, DEFAULT_MAX_INPUT_CHARS, DEFAULT_MAX_OUTPUT_TOKENS,
};

const DEFAULT_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const PROVIDER_ATTEMPTS: usize = 3;
const SUMMARY_OUTPUT_MAX_CHARS: usize = 8_000;

/// The two provider transports share the worker's Workload Identity token
/// source but retain separate model/location choices in each request.
#[derive(Clone)]
pub struct SummaryProviders {
    pub gemini: GeminiVertexAdapter,
    pub claude: ClaudeVertexAdapter,
}

impl SummaryProviders {
    /// Build the production transports when a GCP project is configured. A
    /// missing project leaves the service registered but disabled, preserving
    /// the default-off worker boot contract until feature configuration lands.
    pub fn from_env() -> Result<Option<Self>, VertexError> {
        if std::env::var("NAVIGATOR_GCP_PROJECT_ID")
            .ok()
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Ok(None);
        }
        let metadata_url = std::env::var("GOOGLE_METADATA_URL")
            .unwrap_or_else(|_| DEFAULT_METADATA_TOKEN_URL.to_string());
        let token_source = Arc::new(MetadataTokenSource::new(metadata_url)?);
        Ok(Some(Self {
            gemini: GeminiVertexAdapter::new(token_source.clone())?,
            claude: ClaudeVertexAdapter::new(token_source)?,
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
        SummaryProvider::Claude => providers.claude.raw_predict(request).await,
    }
}

fn retryable(error: &VertexError) -> bool {
    matches!(
        error,
        VertexError::Retryable { .. } | VertexError::TransportRetryable
    )
}

async fn summarize_provider(
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
    if input.input_digest != receipt.raw_digest || config.input_digest != receipt.raw_digest {
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

        if request.gemini.input_digest != receipt.raw_digest
            || request.claude.input_digest != receipt.raw_digest
        {
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

        let claude = {
            let storage = Arc::clone(&self.storage);
            let providers = self.providers.clone();
            let receipt_for_provider = receipt.clone();
            let config = request.claude.clone();
            let project_id = request.project_id.clone();
            ctx.run(move || async move {
                Ok(Json(
                    summarize_provider(
                        storage,
                        providers,
                        SummaryProvider::Claude,
                        receipt_for_provider,
                        config,
                        project_id,
                    )
                    .await?,
                ))
            })
            .name("summarize-claude")
            .await?
            .into_inner()
        };

        let message = SummaryDeliveryMessage {
            receipt_id,
            gemini,
            claude,
        };
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
    use super::retryable;
    use cloud::VertexError;

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
}
