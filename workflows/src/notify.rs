//! Ops-notification seam — the chat sibling of [`crate::email::EmailService`].
//!
//! Neon Law Navigator's durable workflows prove their liveness by notifying firm ops
//! (the six-hourly `Heartbeat`, the nightly `Archives` digest, the
//! `BillingCanary`, …). That signal goes to an incoming **Slack** webhook on
//! the engineering channel — where engineers already watch — and **no longer
//! also goes out as email**: a recurring liveness signal is trivial to lose in
//! an inbox (the very failure mode that hid a real heartbeat gap once), so once
//! Slack delivery proved reliable the firm dropped the duplicate ops email (the
//! follow-up to the dual-send introduced in PR #13).
//!
//! Two pieces:
//!
//! - [`Notifier`] — the trait, with a real [`SlackNotifier`] (POSTs
//!   `{"text": …}` to an incoming webhook) and a [`CapturingNotifier`] that
//!   keeps messages in memory for KIND/tests so nothing leaves the binary.
//! - [`SlackOpsDelivery`] — an [`EmailService`] adapter that delivers an ops
//!   notice to a [`Notifier`] (Slack) *instead of* sending email. The ops
//!   workflows render their notice as an [`OutboundEmail`] and hand it to their
//!   `EmailService`; wiring them with this adapter routes the notice to Slack
//!   and sends no mail at all.
//!
//! **The load-bearing boundary:** [`SlackOpsDelivery`] must back **only**
//! internal/operations services — `BillingCanary` and `BillingDigest`. (`Archives` and `Heartbeat` are also Slack-only but post
//! to the [`Notifier`] directly — their messages are mrkdwn, so they need the
//! links and glyphs a fenced code block would flatten, not the plain-text
//! framing this adapter renders.) Those carry no client, matter, or PII data
//! (their recipients are env-pinned to firm ops).
//! It must **never** back
//! a client-facing email service (such as `Notation`):
//! pushing client content into a chat channel would cross the firm's trust
//! boundary, violating the standing no-content rule (see the `observability`
//! skill). The boundary is enforced at the wiring point in `workflows-service`'s
//! `main.rs`, not by per-message inspection here.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use crate::email::service::{EmailError, EmailService, OutboundEmail, SendReceipt};

/// Why a notification failed to deliver. Distinct from [`EmailError`] because a
/// notifier outage must never be conflated with — nor fail — an email send.
#[derive(Debug, Error)]
pub enum NotifyError {
    /// The HTTP request to the webhook never completed (DNS, TLS, timeout).
    #[error("transport error: {0}")]
    Transport(String),
    /// The webhook returned a non-2xx status (e.g. 404 for a revoked webhook).
    #[error("notification endpoint rejected the message: status {0}")]
    Rejected(u16),
}

/// Why a Slack Web API request failed. The bot client is separate from the
/// legacy incoming-webhook notifier because Web API calls choose a channel at
/// request time and therefore can serve one private channel per Project.
#[derive(Debug, Error)]
pub enum SlackBotError {
    /// The request did not complete.
    #[error("Slack API transport error: {0}")]
    Transport(String),
    /// Slack or its edge rejected the HTTP request.
    #[error("Slack API HTTP status {0}")]
    HttpStatus(u16),
    /// Slack returned a structured API error.
    #[error("Slack API rejected the request: {0}")]
    Api(String),
    /// Slack returned success without the object the caller needs.
    #[error("Slack API returned an incomplete response")]
    IncompleteResponse,
}

/// The subset of a Slack channel response Navigator needs to address later
/// messages. The ID, not the channel name, is the stable posting coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
}

/// Target-aware Slack delivery for internal Project channels.
#[async_trait]
pub trait SlackBot: Send + Sync {
    /// Create one private channel named after the Project code.
    async fn create_private_channel(&self, name: &str) -> Result<SlackChannel, SlackBotError>;
    /// Post a short internal notice to a channel ID.
    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackBotError>;
}

/// Slack Web API client authenticated with a bot token. The token is held only
/// in the HTTP client's request path and is never included in errors or logs.
#[derive(Clone)]
pub struct SlackBotClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl SlackBotClient {
    /// The production Slack Web API origin.
    pub const DEFAULT_BASE_URL: &'static str = "https://slack.com/api";

