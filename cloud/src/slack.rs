//! Firm-private Slack Web API adapter.
//!
//! The adapter accepts only provider-issued member IDs. Navigator never turns
//! an email address or a client participation row into a Slack invite.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SlackError {
    #[error("Slack credential is unavailable")]
    MissingCredential,
    #[error("Slack request failed")]
    Transport,
    #[error("Slack returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Slack rejected the request")]
    Api,
    #[error("Slack returned an incomplete response")]
    IncompleteResponse,
    #[error("Slack channel name is already in use")]
    NameTaken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
}

/// An ID issued by Slack for an eligible Firm participant. It is deliberately
/// not constructible from an email address in this module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlackMemberId(String);

impl SlackMemberId {
    pub fn new(provider_id: impl Into<String>) -> Result<Self, SlackError> {
        let provider_id = provider_id.into();
        if provider_id.is_empty() || !provider_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(SlackError::Api);
        }
        Ok(Self(provider_id))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[async_trait]
pub trait SlackService: Send + Sync {
    async fn find_private_channel(
        &self,
        project_code: &str,
    ) -> Result<Option<SlackChannel>, SlackError>;
    async fn create_private_channel(&self, project_code: &str) -> Result<SlackChannel, SlackError>;
    async fn invite_firm_members(
        &self,
        channel_id: &str,
        members: &[SlackMemberId],
    ) -> Result<(), SlackError>;
    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackError>;
}

/// Idempotent find-then-create channel provisioning. A failed lookup never
/// creates a second channel.
pub async fn ensure_private_channel<S: SlackService + ?Sized>(
    service: &S,
    project_code: &str,
    members: &[SlackMemberId],
) -> Result<(SlackChannel, bool), SlackError> {
    let channel = match service.find_private_channel(project_code).await? {
        Some(channel) => channel,
        None => service.create_private_channel(project_code).await?,
    };
    service.invite_firm_members(&channel.id, members).await?;
    Ok((channel, true))
}

#[derive(Clone, Default)]
pub struct FakeSlack {
    channels: Arc<Mutex<BTreeMap<String, SlackChannel>>>,
    members: Arc<Mutex<BTreeMap<String, BTreeSet<String>>>>,
    messages: Arc<Mutex<Vec<(String, String)>>>,
    unavailable: Arc<Mutex<bool>>,
}

impl FakeSlack {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_unavailable(&self, unavailable: bool) {
        *self.unavailable.lock().expect("Slack fake lock poisoned") = unavailable;
    }

