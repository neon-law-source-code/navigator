//! Bounded Slack rendering and delivery orchestration for email summaries.

use std::sync::Arc;

use thiserror::Error;
use uuid::Uuid;

use crate::email_summary::EmailSummary;
use crate::notify::{SlackBot, SlackBotError, SlackMessageReceipt};

const MAX_SLACK_CHARS: usize = 3_500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderDelivery {
    Succeeded { model: String, result: EmailSummary },
    Failed { model: String, status: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryDeliveryMessage {
    pub receipt_id: Uuid,
    pub gemini: ProviderDelivery,
    pub claude: ProviderDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    Sent(SlackMessageReceipt),
    AlreadyConfirmed,
    Unknown,
}

#[derive(Debug, Error)]
pub enum SummaryDeliveryError {
    #[error("delivery store: {0}")]
    Store(#[from] store::email_deliveries::EmailDeliveryError),
    #[error("Slack delivery: {0}")]
    Slack(#[from] SlackBotError),
}

#[must_use]
pub fn render_summary_message(message: &SummaryDeliveryMessage) -> String {
    let mut output = format!("Support summary receipt: {}\n", message.receipt_id);
    output.push_str(&render_provider("Gemini", &message.gemini));
    output.push('\n');
    output.push_str(&render_provider("Claude", &message.claude));
    truncate(&output, MAX_SLACK_CHARS)
}

fn render_provider(label: &str, provider: &ProviderDelivery) -> String {
    match provider {
        ProviderDelivery::Succeeded { model, result } => format!(
            "*{label}* ({})\nSummary: {}\nRequested actions: {}\nSender-stated dates: {}\nMissing information: {}",
            escape(model),
            escape(&result.summary),
            render_list(&result.requested_actions),
            render_list(&result.sender_stated_dates),
            render_list(&result.missing_information),
        ),
        ProviderDelivery::Failed { model, status } => format!(
            "*{label}* ({})\nFailure: {}",
            escape(model),
            escape(status),
        ),
    }
}

fn render_list(values: &[String]) -> String {
    if values.is_empty() {
        return "(none)".to_string();
    }
    values
        .iter()
        .map(|value| format!("• {}", escape(value)))
        .collect::<Vec<_>>()
        .join("; ")
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars.saturating_sub(15)).collect();
    truncated.push_str("\n[truncated]");
    truncated
}

/// Send one rendered summary through the configured private channel. The
/// store admission is the idempotency boundary; unknown external outcomes
/// stop replay until an operator reconciles them.
pub async fn deliver_summary(
    db: &store::surreal::SurrealDb,
    slack: Arc<dyn SlackBot>,
    receipt_id: Uuid,
    channel_id: &str,
    message: &SummaryDeliveryMessage,
) -> Result<DeliveryResult, SummaryDeliveryError> {
    store::email_deliveries::ensure(db, receipt_id, channel_id).await?;
    let correlation_id = format!("summary-{receipt_id}");
    let admitted = store::email_deliveries::admit_attempt(db, receipt_id, &correlation_id).await?;
    if admitted.delivery.state == store::email_deliveries::CONFIRMED {
        return Ok(DeliveryResult::AlreadyConfirmed);
    }
    if admitted.attempt_id.is_none() {
        return Ok(DeliveryResult::Unknown);
    }

    let rendered = render_summary_message(message);
    match slack
        .post_message_with_receipt(channel_id, &rendered, Some(&correlation_id))
        .await
    {
        Ok(receipt) => {
            store::email_deliveries::mark_confirmed(
                db,
                receipt_id,
                &receipt.channel_id,
                &receipt.timestamp,
            )
            .await?;
            Ok(DeliveryResult::Sent(receipt))
        }
        Err(error @ (SlackBotError::Transport(_) | SlackBotError::RetryableHttpStatus(_))) => {
            store::email_deliveries::mark_unknown(db, receipt_id, &error.to_string()).await?;
            Err(error.into())
        }
        Err(
            error @ (SlackBotError::RateLimited { .. }
            | SlackBotError::HttpStatus(_)
            | SlackBotError::Api(_)
            | SlackBotError::IncompleteResponse),
        ) => {
            store::email_deliveries::mark_failed(db, receipt_id, &error.to_string()).await?;
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> SummaryDeliveryMessage {
        SummaryDeliveryMessage {
            receipt_id: Uuid::nil(),
            gemini: ProviderDelivery::Succeeded {
                model: "gemini-test".to_string(),
                result: EmailSummary {
                    summary: "The sender asks for an update <@U123>.".to_string(),
                    requested_actions: vec!["Review & reply".to_string()],
                    sender_stated_dates: vec!["2026-09-21".to_string()],
                    missing_information: vec![],
                },
            },
            claude: ProviderDelivery::Failed {
                model: "claude-test".to_string(),
                status: "provider timeout".to_string(),
            },
        }
    }

    #[test]
    fn renders_partial_failure_and_escapes_slack_markup() {
        let rendered = render_summary_message(&message());
        assert!(rendered.contains("*Gemini* (gemini-test)"));
        assert!(rendered.contains("*Claude* (claude-test)\nFailure: provider timeout"));
        assert!(rendered.contains("&lt;@U123&gt;"));
        assert!(rendered.contains("Review &amp; reply"));
        assert!(!rendered.contains("<@U123>"));
    }

    #[test]
    fn renderer_bounds_large_provider_text() {
        let mut large = message();
        if let ProviderDelivery::Succeeded { result, .. } = &mut large.gemini {
            result.summary = "x".repeat(10_000);
        }
        let rendered = render_summary_message(&large);
        assert!(rendered.chars().count() <= MAX_SLACK_CHARS);
        assert!(rendered.ends_with("[truncated]"));
    }
}