    /// Build a client for Slack's Web API.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base_url(token, Self::DEFAULT_BASE_URL)
    }

    /// Build a client against an explicit origin for tests.
    #[must_use]
    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    async fn post<T: serde::Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        body: &T,
    ) -> Result<R, SlackBotError> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|error| SlackBotError::Transport(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(SlackBotError::HttpStatus(status.as_u16()));
        }
        response
            .json::<R>()
            .await
            .map_err(|error| SlackBotError::Transport(error.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct CreateChannelResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    channel: Option<SlackChannel>,
}

#[derive(Debug, Deserialize)]
struct PostMessageResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl SlackBot for SlackBotClient {
    async fn create_private_channel(&self, name: &str) -> Result<SlackChannel, SlackBotError> {
        let response: CreateChannelResponse = self
            .post(
                "conversations.create",
                &json!({ "name": name, "is_private": true }),
            )
            .await?;
        if !response.ok {
            return Err(SlackBotError::Api(
                response
                    .error
                    .unwrap_or_else(|| "unknown_error".to_string()),
            ));
        }
        response.channel.ok_or(SlackBotError::IncompleteResponse)
    }

    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackBotError> {
        let response: PostMessageResponse = self
            .post(
                "chat.postMessage",
                &json!({ "channel": channel_id, "text": text }),
            )
            .await?;
        if response.ok {
            Ok(())
        } else {
            Err(SlackBotError::Api(
                response
                    .error
                    .unwrap_or_else(|| "unknown_error".to_string()),
            ))
        }
    }
}

/// In-memory Slack bot used by tests and local development. It proves the
/// target-aware request path without sending anything outside the process.
#[derive(Clone, Default)]
pub struct CapturingSlackBot {
    created: Arc<Mutex<Vec<String>>>,
    posted: Arc<Mutex<Vec<(String, String)>>>,
}

impl CapturingSlackBot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn created_channels(&self) -> Vec<String> {
        self.created
            .lock()
            .expect("Slack bot lock poisoned")
            .clone()
    }

    #[must_use]
    pub fn posted_messages(&self) -> Vec<(String, String)> {
        self.posted.lock().expect("Slack bot lock poisoned").clone()
    }
}

#[async_trait]
impl SlackBot for CapturingSlackBot {
    async fn create_private_channel(&self, name: &str) -> Result<SlackChannel, SlackBotError> {
        let count = {
            let mut created = self.created.lock().expect("Slack bot lock poisoned");
            created.push(name.to_string());
            created.len()
        };
        Ok(SlackChannel {
            id: format!("C{count:016X}"),
            name: name.to_string(),
        })
    }

    async fn post_message(&self, channel_id: &str, text: &str) -> Result<(), SlackBotError> {
        self.posted
            .lock()
            .expect("Slack bot lock poisoned")
            .push((channel_id.to_string(), text.to_string()));
        Ok(())
    }
}

/// A one-way channel for internal operations notifications. Implementors post a
/// short plain-text message somewhere firm engineers watch (Slack today).
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Deliver `text`. Returns `Ok(())` on a successful post.
    async fn notify(&self, text: String) -> Result<(), NotifyError>;
}

/// Captures every message in memory instead of sending it. Used by tests and
/// KIND/dev, where posting to the real engineering channel would be noise.
#[derive(Clone, Default)]
pub struct CapturingNotifier {
    sent: Arc<Mutex<Vec<String>>>,
}

impl CapturingNotifier {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every message handed to [`Notifier::notify`] so far.
    #[must_use]
    pub fn captured(&self) -> Vec<String> {
        self.sent.lock().expect("notifier lock poisoned").clone()
    }
}

#[async_trait]
impl Notifier for CapturingNotifier {
    async fn notify(&self, text: String) -> Result<(), NotifyError> {
        self.sent.lock().expect("notifier lock poisoned").push(text);
        Ok(())
    }
}

