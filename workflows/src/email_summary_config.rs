//! Typed, opt-in configuration for the inbound email-summary lane.
//!
//! The web process and the Restate worker read the same shape. Keeping the
//! gate here prevents one process from believing summaries are enabled while
//! the other silently falls back to a stub or an unconfigured provider.

use std::collections::BTreeSet;

use thiserror::Error;

/// Environment keys owned by the summary lane.
pub const ENABLED_ENV: &str = "NAVIGATOR_SUMMARY_ENABLED";
pub const RECIPIENTS_ENV: &str = "NAVIGATOR_SUMMARY_ENVELOPE_RECIPIENTS";
pub const DEPLOYMENT_ENV: &str = "NAVIGATOR_DEPLOYMENT_ID";
pub const WORKFLOW_INGRESS_ENV: &str = "RESTATE_BROKER_URL";
pub const CHANNEL_ENV: &str = "NAVIGATOR_SUMMARY_CHANNEL_ID";
pub const INBOUND_PUBLIC_KEY_ENV: &str = "SENDGRID_INBOUND_PUBLIC_KEY";
pub const GCP_PROJECT_ENV: &str = "NAVIGATOR_GCP_PROJECT_ID";
pub const GEMINI_MODEL_ENV: &str = "NAVIGATOR_SUMMARY_GEMINI_MODEL";
pub const GEMINI_LOCATION_ENV: &str = "NAVIGATOR_SUMMARY_GEMINI_LOCATION";
pub const CLAUDE_MODEL_ENV: &str = "NAVIGATOR_SUMMARY_CLAUDE_MODEL";
pub const CLAUDE_LOCATION_ENV: &str = "NAVIGATOR_SUMMARY_CLAUDE_LOCATION";
pub const MAX_INPUT_CHARS_ENV: &str = "NAVIGATOR_SUMMARY_MAX_INPUT_CHARS";
pub const MAX_OUTPUT_TOKENS_ENV: &str = "NAVIGATOR_SUMMARY_MAX_OUTPUT_TOKENS";
pub const CI_HARNESS_ENV: &str = "NAVIGATOR_CI_HARNESS";

const DEFAULT_MAX_INPUT_CHARS: usize = 32_000;
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 1_024;

