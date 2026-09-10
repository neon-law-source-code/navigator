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

const SLACK_BASE_URL: &str = "https://slack.com/api";
/// Slack's own cap for one `conversations.list` page.
const CHANNEL_PAGE_SIZE: u32 = 200;
/// A Firm past this many pages of private channels is not a matter list.
/// Bounded so a cursor the provider never clears cannot spin forever.
const MAX_CHANNEL_PAGES: usize = 20;

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
/// creates a second channel. The returned flag distinguishes a channel this
/// call created from one it adopted: an operator reading `ensure` output has
/// to be able to tell a first provisioning from a re-run, and an unconditional
/// `true` would report every re-run as a new Firm-private channel.
pub async fn ensure_private_channel<S: SlackService + ?Sized>(
    service: &S,
    project_code: &str,
    members: &[SlackMemberId],
) -> Result<(SlackChannel, bool), SlackError> {
    let (channel, created) = match service.find_private_channel(project_code).await? {
        Some(channel) => (channel, false),
        None => (service.create_private_channel(project_code).await?, true),
    };
    service.invite_firm_members(&channel.id, members).await?;
    Ok((channel, created))
}

#[derive(Debug, Clone, Default)]
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
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SlackChannelBody {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct BasicResponse {
    ok: bool,
    /// Slack's machine-readable error slug. Carried so a refusal can be told
    /// apart rather than reported as whatever the last caller assumed.
    #[serde(default)]
    error: Option<String>,
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
    /// The production client. The bot token is the Firm's resolved provider
    /// credential, so `SLACK_BOT_TOKEN` is never consulted here.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base_url(token, SLACK_BASE_URL)
    }

    #[must_use]
    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    /// Slack's read methods are `GET` with query parameters, and they do not
    /// accept a JSON body — a JSON `POST` to `conversations.list` comes back
    /// as `invalid_form_data` rather than a channel list. The write methods
    /// below do accept JSON, so the two transports are separate here.
    async fn get(
        &self,
        method: &str,
        query: &[(&str, String)],
    ) -> Result<serde_json::Value, SlackError> {
        let response = self
            .http
            .get(format!("{}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .await
            .map_err(|_| SlackError::Transport)?;
        if !response.status().is_success() {
            return Err(SlackError::HttpStatus(response.status().as_u16()));
        }
        response.json().await.map_err(|_| SlackError::Transport)
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
        let mut cursor = String::new();
        for _ in 0..MAX_CHANNEL_PAGES {
            let mut query = vec![
                ("types", "private_channel".to_string()),
                ("limit", CHANNEL_PAGE_SIZE.to_string()),
                ("exclude_archived", "true".to_string()),
            ];
            if !cursor.is_empty() {
                query.push(("cursor", cursor.clone()));
            }
            let body = self.get("conversations.list", &query).await?;
            let response: ChannelListResponse =
                serde_json::from_value(body).map_err(|_| SlackError::IncompleteResponse)?;
            if !response.ok {
                return Err(SlackError::Api);
            }
            if let Some(channel) = response
                .channels
                .into_iter()
                .find(|channel| channel.name == project_code)
            {
                return Ok(Some(SlackChannel {
                    id: channel.id,
                    name: channel.name,
                }));
            }
            // An empty `next_cursor` is how Slack says "last page"; it is
            // present and empty rather than absent, so both are the end.
            match response.response_metadata.and_then(|meta| meta.next_cursor) {
                Some(next) if !next.is_empty() => cursor = next,
                _ => return Ok(None),
            }
        }
        Ok(None)
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
            // Only `name_taken` means the name is taken. Reporting every
            // refusal as a name collision sends an operator to rename a
            // channel when the real answer is a missing scope or a revoked
            // token.
            return Err(if response.error.as_deref() == Some("name_taken") {
                SlackError::NameTaken
            } else {
                SlackError::Api
            });
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
        // `already_in_channel` is what Slack answers when the member this
        // call names is already there, which is the steady state of an
        // idempotent ensure — treating it as a failure would make every
        // re-run of a provisioned channel report an error.
        if response.ok || response.error.as_deref() == Some("already_in_channel") {
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
    ok: bool,
    #[serde(default)]
    channels: Vec<SlackChannelBody>,
    #[serde(default)]
    response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
struct ResponseMetadata {
    #[serde(default)]
    next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_private_channel, FakeSlack, SlackClient, SlackError, SlackMemberId, SlackService,
    };
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Slack's read methods are `GET` with query parameters. This asserts the
    /// transport and the cursor: the channel lives on the second page, so a
    /// client that read one page and stopped would report it missing — and
    /// `ensure` would then create a duplicate of a channel that exists.
    #[tokio::test]
    async fn channel_lookup_is_a_get_that_follows_the_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .and(query_param("cursor", "page-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "channels": [{ "id": "C2", "name": "sample-project" }],
                "response_metadata": { "next_cursor": "" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "channels": [{ "id": "C1", "name": "another-matter" }],
                "response_metadata": { "next_cursor": "page-2" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url("test-token", server.uri());
        let found = client
            .find_private_channel("sample-project")
            .await
            .unwrap()
            .expect("the channel on the second page is found");
        assert_eq!(found.id, "C2");
    }

    /// Only `name_taken` means the name is taken. A missing scope reported as
    /// a name collision sends an operator to rename a channel that is fine.
    #[tokio::test]
    async fn a_refusal_is_reported_as_itself_not_as_a_name_collision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.create"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "ok": false, "error": "missing_scope" })),
            )
            .mount(&server)
            .await;
        let client = SlackClient::with_base_url("test-token", server.uri());
        assert!(matches!(
            client.create_private_channel("sample-project").await,
            Err(SlackError::Api)
        ));
    }

    /// A member already in the channel is the steady state of an idempotent
    /// ensure, so the re-run must succeed rather than report a failure.
    #[tokio::test]
    async fn an_invite_for_an_existing_member_is_not_a_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.invite"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({ "ok": false, "error": "already_in_channel" }),
                ),
            )
            .mount(&server)
            .await;
        let client = SlackClient::with_base_url("test-token", server.uri());
        client
            .invite_firm_members("C1", &[SlackMemberId::new("U123").unwrap()])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn private_channel_invites_only_external_firm_ids_and_is_idempotent() {
        let slack = FakeSlack::new();
        let members = vec![
            SlackMemberId::new("U123").unwrap(),
            SlackMemberId::new("U456").unwrap(),
        ];
        let (first, created) = ensure_private_channel(&slack, "sample-project", &members)
            .await
            .unwrap();
        let (second, created_again) = ensure_private_channel(&slack, "sample-project", &members)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert!(created, "the first ensure creates the Firm-private channel");
        assert!(
            !created_again,
            "a re-run adopts the existing channel and must not report a creation"
        );
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