/// Posts to a Slack **incoming webhook**. The webhook URL already pins the
/// destination channel, so the only payload is the message text.
#[derive(Clone)]
pub struct SlackNotifier {
    http: reqwest::Client,
    webhook_url: String,
}

impl SlackNotifier {
    /// Production constructor: targets the given incoming-webhook URL
    /// (`SLACK_WEBHOOK_URL`).
    #[must_use]
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            webhook_url: webhook_url.into(),
        }
    }

    /// Build the JSON body for the Slack incoming-webhook endpoint. Pure —
    /// exposed for unit-testing the request shape without an HTTP round-trip.
    #[must_use]
    pub fn build_request_body(text: &str) -> serde_json::Value {
        json!({ "text": text })
    }
}

#[async_trait]
impl Notifier for SlackNotifier {
    async fn notify(&self, text: String) -> Result<(), NotifyError> {
        let resp = self
            .http
            .post(&self.webhook_url)
            .json(&Self::build_request_body(&text))
            .send()
            .await
            .map_err(|e| NotifyError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(NotifyError::Rejected(status.as_u16()))
        }
    }
}

/// Slack rejects a single message's `text` past 4000 characters and splits it
/// into separate messages at that boundary — which lands mid-code-block for a
/// long ops digest, leaving the first message an *unterminated* fence that
/// Slack renders as literal backticks in proportional text. So the body is
/// split here, on line boundaries, into chunks small enough that each fenced
/// message stays well under the limit. Reserves headroom below 4000 for the
/// bold header and the fence lines.
const SLACK_CHUNK_BUDGET: usize = 3500;

/// Render an internal ops email as one or more Slack messages: the subject as a
/// bold header, then the plain-text body in a code block so fixed-width ops
/// tables keep their columns in Slack. A body that would overflow Slack's
/// 4000-character message limit is split into several messages, each an
/// independently valid (self-closed) code block and each numbered `(i/n)` in
/// its header. Pure and exposed so the formatting is unit-tested. The HTML part
/// is intentionally dropped — Slack renders the plain text and the body already
/// reads as a standalone ops notice.
#[must_use]
pub fn ops_slack_messages(email: &OutboundEmail) -> Vec<String> {
    let body = email.body.trim_end().replace("```", "'''");
    let chunks = chunk_body_lines(&body, SLACK_CHUNK_BUDGET);
    let total = chunks.len();
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let header = if total > 1 {
                format!("*{}* ({}/{total})", email.subject, i + 1)
            } else {
                format!("*{}*", email.subject)
            };
            format!("{header}\n```\n{chunk}\n```")
        })
        .collect()
}