/// One deployment's complete summary configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSummaryConfig {
    pub envelope_recipients: Vec<String>,
    pub inbound_public_key: String,
    pub deployment: String,
    pub workflow_ingress: String,
    pub project_id: String,
    pub channel_id: String,
    pub gemini_model: String,
    pub gemini_location: String,
    pub claude_model: String,
    pub claude_location: String,
    pub max_input_chars: usize,
    pub max_output_tokens: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmailSummaryConfigError {
    #[error("{0} is required when {ENABLED_ENV}=true")]
    Missing(&'static str),
    #[error("{0} must be a boolean")]
    InvalidBoolean(&'static str),
    #[error("{0} must be a positive integer")]
    InvalidInteger(&'static str),
    #[error("{0} must be an HTTPS URL")]
    InvalidIngress(&'static str),
    #[error("{0} must be a non-empty token")]
    InvalidToken(&'static str),
    #[error("{0} must contain at least one recipient")]
    InvalidRecipients(&'static str),
}

impl EmailSummaryConfig {
    /// Resolve the production environment, returning `None` for the explicit
    /// default-off posture and an error for an enabled-but-incomplete row.
    pub fn from_env() -> Result<Option<Self>, EmailSummaryConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Testable environment seam. HTTP workflow ingress is accepted only for
    /// the explicit local harness; hosted profiles must use HTTPS.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Option<Self>, EmailSummaryConfigError> {
        let enabled = match non_empty(get(ENABLED_ENV)) {
            None => false,
            Some(value) => parse_bool(ENABLED_ENV, &value)?,
        };
        if !enabled {
            return Ok(None);
        }

        let recipients = required(&get, RECIPIENTS_ENV)?;
        let envelope_recipients = parse_recipients(&recipients)?;
        let inbound_public_key = required(&get, INBOUND_PUBLIC_KEY_ENV)?;
        let deployment = required(&get, DEPLOYMENT_ENV)?;
        let workflow_ingress = required(&get, WORKFLOW_INGRESS_ENV)?;
        let project_id = required(&get, GCP_PROJECT_ENV)?;
        let channel_id = required(&get, CHANNEL_ENV)?;
        let gemini_model = non_empty(get(GEMINI_MODEL_ENV))
            .unwrap_or_else(|| cloud::DEFAULT_GEMINI_SUMMARY_MODEL.to_string());
        let gemini_location = non_empty(get(GEMINI_LOCATION_ENV))
            .unwrap_or_else(|| cloud::DEFAULT_GEMINI_SUMMARY_LOCATION.to_string());
        let claude_model = non_empty(get(CLAUDE_MODEL_ENV))
            .unwrap_or_else(|| cloud::DEFAULT_CLAUDE_SUMMARY_MODEL.to_string());
        let claude_location = non_empty(get(CLAUDE_LOCATION_ENV))
            .unwrap_or_else(|| cloud::DEFAULT_CLAUDE_SUMMARY_LOCATION.to_string());
        let max_input_chars = positive_usize(
            MAX_INPUT_CHARS_ENV,
            get(MAX_INPUT_CHARS_ENV),
            DEFAULT_MAX_INPUT_CHARS,
        )?;
        let max_output_tokens = positive_u32(
            MAX_OUTPUT_TOKENS_ENV,
            get(MAX_OUTPUT_TOKENS_ENV),
            DEFAULT_MAX_OUTPUT_TOKENS,
        )?;

        for (key, value) in [
            (INBOUND_PUBLIC_KEY_ENV, &inbound_public_key),
            (DEPLOYMENT_ENV, &deployment),
            (GCP_PROJECT_ENV, &project_id),
            (CHANNEL_ENV, &channel_id),
            (GEMINI_MODEL_ENV, &gemini_model),
            (GEMINI_LOCATION_ENV, &gemini_location),
            (CLAUDE_MODEL_ENV, &claude_model),
            (CLAUDE_LOCATION_ENV, &claude_location),
        ] {
            if value.chars().any(char::is_whitespace) {
                return Err(EmailSummaryConfigError::InvalidToken(key));
            }
        }
        if !valid_ingress(&workflow_ingress, non_empty(get(CI_HARNESS_ENV)).as_deref()) {
            return Err(EmailSummaryConfigError::InvalidIngress(
                WORKFLOW_INGRESS_ENV,
            ));
        }

        Ok(Some(Self {
            envelope_recipients,
            inbound_public_key,
            deployment,
            workflow_ingress,
            project_id,
            channel_id,
            gemini_model,
            gemini_location,
            claude_model,
            claude_location,
            max_input_chars,
            max_output_tokens,
        }))
    }
}

fn required<F: Fn(&str) -> Option<String>>(
    get: &F,
    key: &'static str,
) -> Result<String, EmailSummaryConfigError> {
    non_empty(get(key)).ok_or(EmailSummaryConfigError::Missing(key))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool(key: &'static str, value: &str) -> Result<bool, EmailSummaryConfigError> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(EmailSummaryConfigError::InvalidBoolean(key)),
    }
}

fn parse_recipients(value: &str) -> Result<Vec<String>, EmailSummaryConfigError> {
    let mut seen = BTreeSet::new();
    for recipient in value.split(',') {
        let recipient = recipient.trim().to_ascii_lowercase();
        if !recipient.is_empty() {
            seen.insert(recipient);
        }
    }
    if seen.is_empty() {
        return Err(EmailSummaryConfigError::InvalidRecipients(RECIPIENTS_ENV));
    }
    Ok(seen.into_iter().collect())
}

fn positive_usize(
    key: &'static str,
    value: Option<String>,
    default: usize,
) -> Result<usize, EmailSummaryConfigError> {
    let Some(value) = non_empty(value) else {
        return Ok(default);
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(EmailSummaryConfigError::InvalidInteger(key))
}

fn positive_u32(
    key: &'static str,
    value: Option<String>,
    default: u32,
) -> Result<u32, EmailSummaryConfigError> {
    let Some(value) = non_empty(value) else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(EmailSummaryConfigError::InvalidInteger(key))
}

fn valid_ingress(value: &str, harness: Option<&str>) -> bool {
    value.starts_with("https://")
        || (harness.is_some_and(|value| value == "1" || value == "true")
            && value.starts_with("http://"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key| values.get(key).cloned()
    }

    fn enabled() -> Vec<(&'static str, &'static str)> {
        vec![
            (ENABLED_ENV, "true"),
            (
                RECIPIENTS_ENV,
                " Intake@Parse.example.com, intake@parse.example.com ",
            ),
            (INBOUND_PUBLIC_KEY_ENV, "public-key"),
            (DEPLOYMENT_ENV, "production"),
            (WORKFLOW_INGRESS_ENV, "https://restate.example.test"),
            (GCP_PROJECT_ENV, "project"),
            (CHANNEL_ENV, "C123"),
        ]
    }

    #[test]
    fn disabled_by_default() {
        assert_eq!(EmailSummaryConfig::from_lookup(lookup(&[])), Ok(None));
    }

    #[test]
    fn enabled_configuration_is_complete_and_deduplicated() {
        let config = EmailSummaryConfig::from_lookup(lookup(&enabled()))
            .expect("valid config")
            .expect("enabled");
        assert_eq!(config.envelope_recipients, vec!["intake@parse.example.com"]);
        assert_eq!(config.max_input_chars, DEFAULT_MAX_INPUT_CHARS);
        assert_eq!(config.max_output_tokens, DEFAULT_MAX_OUTPUT_TOKENS);
    }

    #[test]
    fn enabled_configuration_reports_the_first_missing_field() {
        let mut values = enabled();
        values.retain(|(key, _)| *key != CHANNEL_ENV);
        assert_eq!(
            EmailSummaryConfig::from_lookup(lookup(&values)),
            Err(EmailSummaryConfigError::Missing(CHANNEL_ENV))
        );
    }

    #[test]
    fn hosted_configuration_rejects_http_ingress() {
        let mut values = enabled();
        values.retain(|(key, _)| *key != WORKFLOW_INGRESS_ENV);
        values.push((WORKFLOW_INGRESS_ENV, "http://restate.example.test"));
        assert_eq!(
            EmailSummaryConfig::from_lookup(lookup(&values)),
            Err(EmailSummaryConfigError::InvalidIngress(
                WORKFLOW_INGRESS_ENV
            ))
        );
    }

    #[test]
    fn local_harness_may_use_http_ingress() {
        let mut values = enabled();
        values.retain(|(key, _)| *key != WORKFLOW_INGRESS_ENV);
        values.push((WORKFLOW_INGRESS_ENV, "http://127.0.0.1:9080"));
        values.push((CI_HARNESS_ENV, "1"));
        assert!(EmailSummaryConfig::from_lookup(lookup(&values)).is_ok());
    }
}