    #[must_use]
    pub fn invited_members(&self, channel_id: &str) -> Vec<String> {
        self.members
            .lock()
            .expect("Slack fake lock poisoned")
            .get(channel_id)
            .map(|members| members.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn messages(&self) -> Vec<(String, String)> {
        self.messages
            .lock()
            .expect("Slack fake lock poisoned")
            .clone()
    }

    fn check_available(&self) -> Result<(), SlackError> {
        if *self.unavailable.lock().expect("Slack fake lock poisoned") {
            Err(SlackError::Transport)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl SlackService for FakeSlack {
    async fn find_private_channel(
        &self,
        project_code: &str,
    ) -> Result<Option<SlackChannel>, SlackError> {
        self.check_available()?;
        Ok(self
            .channels
            .lock()
            .expect("Slack fake lock poisoned")
            .get(project_code)
            .cloned())
    }

    async fn create_private_channel(&self, project_code: &str) -> Result<SlackChannel, SlackError> {
        self.check_available()?;
        let mut channels = self.channels.lock().expect("Slack fake lock poisoned");
        if let Some(channel) = channels.get(project_code) {
            return Ok(channel.clone());
        }
        let channel = SlackChannel {
            id: format!("C{:016X}", channels.len() + 1),
            name: project_code.to_string(),
        };
        channels.insert(project_code.to_string(), channel.clone());
        Ok(channel)
    }

    async fn invite_firm_members(
        &self,
        channel_id: &str,
        members: &[SlackMemberId],
    ) -> Result<(), SlackError> {
        self.check_available()?;
        let mut invited = self.members.lock().expect("Slack fake lock poisoned");
        let ids = invited.entry(channel_id.to_string()).or_default();
        ids.extend(members.iter().map(|member| member.0.clone()));
        Ok(())
    }

    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackError> {
        self.check_available()?;
        self.messages
            .lock()
            .expect("Slack fake lock poisoned")
            .push((channel_id.to_string(), text.to_string()));
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ChannelResponse {
    ok: bool,
    #[serde(default)]
    channel: Option<SlackChannelBody>,
}

#[derive(Debug, Deserialize)]
struct SlackChannelBody {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct BasicResponse {
    ok: bool,
}

/// Slack Web API adapter. The bot token is injected by the Firm-secret
/// resolver; `SLACK_BOT_TOKEN` is never consulted here.
pub struct SlackClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl std::fmt::Debug for SlackClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SlackClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl SlackClient {
    #[must_use]
    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    async fn post(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, SlackError> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|_| SlackError::Transport)?;
        if !response.status().is_success() {
            return Err(SlackError::HttpStatus(response.status().as_u16()));
        }
        response.json().await.map_err(|_| SlackError::Transport)
    }
}

#[async_trait]
impl SlackService for SlackClient {
    async fn find_private_channel(
        &self,
        project_code: &str,
    ) -> Result<Option<SlackChannel>, SlackError> {
        let body = self
            .post(
                "conversations.list",
                json!({ "types": "private_channel", "limit": 200 }),
            )
            .await?;
        let response: ChannelListResponse =
            serde_json::from_value(body).map_err(|_| SlackError::IncompleteResponse)?;
        Ok(response
            .channels
            .into_iter()
            .find(|channel| channel.name == project_code)
            .map(|channel| SlackChannel {
                id: channel.id,
                name: channel.name,
            }))
    }

    async fn create_private_channel(&self, project_code: &str) -> Result<SlackChannel, SlackError> {
        let body = self
            .post(
                "conversations.create",
                json!({ "name": project_code, "is_private": true }),
            )
            .await?;
        let response: ChannelResponse =
            serde_json::from_value(body).map_err(|_| SlackError::IncompleteResponse)?;
        if !response.ok {
            return Err(SlackError::NameTaken);
        }
        let channel = response.channel.ok_or(SlackError::IncompleteResponse)?;
        Ok(SlackChannel {
            id: channel.id,
            name: channel.name,
        })
    }

    async fn invite_firm_members(
        &self,
        channel_id: &str,
        members: &[SlackMemberId],
    ) -> Result<(), SlackError> {
        if members.is_empty() {
            return Ok(());
        }
        let body = self
            .post(
                "conversations.invite",
                json!({
                    "channel": channel_id,
                    "users": members.iter().map(SlackMemberId::as_str).collect::<Vec<_>>().join(",")
                }),
            )
            .await?;
        let response: BasicResponse =
            serde_json::from_value(body).map_err(|_| SlackError::IncompleteResponse)?;
        if response.ok {
            Ok(())
        } else {
            Err(SlackError::Api)
        }
    }

    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackError> {
        let body = self
            .post(
                "chat.postMessage",
                json!({ "channel": channel_id, "text": text }),
            )
            .await?;
        let response: BasicResponse =
            serde_json::from_value(body).map_err(|_| SlackError::IncompleteResponse)?;
        if response.ok {
            Ok(())
        } else {
            Err(SlackError::Api)
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChannelListResponse {
    #[serde(default)]
    channels: Vec<SlackChannelBody>,
}

#[cfg(test)]
mod tests {
    use super::{ensure_private_channel, FakeSlack, SlackError, SlackMemberId};

    #[tokio::test]
    async fn private_channel_invites_only_external_firm_ids_and_is_idempotent() {
        let slack = FakeSlack::new();
        let members = vec![
            SlackMemberId::new("U123").unwrap(),
            SlackMemberId::new("U456").unwrap(),
        ];
        let (first, _) = ensure_private_channel(&slack, "sample-project", &members)
            .await
            .unwrap();
        let (second, _) = ensure_private_channel(&slack, "sample-project", &members)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(slack.invited_members(&first.id), vec!["U123", "U456"]);
    }

    #[tokio::test]
    async fn unavailable_lookup_does_not_create_or_invite() {
        let slack = FakeSlack::new();
        slack.set_unavailable(true);
        let member = SlackMemberId::new("U123").unwrap();
        assert!(matches!(
            ensure_private_channel(&slack, "sample-project", &[member]).await,
            Err(SlackError::Transport)
        ));
        assert!(slack.messages().is_empty());
    }

    #[test]
    fn member_ids_reject_email_shaped_values() {
        assert!(SlackMemberId::new("person@example.com").is_err());
    }
}