/// Group `body`'s lines into chunks whose character count stays within
/// `budget`, never splitting a line (a table row or fence must stay intact).
/// Always returns at least one chunk (empty for an empty body). A single line
/// longer than `budget` becomes its own chunk unsplit — ops lines (URLs, table
/// rows) are far shorter than the budget, so this only guards pathological
/// input.
fn chunk_body_lines(body: &str, budget: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in body.split('\n') {
        // +1 for the '\n' that would join this line onto the current chunk.
        let projected = current.chars().count() + line.chars().count() + 1;
        if !current.is_empty() && projected > budget {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    chunks.push(current);
    chunks
}

/// An [`EmailService`] adapter that delivers an ops notice to a [`Notifier`]
/// (Slack) **instead of** sending email. The internal/ops workflows render
/// their notice as an [`OutboundEmail`] and hand it to their `EmailService`;
/// backing them with this adapter posts that notice to the engineering channel
/// and sends no mail.
///
/// Slack is now the single delivery path for the ops signal, so — unlike the
/// former best-effort dual-send mirror — a delivery failure **is** propagated
/// as an [`EmailError`]. That fails the workflow's durable `ctx.run("notify")`
/// step, so Restate retries and redelivers once Slack recovers, rather than
/// silently dropping the only copy of the signal. Back ONLY internal/ops
/// services — see the module-level boundary note.
#[derive(Clone)]
pub struct SlackOpsDelivery {
    notifier: Arc<dyn Notifier>,
}

impl SlackOpsDelivery {
    #[must_use]
    pub fn new(notifier: Arc<dyn Notifier>) -> Self {
        Self { notifier }
    }
}

#[async_trait]
impl EmailService for SlackOpsDelivery {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError> {
        // A long digest posts as several fenced messages; a failure partway
        // through surfaces so Restate retries the whole `notify` step (an ops
        // notice is at-least-once — a duplicate repost is harmless).
        for message in ops_slack_messages(&email) {
            self.notifier
                .notify(message)
                .await
                .map_err(|err| EmailError::Transport(err.to_string()))?;
        }
        // No mail was sent, so there is no provider message id to surface.
        Ok(SendReceipt { message_id: None })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ops_slack_messages, CapturingNotifier, CapturingSlackBot, Notifier, NotifyError, SlackBot,
        SlackBotClient, SlackNotifier, SlackOpsDelivery, SLACK_CHUNK_BUDGET,
    };
    use crate::email::service::{EmailError, EmailService, OutboundEmail};
    use async_trait::async_trait;
    use std::sync::Arc;

    fn ops_email() -> OutboundEmail {
        OutboundEmail::new(
            "nick@neonlaw.com",
            "Durable execution OK — heartbeat 2026-06-19 18:00 UTC",
            "The durable-execution heartbeat ran end to end.",
        )
    }

    #[test]
    fn slack_body_is_text_field() {
        let body = SlackNotifier::build_request_body("hello ops");
        assert_eq!(body, serde_json::json!({ "text": "hello ops" }));
    }

    #[tokio::test]
    async fn bot_creates_private_channel_and_posts_to_its_id() {
        let bot = CapturingSlackBot::new();
        let channel = bot
            .create_private_channel("sample-project")
            .await
            .expect("capturing bot creates a channel");
        bot.post_message(&channel.id, "Client viewed this Project in the portal.")
            .await
            .expect("capturing bot posts");

        assert_eq!(bot.created_channels(), vec!["sample-project"]);
        assert_eq!(
            bot.posted_messages(),
            vec![(
                channel.id,
                "Client viewed this Project in the portal.".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn bot_uses_channel_id_and_bearer_token_on_the_web_api() {
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, ResponseTemplate};

        let server = wiremock::MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.create"))
            .and(header("authorization", "Bearer xoxb-test"))
            .and(body_partial_json(serde_json::json!({
                "name": "sample-project",
                "is_private": true
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"ok":true,"channel":{"id":"C123","name":"sample-project"}}"#,
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(header("authorization", "Bearer xoxb-test"))
            .and(body_partial_json(serde_json::json!({
                "channel": "C123",
                "text": "hello"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .expect(1)
            .mount(&server)
            .await;

        let bot = SlackBotClient::with_base_url("xoxb-test", server.uri());
        let channel = bot
            .create_private_channel("sample-project")
            .await
            .expect("Slack create succeeds");
        bot.post_message(&channel.id, "hello")
            .await
            .expect("Slack post succeeds");
    }

    #[test]
    fn ops_slack_short_body_is_one_bold_subject_then_fenced_body() {
        let msgs = ops_slack_messages(&ops_email());
        assert_eq!(msgs.len(), 1, "a short body fits one message");
        let text = &msgs[0];
        assert!(text.starts_with("*Durable execution OK"));
        assert!(text.contains("\n```"));
        assert!(text.contains("ran end to end"));
        assert!(text.ends_with("```"));
    }

    #[test]
    fn ops_slack_text_escapes_nested_code_fences() {
        let email = OutboundEmail::new("ops@example.com", "Ops", "before ``` after\n");
        let msgs = ops_slack_messages(&email);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("before ''' after"));
        // Only the two fence lines this renderer added — the body's own fence
        // was neutralised to `'''`, so it can't leak an unbalanced ```.
        assert_eq!(msgs[0].matches("```").count(), 2);
    }

    #[test]
    fn ops_slack_long_body_splits_into_self_closed_fenced_messages() {
        // A body that dwarfs Slack's 4000-char limit — the archives digest's
        // failure mode. Each line is short; there are simply many of them.
        let body = (0..400)
            .map(|i| format!("  table_{i:03}: gs://bucket/iceberg/table_{i:03}/data/part.parquet"))
            .collect::<Vec<_>>()
            .join("\n");
        let email = OutboundEmail::new("ops@example.com", "Archives digest", body.clone());
        let msgs = ops_slack_messages(&email);

        assert!(
            msgs.len() > 1,
            "an oversized body must split, got {} message(s)",
            msgs.len()
        );
        for (i, msg) in msgs.iter().enumerate() {
            // Every message is independently under Slack's hard limit...
            assert!(
                msg.chars().count() < 4000,
                "message {} is {} chars, over Slack's 4000 limit",
                i + 1,
                msg.chars().count()
            );
            // ...a self-closed code block (exactly the opening + closing fence,
            // never an unterminated one that renders as literal backticks)...
            assert_eq!(
                msg.matches("```").count(),
                2,
                "message {} has an unbalanced fence",
                i + 1
            );
            assert!(
                msg.ends_with("```"),
                "message {} must close its fence",
                i + 1
            );
            // ...and carries the bold, numbered header so the parts read as one.
            assert!(
                msg.starts_with(&format!("*Archives digest* ({}/{})", i + 1, msgs.len())),
                "message {} header: {:?}",
                i + 1,
                msg.lines().next()
            );
        }

        // No content is lost across the split: concatenating the fenced payloads
        // reproduces every original line.
        let rejoined: String = msgs
            .iter()
            .flat_map(|m| m.lines().skip(2)) // drop header + opening fence
            .filter(|l| *l != "```") // drop each closing fence
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rejoined, body);
    }

    #[test]
    fn chunk_budget_leaves_headroom_below_slack_limit() {
        // The per-message budget plus header and fences must clear 4000.
        const { assert!(SLACK_CHUNK_BUDGET < 4000 - 200) };
    }

    #[tokio::test]
    async fn capturing_notifier_records_messages() {
        let n = CapturingNotifier::new();
        n.notify("first".into())
            .await
            .expect("capturing never fails");
        n.notify("second".into())
            .await
            .expect("capturing never fails");
        assert_eq!(
            n.captured(),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[tokio::test]
    async fn delivery_posts_notice_to_slack_only() {
        let notifier = Arc::new(CapturingNotifier::new());
        let delivery = SlackOpsDelivery::new(notifier.clone());

        delivery.send(ops_email()).await.expect("send succeeds");

        assert_eq!(notifier.captured().len(), 1, "notice posted to Slack");
        assert!(notifier.captured()[0].contains("heartbeat"));
    }

    #[tokio::test]
    async fn delivery_posts_every_chunk_of_an_oversized_digest() {
        let notifier = Arc::new(CapturingNotifier::new());
        let delivery = SlackOpsDelivery::new(notifier.clone());
        let body = (0..400)
            .map(|i| format!("  table_{i:03}: gs://bucket/iceberg/table_{i:03}/part.parquet"))
            .collect::<Vec<_>>()
            .join("\n");
        let email = OutboundEmail::new("ops@example.com", "Archives digest", body);

        delivery.send(email.clone()).await.expect("send succeeds");

        assert_eq!(
            notifier.captured(),
            ops_slack_messages(&email),
            "every fenced chunk is posted, in order"
        );
        assert!(
            notifier.captured().len() > 1,
            "an oversized digest posts as multiple messages"
        );
    }

    /// A notifier whose every send fails — proves the adapter surfaces the error.
    struct FailingNotifier;
    #[async_trait]
    impl Notifier for FailingNotifier {
        async fn notify(&self, _text: String) -> Result<(), NotifyError> {
            Err(NotifyError::Rejected(404))
        }
    }

    #[tokio::test]
    async fn delivery_propagates_failure_so_durable_step_retries() {
        let delivery = SlackOpsDelivery::new(Arc::new(FailingNotifier));

        // Slack is the only delivery path now, so a Slack outage MUST fail the
        // durable notify step (Restate then retries) rather than be swallowed.
        let err = delivery
            .send(ops_email())
            .await
            .expect_err("a Slack failure must surface to the caller");

        assert!(matches!(err, EmailError::Transport(_)));
    }
}
